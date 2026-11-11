mod bootstrap;
mod launch;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};

use crate::{bounded_line, Result};

const MAX_JOURNAL: u64 = 32 * 1024 * 1024;
const MAX_TEXT: u64 = 512 * 1024;
const MAX_PAGE: usize = 512 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

async fn bounded_write(output: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, output.write_all(bytes))
        .await
        .map_err(|_| "Process stdin write timed out; input must be closed before retrying")??;
    Ok(())
}

fn runner_status(root: &Path) -> Value {
    if !root.is_dir() {
        return json!({"status":"notFound"});
    }
    if let Some(status) = std::fs::read(root.join("status.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    {
        return status;
    }
    let recent = |name| {
        std::fs::metadata(root.join(name))
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed < Duration::from_secs(15))
    };
    json!({"status":if recent("heartbeat"){"running"}else if !root.join("heartbeat").exists() && recent("pid"){"starting"}else{"unresponsive"}})
}

async fn journal_page(root: &Path, after: u64) -> Result<Value> {
    let mut events = Vec::new();
    let mut bytes = 0;
    let mut first = None;
    let mut next = after;
    // A page is one prefix across both segments. Never advance past an unfinished append.
    'segments: for name in ["events.previous.jsonl", "events.jsonl"] {
        let file = match tokio::fs::File::open(root.join(name)).await {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut reader = BufReader::new(file);
        loop {
            let mut line = Vec::new();
            let count = (&mut reader)
                .take(1024 * 1024 + 1)
                .read_until(b'\n', &mut line)
                .await?;
            if count == 0 {
                break;
            }
            if count > 1024 * 1024 {
                return Err("Journal event exceeds 1 MiB".into());
            }
            if line.last() != Some(&b'\n') {
                break 'segments;
            }
            let event: Value = serde_json::from_slice(&line)
                .map_err(|_| "Runner journal contains invalid JSON")?;
            let seq = event["seq"]
                .as_u64()
                .ok_or("Runner journal has invalid sequence")?;
            first.get_or_insert(seq);
            if seq <= after {
                continue;
            }
            if bytes + line.len() > MAX_PAGE && !events.is_empty() {
                break 'segments;
            }
            if seq <= next {
                return Err("Runner journal sequence is not increasing".into());
            }
            bytes += line.len();
            next = seq;
            events.push(event);
        }
    }
    Ok(
        json!({"events":events,"nextSeq":next,"truncated":first.is_some_and(|first| after.saturating_add(1)<first)}),
    )
}

fn option(args: &[String], key: &str) -> Result<String> {
    args.windows(2)
        .find(|pair| pair[0] == key)
        .map(|pair| pair[1].clone())
        .ok_or_else(|| format!("Missing {key}").into())
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.contains('\0'))
        .ok_or_else(|| format!("Invalid {key}").into())
}
fn id_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

