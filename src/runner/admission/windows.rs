use crate::Result;
use std::{fs::File, os::windows::io::AsRawHandle};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
};

fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut value = std::mem::MaybeUninit::uninit();
    // The OS initializes the complete output structure on success; the handle
    // is borrowed from a live File throughout the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), value.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { value.assume_init() })
}

pub(super) fn validate(file: &File) -> Result<()> {
    let info = information(file)?;
    if info.dwFileAttributes & 0x400 != 0 || info.nNumberOfLinks != 1 {
        return Err("Unsafe runner admission control file".into());
    }
    Ok(())
}

pub(super) fn same(first: &File, second: &File) -> Result<()> {
    let first = information(first)?;
    let second = information(second)?;
    if (
        first.dwVolumeSerialNumber,
        first.nFileIndexHigh,
        first.nFileIndexLow,
    ) != (
        second.dwVolumeSerialNumber,
        second.nFileIndexHigh,
        second.nFileIndexLow,
    ) {
        return Err("Runner admission object changed".into());
    }
    Ok(())
}
