use serde::Serialize;

pub(super) const MAX_ENTRIES: usize = 256;
pub(super) const MAX_OUTPUT: usize = 16384;
pub(super) const KNOWN: &[&str] = &[
    "pid",
    "heartbeat",
    "STOP",
    "status.json",
    "events.jsonl",
    "events.previous.jsonl",
    "approvals.json",
    "storage.json",
    "inbox",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Artifact {
    pub name: &'static str,
    pub kind: &'static str,
    // Decimal text avoids losing precision across JavaScript/native boundaries.
    pub size_bytes: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Reason {
    AdmissionUnavailable,
    UnsafeDirectory,
    NamespaceChanged,
    EnumerationLimit,
    MetadataUnavailable,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(super) enum Snapshot {
    Missing,
    Present {
        entries: Vec<Artifact>,
        #[serde(rename = "otherEntries")]
        other_entries: usize,
    },
    Unavailable {
        reason: Reason,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Receipt<'a> {
    pub schema_version: u32,
    pub runner_id: &'a str,
    pub token: &'a str,
    pub observation_id: &'a str,
    pub admission: Option<&'static str>,
    pub consistency: &'static str,
    pub cleanup_authorized: bool,
    pub snapshot: Snapshot,
}
