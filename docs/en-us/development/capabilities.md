# Helper capability discovery

Run the explicitly selected helper with `--capabilities` and no other arguments.
It emits one UTF-8 JSON object followed by a newline, then exits with status 0.
Discovery reads no stdin, credentials or task artifacts, creates no files, starts
no runtime and makes no network requests. Additional arguments fail before any
task parsing; diagnostic output does not echo those arguments.

```json
{
  "schemaVersion": 1,
  "helper": "tunnelde-agent",
  "helperVersion": "0.28.0",
  "runnerProtocols": [1],
  "features": ["runner.control.v1", "runner.initialize-config.v1", "runner.admission-fence.v1", "runner.artifact-observation.v1", "files.admission-ledger.v1"]
}
```

`helperVersion` is informational. `schemaVersion` versions this envelope, while
`runnerProtocols` lists supported task protocols. `runner.control.v1` identifies
the existing start/send/poll/stop/status protocol, including the existing config
pathname contract. It does not claim stdin configuration, artifact reconciliation
or artifact cleanup. `runner.initialize-config.v1` separately declares
[initialized configuration](runner-initialization.md). Internal `serve` is not a host entrypoint.

`runner.admission-fence.v1` separately declares [durable task admission](runner-admission.md).

Version 0.28.0 also declares `runner.artifact-observation.v1` for bounded
[task artifact metadata observation](runner-artifacts.md); published 0.27.0
predates admission fencing and artifact observation. Require discovery, not the version.

`files.admission-ledger.v1` separately declares the experimental
[file admission ledger](file-admission.md). It grants no execution, publication
receipt, cleanup authority or recovery of independent SFTP requests.

The packaged experimental `files-save` implementation remains unadvertised.
Its presence does not declare desktop save readiness or a stable file API;
see [native-save consent, recovery and acceptance limits](file-save.md).

Consumers must bound combined stdout/stderr to 4096 bytes and time out discovery.
Require the helper identity and supported schema before interpreting features.
Version 1 requires all five fields with the types above. Strings are printable, non-whitespace
ASCII: helper version is 1–64 bytes; feature names are 1–64 bytes using lowercase
letters, digits, dots and hyphens. Lists contain at most 32 unique entries;
protocols are positive 32-bit integers. Consumers may ignore bounded unknown
fields and unknown features for forward compatibility, but must never treat them
as permission to perform an operation. Duplicate recognized JSON keys are invalid.

An old helper can reject this command. Nonzero exit, missing exit status, timeout,
transport failure, malformed output and unsupported schema are distinct failure
conditions; none proves a specific old version or an available feature. Keep
existing stop/status recovery available without discovery. Do not automatically
download or select another helper or runtime. Discovery cannot settle an unknown
write or delayed launch, prove artifact ownership, or authorize deletion.
