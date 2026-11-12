# File admission ledger (experimental)

`files.admission-ledger.v1` is an independent file-domain protocol discovered via
`--capabilities`, separate from runner admission and the helper release version.
It records admission/cancellation only. There is **no file executor, publication
receipt or cleanup authority**. The desktop does not yet use it, and it cannot
recover existing unknown SFTP requests.

## Consent and transport

Explicitly select the helper and provision a private absolute state directory.
Invoke `files-admission --directory <state-directory>`, send one UTF-8 JSON object
on stdin and close stdin. Input/output each have a 4096-byte limit. Input must
finish within ten seconds, before opening storage. Tokens belong only in stdin,
never argv/environment. Duplicate/unknown fields, extra CLI arguments and invalid
requests fail without echoing input or creating storage.

Initialization explicitly declares local storage:

```json
{"method":"initialize","schemaVersion":1,"storage":"local"}
```

The response contains `schemaVersion: 1`, `protocol: "files.admission-ledger.v1"`,
random `authorityId`, `observation: null`, `executionSupported: false` and
`cleanupAuthorized: false`. Reinitialization returns the same authority epoch.
Existing empty, foreign, truncated or corrupt databases are preserved and
rejected, never reset. Interrupted first initialization may require diagnosis.

Subsequent packets require all fields below:

| Field | Contract |
|---|---|
| `method` | `reserve`, `cancel` or `status` |
| `schemaVersion` | Integer 1 |
| `authorityId` | Previously negotiated ledger epoch UUID |
| `operationId` | Original operation UUID, unchanged on retry |
| `generation` | Original owner/run generation UUID |
| `bindingSha256` | Host/account/scope binding digest |
| `intentSha256` | Immutable operation intent digest |
| `token` | Private random 256-bit token, lowercase hexadecimal |

UUIDs are canonical lowercase hyphenated nonnil values. Digests/tokens are exactly
64 lowercase hexadecimal characters; all-zero tokens are rejected. Only a
domain-separated token digest is stored. Responses never contain the token or
its digest; this protocol accepts no user paths or payloads.

The future host adapter must bind authenticated SSH host key/account, authorized
canonical scope, target parent/object, baseline and payload digest using an
independently versioned canonical encoding. This foundation does not define that
encoding or authenticate a claimed SSH identity. That consent/binding contract
and an accepted server executor are prerequisites for any mutation adapter.

## State machine and persistence

| Existing state | reserve | cancel | status |
|---|---|---|---|
| Missing | Reserved, revision 1 | Cancelled, revision 1 | Missing, revision 0; no record created |
| Reserved, revision 1 | Original record | Cancelled, revision 2 | Original record |
| Cancelled, revision 1 or 2 | Original cancellation | Original cancellation | Original record |

Independent processes serialize through a retained control-file lock, with a
five-second wait deadline. Exact duplicates return the original state. Changed
generation/binding/intent/token under the same operation ID, or a different
authority epoch, fails with `files.admission.identityMismatch`. Cancellation
before reservation leaves a durable tombstone. It fences future consumers of
this ledger only, never an independent SFTP channel.

`observation` contains operation ID, generation, binding/intent digests, revision
and state. **Missing, Reserved and Cancelled are not publication receipts.** Both
negative authority fields remain false. Queries cannot replay work, erase an
unknown, authorize cleanup or create records. Lost stdout, timeout or nonzero
exit is unacknowledged: query the original identity in a fresh process.

The local SQLite store uses immediate transactions, DELETE rollback journaling,
synchronous EXTRA and 4096-byte pages. Bounds are 1024 operation records and 4 MiB
of database pages. Capacity refuses new work without eviction; existing queries
and cancellation remain available subject to successful disk I/O. There is no
expiry, reset, delete or compaction command. Process loss before commit uses
SQLite rollback recovery; durable commit precedes stdout acknowledgement.

Redacted `files.admission.*` errors include `invalidArguments`, `invalidRequest`,
`inputTimeout`, `notInitialized`, `unsafeStorage`, `storageChanged`,
`storageUnavailable`, `corrupt`, `capacity`, `busy`, `identityMismatch`,
`invalidResponse` and `responseUnavailable`. Preserve storage on failure; errors
are never evidence that an operation is Missing or Cancelled.

## Supported boundary and remaining gates

Use trusted, private **local filesystems** on Windows/Linux/macOS with working
file locks and SQLite durability semantics. Unix objects must belong to the
effective user, without group/other permissions. Windows callers must provision
private account ACLs. Symlink/reparse control objects and multiply linked
control/database files are rejected. Keep directory/lock/database/journal
together; never rename, copy, truncate or delete them while delayed commands can
exist. Network/shared/synchronized storage, untrusted writers to this control
namespace, dishonest hardware and power-loss guarantees beyond the
SQLite/OS/filesystem contract are unsupported.

The control lock does not lock user files or provide filesystem CAS against
external writers. `intent fsync -> rename -> receipt fsync` still has an ambiguous
publication crash gap. Execution, authoritative receipt reconstruction, restart
executor exclusion, original-generation host recovery, explicit desktop install
consent and safe moves remain separate gates. Unknown legacy SFTP results stay
unknown without authority for the original operation.
