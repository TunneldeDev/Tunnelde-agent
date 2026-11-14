use super::*;
use std::os::macos::fs::MetadataExt as _;
use std::{os::fd::AsRawFd, time::UNIX_EPOCH};

/// Distinct old timestamps prevent a same-second fixture from hiding STAT's
/// effect on the stage birth time used by the durable ownership record.
pub(super) fn age_original(f: &Fixture) {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.scope.join("target"))
        .unwrap();
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_CRTIME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut created = libc::timespec {
        tv_sec: 946684800,
        tv_nsec: 123456789,
    };
    // SAFETY: live fixture fd, initialized attribute bitmap and matching timespec.
    assert_eq!(
        unsafe {
            libc::fsetattrlist(
                file.as_raw_fd(),
                (&mut attributes as *mut libc::attrlist).cast(),
                (&mut created as *mut libc::timespec).cast(),
                std::mem::size_of_val(&created),
                0,
            )
        },
        0
    );
    file.set_modified(UNIX_EPOCH + Duration::from_secs(946684801))
        .unwrap();
    assert_eq!(
        file.metadata().unwrap().created().unwrap(),
        UNIX_EPOCH + Duration::new(946684800, 123456789)
    );
}

#[test]
fn existing_failure_after_metadata_releases_original_full_identity() {
    let f = Fixture::new();
    let request: Request = serde_json::from_value(f.prepare(true)).unwrap();
    age_original(&f);
    let control = request.control().unwrap();
    execute_checkpoints(&f.storage, control, |point| {
        if point == Checkpoint::Staged {
            return Err("files.save.fixtureFailure".into());
        }
        Ok(())
    })
    .unwrap();
    let observed = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observed.phase == Phase::Failed);
    assert_eq!(observed.error.as_deref(), Some("files.save.fixtureFailure"));
    assert_eq!(observed.live, Some(false));
    lifecycle::release(&f.storage, control, observed.revision, false).unwrap();
    assert_eq!(fs::read(f.scope.join("target")).unwrap(), b"old");
    assert!(!f
        .storage
        .join(filesystem::stage_name(&control.operation_id))
        .exists());
}

#[test]
fn unsupported_source_flags_fail_before_publication_and_stage_still_releases() {
    struct ResetFlags(fs::File);
    impl Drop for ResetFlags {
        fn drop(&mut self) {
            // SAFETY: this guard owns the fixture descriptor and resets only its flags.
            assert_eq!(unsafe { libc::fchflags(self.0.as_raw_fd(), 0) }, 0);
        }
    }
    let f = Fixture::new();
    let request: Request = serde_json::from_value(f.prepare(true)).unwrap();
    age_original(&f);
    let original = ResetFlags(fs::File::open(f.scope.join("target")).unwrap());
    // SAFETY: only the new fixture is marked append-only; the guard restores it.
    assert_eq!(
        unsafe { libc::fchflags(original.0.as_raw_fd(), libc::UF_APPEND) },
        0
    );
    let control = request.control().unwrap();
    execute(&f.storage, control).unwrap();
    let observed = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observed.phase == Phase::Failed);
    assert_eq!(
        observed.error.as_deref(),
        Some("files.save.metadataUnsupported")
    );
    assert_eq!(observed.publication, "none");
    lifecycle::release(&f.storage, control, observed.revision, false).unwrap();
    assert!(!f
        .storage
        .join(filesystem::stage_name(&control.operation_id))
        .exists());
    assert_eq!(fs::read(f.scope.join("target")).unwrap(), b"old");
}

#[test]
fn cancelled_existing_stage_replacement_is_never_deleted() {
    let f = Fixture::new();
    let request: Request = serde_json::from_value(f.prepare(true)).unwrap();
    age_original(&f);
    let control = request.control().unwrap();
    execute_checkpoints(&f.storage, control, |point| {
        if point == Checkpoint::Staged {
            Store::open(&f.storage, None)
                .unwrap()
                .cancel(control)
                .unwrap();
        }
        Ok(())
    })
    .unwrap();
    let observed = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    let stage = f
        .storage
        .join(filesystem::stage_name(&control.operation_id));
    let retained = f.root.join("retained-original-stage");
    fs::rename(&stage, &retained).unwrap();
    fs::write(&stage, b"unrelated").unwrap();
    let error = lifecycle::release(&f.storage, control, observed.revision, false).unwrap_err();
    assert_eq!(error.to_string(), "files.save.artifactChanged");
    assert_eq!(fs::read(&stage).unwrap(), b"unrelated");
    assert_eq!(fs::read(&retained).unwrap(), b"new");
    assert_eq!(fs::read(f.scope.join("target")).unwrap(), b"old");
    let after = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(after.phase == Phase::Unknown);
    assert_eq!(after.error.as_deref(), Some("files.save.cleanupUnknown"));
}

