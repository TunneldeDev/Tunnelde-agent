//! A per-operation kernel lease, retained through the last possible target IO.
use super::{contracts::ObjectIdentity, filesystem};
use crate::Result;
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::Path,
};
pub(super) struct Attempt {
    pub identity: ObjectIdentity,
    pub held: Option<Lease>,
}
pub(super) struct Lease {
    _file: File,
}
pub(super) fn acquire(directory: &Path, operation: &str, create: bool) -> Result<Option<Attempt>> {
    let path = directory.join(format!("save-{operation}.lease"));
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3).custom_flags(0x00200000);
    }
    let file = if create {
        match options.create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => options
                .create_new(false)
                .open(&path)
                .map_err(|_| "files.save.unsafeLease")?,
            Err(_) => return Err("files.save.unsafeLease".into()),
        }
    } else {
        match options.open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("files.save.unsafeLease".into()),
        }
    };
    let md = file.metadata().map_err(|_| "files.save.unsafeLease")?;
    if !md.is_file() || md.len() != 0 {
        return Err("files.save.unsafeLease".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no input arguments.
        if md.nlink() != 1 || md.mode() & 0o077 != 0 || md.uid() != unsafe { libc::geteuid() } {
            return Err("files.save.unsafeLease".into());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if md.file_attributes() & 0x400 != 0 {
            return Err("files.save.unsafeLease".into());
        }
    }
    let identity = filesystem::identity(&file)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Attempt {
            identity,
            held: Some(Lease { _file: file }),
        })),
        Err(TryLockError::WouldBlock) => Ok(Some(Attempt {
            identity,
            held: None,
        })),
        Err(_) => Err("files.save.unsafeLease".into()),
    }
}
