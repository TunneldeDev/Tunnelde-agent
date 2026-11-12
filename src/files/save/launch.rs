//! Secrets are passed only through a bounded stdin bootstrap, never argv/env.
use crate::Result;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;
pub(super) async fn start(directory: &Path, bytes: &[u8]) -> Result<()> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    #[cfg(windows)]
    command.creation_flags(0x08000008);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .arg("files-save")
        .arg("--directory")
        .arg(directory)
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "files.save.launchUnavailable")?;
    let mut input = child.stdin.take().ok_or("files.save.launchUnavailable")?;
    // A failed bootstrap acknowledgement never rolls back an admitted operation.
    tokio::time::timeout(Duration::from_secs(5), async {
        input.write_all(bytes).await?;
        input.shutdown().await
    })
    .await
    .map_err(|_| "files.save.launchUnknown")?
    .map_err(|_| "files.save.launchUnknown")?;
    Ok(())
}
