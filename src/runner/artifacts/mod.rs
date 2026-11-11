//! Metadata only. Never interprets process state, reads payloads or grants cleanup.
mod contracts;
mod observe;
use crate::{
    runner::{admission::Observation, filesystem::Directory},
    Result,
};
use contracts::{Reason, Receipt, Snapshot, MAX_OUTPUT};
use std::{collections::HashSet, io::Write, path::Path};

fn nonce(value: &str) -> Result<()> {
    if uuid::Uuid::parse_str(value)
        .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == value)
    {
        Ok(())
    } else {
        Err("Invalid runner artifact observation identity".into())
    }
}

pub(super) async fn inspect(
    args: &[String],
    directory: &Path,
    id: &str,
    root: &Path,
) -> Result<()> {
    let pairs = &args[1..];
    let mut seen = HashSet::new();
    if pairs.len() != 8
        || !pairs.as_chunks::<2>().0.iter().all(|pair| {
            matches!(
                pair[0].as_str(),
                "--directory" | "--id" | "--admission" | "--observation"
            ) && seen.insert(pair[0].as_str())
        })
    {
        return Err("Invalid runner artifact observation arguments".into());
    }
    let token = super::option(args, "--admission")?;
    let observation_id = super::option(args, "--observation")?;
    nonce(&token)?;
    nonce(&observation_id)?;
    let mut admission = None;
    let snapshot = match Directory::open(directory) {
        Err(_) => Snapshot::Unavailable {
            reason: Reason::UnsafeDirectory,
        },
        Ok(parent) => match Observation::open(directory, id, &token).await {
            Err(_) => Snapshot::Unavailable {
                reason: Reason::AdmissionUnavailable,
            },
            Ok(guard) => {
                admission = Some(guard.state());
                let result = observe::snapshot(root);
                if parent.validate().is_err() || guard.validate(id).is_err() {
                    Snapshot::Unavailable {
                        reason: Reason::NamespaceChanged,
                    }
                } else {
                    result.unwrap_or_else(|reason| Snapshot::Unavailable { reason })
                }
            }
        },
    };
    let receipt = Receipt {
        schema_version: 1,
        runner_id: id,
        token: &token,
        observation_id: &observation_id,
        admission,
        consistency: "pointInTime",
        cleanup_authorized: false,
        snapshot,
    };
    let mut bytes = serde_json::to_vec(&receipt)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_OUTPUT {
        return Err("Runner artifact observation exceeds limit".into());
    }
    let mut output = std::io::stdout().lock();
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}