fn acl(path: &std::path::Path) -> String {
    let output = Command::new("/bin/ls")
        .arg("-led")
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn existing_publication_preserves_permissions_acl_flags_and_bounded_xattrs() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use xattr::FileExt;
    let f = Fixture::new();
    let request: Request = serde_json::from_value(f.prepare(true)).unwrap();
    age_original(&f);
    let path = f.scope.join("target");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let source = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    source
        .set_xattr("user.tunnelde", b"editor metadata")
        .unwrap();
    source
        .set_xattr("com.apple.ResourceFork", b"small fork")
        .unwrap();
    let flags = libc::UF_NODUMP | libc::UF_HIDDEN | libc::UF_TRACKED;
    // SAFETY: owned fixture descriptor and supported owner-settable flags.
    assert_eq!(unsafe { libc::fchflags(source.as_raw_fd(), flags) }, 0);
    assert!(Command::new("/bin/chmod")
        .args([
            "+a",
            "everyone allow read,readattr,readextattr,readsecurity"
        ])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let original_acl = acl(&path);
    assert!(original_acl.contains("everyone allow"));
    let original = source.metadata().unwrap();
    let control = request.control().unwrap();
    execute(&f.storage, control).unwrap();
    let observed = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observed.phase == Phase::Succeeded);
    let published = fs::File::open(&path).unwrap();
    let md = published.metadata().unwrap();
    assert_eq!(
        (md.uid(), md.gid(), md.mode()),
        (original.uid(), original.gid(), original.mode())
    );
    assert_eq!(md.st_flags(), flags);
    assert_eq!(acl(&path), original_acl);
    assert_eq!(
        published.get_xattr("user.tunnelde").unwrap().unwrap(),
        b"editor metadata"
    );
    assert_eq!(
        published
            .get_xattr("com.apple.ResourceFork")
            .unwrap()
            .unwrap(),
        b"small fork"
    );
    assert!(md.modified().unwrap() > original.modified().unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"new");
    let backup = f
        .storage
        .join(filesystem::backup_name(&control.operation_id));
    assert_eq!(fs::read(&backup).unwrap(), b"old");
    assert_eq!(acl(&backup), original_acl);
    lifecycle::release(&f.storage, control, observed.revision, true).unwrap();
    assert!(!backup.exists());
    assert_eq!(fs::read(&path).unwrap(), b"new");
}

#[test]
fn existing_cancellation_after_staging_releases_original_full_identity() {
    let f = Fixture::new();
    let packet = f.prepare(true);
    age_original(&f);
    let request: Request = serde_json::from_value(packet).unwrap();
    let control = request.control().unwrap();
    execute_checkpoints(&f.storage, control, |point| {
        if point == Checkpoint::Staged {
            let mut store = Store::open(&f.storage, None).unwrap();
            let cancelled = store.cancel(control).unwrap();
            drop(store);
            assert!(cancelled.phase == Phase::Cancelled);
            assert!(lifecycle::release(&f.storage, control, cancelled.revision, false).is_err());
        }
        Ok(())
    })
    .unwrap();
    let observation = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observation.phase == Phase::Cancelled);
    assert_eq!(observation.live, Some(false));
    assert_eq!(observation.publication, "none");
    let stage = f
        .storage
        .join(filesystem::stage_name(&control.operation_id));
    assert_eq!(fs::read(&stage).unwrap(), b"new");
    assert_eq!(fs::read(f.scope.join("target")).unwrap(), b"old");
    lifecycle::release(&f.storage, control, observation.revision, false).unwrap();
    assert!(!stage.exists());
    assert_eq!(fs::read(f.scope.join("target")).unwrap(), b"old");
}
