//! Private local storage primitives, independent of a particular wire protocol.
mod namespace;
pub(crate) use namespace::Namespace;

#[derive(Clone, Copy)]
pub(crate) struct Profile {
    pub database: &'static str,
    pub lock: &'static str,
    pub application_id: u32,
    pub version: u32,
    pub maximum: u64,
    pub journal_overhead: u64,
}

#[derive(Debug)]
pub(crate) enum Failure {
    StorageUnavailable,
    UnsafeStorage,
    StorageChanged,
    Capacity,
    Corrupt,
    NotInitialized,
    Busy,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StorageUnavailable => "storageUnavailable",
            Self::UnsafeStorage => "unsafeStorage",
            Self::StorageChanged => "storageChanged",
            Self::Capacity => "capacity",
            Self::Corrupt => "corrupt",
            Self::NotInitialized => "notInitialized",
            Self::Busy => "busy",
        })
    }
}
impl std::error::Error for Failure {}
