//! Native binary tests; fixtures and user targets are isolated from each other.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn hash(domain: &[u8], bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(bytes);
    format!("{:x}", h.finalize())
}
struct Fixture {
    root: PathBuf,
    storage: PathBuf,
    scope: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("tunnelde-native-save-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let scope = root.join("scope");
        fs::create_dir(&scope).unwrap();
        Self {
            storage: root.join("state"),
            root,
            scope,
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tunnelde-agent"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
            .arg("files-save")
            .arg("--directory")
            .arg(&self.storage)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn bytes(&self, bytes: &[u8]) -> Output {
        let mut child = self.command().spawn().unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        child.wait_with_output().unwrap()
    }
    fn call(&self, request: &Value) -> Value {
        let result = self.bytes(&serde_json::to_vec(request).unwrap());
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stderr.is_empty());
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(
            !text.contains(&"e".repeat(64)),
            "response leaked credential"
        );
        let response: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(response["protocol"], "files.save.v1");
        assert_eq!(response["cleanupAuthorized"], false);
        response
    }
    fn reject(&self, request: &Value, error: &str) {
        let result = self.bytes(&serde_json::to_vec(request).unwrap());
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert_eq!(
            String::from_utf8(result.stderr).unwrap(),
            format!("Tunnelde helper: {error}\n")
        );
    }
    fn initialize(&self) -> Value {
        self.call(&json!({"method":"initialize","schemaVersion":1,"storage":"local",
            "context":{"hostKey":format!("SHA256:{}","A".repeat(43)),"account":"test","scope":self.scope}}))
    }
    fn submit(&self, target: &str, content: &str, baseline: Value) -> Value {
        let authority = self.initialize();
        let intent = json!({"schemaVersion":1,"bindingSha256":authority["bindingSha256"],"target":[target],
            "baseline":baseline,"payloadSha256":hash(b"",content.as_bytes()),"payloadLength":content.len()});
        let digest = hash(
            b"tunnelde.files.save.intent.v1\0",
            &serde_json::to_vec(&intent).unwrap(),
        );
        json!({"method":"submit","control":{"schemaVersion":1,"authorityId":authority["authority"]["authorityId"],
            "operationId":uuid::Uuid::new_v4().to_string(),"generation":uuid::Uuid::new_v4().to_string(),"intentSha256":digest,"token":"e".repeat(64)},
            "intent":intent,"content":content})
    }
    fn status(&self, submit: &Value) -> Value {
        self.call(&json!({"method":"status","control":submit["control"]}))
    }
    fn settled(&self, submit: &Value) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let response = self.status(submit);
            let phase = response["observation"]["phase"]
                .as_str()
                .unwrap()
                .to_owned();
            if !["prepared", "running", "publishing"].contains(&phase.as_str())
                && response["observation"]["live"] == false
            {
                return response;
            }
            assert!(
                Instant::now() < deadline,
                "executor did not settle: {phase}"
            );
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    fn operation<'a>(&self, submit: &'a Value) -> &'a str {
        submit["control"]["operationId"].as_str().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn create_receipt_survives_fresh_clients_and_duplicate_submit_never_publishes_again() {
    let f = Fixture::new();
    let submit = f.submit("new.txt", "first\n你好", json!({"kind":"absent"}));
    f.call(&submit);
    let response = f.settled(&submit);
    assert_eq!(response["observation"]["phase"], "succeeded", "{response}");
    assert_eq!(response["observation"]["publication"], "published");
    assert_eq!(
        fs::read_to_string(f.scope.join("new.txt")).unwrap(),
        "first\n你好"
    );
    fs::write(f.scope.join("new.txt"), "external edit").unwrap();
    f.call(&submit);
    assert_eq!(f.status(&submit), response);
    assert_eq!(
        fs::read_to_string(f.scope.join("new.txt")).unwrap(),
        "external edit"
    );
    let db = fs::read(f.storage.join("files-save-v1.sqlite3")).unwrap();
    assert!(!db.windows(64).any(|b| b == "e".repeat(64).as_bytes()));
}

#[test]
fn native_replace_retains_actual_original_and_release_requires_exact_revision_and_consent() {
    let f = Fixture::new();
    let target = f.scope.join("existing.json");
    fs::write(&target, "original").unwrap();
    #[cfg(windows)]
    fs::write(
        target.with_file_name("existing.json:stream"),
        "alternate stream",
    )
    .unwrap();
    #[cfg(windows)]
    let security_before = security(&target);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let submit = f.submit(
        "existing.json",
        "replacement",
        json!({"kind":"existing","sha256":hash(b"",b"original"),"length":8}),
    );
    f.call(&submit);
    let response = f.settled(&submit);
    assert_eq!(response["observation"]["phase"], "succeeded", "{response}");
    assert_eq!(response["observation"]["backupRetained"], true);
    assert_eq!(fs::read_to_string(&target).unwrap(), "replacement");
    #[cfg(windows)]
    assert_eq!(
        fs::read_to_string(target.with_file_name("existing.json:stream")).unwrap(),
        "alternate stream"
    );
    #[cfg(windows)]
    assert_eq!(
        security(&target),
        security_before,
        "native replacement must preserve original security"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o640);
    }
    let suffix = if cfg!(windows) {
        ".stage.backup"
    } else {
        ".stage"
    };
    let backup = f
        .storage
        .join(format!("save-{}{suffix}", f.operation(&submit)));
    assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
    #[cfg(windows)]
    assert_eq!(
        security(&backup),
        security_before,
        "backup must preserve displaced security as well"
    );
    let mut release = json!({"method":"release","control":submit["control"],"revision":response["observation"]["revision"],"discardBackup":false});
    f.reject(&release, "files.save.notReleasable");
    release["discardBackup"] = json!(true);
    release["revision"] = json!(1);
    f.reject(&release, "files.save.notReleasable");
    release["revision"] = response["observation"]["revision"].clone();
    assert_eq!(f.call(&release)["observation"]["phase"], "released");
    assert!(!backup.exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "replacement");
}

