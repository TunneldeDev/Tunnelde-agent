//! Retained directory handles for read-only observation. These checks are NOT an
//! atomic filesystem snapshot, namespace lock, ownership proof or delete primitive.
use crate::Result;
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};
#[cfg(windows)]
mod windows;

pub(super) struct Directory {
    file: File,
    path: PathBuf,
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() || redirected(&metadata) {
            return Err("Unsafe runner observation directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o077 != 0 {
                return Err("Runner observation directory is not private".into());
            }
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options
                .share_mode(0x1 | 0x2)
                .custom_flags(0x02000000 | 0x00200000);
        }
        let file = options.open(path)?;
        let value = Self {
            file,
            path: path.into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        let current = std::fs::symlink_metadata(&self.path)?;
        if !current.is_dir() || redirected(&current) {
            return Err("Runner observation namespace changed".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let retained = self.file.metadata()?;
            if current.mode() & 0o077 != 0
                || (current.dev(), current.ino()) != (retained.dev(), retained.ino())
            {
                return Err("Runner observation namespace changed".into());
            }
        }
        #[cfg(windows)]
        windows::validate(&self.file, &self.path)?;
        Ok(())
    }
}

pub(super) fn redirected(value: &std::fs::Metadata) -> bool {
    if value.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if value.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn retained_observation_directory_detects_or_prevents_replacement() {
        let path = std::env::temp_dir().join(format!(
            "tunnelde-observation-anchor-{}",
            uuid::Uuid::new_v4()
        ));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&path).unwrap();
        let _fixture = Fixture(path.clone());
        let root = path.join("task");
        builder.create(&root).unwrap();
        let anchor = Directory::open(&root).unwrap();
        anchor.validate().unwrap();
        let replaced = std::fs::rename(&root, path.join("retained"));
        #[cfg(windows)]
        assert!(
            replaced.is_err(),
            "retained directory handle denies namespace replacement"
        );
        #[cfg(unix)]
        {
            replaced.unwrap();
            builder.create(&root).unwrap();
            assert!(anchor.validate().is_err());
        }
    }
}
