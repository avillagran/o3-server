# Native Page worker supervisor foundation

This remains an inactive foundation. No startup path selects this worker, no legacy alias changes its active driver, and no public MCP catalog is expanded. Rust Pool/authority adapters, full twelve-tool dispatch, uploads, recording transfer, real OOPIF/scratch ownership, diagnostics collection and long-term artifact reclamation remain open.

## Authority and lifetime

`native_page_worker::Worker` starts one explicitly supplied Node executable/entry in its own process. `GrantOwner` supplies the already tab-scoped endpoint, target identity, revocation watch, synchronous admission fence and independently awaited cleanup receipt. An MCP argument must never construct these inputs. The future production adapter must capture actual Rust thread/tab/session/control generation and must close the existing owned relay, including its late-attachment receipts. A PID, URL or tool argument is not an authority identity.

Revocation is checked before bootstrap, before each queued call, and before publishing results. Cleanup calls the owner's synchronous fence before awaiting anything. The worker process is asked to close; if it stalls, only that owned Node child is killed and reaped. The shared Chromium process is never killed by this supervisor. Owner close is awaited separately with a deadline; an ambiguous/refused/expired owner receipt remains false even after the Node child has exited. Rust owners must also resolve outstanding owned browser execution/control effects before declaring their receipt complete; this leaf does not implement that Pool adapter.

Dropping the last Worker handle signals shutdown. Calls are never replayed after timeout, exit or protocol failure. A timed-out mutation has an unknown application outcome and requires a fresh admitted worker/snapshot; it is not a reason to retry the mutation automatically. Dropped call receivers do not cancel another call. Full per-call native control/handoff orchestration remains with the Rust owner and must be wired before activation.

## Process and protocol bounds

The child receives an empty environment plus its dedicated HOME/XDG paths and a fixed executable PATH. It receives no ambient token, NODE_OPTIONS, user configuration or host profile. The private endpoint travels only in the owner bootstrap over stdin, never argv or logs. The Node entry validates the attached target identity through the actual CDP session before replying ready.

Each IPC frame is eight magic bytes (`O3PG0001`), a four-byte little-endian JSON length and UTF-8 JSON. Frames are bounded to16MiB before body allocation. The actor permits eight correlated active calls and an eight-slot admission queue; additional callers receive Busy. Each call has an owner-provided finite deadline (maximum120seconds), and a nonce plus monotonic request ID prevents response confusion. Unknown/duplicate IDs, wrong nonces, mixed stdout, malformed JSON and oversized frames invalidate the worker. Native operation errors are typed application results, not transport failures. A separate bounded stderr reader discards content and faults after64KiB, preventing diagnostic flooding or credential retention.

Close receipts distinguish child reap, owner grant close and a validated native close acknowledgment followed by successful child exit. Native recursive cleanup requests and executed recursive removals are separate fields. The retaining adapter moves exact owned artifact slots into quarantine; it does not traverse or delete their contents. Unknown recursive requests still hit the unchanged guard. A missing acknowledgment does not certify native artifact cleanup. Escaped isolated UTF-16 surrogates in application results/errors are retained as validated RawValue-backed JavaScriptJson. These results serialize as their original JSON, without conversion to a Rust scalar string or replacement characters. The control envelope remains strict scalar JSON, and raw results still undergo nonce, request-ID, shape and admission checks. Wiring this representation through the actual MCP output boundary remains required before that consumer can activate; ordinary serde Value is not an equivalent representation.

The Node entry has one attached Page and one canonical Playwright keyboard. `preview_act` and internal legacy press/type/click aliases route through that same Page and actual native action codecs. It does not combine the legacy Rust keyboard cache with a Node keyboard. Future production driver selection must be atomic per page: either the existing Rust driver or this worker, with all aliases following the selected driver.

## Exact acquisition and packaging plan

