//! Status excludes the original executor before interpreting an interruption.
//! It never starts a worker, replays a write or cleans an artifact.
use super::{
    contracts::{Control, Observation, Phase},
    filesystem, lease,
    store::{Record, Store},
};
use crate::Result;
use std::path::Path;

pub(super) fn observe(directory: &Path, control: &Control) -> Result<Option<Observation>> {
    let mut store = Store::open(directory, None)?;
    let Some(mut record) = store.get(control)? else {
        return Ok(None);
    };
    let attempt = lease::acquire(directory, &control.operation_id, false)?;
    let (live, excluded) = match &attempt {
        Some(attempt)
            if record
                .lease
                .as_ref()
                .is_none_or(|id| *id == attempt.identity) =>
        {
            (Some(attempt.held.is_none()), attempt.held.is_some())
        }
        None if record.lease.is_none() => (Some(false), true),
        _ => (None, false),
    };
    if excluded
        && matches!(
            record.phase,
            Phase::Running | Phase::Publishing | Phase::Releasing
        )
    {
        record = store.change(control, |r| {
            if r.phase == Phase::Running {
                r.phase = Phase::Failed;
                r.error = Some("files.save.executorInterrupted".into());
            } else if matches!(r.phase, Phase::Publishing | Phase::Releasing) {
                r.error = Some(
                    if r.phase == Phase::Releasing {
                        "files.save.cleanupUnknown"
                    } else {
                        "files.save.publicationUnknown"
                    }
                    .into(),
                );
                r.phase = Phase::Unknown;
            }
            Ok(())
        })?;
    }
    let unknown_lease = live.is_none() && record.phase.active() && record.phase != Phase::Prepared;
    Ok(Some(Observation {
        operation_id: record.operation_id,
        generation: record.generation,
        intent_sha256: record.intent_sha256,
        revision: record.revision,
        phase: if unknown_lease {
            Phase::Unknown
        } else {
            record.phase
        },
        live,
        cancellable: matches!(record.phase, Phase::Prepared | Phase::Running) && !unknown_lease,
        publication: if record.published {
            "published"
        } else if matches!(record.phase, Phase::Publishing | Phase::Unknown) || unknown_lease {
            "unknown"
        } else {
            "none"
        },
        backup_retained: record.backup_retained,
        error: record.error,
    }))
}
pub(super) fn release(
    directory: &Path,
    control: &Control,
    revision: u64,
    discard_backup: bool,
) -> Result<()> {
    let storage = filesystem::Storage::open(directory)?;
    let mut store = Store::open(directory, None)?;
    let record = store.get(control)?.ok_or("files.save.missing")?;
    if storage.identity != store.authority.storage_root {
        return Err("files.save.storageChanged".into());
    }
    if record.phase == Phase::Released && record.revision == revision {
        return Ok(());
    }
    if record.revision != revision
        || !record.phase.terminal_known()
        || (record.backup_retained && !discard_backup)
    {
        return Err("files.save.notReleasable".into());
    }
    let attempt = lease::acquire(directory, &control.operation_id, false)?;
    match &attempt {
        Some(attempt)
            if attempt.held.is_some()
                && record
                    .lease
                    .as_ref()
                    .is_none_or(|id| *id == attempt.identity) => {}
        None if record.lease.is_none() && record.artifact.is_none() => (),
        _ => return Err("files.save.executorNotExcluded".into()),
    }
    // Only the explicit revision-bound release operation can delete an owned
    // artifact. Persist that authorization before even a single unlink.
    store.change(control, |r| {
        r.phase = Phase::Releasing;
        Ok(())
    })?;
    drop(store);
    if let Some(identity) = &record.artifact {
        let name = if record.backup_retained {
            filesystem::backup_name(&record.operation_id)
        } else {
            filesystem::stage_name(&record.operation_id)
        };
        filesystem::release_artifact(&storage, &name, identity)?;
    }
    store = Store::open(directory, None)?;
    store.change(control, |r| {
        if r.phase != Phase::Releasing {
            return Err("files.save.notReleasable".into());
        }
        r.phase = Phase::Released;
        r.artifact = None;
        r.backup_retained = false;
        Ok(())
    })?;
    store.forget_payload(control)?;
    Ok(())
}

pub(super) fn may_launch(record: &Record) -> bool {
    record.phase == Phase::Prepared
}
