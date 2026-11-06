# Webhook receipts and local token rotation

The candidate implements `scheduledTasks.listWebhookDeliveries` and
`scheduledTasks.getWebhookDelivery` over the native ledger imported from T3.
`scheduledTasks.rotateWebhookToken` also rotates local endpoint tokens. All
three methods require `orchestration:operate`, including receipt reads.

The reference is T3 `v0.1.8-nightly.20261106.4262`, commit
`c55db3e9c72fdf584f4b54192fb39beb88667b59`, with Effect 4.1.2.

## Observable behavior

- Lists sort by receive time descending, then insertion order descending. Missing
  task IDs return an empty list. Details require both the task and delivery ID.
- Without `acceptAnyOutcome: true`, both methods expose only `accepted`,
  `dispatch_failed`, `rejected_signature`, `disabled`, `rate_limited` and
  `expired`. An excluded detail returns `Webhook delivery not found.` Newer
  callers receive every stored outcome, including future strings.
- `pendingWindow` is true only for an accepted owner receipt with a `pending`
  or `dispatching` window. Running, completed and failed windows do not qualify.
  Coalesced member receipts do not inherit the owner's pending flag.
- Invalid logged header JSON falls back to an empty object; invalid missing-field
  JSON falls back to an empty array. Content type uses the exact stored
  `content-type` key. Boolean database flags compare to exactly 1.
- Details return the stored redacted headers/query, logged body and truncation
  flag, missing fields, rendered prompt and error. They never expose the raw
  pending request or private signing credentials. Summary queries read no body
  or rendered-prompt bytes.
- Rotation uses 32 random bytes encoded as unpadded base64url, matching native
  token generation. Initial legacy signed-hook creation uses the same helper;
  existing tokens remain unchanged until rotation. A transaction preserves the
  signing secret, task definition, run bookkeeping, pending windows and replay
  keys. It updates the endpoint and timestamp, then notifies subscribers. A
  retired bearer URL returns 404. Missing and non-webhook tasks use native errors.

## Shared insertion order

Native receipt lists and pending workers use SQLite rowid to break equal-time
ties. Copying ledger rows by sorted primary key changed that order. Import and
export now align the relative insertion order of both tables with their owner.
An unchanged old copy is repaired on startup; unexported local rows retain their
own order after the imported rows. Other local definition edits survive.

Alignment changes rowids in two transactional passes, using a disjoint staging
range. Payloads and stable delivery IDs remain intact. It performs no writes
when order already matches and refuses an overflowing staging range. A failed
alignment rolls back imported/exported rows and baseline updates together.
Fresh copies insert in source order directly, avoiding a second rewrite of large
request/body payloads merely to repair their insertion order.

## Evidence

`tools/generate-webhook-history-fixtures.mjs` executes the pinned native header,
missing-field, summary and detail decoders and legacy outcome set. It strips
types and changes import locations only; exact native contracts validate every
generated result. Its 79 cases cover outcome opt-in, window states, malformed
logged JSON and database flags. `tests/webhook_history.rs` checks those results
through the real protocol client, alongside authorization, subscription, retired
URL and transaction-failure cases. Shared-store regressions cover equal-time
ordering, older-copy repair, local insertion order, coalescing-key exchanges and
failed-write retry.

`tools/live-fresh-context --scheduled-store --reference-root <official-release>`
compares eight receipt reads with the official runtime before and after handover.
T3 reads two O3 rotations, and O3 imports a subsequent native rotation. The test
retains the pending request, member IDs and replay claims and starts no provider
turns. The filtered final export validates four task rows with zero rejections.
Tests and generation run in rootless Podman with read-only inputs, private state,
no network and no provider credentials.

## Remaining work

New deliveries now use the native admission/worker adapter described in
[NATIVE-WEBHOOK-ADMISSION.md](NATIVE-WEBHOOK-ADMISSION.md), including command
receipts and a durable pre-provider claim. Previously accepted legacy work retains
its private queue and still needs a safe ownership boundary. Relay verification,
secret providers and global policy remain open. Token rotation returns a local
path with `url: null`; relay-origin resolution remains separate. This does not
certify the complete scheduled-task service or global nightly parity.
