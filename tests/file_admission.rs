use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const DATABASE: &str = "files-admission-v1.sqlite3";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("tunnelde-file-admission-{}", uuid::Uuid::new_v4()));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&root).unwrap();
        // Payload data lives outside the ledger and must remain untouched.
        fs::write(root.join("keep"), b"user data").unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        self.command_at(&self.0)
    }
    fn command_at(&self, directory: &std::path::Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tunnelde-agent"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
            .args(["files-admission", "--directory"])
            .arg(directory)
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
    fn call(&self, packet: &Value) -> Value {
        let output = self.bytes(&serde_json::to_vec(packet).unwrap());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(output.stdout.len() <= 4096);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["protocol"], "files.admission-ledger.v1");
        assert_eq!(value["executionSupported"], false);
        assert_eq!(value["cleanupAuthorized"], false);
        assert!(!String::from_utf8(output.stdout)
            .unwrap()
            .contains(&"e".repeat(64)));
        assert_eq!(fs::read(self.0.join("keep")).unwrap(), b"user data");
        value
    }
    fn initialize(&self) -> Value {
        self.call(&json!({"schemaVersion":1,"method":"initialize","storage":"local"}))
    }
    fn identity(&self) -> Value {
        json!({"schemaVersion":1,"method":"reserve","authorityId":self.initialize()["authorityId"],
            "operationId":uuid::Uuid::new_v4().to_string(),"generation":uuid::Uuid::new_v4().to_string(),
            "bindingSha256":"a".repeat(64),"intentSha256":"b".repeat(64),"token":"e".repeat(64)})
    }
    fn reject(&self, packet: &Value, reason: &str) {
        let output = self.bytes(&serde_json::to_vec(packet).unwrap());
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("Tunnelde helper: {reason}\n")
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cancellation_before_reservation_and_exact_replays_are_durable() {
    let fixture = Fixture::new();
    let mut packet = fixture.identity();
    packet["method"] = json!("cancel");
    let cancelled = fixture.call(&packet);
    assert_eq!(cancelled["observation"]["state"], "cancelled");
    assert_eq!(cancelled["observation"]["revision"], 1);
    for method in ["reserve", "status", "cancel"] {
        packet["method"] = json!(method);
        assert_eq!(fixture.call(&packet), cancelled);
    }
    let bytes = fs::read(fixture.0.join(DATABASE)).unwrap();
    assert!(!bytes
        .windows(64)
        .any(|window| window == "e".repeat(64).as_bytes()));
}

#[test]
fn initialize_is_idempotent_query_is_noncreating_and_cancel_advances_once() {
    let fixture = Fixture::new();
    let mut packet = fixture.identity();
    let authority = packet["authorityId"].clone();
    assert_eq!(fixture.initialize()["authorityId"], authority);
    packet["method"] = json!("status");
    let before = fs::read(fixture.0.join(DATABASE)).unwrap();
    let missing = fixture.call(&packet);
    assert_eq!(missing["observation"]["state"], "missing");
    assert_eq!(missing["observation"]["revision"], 0);
    assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), before);
    packet["method"] = json!("reserve");
    let reserved = fixture.call(&packet);
    assert_eq!(reserved["observation"]["revision"], 1);
    assert_eq!(fixture.call(&packet), reserved);
    packet["method"] = json!("cancel");
    let cancelled = fixture.call(&packet);
    assert_eq!(cancelled["observation"]["revision"], 2);
    packet["method"] = json!("reserve");
    assert_eq!(fixture.call(&packet), cancelled);
}

#[test]
fn changed_authority_generation_intent_binding_and_token_preserve_original_record() {
    let fixture = Fixture::new();
    let packet = fixture.identity();
    let original = fixture.call(&packet);
    let bytes = fs::read(fixture.0.join(DATABASE)).unwrap();
    for (field, value) in [
        ("authorityId", uuid::Uuid::new_v4().to_string()),
        ("generation", uuid::Uuid::new_v4().to_string()),
        ("bindingSha256", "c".repeat(64)),
        ("intentSha256", "d".repeat(64)),
        ("token", "f".repeat(64)),
    ] {
        for method in ["status", "reserve", "cancel"] {
            let mut altered = packet.clone();
            altered[field] = json!(value);
            altered["method"] = json!(method);
            fixture.reject(&altered, "files.admission.identityMismatch");
            assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), bytes);
        }
    }
    assert_eq!(fixture.call(&packet), original);
}

