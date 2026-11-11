//! Launcher owns the detached child until a validated bootstrap has been sent.
use super::{bootstrap::Mode, option, write_private};
use crate::Result;
use serde_json::json;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncWriteExt, BufReader};

fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    // One exclusive create; never merge a task into an existing directory.
    #[cfg(not(unix))]
    std::fs::create_dir(path)?;
    Ok(())
}

pub(super) async fn start(args: &[String], directory: &Path, id: &str, root: &Path) -> Result<()> {
    if root.exists() {
        return Err("Runner ID already exists".into());
    }
    let mode = Mode::select(args)?;
    let executable = option(args, "--executable")?;
    let entry = option(args, "--entry")?;
    // Validate the complete initialized packet before creating even a task directory.
    let initial = mode.read(&mut BufReader::new(tokio::io::stdin())).await?;
    // Preserve legacy callers that let the helper create the parent directory.
    // Initialized launches require the deployment directory to exist already.
    if matches!(&mode, Mode::File(_)) {
        std::fs::create_dir_all(directory)?;
    }
    private_directory(root)?;
    private_directory(&root.join("inbox"))?;
    let null = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(if cfg!(windows) { "NUL" } else { "/dev/null" })?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    #[cfg(windows)]
    command.creation_flags(0x08000008);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .args([
            "serve",
            "--directory",
            &directory.to_string_lossy(),
            "--id",
            id,
        ])
        .args(mode.arguments())
        .args(["--executable", &executable, "--entry", &entry])
        .stdin(Stdio::piped())
        .stdout(null.try_clone()?)
        .stderr(null)
        .spawn()?;
    let result: Result<()> = async {
        let pid = child.id().ok_or("Missing supervisor process ID")?;
        write_private(&root.join("pid"), pid.to_string().as_bytes())?;
        let mut input = child.stdin.take().ok_or("Missing supervisor input")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            input.write_all(initial.as_bytes()).await?;
            input.write_all(b"\n").await?;
            input.shutdown().await
        })
        .await
        .map_err(|_| "Supervisor bootstrap write timed out")??;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        // Best effort only: a lost acknowledgement still requires host recovery.
        // Never remove a pathname or replay startup to conceal an unknown outcome.
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        return Err(error);
    }
    println!("{}", json!({"runnerId":id,"status":"starting"}));
    Ok(())
}
