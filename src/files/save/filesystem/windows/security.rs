//! Copy the original descriptor without merging staging-parent ACEs.
use super::*;
fn descriptor(file: &File) -> io::Result<Vec<usize>> {
    let mut size = 0;
    let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    unsafe {
        GetKernelObjectSecurity(
            file.as_raw_handle(),
            info,
            std::ptr::null_mut(),
            0,
            &mut size,
        );
    }
    let mut bytes = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetKernelObjectSecurity(
            file.as_raw_handle(),
            info,
            bytes.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(bytes)
}
pub(super) fn preserve(source: &File, target: &File) -> io::Result<()> {
    let mut source = descriptor(source)?;
    let mut copied = descriptor(target)?;
    type SidGetter = unsafe extern "system" fn(PSECURITY_DESCRIPTOR, *mut PSID, *mut i32) -> i32;
    // Never acquire ownership privileges silently; fail before publication if
    // the source's owner/group cannot be represented by this private stage.
    for get in [
        GetSecurityDescriptorOwner as SidGetter,
        GetSecurityDescriptorGroup as SidGetter,
    ] {
        let mut original = std::ptr::null_mut();
        let mut observed = std::ptr::null_mut();
        let mut defaulted = 0;
        if unsafe { get(source.as_mut_ptr().cast(), &mut original, &mut defaulted) } == 0
            || unsafe { get(copied.as_mut_ptr().cast(), &mut observed, &mut defaulted) } == 0
            || original.is_null()
            || observed.is_null()
            || unsafe { EqualSid(original, observed) } == 0
        {
            return Err(invalid());
        }
    }
    let mut control = 0;
    let mut revision = 0;
    if unsafe {
        GetSecurityDescriptorControl(source.as_mut_ptr().cast(), &mut control, &mut revision)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let info = DACL_SECURITY_INFORMATION
        | if control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
    // SAFETY: live WRITE_DAC handle and a valid self-relative descriptor; no
    // mutable pathname is used to update the staging file's security.
    if unsafe { SetKernelObjectSecurity(target.as_raw_handle(), info, source.as_mut_ptr().cast()) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut after = descriptor(target)?;
    if sddl(&mut source)? != sddl(&mut after)? {
        return Err(invalid());
    }
    Ok(())
}
fn sddl(data: &mut [usize]) -> io::Result<String> {
    // Windows resets automatic propagation bookkeeping when applying a
    // descriptor. Keep owner/group, ACE order/flags and protection significant.
    if unsafe {
        SetSecurityDescriptorControl(
            data.as_mut_ptr().cast(),
            SE_DACL_AUTO_INHERITED | SE_DACL_AUTO_INHERIT_REQ,
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut text = std::ptr::null_mut();
    let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            data.as_mut_ptr().cast(),
            1,
            info,
            &mut text,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _allocated = Local(text.cast());
    let mut len = 0;
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(text, len) }).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserve_inherited_source_acl_in_private_stage() {
        let root =
            std::env::temp_dir().join(format!("tunnelde-save-security-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source");
        std::fs::write(&source, b"old").unwrap();
        let state = root.join("state");
        initialize_storage(&state).unwrap();
        let storage = Storage::open(&state).unwrap();
        let stage = create_stage(&storage, "stage").unwrap();
        let original = open_regular(&source).unwrap();
        let result = preserve(&original, &stage);
        let expected = sddl(&mut descriptor(&original).unwrap()).unwrap();
        let observed = sddl(&mut descriptor(&stage).unwrap()).unwrap();
        drop(stage);
        drop(original);
        drop(storage);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(observed, expected, "{result:?}");
        assert!(result.is_ok());
    }
}
