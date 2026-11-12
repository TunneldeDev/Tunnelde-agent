//! Private local control storage; not a lock on any user's target file.
use super::{Failure, Profile};
type Result<T> = std::result::Result<T, Failure>;
use std::fs::{File, Metadata, OpenOptions, TryLockError};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn metadata(path: &Path, directory: bool) -> Result<Metadata> {
    let value = std::fs::symlink_metadata(path).map_err(|_| Failure::StorageUnavailable)?;
    if value.file_type().is_symlink()
        || value.is_dir() != directory
        || (!directory && !value.is_file())
    {
        return Err(Failure::UnsafeStorage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments or borrowed memory.
        if value.uid() != unsafe { libc::geteuid() }
            || value.mode() & 0o077 != 0
            || (!directory && value.nlink() != 1)
        {
            return Err(Failure::UnsafeStorage);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if value.file_attributes() & 0x400 != 0 {
            return Err(Failure::UnsafeStorage);
        }
    }
    Ok(value)
}

fn options(directory: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(!directory);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(
            libc::O_NOFOLLOW
                | if directory {
                    libc::O_DIRECTORY
                } else {
                    libc::O_NONBLOCK
                },
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .share_mode(0x1 | 0x2)
            .custom_flags(0x00200000 | if directory { 0x02000000 } else { 0 });
    }
    options
}

fn same(file: &File, path: &Path, directory: bool) -> Result<()> {
    let current = metadata(path, directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let held = file.metadata().map_err(|_| Failure::StorageUnavailable)?;
        if (current.dev(), current.ino()) != (held.dev(), held.ino()) {
            return Err(Failure::StorageChanged);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        fn info(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
            let mut info = std::mem::MaybeUninit::uninit();
            // SAFETY: the borrowed File owns its handle; the OS fills the buffer on success.
            if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
                return Err(Failure::StorageUnavailable);
            }
            // SAFETY: successful GetFileInformationByHandle initialized the buffer.
            Ok(unsafe { info.assume_init() })
        }
        let held = info(file)?;
        let observed = info(
            &options(directory)
                .write(false)
                .open(path)
                .map_err(|_| Failure::StorageUnavailable)?,
        )?;
        if (!directory && held.nNumberOfLinks != 1)
            || (
                held.dwVolumeSerialNumber,
                held.nFileIndexHigh,
                held.nFileIndexLow,
            ) != (
                observed.dwVolumeSerialNumber,
                observed.nFileIndexHigh,
                observed.nFileIndexLow,
            )
        {
            return Err(Failure::StorageChanged);
        }
        let _ = current;
    }
    Ok(())
}

pub(crate) struct Namespace {
    pub path: PathBuf,
    pub created_database: bool,
    profile: Profile,
    directory: File,
    lock: File,
    database: File,
}

impl Namespace {
    pub fn open(path: &Path, initialize: bool, profile: Profile) -> Result<Self> {
        metadata(path, true)?;
        let directory = options(true)
            .open(path)
            .map_err(|_| Failure::StorageUnavailable)?;
        same(&directory, path, true)?;
        // SQLite NOFOLLOW rejects symlinks in *any* path component, including
        // macOS's system /var alias. Resolve ancestors after opening/checking
        // the actual leaf, then prove this canonical name is the retained root.
        // A symlink at the selected root itself remains rejected above.
        #[cfg(unix)]
        let canonical = std::fs::canonicalize(path).map_err(|_| Failure::StorageUnavailable)?;
        #[cfg(unix)]
        let path = canonical.as_path();
        same(&directory, path, true)?;
        let lock_path = path.join(profile.lock);
        let mut opts = options(false);
        opts.create(initialize);
        let lock = opts.open(&lock_path).map_err(|_| Failure::NotInitialized)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(TryLockError::WouldBlock) => return Err(Failure::Busy),
                Err(TryLockError::Error(_)) => return Err(Failure::StorageUnavailable),
            }
        }
        same(&lock, &lock_path, false)?;
        if lock
            .metadata()
            .map_err(|_| Failure::StorageUnavailable)?
            .len()
            != 0
        {
            return Err(Failure::Corrupt);
        }
        let database_path = path.join(profile.database);
        let (database, created_database) = if initialize {
            match options(false).create_new(true).open(&database_path) {
                Ok(file) => (file, true),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
                    options(false)
                        .open(&database_path)
                        .map_err(|_| Failure::StorageUnavailable)?,
                    false,
                ),
                Err(_) => return Err(Failure::StorageUnavailable),
            }
        } else {
            (
                options(false)
                    .open(&database_path)
                    .map_err(|_| Failure::NotInitialized)?,
                false,
            )
        };
        let result = Self {
            path: path.into(),
            created_database,
            profile,
            directory,
            lock,
            database,
        };
        result.validate()?;
        if !created_database {
            // SQLite may recover a hot journal on its first read. Check the
            // retained database header first so a foreign database is never
            // repaired merely because the caller selected the wrong directory.
            let mut header = [0u8; 100];
            (&result.database)
                .read_exact(&mut header)
                .map_err(|_| Failure::Corrupt)?;
            if &header[..16] != b"SQLite format 3\0"
                || u16::from_be_bytes([header[16], header[17]]) != 4096
                || &header[60..64] != profile.version.to_be_bytes().as_slice()
                || &header[68..72] != profile.application_id.to_be_bytes().as_slice()
            {
                return Err(Failure::Corrupt);
            }
        }
        // Ensure new database/control directory entries precede any acknowledgement.
        #[cfg(unix)]
        result
            .directory
            .sync_all()
            .map_err(|_| Failure::StorageUnavailable)?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<()> {
        same(&self.directory, &self.path, true)?;
        same(&self.lock, &self.path.join(self.profile.lock), false)?;
        same(
            &self.database,
            &self.path.join(self.profile.database),
            false,
        )?;
        if self
            .database
            .metadata()
            .map_err(|_| Failure::StorageUnavailable)?
            .len()
            > self.profile.maximum
        {
            return Err(Failure::Capacity);
        }
        // A legitimate rollback journal may need SQLite crash recovery. WAL is
        // never selected by this protocol; preserve unexpected sidecars intact.
        for suffix in ["-journal", "-wal", "-shm"] {
            let path = self.path.join(format!("{}{suffix}", self.profile.database));
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    let observed = metadata(&path, false)?;
                    if suffix != "-journal"
                        || observed.len() > self.profile.maximum + self.profile.journal_overhead
                    {
                        return Err(Failure::Corrupt);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Failure::StorageUnavailable),
            }
        }
        Ok(())
    }
}