#[test]
fn malformed_oversized_duplicate_and_unconsented_input_cannot_initialize() {
    let fixture = Fixture::new();
    for bytes in [
        b"{".to_vec(),
        vec![b'x'; 4097],
        br#"{"method":"initialize","schemaVersion":1,"storage":"network"}"#.to_vec(),
        br#"{"method":"initialize","schemaVersion":1,"schemaVersion":2,"storage":"local"}"#
            .to_vec(),
        br#"{"method":"initialize","schemaVersion":1,"storage":"local","private":"do-not-echo"}"#
            .to_vec(),
    ] {
        let output = fixture.bytes(&bytes);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "Tunnelde helper: files.admission.invalidRequest\n"
        );
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }
    let unknown = json!({"schemaVersion":1,"method":"status","authorityId":uuid::Uuid::new_v4().to_string(),"operationId":uuid::Uuid::new_v4().to_string(),"generation":uuid::Uuid::new_v4().to_string(),"bindingSha256":"a".repeat(64),"intentSha256":"b".repeat(64),"token":"e".repeat(64)});
    fixture.reject(&unknown, "files.admission.notInitialized");
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[test]
fn independent_process_races_cannot_reopen_cancelled_admission() {
    let fixture = Fixture::new();
    let packet = fixture.identity();
    let mut children = Vec::new();
    for n in 0..16 {
        let mut value = packet.clone();
        value["method"] = json!(if n % 2 == 0 { "reserve" } else { "cancel" });
        let mut child = fixture.command().spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&value).unwrap())
            .unwrap();
        children.push(child);
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut status = packet;
    status["method"] = json!("status");
    assert_eq!(fixture.call(&status)["observation"]["state"], "cancelled");
    let connection = Connection::open(fixture.0.join(DATABASE)).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM operations", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[test]
fn capacity_rejects_new_work_without_evicting_records_or_blocking_existing_cancellation() {
    let fixture = Fixture::new();
    let mut packet = fixture.identity();
    fixture.call(&packet);
    {
        let mut connection = Connection::open(fixture.0.join(DATABASE)).unwrap();
        let transaction = connection.transaction().unwrap();
        for n in 1..1024 {
            transaction.execute("INSERT INTO operations SELECT ?1,generation,binding,intent,token_digest,state,revision FROM operations WHERE id=?2",[uuid::Uuid::from_u128(n).to_string(),packet["operationId"].as_str().unwrap().to_owned()]).unwrap();
        }
        transaction.commit().unwrap();
    }
    let mut extra = packet.clone();
    extra["operationId"] = json!(uuid::Uuid::new_v4().to_string());
    fixture.reject(&extra, "files.admission.capacity");
    packet["method"] = json!("cancel");
    assert_eq!(fixture.call(&packet)["observation"]["state"], "cancelled");
    let connection = Connection::open(fixture.0.join(DATABASE)).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM operations", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1024
    );
}

#[test]
fn corrupt_partial_or_unrecognized_database_is_preserved_without_reset() {
    let fixture = Fixture::new();
    let packet = fixture.identity();
    fixture.call(&packet);
    let original = fs::read(fixture.0.join(DATABASE)).unwrap();
    for bytes in [
        Vec::new(),
        b"malformed-database".to_vec(),
        original[..original.len() / 2].to_vec(),
    ] {
        fs::write(fixture.0.join(DATABASE), &bytes).unwrap();
        let output = fixture.bytes(&serde_json::to_vec(&packet).unwrap());
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), bytes);
        let initialize = json!({"schemaVersion":1,"method":"initialize","storage":"local"});
        assert!(!fixture
            .bytes(&serde_json::to_vec(&initialize).unwrap())
            .status
            .success());
        assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), bytes);
    }
    fs::write(fixture.0.join(DATABASE), &original).unwrap();
    assert_eq!(fixture.call(&packet)["observation"]["state"], "reserved");
}

#[test]
fn lost_acknowledgement_is_queried_without_new_reservation() {
    let fixture = Fixture::new();
    let mut packet = fixture.identity();
    // Close the delivery channel before submitting. The commit must survive an
    // actual stdout failure, not only a successfully discarded response.
    let mut child = fixture.command().spawn().unwrap();
    drop(child.stdout.take());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&packet).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Tunnelde helper: files.admission.responseUnavailable\n"
    );
    packet["method"] = json!("status");
    let recovered = fixture.call(&packet);
    assert_eq!(recovered["observation"]["state"], "reserved");
    assert_eq!(recovered["observation"]["revision"], 1);
}

