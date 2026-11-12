//! Retained scope and private staging, independent of the execution journal.
use super::contracts::{sha256, Baseline, Context, ObjectIdentity, MAX_TEXT};
use crate::Result;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

pub(super) struct Scope {
    pub context: Context,
    pub identity: ObjectIdentity,
    root: File,
}
impl Scope {
    pub fn open(context: &Context) -> Result<Self> {
        let path = Path::new(&context.scope);
        let root = platform::directory(path).map_err(|_| "files.save.unsafeScope")?;
        let canonical = std::fs::canonicalize(path).map_err(|_| "files.save.unsafeScope")?;
        let observed = platform::directory(&canonical).map_err(|_| "files.save.unsafeScope")?;
        let identity = platform::identity(&root).map_err(|_| "files.save.unsupportedFilesystem")?;
        if identity != platform::identity(&observed)? {
            return Err("files.save.scopeChanged".into());
        }
        let mut context = context.clone();
        context.scope = canonical.to_str().ok_or("files.save.unsafeScope")?.into();
        Ok(Self {
            context,
            identity,
            root,
        })
    }
    pub fn target(&self, parts: &[String]) -> Result<Target> {
        platform::target(&self.root, Path::new(&self.context.scope), parts)
            .map_err(|_| "files.save.unsafeTarget".into())
    }
}

pub(super) struct Storage {
    pub identity: ObjectIdentity,
    file: File,
    path: std::path::PathBuf,
}
impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        validate_storage(path)?;
        let file = platform::directory(path).map_err(|_| "files.save.unsafeStorage")?;
        Ok(Self {
            identity: identity(&file)?,
            file,
            path: path.into(),
        })
    }
    pub fn validate(&self) -> Result<()> {
        validate_storage(&self.path)?;
        if identity(&self.file)? != self.identity
            || Self::open(&self.path)?.identity != self.identity
        {
            return Err("files.save.storageChanged".into());
        }
        Ok(())
    }
}

pub(super) struct Target {
    // Held Windows directories deny delete; Unix operations use the retained
    // parent descriptor. Neither is a snapshot of another writer's file bytes.
    _ancestors: Vec<File>,
    parent: File,
    #[cfg(windows)]
    path: std::path::PathBuf,
    #[cfg(unix)]
    name: String,
}
impl Target {
    pub fn baseline(&self, expected: &Baseline) -> Result<Option<File>> {
        let file = match platform::open_target(self) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return if matches!(expected, Baseline::Absent) {
                    Ok(None)
                } else {
                    Err("files.save.baselineChanged".into())
                };
            }
            Err(_) => return Err("files.save.unsafeTarget".into()),
        };
        if !matches!(expected, Baseline::Existing { .. }) || !matches_baseline(&file, expected)? {
            return Err("files.save.baselineChanged".into());
        }
        Ok(Some(file))
    }
    pub fn stage(&self, storage: &Storage, name: &str) -> Result<File> {
        storage.validate()?;
        if storage.identity.device != platform::identity(&self.parent)?.device {
            return Err("files.save.crossFilesystem".into());
        }
        platform::create_stage(storage, name).map_err(|_| "files.save.stageUnavailable".into())
    }
    pub fn publish(&self, storage: &Storage, name: &str, replace: bool) -> Result<()> {
        storage.validate()?;
        platform::publish(self, storage, name, replace).map_err(|error| {
            format!(
                "files.save.publicationUnknown.os{}",
                error.raw_os_error().unwrap_or(0)
            )
            .into()
        })
    }
    pub fn flush_published(
        &self,
        expected: &ObjectIdentity,
        storage: &Storage,
        backup: Option<&str>,
    ) -> Result<()> {
        platform::flush_published(self, expected, storage, backup)
            .map_err(|_| "files.save.publicationUnknown".into())
    }
}

pub(super) fn identity(file: &File) -> Result<ObjectIdentity> {
    platform::identity(file).map_err(|_| "files.save.unsupportedFilesystem".into())
}
pub(super) fn initialize_storage(path: &Path) -> Result<()> {
    platform::initialize_storage(path).map_err(|_| "files.save.unsafeStorage".into())
}
pub(super) fn validate_storage(path: &Path) -> Result<()> {
    platform::validate_storage(path).map_err(|_| "files.save.unsafeStorage".into())
}
pub(super) fn stage_name(operation: &str) -> String {
    format!("save-{operation}.stage")
}
pub(super) fn backup_name(operation: &str) -> String {
    if cfg!(windows) {
        format!("save-{operation}.stage.backup")
    } else {
        stage_name(operation)
    }
}
pub(super) fn artifact(storage: &Storage, name: &str) -> Result<File> {
    storage.validate()?;
    platform::open_artifact(storage, name).map_err(|_| "files.save.artifactChanged".into())
}
pub(super) fn matches_baseline(file: &File, expected: &Baseline) -> Result<bool> {
    let Baseline::Existing {
        sha256: digest,
        length,
    } = expected
    else {
        return Ok(false);
    };
    let before = file.metadata().map_err(|_| "files.save.readFailed")?;
    if !before.is_file() || before.len() != *length || before.len() > MAX_TEXT as u64 {
        return Ok(false);
    }
    let mut reader = file;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| "files.save.readFailed")?;
    let mut bytes = Vec::new();
    reader
        .take(MAX_TEXT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "files.save.readFailed")?;
    let after = file.metadata().map_err(|_| "files.save.readFailed")?;
    Ok(before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && bytes.len() as u64 == *length
        && sha256(&bytes) == *digest)
}
pub(super) fn write_stage(file: &mut File, bytes: &[u8], original: Option<&File>) -> Result<()> {
    file.write_all(bytes)
        .map_err(|_| "files.save.stageUnavailable")?;
    if let Some(original) = original {
        platform::metadata(original, file).map_err(|_| "files.save.metadataUnsupported")?;
    }
    // Metadata cloning (notably macOS COPYFILE_STAT) must not restore the old
    // modification time over newly edited content.
    file.set_modified(std::time::SystemTime::now())
        .map_err(|_| "files.save.metadataUnsupported")?;
    file.sync_all().map_err(|_| "files.save.stageUnavailable")?;
    if !matches_baseline(
        file,
        &Baseline::Existing {
            sha256: sha256(bytes),
            length: bytes.len() as u64,
        },
    )? {
        return Err("files.save.stageVerificationFailed".into());
    }
    Ok(())
}
pub(super) fn release_artifact(
    storage: &Storage,
    name: &str,
    expected: &ObjectIdentity,
) -> Result<()> {
    let file = artifact(storage, name)?;
    if identity(&file)? != *expected {
        return Err("files.save.artifactChanged".into());
    }
    platform::remove_artifact(storage, name, file).map_err(|_| "files.save.artifactChanged".into())
}
