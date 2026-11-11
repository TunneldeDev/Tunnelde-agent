//! Task admission and process status are independent. No artifact deletion here.
mod contracts;
mod journal;
mod locking;
mod observation;
pub(super) use observation::Observation;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

use crate::Result;
use contracts::State;
use journal::Journal;
use locking::Lock;
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

pub(super) fn token(args: &[String]) -> Result<String> {
    let token = super::option(args, "--admission")?;
    contracts::token(&token)?;
    Ok(token)
}

fn status(root: &Path) -> Result<String> {
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("notFound".into()),
        Err(error) => return Err(error.into()),
        Ok(_) => {
            locking::metadata(root, true)?;
        }
    }
    match std::fs::File::open(root.join("status.json")) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(4097).read_to_end(&mut bytes)?;
            if bytes.len() > 4096 {
                return Err("Runner process state exceeds limit".into());
            }
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid runner process state")?;
            return match value["status"].as_str() {
                Some(state @ ("stopped" | "failed")) => Ok(state.into()),
                _ => Err("Invalid runner process state".into()),
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let recent = |name| {
        std::fs::metadata(root.join(name))
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed < std::time::Duration::from_secs(15))
    };
    Ok(if recent("heartbeat") {
        "running"
    } else if !root.join("heartbeat").try_exists()? && recent("pid") {
        "starting"
    } else {
        "unresponsive"
    }
    .into())
}

fn receipt(id: &str, token: &str, state: State, status: String) -> Value {
    json!({"schemaVersion":1,"runnerId":id,"token":token,"admission":state,"status":status})
}

pub(super) async fn control(
    args: &[String],
    directory: &Path,
    id: &str,
    root: &Path,
) -> Result<()> {
    let pairs = args.get(1..).ok_or("Invalid admission arguments")?;
    let mut seen = std::collections::HashSet::new();
    if pairs.len() != 6
        || !pairs.as_chunks::<2>().0.iter().all(|pair| {
            matches!(pair[0].as_str(), "--directory" | "--id" | "--admission")
                && seen.insert(pair[0].as_str())
        })
    {
        return Err("Invalid admission arguments".into());
    }
    let token = token(args)?;
    let operation = args[0].as_str();
    let lock = Lock::acquire(directory, operation != "status-fenced").await?;
    let mut journal = Journal::open(&lock, id, &token, operation != "status-fenced")?;
    match operation {
        "reserve-fenced" => {
            if journal.state.is_none() {
                if status(root)? != "notFound" {
                    return Err("Runner ID already exists".into());
                }
                journal.append(id, &token, State::Reserved)?;
            }
        }
        "stop-fenced" => {
            if journal.state != Some(State::Cancelled) {
                journal.append(id, &token, State::Cancelled)?;
            }
            // A crash between cancellation and STOP must remain recoverable.
            // status-fenced never pretends that cancellation alone means exit.
            if status(root)? != "notFound" && !root.join("STOP").try_exists()? {
                super::write_private(&root.join("STOP"), b"stop")?;
            }
        }
        "status-fenced" => {}
        _ => return Err("Unknown runner admission operation".into()),
    }
    lock.validate()?;
    let state = journal.state.ok_or("Incomplete runner admission journal")?;
    println!("{}", receipt(id, &token, state, status(root)?));
    Ok(())
}

/// Held through creation/spawn/bootstrap handoff. Start never creates admission.
pub(super) struct Lease {
    _lock: Lock,
}

pub(super) async fn launch(directory: &Path, id: &str, token: &str) -> Result<Lease> {
    contracts::token(token)?;
    let lock = Lock::acquire(directory, false).await?;
    let mut journal = Journal::open(&lock, id, token, false)?;
    if journal.state != Some(State::Reserved) {
        return Err("Runner admission is cancelled or already used".into());
    }
    journal.append(id, token, State::Launched)?;
    lock.validate()?;
    Ok(Lease { _lock: lock })
}
