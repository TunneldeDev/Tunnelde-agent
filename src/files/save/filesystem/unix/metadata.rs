//! Copy editor metadata without changing the stage's durable object identity.
use super::*;
use std::os::macos::fs::MetadataExt as _;

pub(super) fn copy(source: &File, target: &File, original: &std::fs::Metadata) -> io::Result<()> {
    let attributes = attributes::Attributes::read(source)?;
    let flags = original.st_flags();
    // Other flags may encode old payloads, require privileges, or prevent
    // publication/cleanup. Reject them before placing any on the owned stage.
    let supported = libc::UF_NODUMP | libc::UF_HIDDEN | libc::UF_TRACKED;
    if flags & !supported != 0 {
        return Err(invalid());
    }
    // COPYFILE_STAT can lower birthtime when it restores an older mtime. The
    // identity recorded before writing must remain valid even if copying dies.
    // Copy mode explicitly, then ACLs; never restore source timestamps.
    // SAFETY: retained descriptor and mode obtained from the source metadata.
    if unsafe {
        libc::fchmod(
            target.as_raw_fd(),
            (original.mode() & 0o7777) as libc::mode_t,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: retained descriptors, null optional state, ACL-only copy.
    if unsafe {
        libc::fcopyfile(
            source.as_raw_fd(),
            target.as_raw_fd(),
            std::ptr::null_mut(),
            libc::COPYFILE_ACL,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    attributes.apply(target)?;
    // SAFETY: retained descriptor; only supported, owner-settable flags.
    if unsafe { libc::fchflags(target.as_raw_fd(), flags) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if target.metadata()?.st_flags() != flags {
        return Err(invalid());
    }
    Ok(())
}
