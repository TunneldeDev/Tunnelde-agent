//! Narrow read-only guard. Never creates a reservation or settles a mutation.
use super::{contracts, journal::Journal, locking::Lock};
use crate::Result;
use std::path::Path;

pub(in crate::runner) struct Observation {
    lock: Lock,
    journal: Journal,
}

impl Observation {
    pub async fn open(directory: &Path, id: &str, token: &str) -> Result<Self> {
        contracts::token(token)?;
        let lock = Lock::acquire(directory, false).await?;
        let journal = Journal::observe(&lock, id, token)?;
        if journal.state.is_none() {
            return Err("Incomplete runner admission journal".into());
        }
        Ok(Self { lock, journal })
    }

    pub fn state(&self) -> &'static str {
        match self.journal.state.expect("validated complete journal") {
            contracts::State::Reserved => "reserved",
            contracts::State::Launched => "launched",
            contracts::State::Cancelled => "cancelled",
        }
    }

    pub fn validate(&self, id: &str) -> Result<()> {
        self.journal.validate(&self.lock, id)
    }
}
