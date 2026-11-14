//! Secrets are passed only through a bounded stdin bootstrap, never argv/env.
use crate::Result;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;
pub(super) async fn start(directory: &Path, bytes: &[u8]) -> Result<()> {
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, DETACHED_PROCESS,
        };
        // OpenSSH closes its request job after replying. Console detachment
        // alone leaves the worker in that job; request permitted breakaway.
        // If server policy refuses, preserve admission and fail the launch.
        command.creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    #[cfg(unix)]
    // SAFETY: this child-only hook calls one async-signal-safe syscall and
    // creates a fresh session without a controlling terminal. Merely creating
    // a process group would still inherit the SSH session's terminal lifecycle.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
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