#[test]
fn cancel_before_prepare_is_a_durable_tombstone_not_a_deletable_missing_record() {
    let f = Fixture::new();
    let submit = f.submit("cancel.txt", "never", json!({"kind":"absent"}));
    let cancel = json!({"method":"cancel","control":submit["control"]});
    let response = f.call(&cancel);
    assert_eq!(response["observation"]["phase"], "cancelled");
    assert_eq!(f.call(&submit), response);
    assert!(!f.scope.join("cancel.txt").exists());
    let mut changed = cancel;
    changed["control"]["generation"] = json!(uuid::Uuid::new_v4().to_string());
    f.reject(&changed, "files.save.identityMismatch");
}

#[test]
fn baseline_conflict_and_create_over_existing_leave_target_untouched() {
    let f = Fixture::new();
    let target = f.scope.join("keep.txt");
    fs::write(&target, "keep").unwrap();
    for baseline in [
        json!({"kind":"absent"}),
        json!({"kind":"existing","sha256":hash(b"",b"other"),"length":5}),
    ] {
        let submit = f.submit("keep.txt", "new", baseline);
        f.call(&submit);
        let response = f.settled(&submit);
        assert_eq!(response["observation"]["phase"], "failed");
        assert_eq!(response["observation"]["publication"], "none");
        assert_eq!(
            response["observation"]["error"],
            "files.save.baselineChanged"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }
}

#[test]
fn malformed_packet_never_initializes_and_changed_token_never_authorizes_status() {
    let f = Fixture::new();
    f.reject(&json!({"method":"initialize","schemaVersion":2,"storage":"local","context":{"hostKey":"SHA256:x","account":"a","scope":f.scope}}),"files.save.invalidRequest");
    assert!(!f.storage.exists());
    let submit = f.submit("x.txt", "x", json!({"kind":"absent"}));
    f.call(&submit);
    f.settled(&submit);
    for key in [
        "authorityId",
        "operationId",
        "generation",
        "intentSha256",
        "token",
    ] {
        let mut packet = json!({"method":"status","control":submit["control"]});
        packet["control"][key] = if key.ends_with("Id") || key == "generation" {
            json!(uuid::Uuid::new_v4().to_string())
        } else {
            json!("f".repeat(64))
        };
        if key == "operationId" {
            assert!(f.call(&packet)["observation"].is_null());
        } else {
            f.reject(&packet, "files.save.identityMismatch");
        }
    }
}

#[test]
fn duplicate_processes_and_cancellation_share_one_publication_admission() {
    let f = Fixture::new();
    let submit = f.submit("race.txt", "single publication", json!({"kind":"absent"}));
    let cancel = json!({"method":"cancel","control":submit["control"]});
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..12)
            .map(|i| {
                let f = &f;
                let packet = if i % 3 == 0 { &cancel } else { &submit };
                scope.spawn(move || f.call(packet))
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    });
    let response = f.settled(&submit);
    match response["observation"]["phase"].as_str().unwrap() {
        "succeeded" => assert_eq!(
            fs::read_to_string(f.scope.join("race.txt")).unwrap(),
            "single publication"
        ),
        "cancelled" => assert!(!f.scope.join("race.txt").exists()),
        phase => panic!("unexpected race outcome: {phase}"),
    }
}

