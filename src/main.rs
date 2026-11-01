mod runner;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Tunnelde helper: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--mcp-stdio") => mcp_bridge().await,
        Some("--version") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("start" | "serve" | "send" | "poll" | "stop" | "status") => {
            runner::run(&arguments).await
        }
        _ => Err("Expected --mcp-stdio, --version, start, send, poll, stop or status".into()),
    }
}

/// Native stdio proxy. Secrets stay in environment and headers, never command lines or logs.
async fn mcp_bridge() -> Result<()> {
    let endpoint = std::env::var("TUNNELDE_MCP_URL")?;
    let token = std::env::var("TUNNELDE_MCP_TOKEN")?;
    let url = reqwest::Url::parse(&endpoint)?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "MCP endpoint must be an HTTP loopback address without embedded credentials".into(),
        );
    }
    if token.is_empty() || token.len() > 4096 {
        return Err("Invalid MCP token".into());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let mut reader = BufReader::new(tokio::io::stdin());
    let state = std::sync::Arc::new(tokio::sync::Mutex::new((
        None::<String>,
        "2025-11-25".to_string(),
    )));
    let mut requests = tokio::task::JoinSet::new();
    loop {
        let line = bounded_line(&mut reader, 1024 * 1024).await?;
        let Some(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&line)?;
        while let Some(result) = requests.try_join_next() {
            result??;
        }
        let initialize = message["method"] == "initialize";
        if requests.len() >= 32 {
            return Err("Too many outstanding MCP requests".into());
        }
        let client = client.clone();
        let url = url.clone();
        let token = token.clone();
        let state = state.clone();
        let exchange = async move {
            let (session, version) = state.lock().await.clone();
            let mut request = client
                .post(url.clone())
                .bearer_auth(&token)
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", &version)
                .json(&message);
            if let Some(session) = &session {
                request = request.header("Mcp-Session-Id", session);
            }
            let response = request.send().await?;
            if let Some(value) = response.headers().get("Mcp-Session-Id") {
                state.lock().await.0 = Some(value.to_str()?.to_owned());
            }
            let status = response.status();
            if status.as_u16() == 202 || status.as_u16() == 204 {
                return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
            }
            if !status.is_success() {
                if let Some(id) = message.get("id") {
                    write_message(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("MCP HTTP {}",status.as_u16())}})).await?;
                }
                return Ok(());
            }
            let content_type = response
                .headers()
                .get("Content-Type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                bytes.extend_from_slice(&chunk);
                if bytes.len() > 1024 * 1024 {
                    return Err("MCP response exceeds 1 MiB".into());
                }
            }
            if content_type.contains("text/event-stream") {
                let text = String::from_utf8(bytes)?;
                for event in text.replace("\r\n", "\n").split("\n\n") {
                    let data = event
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !data.is_empty() {
                        write_message(&serde_json::from_str::<Value>(&data)?).await?;
                    }
                }
            } else {
                let response: Value = serde_json::from_slice(&bytes)?;
                if let Some(protocol) = response
                    .pointer("/result/protocolVersion")
                    .and_then(Value::as_str)
                {
                    state.lock().await.1 = protocol.to_owned();
                }
                write_message(&response).await?;
            }
            Ok(())
        };
        if initialize {
            exchange.await?;
        } else {
            requests.spawn(exchange);
        }
    }
    while let Some(result) = requests.join_next().await {
        result??;
    }
    let (session, version) = state.lock().await.clone();
    if let Some(session) = session {
        let _ = client
            .delete(url)
            .bearer_auth(token)
            .header("Mcp-Session-Id", session)
            .header("MCP-Protocol-Version", version)
            .send()
            .await;
    }
    Ok(())
}

async fn write_message(message: &Value) -> Result<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let mut output = tokio::io::stdout();
    output.write_all(format!("{message}\n").as_bytes()).await?;
    output.flush().await?;
    Ok(())
}

pub(crate) async fn bounded_line<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8(bytes)?))
            };
        }
        let size = chunk
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(chunk.len(), |position| position + 1);
        if bytes.len() + size > limit {
            return Err("Input exceeds size limit".into());
        }
        let end = chunk[size - 1] == b'\n';
        bytes.extend_from_slice(&chunk[..size]);
        reader.consume(size);
        if end {
            return Ok(Some(String::from_utf8(bytes)?));
        }
    }
}
