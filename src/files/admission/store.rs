use super::contracts::{Identity, Observation, Request, Response, State, MAX_OPERATIONS};
use super::namespace::{Namespace, DATABASE};
use crate::Result;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::path::Path;

const APPLICATION_ID: i64 = 0x54464131; // TFA1, separate from the runner protocol.

pub(super) struct Store {
    // Connection must close before releasing the retained namespace and lock.
    connection: Connection,
    namespace: Namespace,
    authority_id: String,
}

fn database<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|error| {
        use rusqlite::ErrorCode;
        match error.sqlite_error_code() {
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => "files.admission.corrupt",
            Some(ErrorCode::DiskFull) => "files.admission.capacity",
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => "files.admission.busy",
            _ => "files.admission.storageUnavailable",
        }
        .into()
    })
}

impl Store {
    pub fn open(path: &Path, initialize: bool) -> Result<Self> {
        let namespace = Namespace::open(path, initialize)?;
        let mut connection = database(Connection::open_with_flags(
            namespace.path.join(DATABASE),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        ))?;
        database(connection.busy_timeout(std::time::Duration::from_secs(2)))?;
        database(connection.pragma_update(None, "trusted_schema", false))?;
        // Do not change or repair an unrecognized nonempty database.
        let existing: i64 =
            database(connection.query_row("PRAGMA page_count", [], |row| row.get(0)))?;
        if existing == 0 && namespace.created_database && initialize {
            database(connection.pragma_update(None, "page_size", 4096))?;
            database(connection.pragma_update(None, "synchronous", "EXTRA"))?;
            let transaction =
                database(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            database(transaction.execute_batch("PRAGMA application_id=1413890353; PRAGMA user_version=1;
                CREATE TABLE authority (singleton INTEGER PRIMARY KEY CHECK(singleton=1), id TEXT NOT NULL) STRICT;
                CREATE TABLE operations (id TEXT PRIMARY KEY, generation TEXT NOT NULL,
                    binding TEXT NOT NULL, intent TEXT NOT NULL, token_digest TEXT NOT NULL,
                    state TEXT NOT NULL CHECK(state IN ('reserved','cancelled')),
                    revision INTEGER NOT NULL CHECK(revision IN (1,2))) STRICT;"))?;
            database(transaction.execute(
                "INSERT INTO authority VALUES(1,?1)",
                [uuid::Uuid::new_v4().to_string()],
            ))?;
            database(transaction.commit())?;
        }
        let app: i64 =
            database(connection.query_row("PRAGMA application_id", [], |row| row.get(0)))?;
        let version: i64 =
            database(connection.query_row("PRAGMA user_version", [], |row| row.get(0)))?;
        if app != APPLICATION_ID || version != 1 {
            return Err("files.admission.corrupt".into());
        }
        let page_size: u32 =
            database(connection.query_row("PRAGMA page_size", [], |row| row.get(0)))?;
        if page_size != 4096 || existing > 1024 {
            return Err("files.admission.corrupt".into());
        }
        let check: String =
            database(connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0)))?;
        if check != "ok" {
            return Err("files.admission.corrupt".into());
        }
        let mode: String =
            database(connection.query_row("PRAGMA journal_mode", [], |row| row.get(0)))?;
        if mode != "delete" {
            return Err("files.admission.corrupt".into());
        }
        database(connection.pragma_update(None, "synchronous", "EXTRA"))?;
        let _: i64 =
            database(connection.query_row("PRAGMA max_page_count=1024", [], |row| row.get(0)))?;
        let authority_id: String = database(connection.query_row(
            "SELECT id FROM authority WHERE singleton=1",
            [],
            |row| row.get(0),
        ))?;
        if !uuid::Uuid::parse_str(&authority_id)
            .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == authority_id)
        {
            return Err("files.admission.corrupt".into());
        }
        namespace.validate()?;
        Ok(Self {
            connection,
            namespace,
            authority_id,
        })
    }

    pub fn apply<'a>(&mut self, request: &'a Request) -> Result<Response<'a>> {
        request.validate()?;
        self.namespace.validate()?;
        let (identity, requested) = match request {
            Request::Initialize { .. } => {
                return Ok(Response::new(self.authority_id.clone(), None))
            }
            Request::Reserve(identity) => (identity, Some(State::Reserved)),
            Request::Cancel(identity) => (identity, Some(State::Cancelled)),
            Request::Status(identity) => (identity, None),
        };
        if identity.authority_id != self.authority_id {
            return Err("files.admission.identityMismatch".into());
        }
        let transaction = database(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Immediate),
        )?;
        let existing = database(transaction.query_row("SELECT generation,binding,intent,token_digest,state,revision FROM operations WHERE id=?1", [&identity.operation_id], |row| {
            Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,u32>(5)?))
        }).optional())?;
        let (state, revision) = match existing {
            Some((generation, binding, intent, token, state, revision)) => {
                if generation != identity.generation
                    || binding != identity.binding_sha256
                    || intent != identity.intent_sha256
                    || token != identity.token_digest()
                {
                    return Err("files.admission.identityMismatch".into());
                }
                let state = match (state.as_str(), revision) {
                    ("reserved", 1) => State::Reserved,
                    ("cancelled", 1 | 2) => State::Cancelled,
                    _ => return Err("files.admission.corrupt".into()),
                };
                if matches!(
                    (state, requested),
                    (State::Reserved, Some(State::Cancelled))
                ) {
                    database(transaction.execute("UPDATE operations SET state='cancelled',revision=2 WHERE id=?1 AND revision=1 AND state='reserved'", [&identity.operation_id]))?;
                    (State::Cancelled, 2)
                } else {
                    (state, revision)
                }
            }
            None => match requested {
                None => (State::Missing, 0),
                Some(state) => {
                    let count: u32 = database(transaction.query_row(
                        "SELECT count(*) FROM operations",
                        [],
                        |row| row.get(0),
                    ))?;
                    if count >= MAX_OPERATIONS as u32 {
                        return Err("files.admission.capacity".into());
                    }
                    database(transaction.execute(
                        "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6,1)",
                        params![
                            identity.operation_id,
                            identity.generation,
                            identity.binding_sha256,
                            identity.intent_sha256,
                            identity.token_digest(),
                            if matches!(state, State::Reserved) {
                                "reserved"
                            } else {
                                "cancelled"
                            }
                        ],
                    ))?;
                    (state, 1)
                }
            },
        };
        self.namespace.validate()?;
        database(transaction.commit())?;
        self.namespace.validate()?;
        Ok(Response::new(
            self.authority_id.clone(),
            Some(observation(identity, state, revision)),
        ))
    }
}

fn observation(identity: &Identity, state: State, revision: u32) -> Observation<'_> {
    Observation {
        operation_id: &identity.operation_id,
        generation: &identity.generation,
        binding_sha256: &identity.binding_sha256,
        intent_sha256: &identity.intent_sha256,
        revision,
        state,
    }
}

#[cfg(test)]
mod tests;
