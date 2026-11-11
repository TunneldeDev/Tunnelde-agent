# Durable runner admission

Negotiate `runner.admission-fence.v1` and `runner.initialize-config.v1` with runner
protocol 1. This additive experimental protocol closes admission before treating
an absent task as cancelled. Existing transports retain their previous behavior.
A release version alone never enables fencing.

1. Choose a unique task ID and canonical non-nil UUID admission token. Persist
   both before sending any mutation.
2. Run `reserve-fenced --directory <deployment> --id <task> --admission <token>`.
   Require a successful matching receipt with `reserved` and `notFound`.
3. Start once with `start --directory <deployment> --id <task> --admission <token>
   --bootstrap initialize-fenced-v1 --executable <runtime> --entry <child>`.
   Use the [initialized configuration](runner-initialization.md) input/EOF rules.
4. On cancellation or an unknown launch outcome, use `stop-fenced` with the same
   directory, ID and token. Reconcile using `status-fenced`. Never replay start.

Control commands accept only the three argument pairs above. They return one JSON
document and exit 0 on success; combined stdout/stderr must be bounded to 4096 bytes.
The host uses a 15-second deadline; the helper lock waits at most 8 seconds.

```json
{"schemaVersion":1,"runnerId":"task","token":"217636b5-c7e8-4822-9bdf-717d400fd26e","admission":"cancelled","status":"notFound"}
```

All five fields are required. Reject duplicate/unknown fields, wrong schema,
task/token mismatch, missing/nonzero exit, excess output and invalid states.
Admission is `reserved`, `launched` or `cancelled`; status is `starting`, `running`,
`stopping`, `stopped`, `failed`, `notFound` or `unresponsive`. Reserved admission
has status `notFound`. Cancellation closes admission, while process exit remains
separate: a cancelled running task still needs STOP/reconciliation.

The helper serializes cooperative admission changes using a persistent OS file
lock in private `.admission-v1/`. Bounded append-only journals are flushed before
acknowledgement or process creation. Launch requires an existing matching reserved
journal and consumes it before creating/spawning. A start waiting for complete
stdin can be cancelled; a late reservation cannot reopen cancellation. Corrupt or
partial journals, unexpected objects, links and unsupported locks fail closed.

Each deployment retains at most 4096 task journals, each at most 1024 bytes, plus
one lock. New reservations are refused at capacity; existing cancellations remain
available. Do not delete or expire these receipts while delayed commands may exist.
Retirement requires a separate protocol, not a timer or recursive cleanup.

This contract assumes cooperative commands share the same private deployment and
the filesystem honors OS locks/flushes. It does not withstand arbitrary same-user
namespace replacement, disk rollback or guarantee power-loss recovery. Missing
control data blocks start; unverifiable/corrupt state remains manual review.
Nothing here settles historical SFTP writes or grants artifact deletion authority.
