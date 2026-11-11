use super::{
    contracts::{Entry, State, MAX_JOURNAL, MAX_TASKS},
    locking::{self, Lock},
};
use crate::Result;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

pub(super) struct Journal {
    file: File,
    pub state: Option<State>,
}

impl Journal {
    pub fn validate(&self, lock: &Lock, id: &str) -> Result<()> {
        lock.validate()?;
        locking::validate_file(&self.file, &lock.path.join(format!("{id}.jsonl")))
    }

    pub fn open(lock: &Lock, id: &str, token: &str, create: bool) -> Result<Self> {
        Self::open_inner(lock, id, token, create, false)
    }

    pub fn observe(lock: &Lock, id: &str, token: &str) -> Result<Self> {
        Self::open_inner(lock, id, token, false, true)
    }

    fn open_inner(
        lock: &Lock,
        id: &str,
        token: &str,
        create: bool,
        read_only: bool,
    ) -> Result<Self> {
        lock.validate()?;
        let path = lock.path.join(format!("{id}.jsonl"));
        let exists = path.try_exists()?;
        if exists {
            locking::metadata(&path, false)?;
        } else if create {
            // Serialized with all cooperative creators. Includes malformed/user
            // additions in the bound; never evict another task's cancellation.
            if std::fs::read_dir(&lock.path)?.take(MAX_TASKS + 1).count() > MAX_TASKS {
                return Err("Runner admission capacity reached".into());
            }
        }
        let mut options = locking::options(create);
        if read_only {
            options.write(false);
        }
        let mut file = options.open(&path)?;
        let observed = locking::metadata(&path, false)?;
        locking::validate_file(&file, &path)?;
        if observed.len() > MAX_JOURNAL {
            return Err("Runner admission journal exceeds limit".into());
        }
        let mut bytes = Vec::new();
        (&mut file).take(MAX_JOURNAL + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_JOURNAL || (!bytes.is_empty() && bytes.last() != Some(&b'\n')) {
            return Err("Invalid runner admission journal".into());
        }
        let mut state = None;
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            let entry: Entry =
                serde_json::from_slice(line).map_err(|_| "Invalid runner admission journal")?;
            if entry.schema_version != 1 || entry.runner_id != id || entry.token != token {
                return Err("Runner admission identity mismatch".into());
            }
            match (state, entry.admission) {
                (None, State::Reserved | State::Cancelled)
                | (Some(State::Reserved), State::Launched | State::Cancelled)
                | (Some(State::Launched), State::Cancelled) => state = Some(entry.admission),
                _ => return Err("Invalid runner admission transition".into()),
            }
        }
        Ok(Self { file, state })
    }

    pub fn append(&mut self, id: &str, token: &str, state: State) -> Result<()> {
        let mut bytes = serde_json::to_vec(&Entry {
            schema_version: 1,
            runner_id: id.into(),
            token: token.into(),
            admission: state,
        })?;
        bytes.push(b'\n');
        let size = self.file.seek(SeekFrom::End(0))?;
        if size + bytes.len() as u64 > MAX_JOURNAL {
            return Err("Runner admission journal exceeds limit".into());
        }
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        self.state = Some(state);
        Ok(())
    }
}
