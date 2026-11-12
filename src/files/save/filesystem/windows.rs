use super::{ObjectIdentity, Storage, Target};
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::Path,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};
mod security;

fn invalid() -> io::Error {
    io::Error::other("Unsupported or changed private file namespace")
}
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(invalid());
    }
    value.push(0);
    Ok(value)
}
pub(super) fn directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let md = file.metadata()?;
    if !md.is_dir() || md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(invalid());
    }
    Ok(file)
}
pub(super) fn identity(file: &File) -> io::Result<ObjectIdentity> {
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: live handle; successful call initializes the output.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful call above initialized the structure.
    let info = unsafe { info.assume_init() };
    Ok(ObjectIdentity {
        platform: "windows".into(),
        device: info.dwVolumeSerialNumber.to_string(),
        object: format!("{}:{}", info.nFileIndexHigh, info.nFileIndexLow),
        created: format!(
            "{}:{}",
            info.ftCreationTime.dwHighDateTime, info.ftCreationTime.dwLowDateTime
        ),
    })
}
pub(super) fn target(root: &File, path: &Path, parts: &[String]) -> io::Result<Target> {
    let text = path.to_str().ok_or_else(invalid)?;
    // This protocol intentionally supports local volumes only, not UNC/SMB.
    if text.starts_with(r"\\?\UNC\") || (text.starts_with(r"\\") && !text.starts_with(r"\\?\")) {
        return Err(invalid());
    }
    let mut ancestors = Vec::new();
    let mut dirs: Vec<_> = path.ancestors().filter(|p| p.has_root()).collect();
    dirs.reverse();
    for ancestor in dirs {
        ancestors.push(directory(ancestor)?);
    }
    let mut parent = ancestors.pop().ok_or_else(invalid)?;
    if identity(&parent)? != identity(root)? {
        return Err(invalid());
    }
    let mut target_path = path.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        target_path.push(part);
        let next = directory(&target_path)?;
        ancestors.push(parent);
        parent = next;
    }
    let name = parts.last().ok_or_else(invalid)?.clone();
    target_path.push(&name);
    Ok(Target {
        _ancestors: ancestors,
        parent,
        path: target_path,
    })
}
pub(super) fn open_target(target: &Target) -> io::Result<File> {
    open_regular(&target.path)
}
pub(super) fn open_artifact(storage: &Storage, name: &str) -> io::Result<File> {
    open_regular(&storage.path.join(name))
}
fn open_regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let md = file.metadata()?;
    if !md.is_file() || md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(invalid());
    }
    Ok(file)
}
pub(super) fn create_stage(storage: &Storage, name: &str) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .create_new(true)
        .open(storage.path.join(name))
}
pub(super) fn publish(
    target: &Target,
    storage: &Storage,
    stage: &str,
    replace: bool,
) -> io::Result<()> {
    let stage_path = storage.path.join(stage);
    let stage = wide(&stage_path)?;
    let target_path = wide(&target.path)?;
    let result = if replace {
        let backup_path = stage_path.with_file_name(format!(
            "{}.backup",
            stage_path
                .file_name()
                .ok_or_else(invalid)?
                .to_string_lossy()
        ));
        if std::fs::symlink_metadata(&backup_path).is_ok() {
            return Err(invalid());
        }
        let backup = wide(&backup_path)?;
        // No IGNORE_MERGE_ERRORS/IGNORE_ACL_ERRORS: native metadata/ADS merging
        // must succeed. The actual displaced object is retained by the OS.
        // SAFETY: terminated paths live throughout the call; optional arguments null.
        unsafe {
            ReplaceFileW(
                target_path.as_ptr(),
                stage.as_ptr(),
                backup.as_ptr(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        }
    } else {
        // Without REPLACE_EXISTING, an existing target can never be overwritten.
        // SAFETY: both terminated paths remain live.
        unsafe { MoveFileExW(stage.as_ptr(), target_path.as_ptr(), MOVEFILE_WRITE_THROUGH) }
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub(super) fn metadata(source: &File, target: &File) -> io::Result<()> {
    // ReplaceFile alone can re-inherit the private staging parent's DACL.
    // Preserve and verify the original security before publication as well.
    security::preserve(source, target)
}
pub(super) fn flush_published(
    target: &Target,
    expected: &ObjectIdentity,
    storage: &Storage,
    backup: Option<&str>,
) -> io::Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&target.path)?;
    let actual = identity(&file)?;
    if !file.metadata()?.is_file()
        || file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || actual.device != expected.device
        || actual.object != expected.object
    {
        return Err(invalid());
    }
    if let Some(backup) = backup {
        // ReplaceFile can re-inherit the private staging parent's DACL while
        // moving the replacement. Restore actual displaced security through
        // the verified published handle before recording any success receipt.
        // An interruption here remains an unknown publication with its backup.
        let original = open_artifact(storage, backup)?;
        security::preserve(&original, &file)?;
    }
    file.sync_all()
}
pub(super) fn remove_artifact(storage: &Storage, name: &str, held: File) -> io::Result<()> {
    let mut info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // Reopen with DELETE access through the held identity is not available via
    // std::File. Open a delete handle and compare before marking that handle.
    let file = OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(storage.path.join(name))?;
    if identity(&file)? != identity(&held)? {
        return Err(invalid());
    }
    // SAFETY: retained delete handle and initialized information buffer.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&mut info as *mut FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Token(HANDLE);
impl Drop for Token {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn current_user() -> io::Result<Vec<usize>> {
    let mut token = std::ptr::null_mut();
    // SAFETY: valid process pseudo handle, initialized token output.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Token(token);
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut size);
    }
    let mut user = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            user.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(user)
}
pub(super) fn initialize_storage(path: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(path).is_ok() {
        return validate_storage(path);
    }
    let user = current_user()?;
    // SAFETY: aligned TOKEN_USER buffer initialized by GetTokenInformation.
    let sid = unsafe { &*user.as_ptr().cast::<TOKEN_USER>() }.User.Sid;
    let mut text = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let allocation = Local(text.cast());
    let mut len = 0;
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(text, len) })
        .map_err(|_| invalid())?;
    drop(allocation);
    let sddl: Vec<_> = format!("D:P(A;OICI;FA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let allocation = Local(descriptor);
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name = wide(path)?;
    let result = unsafe { CreateDirectoryW(name.as_ptr(), &attrs) };
    drop(allocation);
    if result == 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
        return Err(io::Error::last_os_error());
    }
    validate_storage(path)
}
pub(super) fn validate_storage(path: &Path) -> io::Result<()> {
    let canonical = std::fs::canonicalize(path)?;
    let text = canonical.to_str().ok_or_else(invalid)?;
    if text.starts_with(r"\\?\UNC\") || !text.starts_with(r"\\?\") {
        return Err(invalid());
    }
    let mut volume = vec![0u16; 32768];
    let name = wide(&canonical)?;
    // Reject network mappings as well as UNC paths. This native protocol does
    // not claim SQLite/rename durability on a remote share.
    if unsafe { GetVolumePathNameW(name.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } == 0
        || unsafe { GetDriveTypeW(volume.as_ptr()) } != 3
    // DRIVE_FIXED
    {
        return Err(invalid());
    }
    let directory = directory(path)?;
    let user = current_user()?;
    let user = unsafe { &*user.as_ptr().cast::<TOKEN_USER>() };
    let mut descriptor = std::ptr::null_mut();
    let mut owner = std::ptr::null_mut();
    let mut dacl = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _allocation = Local(descriptor);
    let mut control = 0;
    let mut revision = 0;
    if owner.is_null()
        || dacl.is_null()
        || unsafe { EqualSid(owner, user.User.Sid) } == 0
        || unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(invalid());
    }
    // Only explicit current-user allow ACEs are accepted. Unknown, inherited or
    // callback ACEs are rejected, rather than inferring privacy from attributes.
    for i in 0..unsafe { (*dacl).AceCount } {
        let mut ace = std::ptr::null_mut();
        if unsafe { GetAce(dacl, i as u32, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if ace.Header.AceType != 0
            || ace.Header.AceFlags & INHERITED_ACE as u8 != 0
            || unsafe {
                EqualSid(
                    (&ace.SidStart as *const u32).cast_mut().cast(),
                    user.User.Sid,
                )
            } == 0
        {
            return Err(invalid());
        }
    }
    Ok(())
}