1. Acquire Node26.13.0 for the admitted platform into a dedicated owned tool directory, using the official Node release and publisher SHASUMS/signature verification. Do not install globally, alter shell configuration or reuse a real profile. Record platform, archive URL, publisher verification, executable SHA256 and version. The entry refuses another Node version; admission is not a PATH lookup.
2. Acquire `playwright-core`1.60.0 from its native-lock URL/SRI and Effect4.1.2 from the recorded npm URL/SRI. Use a private normal tool-acquisition phase, no lifecycle scripts/browser downloads, and return to offline runtime. Normal package installation is separate from runtime tests; do not execute native repository tests or rootfs extraction fixtures.
3. Seal the84 exact native runtime files from nightly4314 (`83a3674ffd84ffb33de768210c806cb1ac53fd80`), including Page/SessionControl and runtime contracts. Ship no native profile-management module or native tests. Preserve the T3 MIT notice, Effect license/notices and complete Playwright license/NOTICE/third-party notices. Runtime manifests verify84 native files,106 Playwright package files and the2343 recorded Effect distribution/license/package files.
4. Preserve canonical module identity. The contracts workspace alias must resolve to the exact sealed `native/packages/contracts` directory; Effect must resolve to the sealed Effect root. Relative module links must remain inside the owned installation and be declared/verified in its manifest. Node's TypeScript stripping rejects ordinary TypeScript packages physically inside node_modules, so blindly copying the contracts source there is not an equivalent package layout. A reviewed bundle build is another possible future layout, not an implemented fallback.
5. Coordinate private runners/CI through `O3_ADMITTED_NODE`, `O3_ADMITTED_NODE_SHA256` and `O3_ADMITTED_NODE_VERSION=26.13.0`. Admission verifies the regular no-follow executable/version/hash, then snapshots the executable with the committed shared no-follow Node admission helper and mounts/passes that immutable copy and receipt to offline runtime. The Page runner checks the exact copied version inside its private container before loading native source. No skips or language polyfills substitute for the actual runtime. The Rust specification currently trusts its sealed caller; a production installer/admission service and rollout are still required.
6. Every process receives a fresh0700 home/artifact slot. Retained/quarantined slots remain explicit evidence; no quota/reclamation implementation is claimed here. The package and owner adapters need review before enabling any capability.

## Focused evidence

Synthetic Node peers exercise real child-process framing, native-error results, malformed/mixed/oversized stdout, exit, hanging calls, actual late replies after owner close, revocation, queue pressure and unknown owner cleanup. These peers use no Playwright and do not certify native Page behavior.

The separate physical probe runs Rust → a distinct Node entry → a private tab-scoped relay → real private Chromium. It tests actual native codecs/actions, accessibility output, the shared keyboard across aliases (including a failed modifier chord), independent synchronous grant fencing and owner close acknowledgment. A second physical case revokes a running native wait and requires the correlated caller to fail and both process/grant receipts to complete. Private container fixtures retain all filesystem evidence and never remove trees; the artifact adapter's physical quarantine behavior is separately qualified.

`GrantOwner` is a target/lifetime boundary, not a replacement for per-operation MCP read/write and controller admission. Its revocation watch is one-way for that captured grant; a replacement gets a new owner/worker/nonce. The future Rust broker must admit each operation and retain its control receipt before using `invoke`; a read-only caller must never receive a generic worker handle. No such network-facing adapter is enabled by this checkpoint.

The private protocol peer may run on the base image's Node because it imports no native source; it prefers an explicit `O3_ADMITTED_NODE` receipt when the shared runner provides one. Every actual Page/contract fixture instead verifies the exact26.13.0 executable inside its private container. The Page runner also checks the common admission path/hash/version receipt when supplied. These synthetic peers are not a Node18 compatibility claim for the native worker.

