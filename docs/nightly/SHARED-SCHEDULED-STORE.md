# Shared scheduled-task persistence: reference intake

Shared scheduled-task storage is still incomplete. Conversation modes execute in
O3, but its definitions, credentials and webhook queue remain private. The
shared-history exporter does not transfer those tables. Do not switch servers
expecting accepted webhook work or task edits to follow the profile yet.

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

The evidence includes a JSON snapshot and a filtered SQLite export. The export
uses the pinned empty schema with only synthetic task-table rows; authentication,
pairing sessions, logs and unrelated profile data are excluded. Its four public
task rows pass the exact contracts with Effect 4.1.2 and zero rejected rows.
The JSON and database stay outside Git because they contain synthetic endpoint
tokens. The committed intake contains hashes, counts and field descriptions.

## Required implementation boundary

Task mutations do not append orchestration events. A thread-event cursor cannot
detect edits or deletions made while T3 owns the profile. Shared task persistence
therefore needs its own transactional baseline and deletion tracking. First
adoption must distinguish absent rows from intentional deletion; later handovers
must preserve pending O3 intent when the native task did not change.

Definitions alone are insufficient. Handover must preserve endpoint token/secret,
receipt history, pending owner and member deliveries, replay expiry, relay
deduplication and dispatch/completion recovery. The current private O3 queue
stores a rendered prompt and expiry, which cannot reconstruct the original
native task/request snapshot. Existing accepted work needs a lossless migration
or an explicit compatibility boundary before ownership can change.

Still required: both handover directions, unchanged/deleted/replaced definitions,
due and expired windows, edits while queued, dispatch crash recovery, successful
and failed completion, relay delivery and policy behavior. The desktop composer
and the remaining platform inventory retain their separate acceptance gaps.