pub async fn run(args: &[String]) -> Result<()> {
    let directory = PathBuf::from(option(args, "--directory")?);
    if !directory.is_absolute() {
        return Err("Runner directory must be absolute".into());
    }
    let id = option(args, "--id")?;
    if !id_valid(&id) {
        return Err("Invalid runner ID".into());
    }
    let root = directory.join(&id);
    match args[0].as_str() {
        "start" => launch::start(args, &directory, &id, &root).await,
        "serve" => {
            let result = serve(args, &root).await;
            if let Err(error) = &result {
                if !root.join("status.json").exists() {
                    let _ = write_private(
                        &root.join("status.json"),
                        serde_json::to_string(
                            &json!({"status":"failed","error":error.to_string()}),
                        )?
                        .as_bytes(),
                    );
                }
            }
            result
        }
        "send" => {
            if !root.join("pid").is_file() || root.join("status.json").is_file() {
                return Err("Runner is not active".into());
            }
            let mut input = BufReader::new(tokio::io::stdin());
            let message = bounded_line(&mut input, 256 * 1024)
                .await?
                .ok_or("Missing message")?;
            let _: Value = serde_json::from_str(&message)?;
            let queue = root.join("inbox");
            if std::fs::read_dir(&queue)?.count() >= 256 {
                return Err("Runner inbox is full".into());
            }
            let name = format!(
                "{:020}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
                uuid::Uuid::new_v4()
            );
            let temporary = queue.join(format!(".{name}.tmp"));
            write_private(&temporary, message.as_bytes())?;
            std::fs::rename(temporary, queue.join(format!("{name}.json")))?;
            println!("{}", json!({"accepted":true}));
            Ok(())
        }
        "poll" => {
            let after: u64 = option(args, "--after")
                .unwrap_or_else(|_| "0".into())
                .parse()?;
            let page = journal_page(&root, after).await?;
            let status = runner_status(&root);
            let approvals: Value = std::fs::read(root.join("approvals.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_else(|| json!([]));
            println!(
                "{}",
                json!({"runnerId":id,"events":page["events"],"nextSeq":page["nextSeq"],"truncated":page["truncated"],"state":status,"pendingApprovals":approvals})
            );
            Ok(())
        }
        "stop" => {
            if !root.join("STOP").exists() {
                write_private(&root.join("STOP"), b"stop")?;
            }
            println!("{}", json!({"stopRequested":true,"status":"stopping"}));
            Ok(())
        }
        "status" => {
            let status = runner_status(&root);
            println!("{status}");
            Ok(())
        }
        _ => Err("Unsupported runner operation".into()),
    }
}

struct Journal {
    root: PathBuf,
    seq: u64,
}
impl Journal {
    async fn append(&mut self, message: Value) -> Result<()> {
        self.seq += 1;
        let path = self.root.join("events.jsonl");
        if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_JOURNAL) {
            let previous = self.root.join("events.previous.jsonl");
            if previous.exists() {
                std::fs::remove_file(&previous)?;
            }
            std::fs::rename(&path, previous)?;
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options.open(path).await?;
        file.write_all(format!("{}\n", json!({"seq":self.seq,"message":message})).as_bytes())
            .await?;
        file.flush().await?;
        Ok(())
    }
}

struct Process {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    events: VecDeque<Value>,
    seq: u64,
    status: String,
    exit_code: Option<i32>,
    deadline: tokio::time::Instant,
    open_outputs: u8,
}
impl Process {
    fn refresh_status(&mut self) -> Result<()> {
        if self.status == "running" {
            if let Some(status) = self.child.try_wait()? {
                self.exit_code = status.code();
                if self.open_outputs == 0 {
                    self.status = if status.success() {
                        "completed"
                    } else {
                        "failed"
                    }
                    .into();
                }
            }
        }
        Ok(())
    }
}
struct Host {
    root: PathBuf,
    config: Option<Value>,
    secrets: Value,
    processes: HashMap<String, Process>,
    output: mpsc::Sender<(String, &'static str, Vec<u8>)>,
}

impl Host {
    async fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "api.describe" => Ok(json!({"version":1,"methods":[]})),
            "app.info" => Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS,"runner":true}),
            ),
            "log" | "events.publish" => Ok(Value::Null),
            "secrets.get" => Ok(self
                .secrets
                .get(text(params, "key")?)
                .cloned()
                .unwrap_or(Value::Null)),
            "secrets.set" => {
                self.secrets[text(params, "key")?] = params["value"].clone();
                Ok(Value::Null)
            }
            "secrets.delete" => {
                if let Some(secrets) = self.secrets.as_object_mut() {
                    secrets.remove(text(params, "key")?);
                }
                Ok(Value::Null)
            }
            "storage.get" | "storage.set" | "storage.delete" => {
                let key = text(params, "key")?;
                if key.is_empty()
                    || key.len() > 128
                    || !key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                    || ["__proto__", "constructor", "prototype"].contains(&key)
                {
                    return Err("Invalid storage key".into());
                }
                let path = self.root.join("storage.json");
                let mut stored: Value = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_else(|| {
                        self.config
                            .as_ref()
                            .map(|config| json!({"config":config}))
                            .unwrap_or_else(|| json!({}))
                    });
                if method == "storage.get" {
                    return Ok(stored.get(key).cloned().unwrap_or(Value::Null));
                }
                if method == "storage.set" {
                    stored[key] = params["value"].clone();
                } else if let Some(stored) = stored.as_object_mut() {
                    stored.remove(key);
                }
                let bytes = serde_json::to_vec(&stored)?;
                if bytes.len() > 1024 * 1024 {
                    return Err("Storage limit exceeded".into());
                }
                let temporary = self
                    .root
                    .join(format!(".storage-{}.tmp", uuid::Uuid::new_v4()));
                write_private(&temporary, &bytes)?;
                std::fs::rename(temporary, path)?;
                Ok(Value::Null)
            }
            "files.local.home" => Ok(json!(std::env::var(if cfg!(windows) {
                "USERPROFILE"
            } else {
                "HOME"
            })?)),
            "files.local.list" => {
                let path = PathBuf::from(text(params, "path")?);
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(&path)? {
                    let entry = entry?;
                    let metadata = entry.metadata()?;
                    entries.push(json!({"name":entry.file_name().to_string_lossy(),"path":entry.path().to_string_lossy(),"isDir":metadata.is_dir(),"isHidden":entry.file_name().to_string_lossy().starts_with('.'),"size":metadata.len()}));
                    if entries.len() >= 10000 {
                        return Err("Directory contains too many entries".into());
                    }
                }
                Ok(json!({"path":path.to_string_lossy(),"entries":entries}))
            }
            "files.local.readText" => {
                let path = text(params, "path")?;
                let file = tokio::fs::File::open(path).await?;
                let mut bytes = Vec::new();
                file.take(MAX_TEXT + 1).read_to_end(&mut bytes).await?;
                if bytes.len() as u64 > MAX_TEXT || bytes.contains(&0) {
                    return Err("File is too large or binary".into());
                }
                let hash = format!("{:x}", Sha256::digest(&bytes));
                Ok(json!({"size":bytes.len(),"sha256":hash,"content":String::from_utf8(bytes)?}))
            }
            "files.local.writeText" => {
                let content = text(params, "content")?;
                if content.len() as u64 > MAX_TEXT {
                    return Err("File too large".into());
                }
                if !Path::new(text(params, "path")?).is_file() {
                    return Err("File does not exist".into());
                }
                if let Some(expected) = params["expectedHash"].as_str() {
                    let existing = tokio::fs::read(text(params, "path")?).await?;
                    if format!("{:x}", Sha256::digest(existing)) != expected {
                        return Err("File changed since it was read".into());
                    }
                }
                tokio::fs::write(text(params, "path")?, content).await?;
                Ok(Value::Null)
            }
            "files.local.createFile" => {
                write_private(Path::new(text(params, "path")?), b"")?;
                Ok(Value::Null)
            }
            "files.local.mkdir" => {
                tokio::fs::create_dir(text(params, "path")?).await?;
                Ok(Value::Null)
            }
            "files.local.rename" | "files.local.copy" => {
                let from = text(params, "from")?;
                let to = text(params, "to")?;
                if Path::new(to).exists() && params["overwrite"].as_bool() != Some(true) {
                    return Err("Destination exists".into());
                }
                if method.ends_with("rename") {
                    tokio::fs::rename(from, to).await?;
                } else {
                    tokio::fs::copy(from, to).await?;
                }
                Ok(Value::Null)
            }
            "files.local.delete" => {
                let path = text(params, "path")?;
                if params["isDir"].as_bool() == Some(true) {
                    tokio::fs::remove_dir_all(path).await?;
                } else {
                    tokio::fs::remove_file(path).await?;
                }
                Ok(Value::Null)
            }
            "sessions.list" => Ok(json!([])),
            "tasks.list" => Ok(json!(self
                .processes
                .iter()
                .map(
                    |(id, task)| json!({"taskId":id,"status":task.status,"exitCode":task.exit_code})
                )
                .collect::<Vec<_>>())),
            "hosts.snapshot" => Ok(json!({"hosts":[],"groups":[]})),
            "tasks.start" => self.start_process(params).await,
            "tasks.status" | "tasks.readOutput" | "tasks.write" | "tasks.closeInput"
            | "tasks.cancel" => {
                let task = self
                    .processes
                    .get_mut(text(params, "taskId")?)
                    .ok_or("Task not found")?;
                task.refresh_status()?;
                match method {
                    "tasks.status" => Ok(
                        json!({"taskId":params["taskId"],"status":task.status,"exitCode":task.exit_code}),
                    ),
                    "tasks.readOutput" => {
                        let after = params["afterSeq"].as_u64().unwrap_or(0);
                        let events: Vec<_> = task
                            .events
                            .iter()
                            .filter(|event| event["seq"].as_u64().unwrap_or(0) > after)
                            .take(64)
                            .cloned()
                            .collect();
                        let next = events
                            .last()
                            .and_then(|event| event["seq"].as_u64())
                            .unwrap_or(after);
                        Ok(
                            json!({"events":events,"nextSeq":next,"status":task.status,"exitCode":task.exit_code,"truncated":task.events.front().is_some_and(|event| after.saturating_add(1)<event["seq"].as_u64().unwrap_or(0))}),
                        )
                    }
                    "tasks.write" => {
                        let result = bounded_write(
                            task.stdin.as_mut().ok_or("Task input closed")?,
                            text(params, "data")?.as_bytes(),
                        )
                        .await;
                        if result.is_err() {
                            task.stdin.take();
                        }
                        result?;
                        Ok(json!(true))
                    }
                    "tasks.closeInput" => {
                        task.stdin.take();
                        Ok(json!(true))
                    }
                    _ => {
                        terminate(&mut task.child).await;
                        task.status = "cancelled".into();
                        Ok(json!(true))
                    }
                }
            }
            "ui.showMessage" => Ok(Value::Null),
            _ => Err(format!("Unsupported remote host method: {method}").into()),
        }
    }
    async fn start_process(&mut self, params: &Value) -> Result<Value> {
        if self
            .processes
            .values()
            .filter(|task| task.status == "running")
            .count()
            >= 8
        {
            return Err("Too many active tasks".into());
        }
        if self.processes.len() >= 128 {
            let expired = self
                .processes
                .iter()
                .find(|(_, task)| task.status != "running")
                .map(|(id, _)| id.clone());
            if let Some(expired) = expired {
                self.processes.remove(&expired);
            } else {
                return Err("Task limit exceeded".into());
            }
        }
        let executable = text(params, "executable")?;
        let args = params
            .get("args")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let args: Vec<_> = args
            .iter()
            .map(|arg| arg.as_str().ok_or("Invalid argument"))
            .collect::<std::result::Result<_, _>>()?;
        let mut command = tokio::process::Command::new(executable);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        #[cfg(unix)]
        command.process_group(0);
        command
            .args(args)
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_REPL_EXTERNAL_MODULE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = params["cwd"].as_str() {
            command.current_dir(cwd);
        }
        let mut child = command.spawn()?;
        let id = uuid::Uuid::new_v4().to_string();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("Missing stdout")?;
        let stderr = child.stderr.take().ok_or("Missing stderr")?;
        tokio::spawn(pipe(stdout, self.output.clone(), id.clone(), "stdout"));
        tokio::spawn(pipe(stderr, self.output.clone(), id.clone(), "stderr"));
        self.processes.insert(
            id.clone(),
            Process {
                child,
                stdin,
                events: VecDeque::new(),
                seq: 0,
                status: "running".into(),
                exit_code: None,
                deadline: tokio::time::Instant::now()
                    + Duration::from_millis(
                        params["timeoutMs"]
                            .as_u64()
                            .unwrap_or(1_800_000)
                            .clamp(100, 86_400_000),
                    ),
                open_outputs: 2,
            },
        );
        Ok(json!({"taskId":id,"status":"running"}))
    }
}