`invoke` retains its explicit hard transport budget for existing callers. The new `invoke_native` separates the original operation timeout from the response deadline: it forwards the unchanged native timeout and adds exactly1000ms for host response when `updateCurrentTab` is true, or no grace when false. Queue time consumes the host response budget. A real private process fixture replies after the operation timeout and proves that grace permits that response without enlarging the forwarded native timeout; the same reply without grace retires the worker. Production broker integration must select this API using its source-derived updateCurrentTab semantics. The current foundation bounds the native timeout to120seconds; native preferred-open660seconds is not yet implemented.

Keep the worker/Page keyboard lifetime tied to the captured browser transport/target grant. Caller/controller control generations belong in per-operation Rust admission and source-backed handoff/ref invalidation; recreating the worker on every ordinary controller transition is not an equivalent keyboard-state implementation. Owner-only ref invalidation/cancellation and physical input-state recovery on an interrupted grant still require the actual broker/Pool adapter. No fallback may mix an independent Rust keyboard into a live native Page.

## Existing control scheduler adapter

`native_page_worker::control::ControlledPage` retains one worker and uses the existing `preview_control::runtime::Control` for agent calls. It does not implement a second control reducer. A controller generation change invokes the exact native Page `invalidateRefs` through a private owner-only command, without replacing Page or Keyboard. Invalidation receipts are serialized and rechecked against the latest generation; a newer generation cannot inherit an older acknowledgment. A control which was already closed before binding, or ends afterward, closes the captured worker. The task holds a weak state reference between events, so it cannot keep an abandoned owner alive indefinitely.

The adapter keeps each exact application result in its own action sidecar until the scheduler receipt completes. It does not put an approximate scalar application value into the control scheduler. Agent ownership, human control and execution-deadline rejection remain with the existing kernel. The host response deadline starts before queue admission, including the original optional1000ms grace. A response deadline can fence the captured worker but never replay an ambiguous mutation. This adapter is an owner library API; the actual authenticated MCP authority checks, observer admission and Pool transport selection are not wired by this checkpoint.

The private real Chromium probe takes/releases actual existing control, requires a previously issued accessibility ref to fail with `PreviewAutomationInvalidSelectorError` and `detail.staleRef=true`, and continues input on the same Page. The existing failed-modifier-chord proof still crosses the legacy alias and `preview_act` paths on that single keyboard. Synthetic private child tests additionally reject a foreign agent and human-controlled action, retain the same process ID across handoffs, and prove a late binding to an already closed control reaps its worker.

## Remaining transport and MCP boundaries

Local Chromium currently uses an inherited CDP pipe; desktop adoption uses the existing channel-owned transport. Neither is an ambient browser-global WebSocket capability. Production integration must bridge the actual captured transport through bounded, target/session-scoped commands and events, retaining the relay's late-attach/unknown-outcome semantics. It must not introduce a raw shared-profile endpoint or let a tool argument choose one.

The current non-dialog CDP lane still decodes results and event parameters as scalar serde JSON. The future bridge therefore needs a separate exact RawValue lane for application payloads, with method, ID, session and target authority decoded and checked independently. Relaxing the global parser or substituting lossy strings would not provide native parity.

The pinned Effect4.1.2 `ai/McpServer` encodes a tool success through its actual success schema and emits both text and `structuredContent` from that encoded value. Exact worker JSON alone is insufficient. The actual broker/MCP consumer must retain exact application JSON through schema encoding and outer HTTP serialization, including escaped isolated units in object keys and values. It must neither omit a native structured result nor replace it with null. This path remains inactive until its source-backed and physical end-to-end proofs pass. Input JSON scalar boundaries and the full twelve-tool native result/admission contracts remain part of that activation work.


The selected `mcp-output` oracle executes the complete pinned toolkit definitions and the actual canonical Effect4.1.2 `McpServer.registerToolkit` registration handler. Only opaque service dependencies and a synthetic application-result handler are injected. Seven cases cover isolated high/low units, a valid pair, an isolated object key, a literal escape, controls and nested data. Both the actual native encoded structured content and its text survive outer JSON encoding. The retained fixture records those exact native outcomes; this is a result-codec oracle, not evidence of real provider, authority, HTTP or CDP dispatch. The private Page runner verifies the full pinned Effect manifest before loading any selected fixture.

