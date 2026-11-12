use super::super::contracts::{sha256, Baseline};
use super::*;
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    scope: PathBuf,
    state: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("tunnelde-save-store-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let scope = root.join("scope");
        fs::create_dir(&scope).unwrap();
        Self {
            state: root.join("state"),
            root,
            scope,
        }
    }
    fn open(&self) -> Store {
        Store::open(
            &self.state,
            Some(&Context {
                host_key: format!("SHA256:{}", "A".repeat(43)),
                account: "fixture".into(),
                scope: self.scope.to_str().unwrap().into(),
            }),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn operation(store: &Store, name: &str, bytes: &[u8]) -> (Control, Intent) {
    let intent = Intent {
        schema_version: 1,
        binding_sha256: store.authority.binding().unwrap(),
        target: vec![name.into()],
        baseline: Baseline::Absent,
        payload_sha256: sha256(bytes),
        payload_length: bytes.len() as u64,
    };
    let control = Control {
        schema_version: 1,
        authority_id: store.authority.authority_id.clone(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        generation: uuid::Uuid::new_v4().to_string(),
        intent_sha256: intent.digest().unwrap(),
        token: "e".repeat(64),
    };
    (control, intent)
}
#[test]
fn exact_duplicate_does_not_consume_capacity_and_unknowns_quarantine_new_admission() {
    let f = Fixture::new();
    let mut store = f.open();
    let (control, intent) = operation(&store, "a", b"x");
    store.submit(&control, &intent, b"x").unwrap();
    let first = store.submit(&control, &intent, b"x").unwrap();
    assert_eq!(first.revision, 1);
    let (other, other_intent) = operation(&store, "a", b"y");
    assert_eq!(
        store
            .submit(&other, &other_intent, b"y")
            .err()
            .unwrap()
            .to_string(),
        "files.save.targetBusy"
    );
    store
        .change(&control, |r| {
            r.phase = Phase::Unknown;
            Ok(())
        })
        .unwrap();
    let (other, other_intent) = operation(&store, "different", b"y");
    assert_eq!(
        store
            .submit(&other, &other_intent, b"y")
            .err()
            .unwrap()
            .to_string(),
        "files.save.authorityQuarantined"
    );
    assert!(store.cancel(&control).unwrap().phase == Phase::Unknown);
    assert!(store.submit(&control, &intent, b"x").unwrap().phase == Phase::Unknown);
}
#[test]
fn retained_cancelled_payload_is_counted_until_explicit_release_without_evicting_identity() {
    let f = Fixture::new();
    let mut store = f.open();
    let content = vec![b'a'; super::super::contracts::MAX_TEXT];
    let mut saved = Vec::new();
    for index in 0..MAX_ACTIVE {
        let (control, intent) = operation(&store, &format!("file{index}"), &content);
        store.submit(&control, &intent, &content).unwrap();
        store.cancel(&control).unwrap();
        saved.push(control);
    }
    let (extra, intent) = operation(&store, "extra", &content);
    assert_eq!(
        store
            .submit(&extra, &intent, &content)
            .err()
            .unwrap()
            .to_string(),
        "files.save.capacity"
    );
    let original = &saved[0];
    let revision = store.get(original).unwrap().unwrap().revision;
    drop(store);
    super::super::lifecycle::release(&f.state, original, revision, true).unwrap();
    store = f.open();
    assert!(store.get(original).unwrap().unwrap().phase == Phase::Released);
    store.submit(&extra, &intent, &content).unwrap();
}
#[test]
fn copied_database_cannot_retain_authority_on_a_different_private_root() {
    let f = Fixture::new();
    let store = f.open();
    drop(store);
    let second = f.root.join("other");
    filesystem::initialize_storage(&second).unwrap();
    fs::copy(
        f.state.join(PROFILE.database),
        second.join(PROFILE.database),
    )
    .unwrap();
    fs::write(second.join(PROFILE.lock), []).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(second.join(PROFILE.lock), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let error = Store::open(&second, None).err().unwrap().to_string();
    assert_eq!(error, "files.save.storageChanged");
}
#[test]
fn a_lease_path_replacement_cannot_falsely_prove_original_executor_exited() {
    let f = Fixture::new();
    let mut store = f.open();
    let (control, intent) = operation(&store, "target", b"data");
    store.submit(&control, &intent, b"data").unwrap();
    let lease = super::super::lease::acquire(&f.state, &control.operation_id, true)
        .unwrap()
        .unwrap();
    store
        .change(&control, |r| {
            r.phase = Phase::Publishing;
            r.lease = Some(lease.identity.clone());
            Ok(())
        })
        .unwrap();
    drop(store);
    drop(lease);
    let name = f.state.join(format!("save-{}.lease", control.operation_id));
    fs::rename(&name, name.with_extension("original")).unwrap();
    fs::write(&name, []).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&name, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let observed = super::super::lifecycle::observe(&f.state, &control)
        .unwrap()
        .unwrap();
    assert!(observed.phase == Phase::Unknown);
    assert_eq!(observed.live, None);
    assert_eq!(observed.publication, "unknown");
    assert!(!observed.cancellable);
    assert!(super::super::lifecycle::release(&f.state, &control, observed.revision, true).is_err());
}

#[test]
fn a_malformed_persisted_intent_is_preserved_and_rejected_before_executor_io() {
    let f = Fixture::new();
    let mut store = f.open();
    let (control, intent) = operation(&store, "target", b"data");
    store.submit(&control, &intent, b"data").unwrap();
    let mut record = store.get(&control).unwrap().unwrap();
    record.intent.as_mut().unwrap().target.clear();
    let damaged = serde_json::to_string(&record).unwrap();
    db(store.connection.execute(
        "UPDATE operations SET body=?2 WHERE id=?1",
        params![control.operation_id, damaged],
    ))
    .unwrap();
    assert_eq!(
        store.get(&control).err().unwrap().to_string(),
        "files.save.corrupt"
    );
    drop(store);
    let before = fs::read(f.state.join(PROFILE.database)).unwrap();
    assert_eq!(
        super::super::executor::execute(&f.state, &control)
            .err()
            .unwrap()
            .to_string(),
        "files.save.corrupt"
    );
    assert_eq!(fs::read(f.state.join(PROFILE.database)).unwrap(), before);
    assert!(fs::read_dir(&f.scope).unwrap().next().is_none());
    assert!(!f
        .state
        .join(format!("save-{}.lease", control.operation_id))
        .exists());
}