async fn pipe(
    mut input: impl tokio::io::AsyncRead + Unpin,
    output: mpsc::Sender<(String, &'static str, Vec<u8>)>,
    id: String,
    stream: &'static str,
) {
    let mut bytes = vec![0; 8192];
    let mut pending = Vec::new();
    loop {
        match input.read(&mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(size) => {
                pending.extend_from_slice(&bytes[..size]);
                let end = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(error) if error.error_len().is_none() => error.valid_up_to(),
                    Err(_) => pending.len(),
                };
                if end == 0 {
                    continue;
                }
                let data = pending.drain(..end).collect();
                if output.send((id.clone(), stream, data)).await.is_err() {
                    break;
                }
            }
        }
    }
    if !pending.is_empty() {
        let _ = output.send((id.clone(), stream, pending)).await;
    }
    let _ = output.send((id, stream, Vec::new())).await;
}

fn store_approvals(root: &Path, pending: &HashMap<String, Value>) -> Result<()> {
    let approvals:Vec<_>=pending.iter().map(|(id,request)|json!({"type":"approval","approvalId":id,"method":request["method"],"params":request["params"]})).collect();
    let temporary = root.join(format!(".approvals-{}.tmp", uuid::Uuid::new_v4()));
    write_private(&temporary, serde_json::to_string(&approvals)?.as_bytes())?;
    std::fs::rename(temporary, root.join("approvals.json"))?;
    Ok(())
}

