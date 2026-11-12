//! Bounded macOS xattrs, including resource forks. Never let COPYFILE_XATTR
//! silently expand a 2 MiB editor save into an unbounded metadata copy.
use super::*;
use xattr::FileExt;
const MAX_VALUE: usize = 65536;
const MAX_TOTAL: usize = 1024 * 1024;
pub(super) struct Attributes(Vec<(CString, Vec<u8>)>);
fn names(file: &File) -> io::Result<Vec<CString>> {
    // SAFETY: retained descriptor; null/zero asks only for the required length.
    let size = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0, 0) };
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    if size as usize > 65536 {
        return Err(invalid());
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut bytes = vec![0u8; size as usize];
    let read =
        unsafe { libc::flistxattr(file.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    if read as usize > bytes.len() {
        return Err(invalid());
    }
    bytes.truncate(read as usize);
    let mut result = Vec::new();
    for name in bytes.split(|b| *b == 0).filter(|name| !name.is_empty()) {
        if result.len() >= 256 || name.len() > 255 {
            return Err(invalid());
        }
        result.push(CString::new(name).map_err(|_| invalid())?);
    }
    Ok(result)
}
fn value(file: &File, name: &CString) -> io::Result<Vec<u8>> {
    // SAFETY: live fd/terminated name. Both calls remain bounded; growth yields
    // ERANGE rather than an automatic allocation/retry with a larger size.
    let size = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    if size as usize > MAX_VALUE {
        return Err(invalid());
    }
    let mut bytes = vec![0; MAX_VALUE];
    let read = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            0,
            0,
        )
    };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    if read as usize > MAX_VALUE {
        return Err(invalid());
    }
    bytes.truncate(read as usize);
    Ok(bytes)
}
impl Attributes {
    pub fn read(file: &File) -> io::Result<Self> {
        let mut attributes = Vec::new();
        let mut total = 0;
        for name in names(file)? {
            let value = value(file, &name)?;
            total += value.len();
            if total > MAX_TOTAL {
                return Err(invalid());
            }
            attributes.push((name, value));
        }
        Ok(Self(attributes))
    }
    pub fn apply(&self, target: &File) -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        for name in names(target)? {
            if !self.0.iter().any(|(original, _)| *original == name) {
                target.remove_xattr(std::ffi::OsStr::from_bytes(name.as_bytes()))?;
            }
        }
        for (name, bytes) in &self.0 {
            target.set_xattr(std::ffi::OsStr::from_bytes(name.as_bytes()), bytes)?;
            if value(target, name)? != *bytes {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_forks_are_bounded_before_copy_and_small_attributes_survive() {
        let root =
            std::env::temp_dir().join(format!("tunnelde-save-attributes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let open = |name: &str| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(root.join(name))
                .unwrap()
        };
        let original = open("source");
        let target = open("target");
        original.set_xattr("user.tunnelde", b"metadata").unwrap();
        let attributes = Attributes::read(&original).unwrap();
        attributes.apply(&target).unwrap();
        assert_eq!(
            target.get_xattr("user.tunnelde").unwrap().unwrap(),
            b"metadata"
        );
        original
            .set_xattr("com.apple.ResourceFork", &vec![0u8; MAX_VALUE + 1])
            .unwrap();
        assert!(Attributes::read(&original).is_err());
        drop(original);
        drop(target);
        std::fs::remove_dir_all(root).unwrap();
    }
}
