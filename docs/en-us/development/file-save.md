# Native file saves (experimental, acceptance pending)

Version 0.28.0 includes the experimental `files.save.v1` implementation, still
under review and absent from capability discovery. Desktop source has protocol
and query-only recovery adapters, but new editor saves and installation/setup
remain disabled pending their consent and acceptance work. This is not a stable
plugin API and cannot settle legacy SFTP unknowns. The separate admission ledger
remains admission-only.

## Consent and identity

An explicitly selected helper accepts `files-save --directory <absolute-state>`.
One bounded UTF-8 JSON packet must finish on stdin within ten seconds. Tokens
travel only through stdin, including native-worker bootstrap, never argv/env.
No automatic installation or JavaScript runtime is involved.

`initialize` requires `schemaVersion: 1`, `storage: "local"` and `context` with
`hostKey` (unpadded SSH SHA256 fingerprint), `account`, and absolute `scope`.
The adapter must supply identity from its authenticated transport; a caller
string is not independent SSH attestation by the helper. Initialization creates
the explicitly selected missing private directory or checks an existing one;
it never resets an existing database. Windows requires a protected current-user
ACL on fixed local storage; Unix requires current-user ownership and no group
or other permission bits. Authority binds an epoch, canonical scope and actual
scope/state directory identities. Copying the database does not migrate it.

## Requests

`submit` has `control`, `intent` and UTF-8 `content`. Control contains
`schemaVersion`, `authorityId`, `operationId`, `generation`, `intentSha256` and
random private 256-bit lowercase hexadecimal `token`. Only its domain-separated
digest is stored. Changed identities fail; exact duplicates cannot republish
running or settled work.

Intent contains `schemaVersion`, `bindingSha256`, relative `target` components,
`baseline` (`kind: absent`, or `kind: existing` with `sha256`/`length`),
`payloadSha256` and `payloadLength`. Text is limited to 2 MiB. Hashes use compact
UTF-8 JSON with recursively sorted object keys, unchanged array order and
serde-compatible string escaping. Structures contain only strings, bounded
integers, objects and arrays. Domain prefixes are
`tunnelde.files.save.binding.v1\0`, `tunnelde.files.save.intent.v1\0` and
`tunnelde.files.save.token.v1\0` (last byte NUL); payload uses raw SHA256.

`status`, `cancel` and `release` use original control. Release additionally
requires the exact receipt `revision`, original-executor exclusion and
`discardBackup: true` when a retained backup exists.

## Execution and recovery

Short SQLite transactions record intent/payload before detached execution.
An operation-specific kernel lease covers all actual IO; only `prepared` may
start. The worker records exclusive private staging ownership, checks the
baseline/content/available metadata, and durably enters `publishing` before
target mutation. Cancellation can win before this admission, never afterwards.
A five-minute lifetime bounds the worker; termination does not invent a result.
On Windows the worker requests permitted job breakaway as well as console
detachment, so an OpenSSH request ending does not terminate its admitted work.
A policy that refuses breakaway causes an explicit launch error; the admitted
record remains queryable, without a session-bound fallback or implicit retry.
OS session/job policies may still terminate a worker; recovery then uses its
original lease and journal rather than promising uninterrupted execution.

Create uses native atomic no-replace publication. Replacement retains the actual
displaced object using Linux/macOS exchange or Windows backup replacement. No
truncate/delete-first or copy-and-delete fallback exists. A pre-publication
conflict leaves the target unchanged. A different displaced baseline becomes a
**published conflict**, preserving the concurrent version without blind rollback.
Exchange is not filesystem compare-and-swap against arbitrary external writers;
metadata/security is observed at a point in time. Retained directory authority
is not a sandbox against hostile same-account native processes or a permanent
guarantee against another process relocating a directory after resolution.

Windows replacement can re-inherit the staging parent's ACL. Before the success
receipt, the executor restores and verifies owner/group/DACL from the actual
displaced backup through the identified published handle. Interrupted metadata
restoration remains unknown with its backup; metadata and namespace publication
are not one atomic operation. Newly created files use private access by default.

Final receipts survive client/SSH delivery loss. Status never starts, replays or
cleans work. It checks the original lease identity; `live: null` means exclusion
is unproven. Excluded pre-publication interruption may become failure; interrupted
publication or ambiguous native errors become `unknown`. Matching bytes/missing
paths do not prove success. Unknown operations quarantine new admission in that
authority. These are process-loss semantics, not universal power-loss, network
storage, cross-mount move or legacy SFTP recovery guarantees.

`backupRetained: null` denotes uncertain artifact availability during unknown
publication/cleanup. An exact duplicate release uses its original revision and
discard decision and returns the settled receipt without deleting again.

Explicit release removes only the verified owned artifact, never a user target
or directory recursively. Release intent precedes deletion; interrupted cleanup
becomes unknown. Unknown saves cannot release. `cleanupAuthorized: false`
continues to forbid generic host cleanup; narrow helper release is separate.

Limits: 1024 records, four active operations, 8 MiB retained payload, 32 MiB
database. No receipt, tombstone or unknown is evicted to admit new work. Known
terminal payloads may be reclaimed while operation identity remains. Corrupt,
foreign or unsafe storage is preserved and rejected.

Extended metadata is bounded separately (256 attributes, 64 KiB per value,
1 MiB combined). Oversized macOS resource forks and content-specific compressed
metadata are rejected before publication rather than copied without limits.

On macOS, the stage retains the device/inode/birthtime identity recorded before
writing, including interruption during metadata copying. Replacement copies
owner/group, mode, ACLs, bounded xattrs and the `nodump`, `hidden` and `tracked`
flags explicitly; it does not restore the source timestamps with `COPYFILE_STAT`.
Other file flags are rejected before publication because they may encode old
contents, require privileges or prevent publication and revision-bound cleanup.
Existing recovery identities are never rewritten, and changed artifacts remain
ineligible for cleanup.

Capability advertisement still requires filesystem/metadata acceptance, old-client
combinations, physical macOS/Linux, desktop consent/recovery and package delivery.