fn canonical_target(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("Absolute path required".into());
    }
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(std::fs::canonicalize(path.parent().ok_or("Invalid path")?)?
                .join(path.file_name().ok_or("Invalid filename")?))
        }
        Err(error) => Err(error.into()),
    }
}

async fn scoped_tool(host: &mut Host, params: &Value, human_approved: bool) -> Result<Value> {
    let method = text(params, "method")?;
    let mode = text(params, "mode")?;
    let allowed_reads = [
        "files.local.home",
        "files.local.list",
        "files.local.readText",
        "tasks.status",
        "tasks.readOutput",
        "tasks.list",
        "sessions.list",
        "hosts.snapshot",
    ];
    let allowed_writes = [
        "files.local.writeText",
        "files.local.createFile",
        "files.local.mkdir",
        "files.local.rename",
        "files.local.copy",
        "files.local.delete",
        "tasks.start",
        "tasks.cancel",
    ];
    if !allowed_reads.contains(&method) && !allowed_writes.contains(&method) {
        return Err("Tool unavailable on remote runner".into());
    }
    let readonly = allowed_reads.contains(&method);
    if !matches!(mode, "ask" | "approve" | "full" | "readOnly") || mode == "readOnly" && !readonly {
        return Err("ReadOnly prohibits this tool".into());
    }
    // Native runner returns a structured approval request, never silently skips policy.
    let approved = human_approved;
    if !(readonly
        || mode == "full"
        || approved
        || mode == "approve"
            && params.pointer("/review/approved").and_then(Value::as_bool) == Some(true))
    {
        return Err("approval_required".into());
    }
    let roots = params
        .pointer("/scope/localRoots")
        .and_then(Value::as_array)
        .ok_or("Local scope is required")?;
    let mut input = params["params"].clone();
    let fields: &[&str] = if method.ends_with(".rename") || method.ends_with(".copy") {
        &["from", "to"]
    } else if method.starts_with("files.local.") && !method.ends_with(".home") {
        &["path"]
    } else if method == "tasks.start" {
        &["cwd"]
    } else {
        &[]
    };
    for field in fields {
        let target = canonical_target(text(&input, field)?)?;
        let allowed = roots
            .iter()
            .filter_map(Value::as_str)
            .any(|root| std::fs::canonicalize(root).is_ok_and(|root| target.starts_with(root)));
        if !allowed {
            return Err("Tool target outside authorized roots".into());
        }
        input[*field] = json!(target.to_string_lossy());
    }
    host.dispatch(method, &input).await
}

