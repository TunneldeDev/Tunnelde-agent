use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const MAX_CONFIGURATION: usize = 200 * 1024;
pub(super) const MAX_SECRETS: usize = 256 * 1024;
pub(super) const MAX_INPUT: usize = 512 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Initialized {
    pub schema_version: u32,
    pub configuration: Value,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

impl Initialized {
    pub(super) fn parse(bytes: &[u8]) -> crate::Result<Self> {
        // Parser diagnostics can echo invalid field values; never forward them.
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| "Invalid initialized runner input")?;
        if value.schema_version != 1
            || !value.configuration.is_object()
            || serde_json::to_vec(&value.configuration)?.len() > MAX_CONFIGURATION
            || serde_json::to_vec(&value.secrets)?.len() > MAX_SECRETS
        {
            return Err("Invalid initialized runner input".into());
        }
        Ok(value)
    }
}