### Entered callbacks and handoffs

The pinned whole `SessionControl` oracle proves four queue/close/deadline cases.
The independent consumer oracle executes the complete original `requireTab`
and `runOperation` methods with a single-tab store and injected I/O. Its four
cases distinguish the creator's read during human control from a foreign
observer, reject foreign writes and cross-thread explicit tabs, and prove the
second reference invalidation in the operation's `finally` after a takeover.
An already entered callback drains across a generation change; a callback still
queued rejects the stale generation before transport. These results do not
prove MCP admission or an attached production Page consumer.

Both fixtures run under the recursive-removal guard with the admitted Node
26.13.0 binary reused by hash and receipt. The runner's optional
`--reuse-node-artifact` avoids another persistent binary copy while preserving
shared admission, source identity, hash and isolated version verification.

### Entered callback drainage in an attached Page

`ControlledPage` now retains the admitted callback generation and repeats native
reference invalidation after an entered operation finishes across a handoff.
Both normal `ControlledPage::close` and the Control-close watcher fence
arrivals and wait for that entered tail before closing the worker. Hard
transport/watchdog faults retire the captured worker directly with unknown
operation outcome. Target revocation remains a separate authority boundary. The owner-only
`synchronize_references` acknowledgment grants no action rights; it lets the
owner establish that the generation invalidation completed before observing
new references.

The private actual Chromium probe holds an original native evaluate Promise
open, takes human control, waits for this acknowledgment, creates references
through the original snapshot operation, and then releases the Promise through
the fixture's captured target/session. The completed operation must invalidate
those new references. A second entered Promise resolves successfully after the normal
`ControlledPage::close` consumer fences Control, before the worker is reaped. This is an attached owner-library
consumer proof, not public MCP admission. Existing cross-alias Keyboard and raw
JavaScript/CDP assertions remain in the same probe. Cleanup requests retained
by the artifact adapter are reported separately from physical recursive
removals; no recursive removal executes.

The full public toolkit remains inactive. Complete operation consumers and
authenticated provider-to-MCP selection still require acceptance. The following
checkpoint covers nonretiring metadata and the reserved owner control lane.

### Metadata deadlines and the owner lane

The complete pinned `PreviewAutomationBroker` source oracle distinguishes
background metadata requests (`updateCurrentTab: false`) from foreground
requests. Metadata expiration removes its caller receipt without evicting the
host or changing the assigned tab. The native foreground response budget adds
exactly 1,000 ms, then evicts the host on expiration. The oracle uses actual
Effect scheduling and host streams, with only UUID generation and the host
response transport injected; it does not prove authentication.

The Rust supervisor now preserves its retained worker on metadata expiration.
It keeps a bounded wire tombstone and its application slot until the real
response arrives. An ingress request still has its own deadline while all eight
wire slots are occupied: an expired metadata request is never dispatched.
Foreground expiration also reaches the actor through a separate retirement
signal, so an ingress backlog cannot suppress the owned-grant fence. Late
results cannot complete another request. Persistent unresolved native requests
still consume the explicit eight-slot worker capacity; this is a resource bound,
not a claim of native unlimited concurrency.

Reference invalidation uses a separate bounded owner-control channel and reply
identifier sequence, independent of application capacity and native Keyboard
state. Its writer gives owner frames priority between complete frames; it never
interrupts an in-progress frame. The fixed action, nonce, identifier and exact
acknowledgment envelope are checked. Application arguments cannot become owner
frames. A missing or malformed owner acknowledgment retires the captured grant
rather than certifying reference invalidation.

The actual Chromium probe admits eight original native waits, expires their
metadata callers, expires a ninth request still in ingress without executing
its expression, invalidates references through the owner lane, then releases
and drains the eight actual late responses. The same Page and Keyboard remain
usable. These owner-library transport receipts do not activate the public
catalog or establish authenticated MCP admission.