#[test]
fn lost_stdout_does_not_stop_detached_execution_or_erase_its_receipt() {
    let f = Fixture::new();
    let submit = f.submit("detached.txt", "survives client", json!({"kind":"absent"}));
    let mut child = f.command().spawn().unwrap();
    drop(child.stdout.take());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&submit).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Tunnelde helper: files.save.responseUnavailable\n"
    );
    let response = f.settled(&submit);
    assert_eq!(response["observation"]["phase"], "succeeded");
    assert_eq!(
        fs::read_to_string(f.scope.join("detached.txt")).unwrap(),
        "survives client"
    );
}

#[test]
fn missing_status_and_invalid_target_do_not_create_an_operation_or_lease() {
    let f = Fixture::new();
    let mut submit = f.submit("missing.txt", "unused", json!({"kind":"absent"}));
    let names = || {
        let mut names: Vec<_> = fs::read_dir(&f.storage)
            .unwrap()
            .map(|v| v.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = names();
    assert!(f.status(&submit)["observation"].is_null());
    assert_eq!(names(), before);
    for target in [vec![".."], vec!["/"], vec!["."], vec!["x/y"]] {
        submit["intent"]["target"] = json!(target);
        f.reject(&submit, "files.save.invalidRequest");
    }
    assert_eq!(names(), before);
    assert!(fs::read_dir(&f.scope).unwrap().next().is_none());
}

#[cfg(windows)]
fn security(path: &std::path::Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW, *},
    };
    let name: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut size = 0;
    let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    unsafe {
        GetFileSecurityW(name.as_ptr(), info, std::ptr::null_mut(), 0, &mut size);
    }
    let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    assert_ne!(
        unsafe {
            GetFileSecurityW(
                name.as_ptr(),
                info,
                data.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        },
        0
    );
    let mut text = std::ptr::null_mut();
    let mut len = 0;
    assert_ne!(
        unsafe {
            SetSecurityDescriptorControl(
                data.as_mut_ptr().cast(),
                SE_DACL_AUTO_INHERITED | SE_DACL_AUTO_INHERIT_REQ,
                0,
            )
        },
        0
    );
    assert_ne!(
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                data.as_mut_ptr().cast(),
                1,
                info,
                &mut text,
                &mut len,
            )
        },
        0
    );
    let mut actual = 0;
    while unsafe { *text.add(actual) } != 0 {
        actual += 1;
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(text, actual) }).unwrap();
    unsafe {
        LocalFree(text.cast());
    }
    value
}
