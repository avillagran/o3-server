# Shared scheduled-task persistence

Task definitions, private endpoint credentials and native webhook ledger rows now
travel with migration-80 shared profiles. O3 imports native edits and deletions
even when the thread-event sequence has not advanced. Its task edits export back
to the native tables without inventing orchestration events.

Webhook execution handover remains incomplete: O3's existing ingress and worker
still use its private legacy queue, while imported native pending rows are
preserved for the next owner. The new ingress/dispatch adapter and lossless
legacy-work migration are still required before using this draft for due webhook
work. No additional shared-task capability is advertised.

The reference proof is pinned to official nightly 4262, commit `c55db3e9`, with
the release digest and source hashes in `scheduled-store-intake.json`. Nightly
4275 remains the latest published release at this review; the covered scheduled
sources and contracts have no changes from 4262. This does not certify other
4275 surfaces.

## Reproduce the reference

```sh
tools/build-test-image
tools/live-fresh-context --scheduled-store --reference-root <official-4262-directory>
```

The runner compiles without executing tests on the host. The rootless container
has read-only source and binaries, private temporary state, no external network
and no provider authentication mounts. Only the unique evidence directory is
writable on the host. It uses synthetic signing material and a local test session
issued by the reference; it never starts inference.

The scenario saves four definitions: disabled interval tasks in `continue` and
`fresh_session` modes, a disabled unsigned webhook and an enabled signed webhook.
The signed task has a 3600-second coalescing window. Two distinct signed requests
with the same group and a repeated first request produce:

| Native table | Observed content |
| --- | --- |
| `scheduled_tasks` | Four definitions, zero runs. Webhook token and secret are private SQL columns; the public task omits the secret. |
| `scheduled_task_webhook_deliveries` | Three receipts with outcomes `accepted`, `coalesced` and `duplicate`. All three HTTP responses use status 202. |
| `scheduled_task_webhook_pending` | One future pending window, an original public task snapshot, the latest request and two delivery IDs. |
| `scheduled_task_webhook_replay_keys` | Two hashed replay keys, both associated with the pending owner delivery. |
| `scheduled_task_webhook_relay_deliveries` | Empty; relay ingestion is outside this proof. |

`task_json` stores a complete public `ScheduledTask`. `request_json` stores
`method`, `path`, `query`, `headers` and `bodyText`. `delivery_ids_json` is an array
of receipt IDs. Restarting the actual reference retains those JSON values, the
due time, pending state and replay count without creating any run.

The proof then hands the actual disposable profile to O3. An edit through O3's
task service is read by the real T3 `scheduledTasks.subscribe` stream. T3 edits a
second task through RPC; a new O3 startup imports that edit. The original pending
task/request/member IDs survive both handovers with zero inference turns.

The evidence includes the initial JSON snapshot and a final filtered SQLite export. The export
uses the pinned empty schema with only synthetic task-table rows; authentication,
pairing sessions, logs and unrelated profile data are excluded. Its four public
task rows pass the exact contracts with Effect 4.1.2 and zero rejected rows.
The raw JSON and database stay outside Git because they contain synthetic endpoint
tokens. A separate committed fixture replaces tokens and delivery IDs with explicit
placeholders, fixes timestamps and moves deadlines to 2100. The intake records
its source and the scope of the runtime proof.

## Independent synchronization boundary

Task mutations do not append orchestration events. A thread-event cursor cannot
detect edits or deletions made while T3 owns the profile. Shared task persistence
therefore uses its own native-side baseline of row hashes and local database
identity/generation. Startup compares both copies with that baseline before
changing task rows. It imports native changes, retains unexported local intent
when native state is unchanged, and refuses conflicting changes while retaining
both copies. Deletions compare against the previous baseline rather than treating
every absent row as an initial empty profile. JSON property order and whitespace
do not create false conflicts.

Definitions, private credentials, last webhook delivery identity, receipt history,
pending snapshots/member IDs, replay keys/expiry and relay-deduplication rows are
synchronized. The public task never includes the signing secret. Updates use
UPSERT rather than REPLACE so task edits do not cascade-delete pending work.
Pending windows can exchange their unique coalescing keys inside a transaction.

Local imports commit before the native baseline. A native commit failure can
therefore replay an already committed local import safely; it cannot acknowledge
an import that never committed. Background export observes task notifications as
well as history changes. An unchanged local generation skips ledger scans; scans
hold row keys and hashes and read large bodies one at a time. A recreated local
database has a new identity and restores unchanged native rows instead of
interpreting the missing local rows as deletions.

`tools/test-scheduled-modes --integration shared_tasks` exercises both directions,
event-free edits/deletions, private fields, retained local intent, conflicts,
atomic export failure, coalescing-key exchanges, database recovery, native commit
failure and task-only background notifications. The legacy shared-history tests
retain their separate schema-73 and schema-80 coverage.

## Still required

Dispatch/completion recovery and new O3 ingress still require the native ledger
adapter. The current private O3 queue
stores a rendered prompt and expiry, which cannot reconstruct the original
native task/request snapshot. Existing accepted work needs a lossless migration
or an explicit compatibility boundary before ownership can change.

Still required: due and expired windows, edits while queued, dispatch crash
recovery, successful and failed completion, relay ingestion and policy behavior.
Preserving a relay row does not prove relay ingestion or verification. The desktop composer
and the remaining platform inventory retain their separate acceptance gaps.
