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
