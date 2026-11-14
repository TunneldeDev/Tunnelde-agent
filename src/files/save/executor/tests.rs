use super::*;
use crate::files::save::{
    contracts::{Baseline, Context, Intent, Request},
    lifecycle,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
mod macos;

struct Fixture {
    root: PathBuf,
    storage: PathBuf,
    scope: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("tunnelde-save-crash-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let scope = root.join("scope");
        fs::create_dir(&scope).unwrap();
        Self {
            storage: root.join("state"),
            root,
            scope,
        }
    }
    fn prepare(&self, replace: bool) -> Value {
        let context = Context {
            host_key: format!("SHA256:{}", "A".repeat(43)),
            account: "fixture".into(),
            scope: self.scope.to_str().unwrap().into(),
        };
        let mut store = Store::open(&self.storage, Some(&context)).unwrap();
        let intent = Intent {
            schema_version: 1,
            binding_sha256: store.authority.binding().unwrap(),
            target: vec!["target".into()],
            baseline: if replace {
                fs::write(self.scope.join("target"), "old").unwrap();
                Baseline::Existing {
                    sha256: crate::files::save::contracts::sha256(b"old"),
                    length: 3,
                }
            } else {
                Baseline::Absent
            },
            payload_sha256: crate::files::save::contracts::sha256(b"new"),
            payload_length: 3,
        };
        let packet = json!({"method":"submit","control":{"schemaVersion":1,"authorityId":store.authority.authority_id,
            "operationId":uuid::Uuid::new_v4().to_string(),"generation":uuid::Uuid::new_v4().to_string(),"intentSha256":intent.digest().unwrap(),"token":"e".repeat(64)},"intent":intent,"content":"new"});
        let request: Request = serde_json::from_value(packet.clone()).unwrap();
        request.validate().unwrap();
        let Request::Submit {
            control,
            intent,
            content,
        } = request
        else {
            unreachable!()
        };
        store.submit(&control, &intent, content.as_bytes()).unwrap();
        packet
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "subprocess fixture: invoked by kill_original_executor_at_publication_checkpoints"]
fn subprocess() {
    let directory = PathBuf::from(std::env::var_os("TUNNELDE_SAVE_CRASH_DIRECTORY").unwrap());
    let signal = PathBuf::from(std::env::var_os("TUNNELDE_SAVE_CRASH_SIGNAL").unwrap());
    let selected = std::env::var("TUNNELDE_SAVE_CRASH_CHECKPOINT").unwrap();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let request: Request = serde_json::from_str(&input).unwrap();
    request.validate().unwrap();
    let control = request.control().unwrap();
    execute_checkpoints(&directory, control, |point| {
        if format!("{point:?}") == selected {
            fs::write(&signal, b"ready").unwrap();
            loop {
                std::thread::park_timeout(Duration::from_secs(10));
            }
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn kill_original_executor_at_publication_checkpoints() {
    for replace in [false, true] {
        for point in [
            Checkpoint::Running,
            Checkpoint::Staged,
            Checkpoint::Publishing,
            Checkpoint::NativePublished,
            Checkpoint::Published,
        ] {
            let f = Fixture::new();
            let packet = f.prepare(replace);
            #[cfg(target_os = "macos")]
            if replace {
                macos::age_original(&f);
            }
            let request: Request = serde_json::from_value(packet.clone()).unwrap();
            let control = request.control().unwrap();
            let signal = f.root.join("checkpoint");
            let mut command = Command::new(std::env::current_exe().unwrap());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            command
                .args([
                    "--exact",
                    "files::save::executor::tests::subprocess",
                    "--ignored",
                    "--nocapture",
                ])
                .env("TUNNELDE_SAVE_CRASH_DIRECTORY", &f.storage)
                .env("TUNNELDE_SAVE_CRASH_SIGNAL", &signal)
                .env("TUNNELDE_SAVE_CRASH_CHECKPOINT", format!("{point:?}"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&packet).unwrap())
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if signal.exists() {
                    break;
                }
                if let Some(status) = child.try_wait().unwrap() {
                    let result = child.wait_with_output().unwrap();
                    panic!(
                        "fixture exited {status}: {} {}",
                        String::from_utf8_lossy(&result.stdout),
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
                if Instant::now() > deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("checkpoint {point:?} not reached");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            let observed = lifecycle::observe(&f.storage, control).unwrap().unwrap();
            assert_eq!(observed.live, Some(false));
            if matches!(point, Checkpoint::Running | Checkpoint::Staged) {
                assert!(observed.phase == Phase::Failed);
                assert_eq!(observed.publication, "none");
            } else {
                assert!(observed.phase == Phase::Unknown);
                assert_eq!(observed.publication, "unknown");
            }
            let before = fs::read(f.scope.join("target")).ok();
            execute(&f.storage, control).unwrap();
            assert_eq!(
                fs::read(f.scope.join("target")).ok(),
                before,
                "duplicate must not replay interrupted work"
            );
            if matches!(point, Checkpoint::NativePublished | Checkpoint::Published) {
                assert_eq!(before.unwrap(), b"new");
            } else if replace {
                assert_eq!(before.unwrap(), b"old");
            } else {
                assert!(before.is_none());
            }
            if matches!(
                point,
                Checkpoint::Publishing | Checkpoint::NativePublished | Checkpoint::Published
            ) {
                assert!(lifecycle::release(&f.storage, control, observed.revision, true).is_err());
            } else {
                lifecycle::release(&f.storage, control, observed.revision, false).unwrap();
                assert!(!f
                    .storage
                    .join(filesystem::stage_name(&control.operation_id))
                    .exists());
            }
        }
    }
}

#[test]
fn displaced_external_write_is_preserved_as_a_published_conflict() {
    let f = Fixture::new();
    let packet = f.prepare(true);
    let request: Request = serde_json::from_value(packet).unwrap();
    let control = request.control().unwrap();
    execute_checkpoints(&f.storage, control, |point| {
        if point == Checkpoint::Publishing {
            fs::write(f.scope.join("target"), "concurrent edit").unwrap();
        }
        Ok(())
    })
    .unwrap();
    let observed = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observed.phase == Phase::Conflict);
    assert_eq!(observed.publication, "published");
    assert_eq!(observed.backup_retained, Some(true));
    assert_eq!(fs::read(f.scope.join("target")).unwrap(), b"new");
    let backup = f
        .storage
        .join(filesystem::backup_name(&control.operation_id));
    assert_eq!(fs::read(backup).unwrap(), b"concurrent edit");
}

#[test]
fn create_cannot_overwrite_a_file_created_after_publication_admission() {
    let f = Fixture::new();
    let packet = f.prepare(false);
    let request: Request = serde_json::from_value(packet).unwrap();
    let control = request.control().unwrap();
    execute_checkpoints(&f.storage, control, |point| {
        if point == Checkpoint::Publishing {
            fs::write(f.scope.join("target"), "concurrent file").unwrap();
        }
        Ok(())
    })
    .unwrap();
    let observed = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observed.phase == Phase::Unknown);
    assert_eq!(
        fs::read(f.scope.join("target")).unwrap(),
        b"concurrent file"
    );
    assert!(f
        .storage
        .join(filesystem::stage_name(&control.operation_id))
        .exists());
    assert!(lifecycle::release(&f.storage, control, observed.revision, true).is_err());
}

#[test]
fn cancellation_after_publication_admission_cannot_claim_to_have_stopped_the_save() {
    let f = Fixture::new();
    let packet = f.prepare(false);
    let request: Request = serde_json::from_value(packet).unwrap();
    let control = request.control().unwrap();
    execute_checkpoints(&f.storage, control, |point| {
        if point == Checkpoint::Publishing {
            let mut store = Store::open(&f.storage, None).unwrap();
            assert!(store.cancel(control).unwrap().phase == Phase::Publishing);
            drop(store);
            let observation = lifecycle::observe(&f.storage, control).unwrap().unwrap();
            assert_eq!(observation.live, Some(true));
            assert!(!observation.cancellable);
        }
        Ok(())
    })
    .unwrap();
    let observation = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observation.phase == Phase::Succeeded);
    assert_eq!(fs::read(f.scope.join("target")).unwrap(), b"new");
}

#[test]
fn cancellation_during_staging_blocks_publication_and_requires_exclusion_before_cleanup() {
    let f = Fixture::new();
    let packet = f.prepare(false);
    let request: Request = serde_json::from_value(packet).unwrap();
    let control = request.control().unwrap();
    execute_checkpoints(&f.storage, control, |point| {
        if point == Checkpoint::Staged {
            let mut store = Store::open(&f.storage, None).unwrap();
            let cancelled = store.cancel(control).unwrap();
            drop(store);
            assert!(cancelled.phase == Phase::Cancelled);
            assert!(lifecycle::release(&f.storage, control, cancelled.revision, true).is_err());
        }
        Ok(())
    })
    .unwrap();
    let observation = lifecycle::observe(&f.storage, control).unwrap().unwrap();
    assert!(observation.phase == Phase::Cancelled);
    assert_eq!(observation.publication, "none");
    assert!(!f.scope.join("target").exists());
    lifecycle::release(&f.storage, control, observation.revision, true).unwrap();
    assert!(!f
        .storage
        .join(filesystem::stage_name(&control.operation_id))
        .exists());
}
