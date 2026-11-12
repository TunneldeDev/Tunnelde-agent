//! One lease, one durable admission to publication, no replay of running work.
use super::{
    contracts::{Baseline, Control, Phase},
    filesystem, lease,
    store::Store,
};
use crate::Result;
use std::path::Path;

pub(super) fn execute(directory: &Path, control: &Control) -> Result<()> {
    execute_checkpoints(directory, control, |_| Ok(()))
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Checkpoint {
    Running,
    Staged,
    Publishing,
    NativePublished,
    Published,
}
fn execute_checkpoints(
    directory: &Path,
    control: &Control,
    mut checkpoint: impl FnMut(Checkpoint) -> Result<()>,
) -> Result<()> {
    let storage = filesystem::Storage::open(directory)?;
    let mut store = Store::open(directory, None)?;
    let record = store.get(control)?.ok_or("files.save.missing")?;
    if record.phase != Phase::Prepared {
        return Ok(());
    }
    let authority = store.authority.clone();
    if storage.identity != authority.storage_root {
        return Err("files.save.storageChanged".into());
    }
    // The private root/global DB lock is released before waiting for any lease.
    drop(store);
    let attempt =
        lease::acquire(directory, &control.operation_id, true)?.ok_or("files.save.unsafeLease")?;
    let Some(_lease) = attempt.held else {
        return Ok(());
    };
    store = Store::open(directory, None)?;
    let record = store.get(control)?.ok_or("files.save.missing")?;
    if record.phase != Phase::Prepared {
        return Ok(());
    }
    let record = store.change(control, |r| {
        if r.phase != Phase::Prepared {
            return Err("files.save.notPrepared".into());
        }
        r.phase = Phase::Running;
        r.lease = Some(attempt.identity.clone());
        Ok(())
    })?;
    let intent = record.intent.ok_or("files.save.corrupt")?;
    let content = store.payload(control)?;
    drop(store);
    checkpoint(Checkpoint::Running)?;
    let prepared: Result<_> = (|| {
        if super::contracts::sha256(&content) != intent.payload_sha256
            || content.len() as u64 != intent.payload_length
        {
            return Err("files.save.corrupt".into());
        }
        let scope = filesystem::Scope::open(&authority.context)?;
        if scope.identity != authority.root {
            return Err("files.save.scopeChanged".into());
        }
        let target = scope.target(&intent.target)?;
        let original = target.baseline(&intent.baseline)?;
        let name = filesystem::stage_name(&control.operation_id);
        let mut stage = target.stage(&storage, &name)?;
        let identity = filesystem::identity(&stage)?;
        let mut store = Store::open(directory, None)?;
        // Even cancellation must retain ownership of an already created stage.
        store.change(control, |r| {
            if !matches!(r.phase, Phase::Running | Phase::Cancelled) || r.artifact.is_some() {
                return Err("files.save.notPrepared".into());
            }
            r.artifact = Some(identity);
            Ok(())
        })?;
        drop(store);
        filesystem::write_stage(&mut stage, &content, original.as_ref())?;
        checkpoint(Checkpoint::Staged)?;
        // Recheck immediately before admission, while retaining all scope handles.
        target.baseline(&intent.baseline)?;
        let current = filesystem::Scope::open(&authority.context)?;
        if current.identity != authority.root {
            return Err("files.save.scopeChanged".into());
        }
        Ok((scope, target, name, stage))
    })();
    let (_scope, target, name, stage) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let mut store = Store::open(directory, None)?;
            store.change(control, |r| {
                if r.phase == Phase::Running {
                    r.phase = Phase::Failed;
                    r.error = Some(safe_error(&*error));
                }
                Ok(())
            })?;
            return Ok(());
        }
    };
    let mut store = Store::open(directory, None)?;
    let record = store.get(control)?.ok_or("files.save.missing")?;
    if record.phase != Phase::Running {
        return Ok(());
    }
    store.change(control, |r| {
        if r.phase != Phase::Running {
            return Err("files.save.notPrepared".into());
        }
        r.phase = Phase::Publishing;
        Ok(())
    })?;
    drop(store);
    checkpoint(Checkpoint::Publishing)?;
    // Any failure from this point is ambiguous, including a failed directory
    // fsync or metadata merge. Preserve artifacts and never claim cancellation.
    let replace = matches!(intent.baseline, Baseline::Existing { .. });
    let staged_identity = filesystem::identity(&stage)?;
    // Windows ReplaceFile opens the replacement without sharing. A retained
    // staging handle would itself cause ERROR_SHARING_VIOLATION.
    drop(stage);
    let publication: Result<_> = (|| {
        target.publish(&storage, &name, replace)?;
        checkpoint(Checkpoint::NativePublished)?;
        let backup = replace.then(|| filesystem::backup_name(&control.operation_id));
        target.flush_published(&staged_identity, &storage, backup.as_deref())?;
        checkpoint(Checkpoint::Published)?;
        if replace {
            let backup =
                filesystem::artifact(&storage, &filesystem::backup_name(&control.operation_id))?;
            let same = filesystem::matches_baseline(&backup, &intent.baseline)?;
            Ok((
                if same {
                    Phase::Succeeded
                } else {
                    Phase::Conflict
                },
                Some(filesystem::identity(&backup)?),
            ))
        } else {
            Ok((Phase::Succeeded, None))
        }
    })();
    let mut store = Store::open(directory, None)?;
    store.change(control, |r| {
        if r.phase != Phase::Publishing {
            return Err("files.save.notPublishing".into());
        }
        match publication {
            Ok((phase, artifact)) => {
                r.phase = phase;
                r.artifact = artifact;
                r.published = true;
                r.backup_retained = replace;
                if phase == Phase::Conflict {
                    r.error = Some("files.save.displacedBaselineChanged".into());
                }
            }
            Err(error) => {
                r.phase = Phase::Unknown;
                r.error = Some(safe_error(&*error));
            }
        }
        Ok(())
    })?;
    // Terminal receipt is durable before optional payload reclamation.
    let _ = store.forget_payload(control);
    Ok(())
}

#[cfg(test)]
mod tests;
fn safe_error(error: &(dyn std::error::Error + Send + Sync)) -> String {
    let value = error.to_string();
    if value.starts_with("files.save.")
        && value.len() < 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.')
    {
        value
    } else {
        "files.save.executionFailed".into()
    }
}
