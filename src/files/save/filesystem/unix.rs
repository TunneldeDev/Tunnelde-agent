use super::{ObjectIdentity, Storage, Target};
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};
#[cfg(target_os = "macos")]
mod attributes;

fn invalid() -> io::Error {
    io::Error::other("Unsupported or changed file namespace")
}
fn cstring(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(|_| invalid())
}
pub(super) fn directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}
pub(super) fn identity(file: &File) -> io::Result<ObjectIdentity> {
    let metadata = file.metadata()?;
    let created = metadata
        .created()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid())?;
    Ok(ObjectIdentity {
        platform: std::env::consts::OS.into(),
        device: metadata.dev().to_string(),
        object: metadata.ino().to_string(),
        created: created.as_nanos().to_string(),
    })
}
fn openat(parent: &File, name: &str, flags: i32) -> io::Result<File> {
    let name = cstring(name)?;
    // SAFETY: parent is retained, name is terminated; no creation flag uses a missing mode.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat returns a newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub(super) fn target(root: &File, _path: &Path, parts: &[String]) -> io::Result<Target> {
    let mut parent = root.try_clone()?;
    let mut ancestors = Vec::new();
    for part in &parts[..parts.len() - 1] {
        let next = openat(&parent, part, libc::O_RDONLY | libc::O_DIRECTORY)?;
        ancestors.push(parent);
        parent = next;
    }
    Ok(Target {
        _ancestors: ancestors,
        parent,
        name: parts.last().ok_or_else(invalid)?.clone(),
    })
}
pub(super) fn open_target(target: &Target) -> io::Result<File> {
    let file = openat(
        &target.parent,
        &target.name,
        libc::O_RDONLY | libc::O_NONBLOCK,
    )?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    Ok(file)
}
pub(super) fn create_stage(storage: &Storage, name: &str) -> io::Result<File> {
    let name = cstring(name)?;
    // SAFETY: retained private directory and terminated component, exclusive
    // creation with initialized mode; successful open returns a new descriptor.
    let fd = unsafe {
        libc::openat(
            storage.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_RDWR,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    storage.file.sync_all()?;
    Ok(file)
}
pub(super) fn open_artifact(storage: &Storage, name: &str) -> io::Result<File> {
    let file = openat(&storage.file, name, libc::O_RDONLY | libc::O_NONBLOCK)?;
    let md = file.metadata()?;
    if !md.is_file() || md.nlink() != 1 {
        return Err(invalid());
    }
    Ok(file)
}
pub(super) fn initialize_storage(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {
            directory(path.parent().ok_or_else(invalid)?)?.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    validate_storage(path)
}
pub(super) fn validate_storage(path: &Path) -> io::Result<()> {
    let directory = directory(path)?;
    let md = directory.metadata()?;
    // SAFETY: geteuid has no inputs or memory arguments.
    if md.uid() != unsafe { libc::geteuid() } || md.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: retained directory descriptor and writable statfs output.
    if unsafe { libc::fstatfs(directory.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    #[cfg(target_os = "linux")]
    // A declaration of local storage does not authorize SQLite journals on
    // NFS/SMB/FUSE or another filesystem whose publication rules are unknown.
    if !matches!(
        stat.f_type as u64,
        0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630
    ) {
        return Err(invalid());
    }
    #[cfg(target_os = "macos")]
    {
        let name = unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) };
        if stat.f_flags & libc::MNT_LOCAL as u32 == 0
            || !matches!(name.to_bytes(), b"apfs" | b"hfs")
        {
            return Err(invalid());
        }
    }
    Ok(())
}
pub(super) fn publish(
    target: &Target,
    storage: &Storage,
    stage: &str,
    replace: bool,
) -> io::Result<()> {
    let directory = &storage.file;
    let stage = cstring(stage)?;
    let name = cstring(&target.name)?;
    #[cfg(target_os = "linux")]
    // SAFETY: retained directory descriptors and terminated component names.
    let result = unsafe {
        libc::renameat2(
            directory.as_raw_fd(),
            stage.as_ptr(),
            target.parent.as_raw_fd(),
            name.as_ptr(),
            if replace {
                libc::RENAME_EXCHANGE
            } else {
                libc::RENAME_NOREPLACE
            },
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: retained directory descriptors and terminated component names.
    let result = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            stage.as_ptr(),
            target.parent.as_raw_fd(),
            name.as_ptr(),
            if replace {
                libc::RENAME_SWAP
            } else {
                libc::RENAME_EXCL
            },
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    directory.sync_all()?;
    target.parent.sync_all()
}
pub(super) fn metadata(source: &File, target: &File) -> io::Result<()> {
    let original = source.metadata()?;
    let current = target.metadata()?;
    if (original.uid(), original.gid()) != (current.uid(), current.gid()) {
        // SAFETY: valid retained descriptor and uid/gid copied from the original.
        if unsafe { libc::fchown(target.as_raw_fd(), original.uid(), original.gid()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(target_os = "macos")]
    {
        let attributes = attributes::Attributes::read(source)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(source.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // Compressed-file metadata encodes the old contents. It cannot be
        // copied onto a different payload as ordinary editor metadata.
        if unsafe { stat.assume_init() }.st_flags & libc::UF_COMPRESSED != 0 {
            return Err(invalid());
        }
        // COPYFILE_ACL | COPYFILE_STAT only; bounded xattrs are handled below.
        // SAFETY: retained descriptors and null optional copy state.
        if unsafe {
            libc::fcopyfile(
                source.as_raw_fd(),
                target.as_raw_fd(),
                std::ptr::null_mut(),
                3,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        attributes.apply(target)?;
    }
    #[cfg(target_os = "linux")]
    {
        use xattr::FileExt;
        let mut attributes = Vec::new();
        let mut bytes = 0;
        for name in source.list_xattr()? {
            let value = source.get_xattr(&name)?.ok_or_else(invalid)?;
            bytes += value.len();
            if attributes.len() >= 256 || value.len() > 65536 || bytes > 1024 * 1024 {
                return Err(invalid());
            }
            attributes.push((name, value));
        }
        for name in target.list_xattr()? {
            if !attributes.iter().any(|(saved, _)| saved == &name) {
                target.remove_xattr(&name)?;
            }
        }
        for (name, value) in &attributes {
            target.set_xattr(name, value)?;
        }
        target.set_permissions(std::fs::Permissions::from_mode(original.mode()))?;
        for (name, value) in attributes {
            if target.get_xattr(&name)?.as_ref() != Some(&value) {
                return Err(invalid());
            }
        }
    }
    let copied = target.metadata()?;
    if (original.uid(), original.gid(), original.mode())
        != (copied.uid(), copied.gid(), copied.mode())
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn flush_published(
    target: &Target,
    expected: &ObjectIdentity,
    _storage: &Storage,
    _backup: Option<&str>,
) -> io::Result<()> {
    let file = openat(
        &target.parent,
        &target.name,
        libc::O_RDONLY | libc::O_NONBLOCK,
    )?;
    if !file.metadata()?.is_file() || identity(&file)? != *expected {
        return Err(invalid());
    }
    file.sync_all()
}
pub(super) fn remove_artifact(storage: &Storage, name: &str, _held: File) -> io::Result<()> {
    let directory = &storage.file;
    let name = cstring(name)?;
    // SAFETY: private directory and validated, terminated single component; no recursion.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    directory.sync_all()
}