async fn terminate(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        #[cfg(windows)]
        {
            let mut cmd = tokio::process::Command::new(
                std::env::var_os("SystemRoot")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| "C:\\Windows".into())
                    .join("System32/taskkill.exe"),
            );
            cmd.creation_flags(0x08000000);
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                cmd.args(["/PID", &pid.to_string(), "/T", "/F"]).output(),
            )
            .await;
        }
        #[cfg(unix)]
        {
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                tokio::process::Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{pid}")])
                    .output(),
            )
            .await;
        }
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
}

async fn serve(args: &[String], root: &Path) -> Result<()> {
    let bootstrap = bootstrap::load(args).await?;
    let (output, mut task_output) = mpsc::channel(64);
    let host = Arc::new(Mutex::new(Host {
        root: root.to_owned(),
        config: bootstrap.configuration_seed,
        secrets: bootstrap.secrets,
        processes: HashMap::new(),
        output,
    }));
    let journal = Arc::new(Mutex::new(Journal {
        root: root.to_owned(),
        seq: 0,
    }));
    let mut command = tokio::process::Command::new(option(args, "--executable")?);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .arg(option(args, "--entry")?)
        .args(bootstrap.child_arguments)
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_REPL_EXTERNAL_MODULE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = Arc::new(Mutex::new(
        child.stdin.take().ok_or("Missing plugin stdin")?,
    ));
    let mut stdout = BufReader::new(child.stdout.take().ok_or("Missing plugin stdout")?);
    // Read whole records outside select: cancelling bounded_line would discard consumed fragments.
    let (plugin_lines, mut plugin_output) = mpsc::channel(4);
    let output_reader = tokio::spawn(async move {
        loop {
            let line = bounded_line(&mut stdout, 1024 * 1024).await;
            let finished = !matches!(&line, Ok(Some(_)));
            if plugin_lines.send(line).await.is_err() || finished {
                break;
            }
        }
    });
    let initialize =
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":bootstrap.initialize});
    bounded_write(
        &mut *stdin.lock().await,
        format!("{initialize}\n").as_bytes(),
    )
    .await?;
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let result:Result<()>=async {
        loop {tokio::select!{
            line=plugin_output.recv()=>{
                let Some(line)=line else{return Err("Plugin output reader stopped unexpectedly".into());};
                let Some(line)=line? else{if root.join("STOP").exists(){break;}return Err("Plugin exited unexpectedly (stdout closed)".into());};
                let message=serde_json::from_str::<Value>(&line).map_err(|_|"Plugin emitted invalid protocol JSON")?;
                if let Some(method)=message["method"].as_str(){
                    let params=message.get("params").cloned().unwrap_or_else(||json!({}));
                    if method=="events.publish" {journal.lock().await.append(json!({"type":"event","event":params["event"],"payload":params["payload"]})).await?;}
                    let response=if method=="ui.confirm" {Err("approval_required".into())}else if method=="tools.invoke"{scoped_tool(&mut *host.lock().await,&params,false).await}else{host.lock().await.dispatch(method,&params).await};
                    if response.as_ref().is_err_and(|error|error.to_string()=="approval_required"){
                        if pending.len()>=128{return Err("Too many pending approvals".into());}
                        let approval_id=uuid::Uuid::new_v4().to_string();pending.insert(approval_id.clone(),message.clone());store_approvals(root,&pending)?;
                        journal.lock().await.append(json!({"type":"approval","approvalId":approval_id,"method":method,"params":params})).await?;
                    }else if let Some(id)=message.get("id"){
                        let reply=match response{Ok(result)=>json!({"jsonrpc":"2.0","id":id,"result":result}),Err(error)=>json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":error.to_string()}})};
                        bounded_write(&mut *stdin.lock().await,format!("{reply}\n").as_bytes()).await?;
                    }
                }else if message.get("id").is_some(){journal.lock().await.append(json!({"type":"response","message":message})).await?;}
            },
            task=task_output.recv()=>if let Some((id,stream,bytes))=task{
                if let Some(task)=host.lock().await.processes.get_mut(&id){if bytes.is_empty(){task.open_outputs=task.open_outputs.saturating_sub(1);task.refresh_status()?;}else{task.seq+=1;task.events.push_back(json!({"taskId":id,"seq":task.seq,"stream":stream,"data":String::from_utf8_lossy(&bytes),"status":task.status,"exitCode":task.exit_code}));while task.events.len()>512{task.events.pop_front();}}}
            },
            _=interval.tick()=>{
                if root.join("STOP").exists(){break;}
                if let Some(status)=child.try_wait()?{return Err(format!("Plugin exited unexpectedly ({status})").into());}
                tokio::fs::write(root.join("heartbeat"),b"running").await?;
                for task in host.lock().await.processes.values_mut(){task.refresh_status()?;if task.status=="running"&&tokio::time::Instant::now()>task.deadline{terminate(&mut task.child).await;task.status="timedOut".into();}}
                let mut files:Vec<_>=std::fs::read_dir(root.join("inbox"))?.filter_map(|entry|entry.ok()).filter(|entry|entry.path().extension().is_some_and(|extension|extension=="json")).collect();files.sort_by_key(|entry|entry.file_name());
                for file in files.into_iter().take(16){
                    let message:Value=serde_json::from_slice(&std::fs::read(file.path())?)?;std::fs::remove_file(file.path())?;
                    if message["type"]=="approval"{
                        let approval_id=text(&message,"approvalId")?;
                        if let Some(request)=pending.remove(approval_id){
                            store_approvals(root,&pending)?;
                            let approved=message["approved"].as_bool()==Some(true);
                            let result=if request["method"]=="ui.confirm"{Ok(json!(approved))}else if approved{scoped_tool(&mut *host.lock().await,&request["params"],true).await}else{Err("Operation denied".into())};
                            let reply=match result{Ok(result)=>json!({"jsonrpc":"2.0","id":request["id"],"result":result}),Err(error)=>json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":error.to_string()}})};
                            bounded_write(&mut *stdin.lock().await,format!("{reply}\n").as_bytes()).await?;
                        }
                    }else{bounded_write(&mut *stdin.lock().await,format!("{message}\n").as_bytes()).await?;}
                }
            }
        }}Ok(())
    }.await;
    output_reader.abort();
    let _ = output_reader.await;
    terminate(&mut child).await;
    for process in host.lock().await.processes.values_mut() {
        terminate(&mut process.child).await;
    }
    let status = json!({"status":if result.is_ok(){"stopped"}else{"failed"},"error":result.as_ref().err().map(ToString::to_string)});
    write_private(
        &root.join("status.json"),
        serde_json::to_string(&status)?.as_bytes(),
    )?;
    journal
        .lock()
        .await
        .append(json!({"type":"state","state":status}))
        .await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_identifiers_never_become_paths() {
        for id in ["../x", "a/b", "a\\b", "", "."] {
            assert!(!id_valid(id));
        }
        assert!(id_valid("abc-123"));
    }
    #[test]
    fn paths_must_be_absolute() {
        assert!(canonical_target("relative/file").is_err());
    }
    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("tunnelde-runner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn record(seq: u64, size: usize) -> String {
        format!(
            "{}\n",
            json!({"seq":seq,"message":{"data":"x".repeat(size)}})
        )
    }
    #[tokio::test]
    async fn pagination_never_skips_remaining_events_in_previous_segment() {
        let directory = TestDirectory::new();
        std::fs::write(
            directory.0.join("events.previous.jsonl"),
            record(1, 400 * 1024) + &record(2, 200 * 1024),
        )
        .unwrap();
        std::fs::write(directory.0.join("events.jsonl"), record(3, 20)).unwrap();
        let first = journal_page(&directory.0, 0).await.unwrap();
        assert_eq!(first["events"].as_array().unwrap().len(), 1);
        assert_eq!(first["nextSeq"], 1);
        let second = journal_page(&directory.0, 1).await.unwrap();
        assert_eq!(
            second["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|event| event["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(second["nextSeq"], 3);
    }
    #[tokio::test]
    async fn unfinished_append_never_advances_cursor_to_later_segment() {
        let directory = TestDirectory::new();
        std::fs::write(
            directory.0.join("events.previous.jsonl"),
            record(1, 1).trim_end(),
        )
        .unwrap();
        std::fs::write(directory.0.join("events.jsonl"), record(2, 1)).unwrap();
        let page = journal_page(&directory.0, 0).await.unwrap();
        assert!(page["events"].as_array().unwrap().is_empty());
        assert_eq!(page["nextSeq"], 0);
        std::fs::write(directory.0.join("events.previous.jsonl"), record(1, 1)).unwrap();
        assert_eq!(journal_page(&directory.0, 0).await.unwrap()["nextSeq"], 2);
    }
    #[tokio::test]
    async fn invalid_complete_event_is_reported_without_skipping() {
        let directory = TestDirectory::new();
        std::fs::write(
            directory.0.join("events.jsonl"),
            "invalid\n".to_owned() + &record(2, 1),
        )
        .unwrap();
        assert!(journal_page(&directory.0, 0).await.is_err());
    }
    #[test]
    fn status_checks_existence_heartbeat_and_terminal_record() {
        let directory = TestDirectory::new();
        assert_eq!(
            runner_status(&directory.0.join("missing"))["status"],
            "notFound"
        );
        assert_eq!(runner_status(&directory.0)["status"], "unresponsive");
        std::fs::write(directory.0.join("pid"), "123").unwrap();
        assert_eq!(runner_status(&directory.0)["status"], "starting");
        std::fs::write(directory.0.join("heartbeat"), "running").unwrap();
        assert_eq!(runner_status(&directory.0)["status"], "running");
        std::fs::write(directory.0.join("status.json"), r#"{"status":"failed"}"#).unwrap();
        assert_eq!(runner_status(&directory.0)["status"], "failed");
    }
    #[tokio::test]
    async fn blocked_stdin_write_returns_before_control_loop_stalls() {
        let (mut writer, _reader) = tokio::io::duplex(1);
        let started = tokio::time::Instant::now();
        let error = bounded_write(&mut writer, &[0; 4096]).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[tokio::test]
    async fn task_completion_waits_for_both_output_streams_and_keeps_cancellation() {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().unwrap();
        assert!(child.wait().await.unwrap().success());
        let mut task = Process {
            child,
            stdin: None,
            events: VecDeque::new(),
            seq: 0,
            status: "running".into(),
            exit_code: None,
            deadline: tokio::time::Instant::now() + Duration::from_secs(5),
            open_outputs: 2,
        };
        task.refresh_status().unwrap();
        assert_eq!(task.status, "running");
        task.open_outputs = 0;
        task.refresh_status().unwrap();
        assert_eq!(task.status, "completed");
        task.status = "cancelled".into();
        task.refresh_status().unwrap();
        assert_eq!(task.status, "cancelled");
    }
}
