# Initialized runner configuration

For task-bound cancellation of delayed launches, negotiate the separate
[durable admission protocol](runner-admission.md).

Discover `runner.initialize-config.v1` with [capability discovery](capabilities.md)
before selecting this transport. It is an explicit alternative to the existing
`--config` contract, not a version-number heuristic or automatic fallback.

```text
tunnelde-agent start --directory <existing-absolute-directory> --id <unique-id> --bootstrap initialize-v1 --executable <selected-runtime> --entry <selected-child>
```

Write exactly one UTF-8 JSON document to stdin and close it:

```json
{"schemaVersion":1,"configuration":{},"secrets":{"provider.main":"example-only"}}
```

Configuration must be an object and serialize to at most 200 KiB. Secrets must be
a string map and serialize to at most 256 KiB. The entire input is limited to
512 KiB; input must end within 10 seconds. Unknown envelope fields, duplicate
recognized fields, incompatible versions, invalid UTF-8, trailing documents and
mixing `--config` with `--bootstrap` are rejected. Errors do not echo input values.
No task directory is created until the complete input has been accepted. Each task
directory is created exclusively and has mode 0700 from creation on Unix.

The supervisor launches the selected child with `--initialize-config`. It passes
`remote: true`, `configurationTransport: "initialize-v1"`, `configuration` and
`secrets` in the child's JSON-RPC initialize parameters. The child must explicitly
support this contract and validate its own package identity and configuration
before using them. Configuration and secrets are not placed in argv or a helper
configuration file. `storage.get` starts from empty storage in this mode rather
than implicitly seeding it with the launch configuration. `secrets.get` remains
available in memory.

Explicit `storage.set`, child code, runtime behavior and operating-system crash
handling may persist data. This transport does not promise that arbitrary
configuration can never reach disk. The supervisor also keeps ordinary task
journals and approvals; plugins must avoid publishing secrets in those messages.

The existing file transport, MCP bridge, task polling and stopping remain
supported. A successful start response acknowledges dispatch to the supervisor,
not successful plugin initialization. A lost response leaves the outcome unknown;
callers must inspect and stop that same task instead of replaying start. A failed
bootstrap delivery makes a best effort to terminate its owned supervisor, while
preserving task artifacts for recovery. No recursive artifact cleanup is implied.
