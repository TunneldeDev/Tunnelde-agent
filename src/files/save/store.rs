//! Short, synchronous transactions. Never hold this store across target IO.
use super::{
    contracts::{
        Authority, Context, Control, Intent, ObjectIdentity, Phase, MAX_ACTIVE, MAX_RECORDS,
    },
    filesystem,
};
use crate::{
    files::storage::{Namespace, Profile},
    Result,
};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

const PROFILE: Profile = Profile {
    database: "files-save-v1.sqlite3",
    lock: "files-save-v1.lock",
    application_id: 0x54465331,
    version: 1,
    maximum: 32 * 1024 * 1024,
    journal_overhead: 65536,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Record {
    pub operation_id: String,
    pub generation: String,
    pub intent_sha256: String,
    pub token_digest: String,
    pub revision: u64,
    pub phase: Phase,
    pub intent: Option<Intent>,
    pub artifact: Option<ObjectIdentity>,
    pub lease: Option<ObjectIdentity>,
    pub backup_retained: bool,
    pub published: bool,
    pub error: Option<String>,
}
impl Record {
    fn matches(&self, control: &Control) -> bool {
        self.operation_id == control.operation_id
            && self.generation == control.generation
            && self.intent_sha256 == control.intent_sha256
            && self.token_digest == control.token_digest()
    }
    fn initial(control: &Control, phase: Phase, intent: Option<Intent>) -> Self {
        Self {
            operation_id: control.operation_id.clone(),
            generation: control.generation.clone(),
            intent_sha256: control.intent_sha256.clone(),
            token_digest: control.token_digest(),
            revision: 1,
            phase,
            intent,
            artifact: None,
            lease: None,
            backup_retained: false,
            published: false,
            error: None,
        }
    }
}
pub(super) struct Store {
    connection: Connection,
    namespace: Namespace,
    pub authority: Authority,
}
fn db<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|error| {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
                "files.save.corrupt"
            }
            Some(rusqlite::ErrorCode::DiskFull) => "files.save.capacity",
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                "files.save.busy"
            }
            _ => "files.save.storageUnavailable",
        }
        .into()
    })
}
impl Store {
    pub fn open(path: &Path, initialize: Option<&Context>) -> Result<Self> {
        if initialize.is_some() {
            filesystem::initialize_storage(path)?;
        }
        filesystem::validate_storage(path)?;
        let namespace = Namespace::open(path, initialize.is_some(), PROFILE)
            .map_err(|e| format!("files.save.{e}"))?;
        let mut connection = db(Connection::open_with_flags(
            namespace.path.join(PROFILE.database),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        ))?;
        db(connection.busy_timeout(std::time::Duration::from_secs(2)))?;
        db(connection.pragma_update(None, "trusted_schema", false))?;
        let pages: i64 = db(connection.query_row("PRAGMA page_count", [], |r| r.get(0)))?;
        if pages == 0 && namespace.created_database {
            let scope = filesystem::Scope::open(initialize.ok_or("files.save.notInitialized")?)?;
            let authority = Authority {
                authority_id: uuid::Uuid::new_v4().to_string(),
                context: scope.context,
                root: scope.identity,
                storage_root: filesystem::Storage::open(path)?.identity,
            };
            db(connection.pragma_update(None, "page_size", 4096))?;
            db(connection.pragma_update(None, "synchronous", "EXTRA"))?;
            let transaction =
                db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            db(transaction.execute_batch("PRAGMA application_id=1413894961; PRAGMA user_version=1;
                CREATE TABLE authority (singleton INTEGER PRIMARY KEY CHECK(singleton=1), body TEXT NOT NULL) STRICT;
                CREATE TABLE operations (id TEXT PRIMARY KEY, body TEXT NOT NULL, payload BLOB) STRICT;"))?;
            db(transaction.execute(
                "INSERT INTO authority VALUES(1,?1)",
                [serde_json::to_string(&authority)?],
            ))?;
            commit(transaction, &namespace)?;
        }
        let app: u32 = db(connection.query_row("PRAGMA application_id", [], |r| r.get(0)))?;
        let version: u32 = db(connection.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
        let size: u32 = db(connection.query_row("PRAGMA page_size", [], |r| r.get(0)))?;
        if app != PROFILE.application_id || version != 1 || size != 4096 || pages > 8192 {
            return Err("files.save.corrupt".into());
        }
        let check: String = db(connection.query_row("PRAGMA quick_check(1)", [], |r| r.get(0)))?;
        let journal: String = db(connection.query_row("PRAGMA journal_mode", [], |r| r.get(0)))?;
        if check != "ok" || journal != "delete" {
            return Err("files.save.corrupt".into());
        }
        db(connection.pragma_update(None, "synchronous", "EXTRA"))?;
        let _: u32 = db(connection.query_row("PRAGMA max_page_count=8192", [], |r| r.get(0)))?;
        let json: String = db(connection.query_row(
            "SELECT body FROM authority WHERE singleton=1",
            [],
            |r| r.get(0),
        ))?;
        let authority: Authority = serde_json::from_str(&json).map_err(|_| "files.save.corrupt")?;
        if !super::contracts::valid_uuid(&authority.authority_id) {
            return Err("files.save.corrupt".into());
        }
        let scope = filesystem::Scope::open(&authority.context)?;
        if scope.context != authority.context || scope.identity != authority.root {
            return Err("files.save.scopeChanged".into());
        }
        if filesystem::Storage::open(path)?.identity != authority.storage_root {
            return Err("files.save.storageChanged".into());
        }
        if let Some(context) = initialize {
            if filesystem::Scope::open(context)?.context != authority.context {
                return Err("files.save.identityMismatch".into());
            }
        }
        namespace
            .validate()
            .map_err(|e| format!("files.save.{e}"))?;
        Ok(Self {
            connection,
            namespace,
            authority,
        })
    }
    fn validate(&self, control: &Control) -> Result<()> {
        control.validate()?;
        if control.authority_id != self.authority.authority_id {
            return Err("files.save.identityMismatch".into());
        }
        self.namespace
            .validate()
            .map_err(|e| format!("files.save.{e}"))?;
        Ok(())
    }
    pub fn get(&self, control: &Control) -> Result<Option<Record>> {
        self.validate(control)?;
        let value = read(&self.connection, &control.operation_id)?;
        if value.as_ref().is_some_and(|r| !r.matches(control)) {
            return Err("files.save.identityMismatch".into());
        }
        Ok(value)
    }
    pub fn payload(&self, control: &Control) -> Result<Vec<u8>> {
        self.get(control)?.ok_or("files.save.missing")?;
        db(self.connection.query_row(
            "SELECT payload FROM operations WHERE id=?1",
            [&control.operation_id],
            |r| r.get(0),
        ))
    }
    pub fn submit(&mut self, control: &Control, intent: &Intent, content: &[u8]) -> Result<Record> {
        self.validate(control)?;
        if intent.binding_sha256 != self.authority.binding()?
            || intent.digest()? != control.intent_sha256
            || intent.payload_length != content.len() as u64
            || intent.payload_sha256 != super::contracts::sha256(content)
        {
            return Err("files.save.identityMismatch".into());
        }
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        if let Some(record) = read(&transaction, &control.operation_id)? {
            if !record.matches(control) {
                return Err("files.save.identityMismatch".into());
            }
            return Ok(record);
        }
        capacity(&transaction)?;
        let mut statement = db(transaction.prepare("SELECT body FROM operations"))?;
        let mut rows = db(statement.query([]))?;
        let mut active = 0;
        while let Some(row) = db(rows.next())? {
            let value = decode(&db(row.get::<_, String>(0))?)?;
            if value.phase == Phase::Unknown {
                return Err("files.save.authorityQuarantined".into());
            }
            if value.phase.active()
                && value
                    .intent
                    .as_ref()
                    .is_some_and(|other| other.target == intent.target)
            {
                return Err("files.save.targetBusy".into());
            }
            if value.phase.active() || value.phase == Phase::Unknown {
                active += 1;
            }
        }
        drop(rows);
        drop(statement);
        if active >= MAX_ACTIVE {
            return Err("files.save.capacity".into());
        }
        let retained: i64 = db(transaction.query_row(
            "SELECT coalesce(sum(length(payload)),0) FROM operations",
            [],
            |r| r.get(0),
        ))?;
        if retained < 0
            || retained as usize + content.len() > MAX_ACTIVE * super::contracts::MAX_TEXT
        {
            return Err("files.save.capacity".into());
        }
        let record = Record::initial(control, Phase::Prepared, Some(intent.clone()));
        db(transaction.execute(
            "INSERT INTO operations VALUES(?1,?2,?3)",
            params![
                control.operation_id,
                serde_json::to_string(&record)?,
                content
            ],
        ))?;
        commit(transaction, &self.namespace)?;
        Ok(record)
    }
    pub fn cancel(&mut self, control: &Control) -> Result<Record> {
        self.validate(control)?;
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let mut record = if let Some(record) = read(&transaction, &control.operation_id)? {
            if !record.matches(control) {
                return Err("files.save.identityMismatch".into());
            }
            record
        } else {
            capacity(&transaction)?;
            let record = Record::initial(control, Phase::Cancelled, None);
            db(transaction.execute(
                "INSERT INTO operations VALUES(?1,?2,NULL)",
                params![control.operation_id, serde_json::to_string(&record)?],
            ))?;
            commit(transaction, &self.namespace)?;
            return Ok(record);
        };
        if matches!(record.phase, Phase::Prepared | Phase::Running) {
            record.phase = Phase::Cancelled;
            record.revision += 1;
            write(&transaction, &record)?;
        }
        commit(transaction, &self.namespace)?;
        Ok(record)
    }
    pub fn change(
        &mut self,
        control: &Control,
        update: impl FnOnce(&mut Record) -> Result<()>,
    ) -> Result<Record> {
        self.validate(control)?;
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let mut record = read(&transaction, &control.operation_id)?.ok_or("files.save.missing")?;
        if !record.matches(control) {
            return Err("files.save.identityMismatch".into());
        }
        update(&mut record)?;
        record.revision = record.revision.checked_add(1).ok_or("files.save.corrupt")?;
        write(&transaction, &record)?;
        commit(transaction, &self.namespace)?;
        Ok(record)
    }
    pub fn forget_payload(&mut self, control: &Control) -> Result<()> {
        let record = self.get(control)?.ok_or("files.save.missing")?;
        if !record.phase.terminal_known() && record.phase != Phase::Released {
            return Err("files.save.notSettled".into());
        }
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        db(transaction.execute(
            "UPDATE operations SET payload=NULL WHERE id=?1",
            [&control.operation_id],
        ))?;
        commit(transaction, &self.namespace)
    }
}
fn read(connection: &Connection, id: &str) -> Result<Option<Record>> {
    let body: Option<String> = db(connection
        .query_row("SELECT body FROM operations WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional())?;
    body.map(|s| decode(&s)).transpose()
}
fn decode(json: &str) -> Result<Record> {
    let record: Record = serde_json::from_str(json).map_err(|_| "files.save.corrupt")?;
    if record.revision == 0
        || record.revision > i64::MAX as u64
        || !super::contracts::valid_uuid(&record.operation_id)
        || !super::contracts::valid_uuid(&record.generation)
    {
        return Err("files.save.corrupt".into());
    }
    if record
        .intent
        .as_ref()
        .is_some_and(|i| i.digest().ok().as_ref() != Some(&record.intent_sha256))
    {
        return Err("files.save.corrupt".into());
    }
    Ok(record)
}
fn write(connection: &Connection, record: &Record) -> Result<()> {
    db(connection.execute(
        "UPDATE operations SET body=?2 WHERE id=?1",
        params![record.operation_id, serde_json::to_string(record)?],
    ))?;
    Ok(())
}
fn capacity(connection: &Connection) -> Result<()> {
    let count: u32 = db(connection.query_row("SELECT count(*) FROM operations", [], |r| r.get(0)))?;
    if count >= MAX_RECORDS as u32 {
        return Err("files.save.capacity".into());
    }
    Ok(())
}
fn commit(transaction: Transaction<'_>, namespace: &Namespace) -> Result<()> {
    namespace
        .validate()
        .map_err(|e| format!("files.save.{e}"))?;
    db(transaction.commit())?;
    namespace
        .validate()
        .map_err(|e| format!("files.save.{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
