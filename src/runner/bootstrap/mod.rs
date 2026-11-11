//! Configuration transport is explicit and independent of task persistence.
mod contracts;
use crate::{bounded_line, Result};
use contracts::{Initialized, MAX_INPUT};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncReadExt, BufReader};

pub(super) enum Mode {
    File(String),
    Initialize,
}

impl Mode {
    pub(super) fn select(args: &[String]) -> Result<Self> {
        let count = args
            .iter()
            .filter(|value| value.as_str() == "--bootstrap")
            .count();
        if count == 0 {
            return Ok(Self::File(super::option(args, "--config")?));
        }
        // This new mode is strict. Misspelled/ambiguous options cannot select a
        // file fallback, and user values are never echoed in diagnostics.
        let pairs = args.get(1..).ok_or("Invalid runner arguments")?;
        let mut seen = std::collections::HashSet::new();
        if count != 1 || !pairs.len().is_multiple_of(2) {
            return Err("Invalid initialized runner arguments".into());
        }
        for pair in pairs.as_chunks::<2>().0 {
            if !matches!(
                pair[0].as_str(),
                "--directory" | "--id" | "--bootstrap" | "--entry" | "--executable"
            ) || pair[1].is_empty()
                || pair[1].contains('\0')
                || !seen.insert(pair[0].as_str())
            {
                return Err("Invalid initialized runner arguments".into());
            }
        }
        if super::option(args, "--bootstrap")? != "initialize-v1" {
            return Err("Unsupported runner bootstrap".into());
        }
        Ok(Self::Initialize)
    }

    pub(super) fn arguments(&self) -> Vec<String> {
        match self {
            Self::File(path) => vec!["--config".into(), path.clone()],
            Self::Initialize => vec!["--bootstrap".into(), "initialize-v1".into()],
        }
    }

    pub(super) async fn read(&self, input: &mut (impl AsyncBufRead + Unpin)) -> Result<String> {
        match self {
            Self::File(_) => {
                let text = bounded_line(input, 256 * 1024)
                    .await?
                    .unwrap_or_else(|| "{}".into());
                let _: Value = serde_json::from_str(&text)?;
                Ok(text)
            }
            Self::Initialize => {
                let mut bytes = Vec::new();
                // Wait for EOF before mutation: a valid prefix followed by extra
                // input must not launch a child. Both memory and waiting are bounded.
                tokio::time::timeout(
                    Duration::from_secs(10),
                    input.take((MAX_INPUT + 1) as u64).read_to_end(&mut bytes),
                )
                .await
                .map_err(|_| "Initialized runner input timed out")??;
                if bytes.len() > MAX_INPUT {
                    return Err("Initialized runner input exceeds limit".into());
                }
                Initialized::parse(&bytes)?;
                String::from_utf8(bytes).map_err(|_| "Invalid initialized runner input".into())
            }
        }
    }
}

pub(super) struct Bootstrap {
    pub configuration_seed: Option<Value>,
    pub secrets: Value,
    pub initialize: Value,
    pub child_arguments: Vec<String>,
}

pub(super) async fn load(args: &[String]) -> Result<Bootstrap> {
    let mode = Mode::select(args)?;
    let file = match &mode {
        Mode::File(path) => Some(serde_json::from_slice::<Value>(&std::fs::read(path)?)?),
        Mode::Initialize => None,
    };
    let text = mode.read(&mut BufReader::new(tokio::io::stdin())).await?;
    match (mode, file) {
        (Mode::File(path), Some(config)) => {
            let initial: Value = serde_json::from_str(&text)?;
            let secrets = initial.get("secrets").cloned().unwrap_or_else(|| json!({}));
            Ok(Bootstrap {
                configuration_seed: Some(config.get("config").cloned().unwrap_or(config)),
                secrets: secrets.clone(),
                initialize: json!({"remote":true,"secrets":secrets}),
                child_arguments: vec!["--config".into(), path],
            })
        }
        (Mode::Initialize, _) => {
            let initial = Initialized::parse(text.as_bytes())?;
            let secrets = serde_json::to_value(initial.secrets)?;
            Ok(Bootstrap {
                configuration_seed: None,
                secrets: secrets.clone(),
                initialize: json!({"remote":true,"configurationTransport":"initialize-v1", "configuration":initial.configuration,"secrets":secrets}),
                child_arguments: vec!["--initialize-config".into()],
            })
        }
        _ => Err("Invalid runner bootstrap".into()),
    }
}

#[cfg(test)]
mod tests;
