//! Persistent cooperative lock. Never unlink it while delayed commands can exist.
use crate::Result;
use std::{
    fs::{File, Metadata, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) fn metadata(path: &Path, directory: bool) -> Result<Metadata> {
    let value = std::fs::symlink_metadata(path)?;
    if value.file_type().is_symlink()
        || (directory && !value.is_dir())
        || (!directory && !value.is_file())
    {
        return Err("Unsafe runner admission object".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if value.file_attributes() & 0x400 != 0 {
            return Err("Unsafe runner admission reparse point".into());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if value.mode() & 0o077 != 0 || (!directory && value.nlink() != 1) {
            return Err("Runner admission object is not private".into());
        }
    }
    Ok(value)
}

pub(super) fn options(create: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
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
        // Deny deletion of the open control file; do not follow reparse points.
        options.share_mode(0x1 | 0x2).custom_flags(0x00200000);
    }
    options
}

pub(super) fn same_file(before: &Metadata, after: &Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err("Runner admission object changed".into());
        }
    }
    #[cfg(not(unix))]
    let _ = (before, after);
    Ok(())
}

pub(super) fn validate_file(file: &File, path: &Path) -> Result<()> {
    same_file(&file.metadata()?, &metadata(path, false)?)?;
    #[cfg(windows)]
    {
        super::windows::validate(file)?;
        super::windows::same(file, &options(false).open(path)?)?;
    }
    Ok(())
}

fn directory_handle(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .share_mode(0x1 | 0x2)
            .custom_flags(0x02000000 | 0x00200000);
    }
    Ok(options.open(path)?)
}

pub(super) struct Lock {
    _file: File,
    pub path: PathBuf,
    original: Metadata,
    directory: File,
}

impl Lock {
    pub async fn acquire(directory: &Path, create: bool) -> Result<Self> {
        // Do not create the deployment or accept a missing/non-directory parent.
        let parent = std::fs::symlink_metadata(directory)?;
        if !parent.is_dir() || parent.file_type().is_symlink() {
            return Err("Invalid runner admission directory".into());
        }
        let path = directory.join(".admission-v1");
        if create {
            let builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            if let Err(error) = builder.create(&path) {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error.into());
                }
            }
        }
        let original = metadata(&path, true)?;
        let directory = directory_handle(&path)?;
        let lock_path = path.join("control.lock");
        if lock_path.try_exists()? {
            metadata(&lock_path, false)?;
        }
        let file = options(create).open(&lock_path)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(TryLockError::WouldBlock) => {
                    return Err("Runner admission lock timed out".into())
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        let observed = metadata(&lock_path, false)?;
        validate_file(&file, &lock_path)?;
        if observed.len() != 0 {
            return Err("Invalid runner admission lock".into());
        }
        let result = Self {
            _file: file,
            path,
            original,
            directory,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<()> {
        same_file(&self.original, &metadata(&self.path, true)?)?;
        same_file(&self.directory.metadata()?, &metadata(&self.path, true)?)?;
        #[cfg(windows)]
        super::windows::same(&self.directory, &directory_handle(&self.path)?)?;
        validate_file(&self._file, &self.path.join("control.lock"))
    }
}
