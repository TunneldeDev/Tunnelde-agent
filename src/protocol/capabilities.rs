use serde::Serialize;
use std::io::Write;

/// Discovery schema versions are independent of the binary release version.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    schema_version: u32,
    helper: &'static str,
    helper_version: &'static str,
    runner_protocols: &'static [u32],
    features: &'static [&'static str],
}

pub(crate) fn write(arguments: &[String]) -> crate::Result<()> {
    if arguments.len() != 1 {
        return Err("--capabilities accepts no additional arguments".into());
    }
    // Static metadata only: no task directories, environment, stdin or network.
    let envelope = Envelope {
        schema_version: 1,
        helper: "tunnelde-agent",
        helper_version: env!("CARGO_PKG_VERSION"),
        runner_protocols: &[1],
        features: &["runner.control.v1"],
    };
    let mut bytes = serde_json::to_vec(&envelope)?;
    bytes.push(b'\n');
    if bytes.len() > 4096 {
        return Err("Capability response exceeds 4096 bytes".into());
    }
    let mut output = std::io::stdout().lock();
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}
