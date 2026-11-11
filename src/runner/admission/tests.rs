use super::*;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("tunnelde-admission-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn held_lock_waits_without_blocking_runtime_and_drop_releases_same_lock_file() {
    let fixture = Fixture::new();
    let first = Lock::acquire(&fixture.0, true).await.unwrap();
    let path = fixture.0.clone();
    let waiter = tokio::spawn(async move { Lock::acquire(&path, true).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!waiter.is_finished());
    drop(first);
    let second = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    second.validate().unwrap();
    assert!(fixture.0.join(".admission-v1/control.lock").is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn replaced_namespace_is_rejected_by_the_held_guard() {
    use std::os::unix::fs::DirBuilderExt;
    let fixture = Fixture::new();
    let first = Lock::acquire(&fixture.0, true).await.unwrap();
    std::fs::rename(fixture.0.join(".admission-v1"), fixture.0.join("retained")).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(fixture.0.join(".admission-v1"))
        .unwrap();
    assert!(first.validate().is_err());
}

#[tokio::test]
async fn invalid_lock_content_does_not_get_truncated_or_replaced() {
    let fixture = Fixture::new();
    drop(Lock::acquire(&fixture.0, true).await.unwrap());
    let path = fixture.0.join(".admission-v1/control.lock");
    std::fs::write(&path, b"retain").unwrap();
    assert!(Lock::acquire(&fixture.0, true).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"retain");
}
