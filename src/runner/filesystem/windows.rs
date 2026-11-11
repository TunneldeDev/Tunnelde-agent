use crate::Result;
use std::{
    fs::{File, OpenOptions},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
};

fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut result = std::mem::MaybeUninit::uninit();
    // Borrowed live handle; the OS initializes the output on success.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), result.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { result.assume_init() })
}

pub(super) fn validate(file: &File, path: &Path) -> Result<()> {
    let current = OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .custom_flags(0x02000000 | 0x00200000)
        .open(path)?;
    let first = information(file)?;
    let second = information(&current)?;
    if first.dwFileAttributes & 0x400 != 0
        || second.dwFileAttributes & 0x400 != 0
        || (
            first.dwVolumeSerialNumber,
            first.nFileIndexHigh,
            first.nFileIndexLow,
        ) != (
            second.dwVolumeSerialNumber,
            second.nFileIndexHigh,
            second.nFileIndexLow,
        )
    {
        return Err("Runner observation namespace changed".into());
    }
    Ok(())
}
