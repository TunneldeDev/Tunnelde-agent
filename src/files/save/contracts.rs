use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) const PROTOCOL: &str = "files.save.v1";
pub(super) const MAX_TEXT: usize = 2 * 1024 * 1024;
pub(super) const MAX_PACKET: u64 = (MAX_TEXT * 6 + 32768) as u64;
pub(super) const MAX_RECORDS: usize = 1024;
pub(super) const MAX_ACTIVE: usize = 4;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Context {
    pub host_key: String,
    pub account: String,
    pub scope: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ObjectIdentity {
    pub platform: String,
    pub device: String,
    pub object: String,
    pub created: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Authority {
    pub authority_id: String,
    pub context: Context,
    pub root: ObjectIdentity,
    pub storage_root: ObjectIdentity,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Baseline {
    Absent,
    Existing { sha256: String, length: u64 },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Intent {
    pub schema_version: u32,
    pub binding_sha256: String,
    pub target: Vec<String>,
    pub baseline: Baseline,
    pub payload_sha256: String,
    pub payload_length: u64,
}

// Secrets deliberately have no Debug implementation or serialization.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Control {
    pub schema_version: u32,
    pub authority_id: String,
    pub operation_id: String,
    pub generation: String,
    pub intent_sha256: String,
    pub token: String,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Request {
    Initialize {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
        storage: String,
        context: Context,
    },
    Submit {
        control: Control,
        intent: Intent,
        content: String,
    },
    Status {
        control: Control,
    },
    Cancel {
        control: Control,
    },
    Release {
        control: Control,
        revision: u64,
        #[serde(rename = "discardBackup")]
        discard_backup: bool,
    },
}

pub(super) fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}
pub(super) fn sha256(bytes: &[u8]) -> String {
    digest(b"", bytes)
}
pub(super) fn json_digest(domain: &[u8], value: &impl Serialize) -> Result<String> {
    fn sort(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for value in map.values_mut() {
                    sort(value);
                }
                map.sort_keys();
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    sort(value);
                }
            }
            _ => (),
        }
    }
    let mut value = serde_json::to_value(value).map_err(|_| "files.save.invalidRequest")?;
    sort(&mut value);
    Ok(digest(
        domain,
        &serde_json::to_vec(&value).map_err(|_| "files.save.invalidRequest")?,
    ))
}
pub(super) fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == value)
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Authority {
    pub fn binding(&self) -> Result<String> {
        json_digest(b"tunnelde.files.save.binding.v1\0", self)
    }
}
impl Control {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !valid_uuid(&self.authority_id)
            || !valid_uuid(&self.operation_id)
            || !valid_uuid(&self.generation)
            || !valid_digest(&self.intent_sha256)
            || !valid_digest(&self.token)
            || self.token.bytes().all(|b| b == b'0')
        {
            return Err("files.save.invalidRequest".into());
        }
        Ok(())
    }
    pub fn token_digest(&self) -> String {
        digest(b"tunnelde.files.save.token.v1\0", self.token.as_bytes())
    }
}
impl Intent {
    pub fn digest(&self) -> Result<String> {
        json_digest(b"tunnelde.files.save.intent.v1\0", self)
    }
    fn validate(&self) -> Result<()> {
        let total: usize = self.target.iter().map(String::len).sum();
        if self.schema_version != 1
            || !valid_digest(&self.binding_sha256)
            || !valid_digest(&self.payload_sha256)
            || self.payload_length > MAX_TEXT as u64
            || self.target.is_empty()
            || self.target.len() > 64
            || total > 4096
            || self.target.iter().any(|part| {
                part.is_empty()
                    || part.len() > 255
                    || part == "."
                    || part == ".."
                    || part.contains(['\0', '/'])
            })
            || matches!(&self.baseline,Baseline::Existing{sha256,length} if !valid_digest(sha256) || *length>MAX_TEXT as u64)
        {
            return Err("files.save.invalidRequest".into());
        }
        #[cfg(windows)]
        if self.target.iter().any(|part| {
            part.contains(['\\', ':']) || part.ends_with(['.', ' ']) || {
                let name = part.split('.').next().unwrap_or("").to_ascii_uppercase();
                matches!(name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || (name.len() == 4
                        && (name.starts_with("COM") || name.starts_with("LPT"))
                        && (b'1'..=b'9').contains(&name.as_bytes()[3]))
            }
        }) {
            return Err("files.save.invalidRequest".into());
        }
        Ok(())
    }
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Initialize {
                schema_version,
                storage,
                context,
            } => {
                if *schema_version != 1
                    || storage != "local"
                    || context.host_key.len() != 50
                    || !context.host_key.starts_with("SHA256:")
                    || !context.host_key[7..]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
                    || !b"AEIMQUYcgkosw048"
                        .contains(context.host_key.as_bytes().last().unwrap_or(&0))
                    || context.account.is_empty()
                    || context.account.len() > 256
                    || context.account.chars().any(char::is_control)
                    || context.scope.len() > 8192
                    || context.scope.contains('\0')
                    || !std::path::Path::new(&context.scope).is_absolute()
                {
                    return Err("files.save.invalidRequest".into());
                }
            }
            Self::Submit {
                control,
                intent,
                content,
            } => {
                control.validate()?;
                intent.validate()?;
                if content.len() > MAX_TEXT
                    || intent.payload_length != content.len() as u64
                    || intent.payload_sha256 != sha256(content.as_bytes())
                    || control.intent_sha256 != intent.digest()?
                {
                    return Err("files.save.identityMismatch".into());
                }
            }
            Self::Status { control } | Self::Cancel { control } | Self::Release { control, .. } => {
                control.validate()?
            }
        }
        Ok(())
    }
    pub fn control(&self) -> Option<&Control> {
        match self {
            Self::Initialize { .. } => None,
            Self::Submit { control, .. }
            | Self::Status { control }
            | Self::Cancel { control }
            | Self::Release { control, .. } => Some(control),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Phase {
    Prepared,
    Running,
    Publishing,
    Succeeded,
    Conflict,
    Failed,
    Cancelled,
    Unknown,
    Releasing,
    Released,
}
impl Phase {
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Prepared | Self::Running | Self::Publishing | Self::Releasing
        )
    }
    pub fn terminal_known(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Conflict | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Observation {
    pub operation_id: String,
    pub generation: String,
    pub intent_sha256: String,
    pub revision: u64,
    pub phase: Phase,
    pub live: Option<bool>,
    pub cancellable: bool,
    pub publication: &'static str,
    pub backup_retained: bool,
    pub error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Response {
    pub schema_version: u32,
    pub protocol: &'static str,
    pub authority: Authority,
    pub binding_sha256: String,
    pub observation: Option<Observation>,
    pub cleanup_authorized: bool,
}
