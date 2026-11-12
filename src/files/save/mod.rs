//! Experimental file execution. Not exposed until the platform and recovery
//! acceptance suite passes; admission remains a separate wire protocol.
mod contracts;
mod executor;
mod filesystem;
mod launch;
mod lease;
mod lifecycle;
mod store;

use contracts::{Request, Response, MAX_PACKET, PROTOCOL};
use std::{
    io::{Read, Write},
    path::Path,
};

pub(crate) async fn run(arguments: &[String]) -> crate::Result<()> {
    let worker = arguments.len() == 4 && arguments[3] == "--worker";
    if (!worker && arguments.len() != 3)
        || arguments[1] != "--directory"
        || !Path::new(&arguments[2]).is_absolute()
    {
        return Err("files.save.invalidArguments".into());
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::io::stdin()
            .take(MAX_PACKET + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let bytes = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "files.save.inputTimeout")?
        .map_err(|_| "files.save.invalidRequest")?;
    if bytes.len() as u64 > MAX_PACKET {
        return Err("files.save.invalidRequest".into());
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|_| "files.save.invalidRequest")?;
    request.validate()?;
    let directory = Path::new(&arguments[2]);
    if worker {
        let Request::Submit { control, .. } = &request else {
            return Err("files.save.invalidRequest".into());
        };
        // This worker never initializes authority or prepares an unadmitted save.
        // An absolute lifetime bounds stalled native IO; kernel lease loss is
        // interpreted by status, never converted into an invented success.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(300));
            std::process::exit(1);
        });
        return executor::execute(directory, control);
    }
    let initialize = match &request {
        Request::Initialize { context, .. } => Some(context),
        _ => None,
    };
    let mut store = store::Store::open(directory, initialize)?;
    let authority = store.authority.clone();
    let mut launch = false;
    match &request {
        Request::Submit {
            control,
            intent,
            content,
        } => launch = lifecycle::may_launch(&store.submit(control, intent, content.as_bytes())?),
        Request::Cancel { control } => {
            store.cancel(control)?;
        }
        _ => (),
    }
    drop(store);
    if let Request::Release {
        control,
        revision,
        discard_backup,
    } = &request
    {
        lifecycle::release(directory, control, *revision, *discard_backup)?;
    }
    if launch {
        launch::start(directory, &bytes).await?;
    }
    let observation = match request.control() {
        Some(control) => lifecycle::observe(directory, control)?,
        None => None,
    };
    let response = Response {
        schema_version: 1,
        protocol: PROTOCOL,
        binding_sha256: authority.binding()?,
        authority,
        observation,
        cleanup_authorized: false,
    };
    let mut output = serde_json::to_vec(&response).map_err(|_| "files.save.invalidResponse")?;
    output.push(b'\n');
    if output.len() > 32768 {
        return Err("files.save.invalidResponse".into());
    }
    std::io::stdout()
        .lock()
        .write_all(&output)
        .map_err(|_| "files.save.responseUnavailable")?;
    Ok(())
}
