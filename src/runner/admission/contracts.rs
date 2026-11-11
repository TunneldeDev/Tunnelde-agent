use crate::Result;
use serde::{Deserialize, Serialize};

pub(super) const MAX_TASKS: usize = 4096;
pub(super) const MAX_JOURNAL: u64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum State {
    Reserved,
    Launched,
    Cancelled,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Entry {
    pub schema_version: u32,
    pub runner_id: String,
    pub token: String,
    pub admission: State,
}

pub(super) fn token(value: &str) -> Result<()> {
    if uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.hyphenated().to_string() == value && !id.is_nil())
    {
        Ok(())
    } else {
        Err("Invalid runner admission token".into())
    }
}
