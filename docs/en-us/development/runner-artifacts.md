# Task artifact metadata observation

[Development](README.md) · [中文](../../zh-cn/development/runner-artifacts.md)

`runner.artifact-observation.v1` declares the read-only `inspect-artifacts` command.
Negotiate this feature together with `runner.initialize-config.v1`,
`runner.admission-fence.v1` and runner protocol 1. A release number alone never
enables it. The feature is additive source functionality; the published 0.27.0
binary predates it. Existing stop/status operations remain unchanged.

The command takes exactly four option/value pairs: `--directory` (absolute private
deployment), `--id` (existing safe task identifier), `--admission` (original token)
and `--observation` (fresh request UUID). UUIDs must be canonical, hyphenated and
nonnil. Unknown/duplicate options are rejected without echoing their values.

The helper opens the existing admission lock and complete, matching journal; it
never creates a deployment, task, lock, journal or reservation for inspection. It
retains the cooperative lock throughout enumeration, checking retained directory
handles and journal identity before returning. Missing/corrupt/mismatched admission
is unavailable. This is metadata observation, not process-status interpretation.

One schema-1 receipt binds `runnerId`, `token` and `observationId`, with nullable
`admission`, `consistency: "pointInTime"`, `cleanupAuthorized: false` and `snapshot`:

- `missing`: no task directory was observed; a matching admission was read.
- `present`: `entries` lists only pid, heartbeat, STOP, status.json, events.jsonl,
  events.previous.jsonl, approvals.json, storage.json and inbox when observed.
  Each entry contains name, kind (file/directory/link/other), and sizeBytes
  (canonical u64 decimal text for regular files; null otherwise). `otherEntries`
  counts unrecognized immediate children without returning their names.
- `unavailable`: reason is admissionUnavailable, unsafeDirectory, namespaceChanged,
  enumerationLimit or metadataUnavailable. Raw paths, OS errors and contents are
  not included.

At most 256 immediate children are enumerated. No recursion or configuration,
status, log, message or storage contents are read. Symlinks/reparse points on the
deployment or task directory are refused; known leaf links are classified without
following their targets. POSIX deployment/task directories must be private. Output
is bounded to 16384 bytes; consumers should impose a 12-second deadline (including
the existing 8-second cooperative lock wait) and a combined stdout/stderr bound.

The view is non-atomic and can change during or immediately after inspection.
Filesystem operations are pathname observations, with retained-handle checks;
they do not lock arbitrary writers or prove replacement cannot occur. Matching
names, sizes, directory identities or admission states never establish artifact
ownership, settlement of unknown writes, or a safe deletion primitive. The
admission journal remains an independent cooperative protocol, not a filesystem
ownership journal. No cleanup or mutation receipt is produced. Historical SFTP
mutations cannot be settled by a helper which did not own them.

The host must check the exact recovery record and original SSH destination after
capability discovery and after this request, including failed observations. Results
are transient; discard stale results. Do not cache capabilities as authority,
replay a launch/write, or erase unknown recovery evidence based on this receipt.
