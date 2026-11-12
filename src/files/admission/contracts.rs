use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) const PROTOCOL: &str = "files.admission-ledger.v1";
pub(super) const MAX_REQUEST: u64 = 4096;
pub(super) const MAX_OPERATIONS: usize = 1024;
pub(super) const MAX_DATABASE: u64 = 4 * 1024 * 1024;

// No Debug implementation: the authority token must never enter diagnostics.
#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Request {
    Initialize {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
        storage: String,
    },
    Reserve(Identity),
    Cancel(Identity),
    Status(Identity),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Identity {
    pub schema_version: u32,
    pub authority_id: String,
    pub operation_id: String,
    pub generation: String,
    pub binding_sha256: String,
    pub intent_sha256: String,
    pub token: String,
}

fn identifier(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == value)
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Identity {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !identifier(&self.authority_id)
            || !identifier(&self.operation_id)
            || !identifier(&self.generation)
            || !digest(&self.binding_sha256)
            || !digest(&self.intent_sha256)
            || !digest(&self.token)
            || self.token.bytes().all(|byte| byte == b'0')
        {
            return Err("files.admission.invalidRequest".into());
        }
        Ok(())
    }

    pub fn token_digest(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"tunnelde.files.admission.token.v1\0");
        digest.update(self.token.as_bytes());
        format!("{:x}", digest.finalize())
    }
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Initialize {
                schema_version: 1,
                storage,
            } if storage == "local" => Ok(()),
            Self::Reserve(identity) | Self::Cancel(identity) | Self::Status(identity) => {
                identity.validate()
            }
            _ => Err("files.admission.invalidRequest".into()),
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum State {
    Missing,
    Reserved,
    Cancelled,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Observation<'a> {
    pub operation_id: &'a str,
    pub generation: &'a str,
    pub binding_sha256: &'a str,
    pub intent_sha256: &'a str,
    pub revision: u32,
    pub state: State,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Response<'a> {
    pub schema_version: u32,
    pub protocol: &'static str,
    pub authority_id: String,
    pub observation: Option<Observation<'a>>,
    // Explicit negative authority; neither admission nor cancellation is a file receipt.
    pub execution_supported: bool,
    pub cleanup_authorized: bool,
}

impl<'a> Response<'a> {
    pub fn new(authority_id: String, observation: Option<Observation<'a>>) -> Self {
        Self {
            schema_version: 1,
            protocol: PROTOCOL,
            authority_id,
            observation,
            execution_supported: false,
            cleanup_authorized: false,
        }
    }
}