#[test]
fn foreign_sqlite_header_and_stale_epoch_never_authorize_a_new_record() {
    let fixture = Fixture::new();
    let packet = fixture.identity();
    let original = fs::read(fixture.0.join(DATABASE)).unwrap();
    let foreign = fixture.0.join("foreign.sqlite3");
    {
        let connection = Connection::open(&foreign).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE keep(value TEXT); INSERT INTO keep VALUES('original foreign data');",
            )
            .unwrap();
    }
    let foreign_bytes = fs::read(&foreign).unwrap();
    fs::write(fixture.0.join(DATABASE), &foreign_bytes).unwrap();
    fixture.reject(&packet, "files.admission.corrupt");
    fixture.reject(
        &json!({"method":"initialize","schemaVersion":1,"storage":"local"}),
        "files.admission.corrupt",
    );
    assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), foreign_bytes);
    fs::write(fixture.0.join(DATABASE), original).unwrap();
    // Simulate explicit administrator replacement between invocations. Even a
    // missing database never allows an old authority epoch to be reused.
    fs::remove_file(fixture.0.join(DATABASE)).unwrap();
    assert_ne!(fixture.initialize()["authorityId"], packet["authorityId"]);
    fixture.reject(&packet, "files.admission.identityMismatch");
}

#[test]
fn invalid_identifiers_and_nonexistent_mutation_methods_leave_storage_unchanged() {
    let fixture = Fixture::new();
    let packet = fixture.identity();
    let before = fs::read(fixture.0.join(DATABASE)).unwrap();
    for (key, value) in [
        ("schemaVersion", json!(2)),
        ("generation", json!("../private-path")),
        ("operationId", json!(uuid::Uuid::nil().to_string())),
        ("token", json!("0".repeat(64))),
        ("bindingSha256", json!("A".repeat(64))),
        ("intentSha256", json!("b".repeat(63))),
        ("method", json!("execute")),
        ("method", json!("cleanup")),
    ] {
        let mut altered = packet.clone();
        altered[key] = value;
        fixture.reject(&altered, "files.admission.invalidRequest");
        assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), before);
    }
}

#[cfg(unix)]
#[test]
fn nonprivate_directory_and_symlinked_control_names_are_rejected() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::new();
    let packet = fixture.identity();
    let before = fs::read(fixture.0.join(DATABASE)).unwrap();
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
    fixture.reject(&packet, "files.admission.unsafeStorage");
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
    let original = fixture.0.join("original.sqlite3");
    fs::rename(fixture.0.join(DATABASE), &original).unwrap();
    symlink(&original, fixture.0.join(DATABASE)).unwrap();
    let output = fixture.bytes(&serde_json::to_vec(&packet).unwrap());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(original).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn system_ancestor_aliases_resolve_to_the_retained_root_without_allowing_a_linked_root() {
    use std::os::unix::fs::{symlink, DirBuilderExt};
    let fixture = Fixture::new();
    let state = fixture.0.join("state");
    fs::DirBuilder::new().mode(0o700).create(&state).unwrap();
    let alias = fixture.0.join("system-alias");
    symlink(&fixture.0, &alias).unwrap();
    let mut child = fixture.command_at(&alias.join("state")).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"method":"initialize","schemaVersion":1,"storage":"local"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(state.join(DATABASE).exists());
    let leaf = fixture.0.join("linked-root");
    symlink(&state, &leaf).unwrap();
    let mut child = fixture.command_at(&leaf).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"method":"initialize","schemaVersion":1,"storage":"local"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Tunnelde helper: files.admission.unsafeStorage\n"
    );
}

#[test]
fn withheld_eof_times_out_without_opening_state_or_hanging_runtime() {
    let fixture = Fixture::new();
    let mut child = fixture.command().spawn().unwrap();
    let _stdin = child.stdin.take().unwrap();
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(14) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("helper did not exit after its input timeout");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("files.admission.inputTimeout"));
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[test]
fn links_to_control_database_are_rejected_without_modifying_either_name() {
    let fixture = Fixture::new();
    let packet = fixture.identity();
    fixture.call(&packet);
    let original = fs::read(fixture.0.join(DATABASE)).unwrap();
    let link = fixture.0.join("database-alias");
    fs::hard_link(fixture.0.join(DATABASE), &link).unwrap();
    let output = fixture.bytes(&serde_json::to_vec(&packet).unwrap());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(&link).unwrap(), original);
    assert_eq!(fs::read(fixture.0.join(DATABASE)).unwrap(), original);
}
