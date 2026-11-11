use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("tunnelde-capabilities-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep.json"), b"private fixture data").unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tunnelde-agent"));
        command
            .current_dir(&self.0)
            .env("TUNNELDE_MCP_URL", "invalid:must-not-be-read")
            .env("TUNNELDE_MCP_TOKEN", "private-test-token")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn assert_untouched(&self) {
        assert_eq!(fs::read_dir(&self.0).unwrap().count(), 1);
        assert_eq!(
            fs::read(self.0.join("keep.json")).unwrap(),
            b"private fixture data"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn finish(mut child: Child) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Discovery blocked while stdin remained open");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn discovery_needs_no_task_or_runtime_and_never_waits_for_stdin() {
    let fixture = Fixture::new();
    let mut command = fixture.command();
    let mut child = command.arg("--capabilities").spawn().unwrap();
    let _open_stdin = child.stdin.take().unwrap();
    let output = finish(child);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 4096);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        json!({
            "schemaVersion": 1,
            "helper": "tunnelde-agent",
            "helperVersion": env!("CARGO_PKG_VERSION"),
            "runnerProtocols": [1],
            "features": ["runner.control.v1", "runner.initialize-config.v1", "runner.admission-fence.v1"]
        })
    );
    fixture.assert_untouched();
}

#[test]
fn discovery_rejects_task_and_config_arguments_without_echoing_or_creating_them() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["--capabilities", "--directory", "private-task-directory"],
        vec!["--capabilities", "--config", "private-config-secret"],
        vec!["--capabilities", "start"],
    ] {
        let output = finish(fixture.command().args(arguments).spawn().unwrap());
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "Tunnelde helper: --capabilities accepts no additional arguments\n"
        );
        fixture.assert_untouched();
    }
}
