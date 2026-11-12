//! Durable admission only. No execution, file mutation, cleanup or replay path.
mod contracts;
mod namespace;
mod store;

use contracts::{Request, MAX_REQUEST};
use std::io::{Read, Write};
use std::path::Path;

pub(crate) async fn run(arguments: &[String]) -> crate::Result<()> {
    if arguments.len() != 3
        || arguments[1] != "--directory"
        || !Path::new(&arguments[2]).is_absolute()
    {
        return Err("files.admission.invalidArguments".into());
    }
    // One bounded packet, completed by EOF. A caller withholding EOF cannot hold
    // a filesystem lock; even malformed packets are rejected before opening it.
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    // Tokio stdin uses an uncancellable blocking read, which can prevent runtime
    // shutdown after a timeout. An independent bounded reader does not.
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::io::stdin()
            .take(MAX_REQUEST + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let bytes = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "files.admission.inputTimeout")?
        .map_err(|_| "files.admission.invalidRequest")?;
    if bytes.len() as u64 > MAX_REQUEST {
        return Err("files.admission.invalidRequest".into());
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|_| "files.admission.invalidRequest")?;
    request.validate()?;
    let mut store = store::Store::open(
        Path::new(&arguments[2]),
        matches!(request, Request::Initialize { .. }),
    )?;
    let response = store.apply(&request)?;
    let mut bytes = serde_json::to_vec(&response).map_err(|_| "files.admission.invalidResponse")?;
    bytes.push(b'\n');
    if bytes.len() > 4096 {
        return Err("files.admission.invalidResponse".into());
    }
    // A lost acknowledgement is resolved by status in a fresh process. Never
    // roll back a committed admission because writing stdout failed.
    let mut output = std::io::stdout().lock();
    output
        .write_all(&bytes)
        .map_err(|_| "files.admission.responseUnavailable")?;
    output
        .flush()
        .map_err(|_| "files.admission.responseUnavailable")?;
    Ok(())
}
