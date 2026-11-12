//! Admission-specific error mapping; storage itself is shared with file execution.
use crate::files::storage;
use crate::Result;
use std::{ops::Deref, path::Path};
const PROFILE: storage::Profile = storage::Profile {
    database: "files-admission-v1.sqlite3",
    lock: "files-admission-v1.lock",
    application_id: 0x54464131,
    version: 1,
    maximum: super::contracts::MAX_DATABASE,
    journal_overhead: 65536,
};
pub(super) const DATABASE: &str = PROFILE.database;
pub(super) struct Namespace(storage::Namespace);
impl Deref for Namespace {
    type Target = storage::Namespace;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Namespace {
    pub fn open(path: &Path, initialize: bool) -> Result<Self> {
        storage::Namespace::open(path, initialize, PROFILE)
            .map(Self)
            .map_err(|error| format!("files.admission.{error}").into())
    }
    pub fn validate(&self) -> Result<()> {
        self.0
            .validate()
            .map_err(|error| format!("files.admission.{error}").into())
    }
}
