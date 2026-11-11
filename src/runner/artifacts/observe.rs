use super::contracts::{Artifact, Reason, Snapshot, KNOWN, MAX_ENTRIES};
use crate::runner::filesystem::{redirected, Directory};
use std::path::Path;

pub(super) fn snapshot(root: &Path) -> Result<Snapshot, Reason> {
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Snapshot::Missing),
        Err(_) => return Err(Reason::MetadataUnavailable),
        Ok(_) => {}
    }
    let anchor = Directory::open(root).map_err(|_| Reason::UnsafeDirectory)?;
    let mut entries = Vec::new();
    let mut other_entries = 0;
    for (index, entry) in std::fs::read_dir(root)
        .map_err(|_| Reason::MetadataUnavailable)?
        .enumerate()
    {
        if index >= MAX_ENTRIES {
            return Err(Reason::EnumerationLimit);
        }
        let entry = entry.map_err(|_| Reason::MetadataUnavailable)?;
        let name = entry.file_name();
        let Some(known) = KNOWN.iter().copied().find(|known| name == *known) else {
            other_entries += 1;
            continue;
        };
        // symlink_metadata never reads a target's contents or follows a leaf link.
        let metadata =
            std::fs::symlink_metadata(entry.path()).map_err(|_| Reason::MetadataUnavailable)?;
        let kind = if redirected(&metadata) {
            "link"
        } else if metadata.is_file() {
            "file"
        } else if metadata.is_dir() {
            "directory"
        } else {
            "other"
        };
        entries.push(Artifact {
            name: known,
            kind,
            size_bytes: (kind == "file").then(|| metadata.len().to_string()),
        });
    }
    anchor.validate().map_err(|_| Reason::NamespaceChanged)?;
    entries.sort_by_key(|entry| KNOWN.iter().position(|name| *name == entry.name));
    Ok(Snapshot::Present {
        entries,
        other_entries,
    })
}
