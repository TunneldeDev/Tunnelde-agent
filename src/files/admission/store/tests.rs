use super::*;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

fn directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tunnelde-file-ledger-crash-{}",
        uuid::Uuid::new_v4()
    ));
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder.create(&path).unwrap();
    path
}

#[test]
fn process_loss_rolls_back_unacknowledged_changes_and_releases_original_lock() {
    let root = directory();
    let mut store = Store::open(&root, true).unwrap();
    let request: Request = serde_json::from_value(json!({"method":"reserve","schemaVersion":1,
        "authorityId":store.authority_id,"operationId":uuid::Uuid::new_v4().to_string(),
        "generation":uuid::Uuid::new_v4().to_string(),"bindingSha256":"a".repeat(64),
        "intentSha256":"b".repeat(64),"token":"c".repeat(64)}))
    .unwrap();
    let expected = serde_json::to_value(store.apply(&request).unwrap()).unwrap();
    drop(store);
    let mut command = Command::new(std::env::current_exe().unwrap());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .args([
            "--exact",
            "files::admission::store::tests::crash_writer",
            "--ignored",
            "--nocapture",
        ])
        .env("TUNNELDE_TEST_LEDGER_CRASH_DIRECTORY", &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !root.join("test-staged").exists() {
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("crash fixture did not reach the uncommitted transaction");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(root.join(format!("{DATABASE}-journal")).exists());
    child.kill().unwrap();
    child.wait().unwrap();
    // Fresh Store performs SQLite hot-journal recovery under the original lock.
    let mut recovered = Store::open(&root, false).unwrap();
    assert_eq!(
        serde_json::to_value(recovered.apply(&request).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        recovered
            .connection
            .query_row("SELECT count(*) FROM operations", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
    drop(recovered);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "subprocess fixture, invoked by process_loss test"]
fn crash_writer() {
    let root = PathBuf::from(
        std::env::var_os("TUNNELDE_TEST_LEDGER_CRASH_DIRECTORY").expect("fixture directory"),
    );
    let mut store = Store::open(&root, false).unwrap();
    store
        .connection
        .pragma_update(None, "cache_size", 1)
        .unwrap();
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    transaction
        .execute("UPDATE operations SET state='cancelled',revision=2", [])
        .unwrap();
    for n in 1..64 {
        transaction.execute("INSERT INTO operations SELECT ?1,generation,binding,intent,token_digest,state,revision FROM operations LIMIT 1",[uuid::Uuid::from_u128(n).to_string()]).unwrap();
    }
    transaction.cache_flush().unwrap();
    fs::write(root.join("test-staged"), b"uncommitted").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
