# Private native toolkit caller composition

This foundation is not installed in the public MCP catalog. It does not change
authentication, the global shell projection, or readiness.

`native_page_worker::caller::Caller` captures the actual provider Context,
provider thread, provider session, environment, and request namespace. A
credential identifier is never a browser agent-session identity. Foreign-thread
reads and provider-session rebinding fail closed. A read transaction supplies
complete plain owned-thread records instead of the UI's 100-record window.

The private Node composition loads the complete pinned 4314 ProjectionStore,
derived-background-work, ThreadAccess, McpToolAccess, preview handlers, toolkit,
and PreviewAutomationBroker modules. The full projection codec runs before the
original shell derivation. The complete shell remains intact. Original broker
assignment keys retain environment, provider session, and thread components.
Unrelated filesystem, peer, and dispatch capabilities refuse access.

## Evidence

The whole-source oracle covers 26 full-codec projections: all seven run states
with and without provider-thread background subagents, an older active run below
a newer completed or waiting run, activity timestamp precedence, automatic and
manual compaction, JavaScript whitespace, and failed compaction. Canonical
database projection timestamps use `thread.updatedAt`, as the original database
reader does. Native run ordinals cannot tie within one thread: migration 055
creates the unique `(thread_id, ordinal)` index.

Private actual-State tests insert those records through the real store, obtain
real ephemeral provider credentials, check every extracted projection field,
and fence foreign reads and successor bindings. Both feature modes then pass
the extracted records through the complete original projection and full shell
codec, comparing every shell field. Evidence:
`/var/tmp/o3-server/page-caller-db-physical-2/result.json`. The earlier failed
timestamp fixture receipt remains retained separately. Library-only strict
checks pass both modes; this is not an all-target or full-suite claim.

A separate original-handler/broker composition proof uses an explicitly
synthetic host. Status remains readable for inactive callers, while evaluate
uses the original live-caller/background checks. Its original metadata status
follow-up and both MCP output channels preserve lone UTF-16 code units. This
does not certify an actual State-to-CDP or HTTP MCP caller.

## Boundaries before activation

The plain owned-thread snapshot is not yet a complete implementation of native
`getThreadRecords` or `readProjection`: native ordering, related provider-thread
and session bindings, context-transfer endpoints, and inherited history require
additional real-row consumers and proofs. No partial projection is substituted
for invalid full input; full codec failures remain failures.

Existing Rust provider write admission still checks legacy background task
tables. A provider-thread-only background caller accepted by original
ThreadAccess can therefore still be refused before CDP. That gate has not been
weakened. Actual original-handler admission must be bound to the immutable tool,
real caller, RunContext, and owned session before public selection.

The proposed global shell correction remains separate: native waiting is an
activity state but has no interruptible `activeRunId`; terminal-only activity
fields are null. Client detailed-projection helpers still include waiting in
their active-run predicate. Sidebar, cleanup, session-memory, and detailed UI
consumers need coordinated review before that global change is merged.

All twelve real toolkit consumers, recording/file capabilities, legacy alias
keyboard unification, and actual outer MCP transport still gate activation.
The existing CDP 4 MiB, application-worker eight-call/16 MiB, and grant/session
bounds remain separately documented foundation gaps, not native parity.

## Original host callback and execution lifetime

The next private bridge executes the complete original
`ServerBrowser.handleRequest` method with explicit operation, `markUsed`, and
broker dependencies. Its source and selected method hash are checked before
evaluation. The browser service itself is not imported, so its profile-launch
and cleanup capabilities cannot be acquired through this adapter.

The original callback awaits `runOperation(...).finally(markUsed)`, classifies
failures with the complete original `ServerBrowserPage.toOperationError`, and
responds through the original broker. A thrown operation is not silently turned
into a broker timeout. Original scoped host fibers remain independent of the
calling MCP fiber: cancellation removes the caller's pending response, while
an already entered operation still completes its own original `finally`.
Late responses are ignored by the original broker and never replayed.

The source-only synthetic-host proof covers thrown failures, first-line native
error classification, caller interruption during an entered callback, late
`markUsed`, a surviving host, and exact text/structured output. It is not a
physical CDP or cleanup receipt. `disposeBroker` only disposes logical fibers;
it is deliberately not an owner-close API. Rust must fence new admissions,
drain entered RunContext callbacks and their finalizers, and obtain independent
worker/session cleanup receipts. Ambiguous closure remains unknown. Rebinding
must invalidate the old caller without retagging its pending callbacks or
assigning their cleanup to a successor.

Original handler-generated status enrichment is a separate metadata operation
with `updateCurrentTab=false`. It must receive an owner-constructed status
capability; expanding an evaluate receipt to arbitrary status calls would
weaken the existing fixed operation gate. Human handoff, source control queue
drain, dialog bypass, callback deadline, and ordinary owner close continue to
use the original source distinctions. No new grace period is introduced.

## Producer reservations and source completion

The optional private owner adapter reserves each original broker producer
before `Queue.offer`. Reservation completion is uninterruptible; the actual
offer remains interruptible. A failed or interrupted offer abandons its
unstarted reservation. Native Effect also returns `false` when a queue is
closed, so successful `false` must abandon the reservation as well. It must not
be mistaken for a callback that can still reach the host.

Source completion follows the original invocation's real `finally`. It fences
new producers, while an already reserved and entered callback retains its own
execution and `markUsed` finalizer after the MCP observer returns. A private
receipt can never change the original tool class or transfer cleanup to a
later invocation.

The focused private Node proof uses pinned Effect 4.1.2 queues and the whole
original toolkit/broker/host callback. Direct queue cases cover accepted,
closed, and interrupted offers. Whole-source cases cover normal completion,
interruption while reservation is pending, and interruption after callback
entry. The last case observes source end before the actual late `markUsed`.
Receipt: `/var/tmp/o3-server/page-toolkit-producers-node-1/page-blnepgmq/result.json`.
The earlier protocol and clock receipts remain separately qualified in
`tests/fixtures/native-preview-toolkit-producers-4314.json`.

This verifies source producer accounting with a synthetic host. The concrete
Rust continuation owner, physical per-tab dispatch, absolute callback deadline
conversion, reconnect/live-tab reporting, and outer MCP transport remain
unaccepted. In-progress Rust raw-input and callback-ledger changes are not
covered by this Node result and remain outside this checkpoint.
