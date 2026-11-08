# Captured Page grants

The inherited-pipe and desktop actors now retain this policy behind a trusted
owner-library factory. `Manager::capture_native_page` captures the real existing
tab, target, registry and Control. It allocates distinct native CDP sessions;
the existing legacy observer session is borrowed for identity only and is never
granted or detached. No public tool selects this factory, and no worker endpoint
or complete MCP dispatch is activated by this checkpoint.

The owner supplies a captured target. Root commands cannot enumerate targets,
create targets or contexts, close the shared browser, change download paths, or
mutate global cookies. A child target requires an existing owned session as its
witness. A reply arriving after that witness disappears records a cleanup-only
session. Unsupported child target types also remain owned cleanup obligations,
without becoming usable worker sessions.

Close prevents new ordinary reservations. Already entered commands remain
tracked until their actual completion, including commands other than attach
that can cause later events. Caller cancellation releases an unsent reservation;
it does not certify an entered command's outcome. An explicit lost wire outcome
is sticky even if a later reply arrives. Parent detach does not fabricate a
child detach receipt. Cleanup may target only individually recorded sessions.

Security metadata rejects duplicate fields. Application JSON remains raw, so
isolated UTF-16 escapes do not pass through a Rust scalar-string conversion.
The bounded request/session/attachment counts are resource limits of this
foundation, not certified native payload or concurrency parity.

## Focused evidence

`/var/tmp/o3-server/page-grant-policy-drain-check/result.json` records the exact
source fence, strict Clippy, and eight passing private-container cases in each
feature mode. Only the integration executable was snapshotted: 4,727,248 bytes
for minimal and 6,875,624 bytes for default. Fixtures perform no filesystem
operations beyond the shared container admission check. No recursive removal or
host runtime test is involved.

## Actual actor and caller acceptance

`/var/tmp/o3-server/page-real-grant-physical-4/result.json` records the actual
Chromium acceptance with immutable executable SHA
`2ff75a7bb24f55bee30221773f72c973ff33cdf0952a329dbb8c759a14d33a4c`.
The private pipe proxy delays one actual attach response by 350 ms; it never
manufactures a response. The caller is cancelled before that response arrives.
The actor still registers the late session, drains it, and records its individual
detach acknowledgment. A separate actual auto-attached Worker executes through
its own session. Its detach uses the real CDP parent session; an explicit root
attach's lineage witness is deliberately not mistaken for that protocol parent.
Deepest children close first. Remote detach rejection produces an unknown
receipt instead of an unbounded retry. The v3 child-close failure is retained;
only v4 is acceptance for the corrected implementation.

Owned raw events route before the scalar decoder. Events from unrelated targets
still use the original decoder. A duplicate lone-surrogate application event on
the borrowed observer cannot retire the shared profile. The proof verifies an
exact console event and result, foreign-target/global-close refusal, ended
callback refusal, the borrowed observer's continued evaluation, a neighboring
tab's survival, and zero profile removal requests or recursive-removal attempts.

`NativePage::admitted_provider` accepts the actual opaque provider `Context`.
Its ticket binds the original RunContext, actual thread and registry identity,
and dynamic `Scope::caller` checks. Native status/snapshot/wait classification
uses the fixed `NativeOperation` enum, not a caller-provided metadata flag.
Every command checks credential lifetime, capability availability, provider
generation, and the original callback before reaching the actor. Opaque 24-byte
random receipts never resolve to a successor context. Owner-library admission
is explicitly separate from provider admission and is not exposed as an MCP
argument or endpoint.

`/var/tmp/o3-server/page-real-owner-focused-3/result.json` records 36 browser/actor
units plus ten grant cases per feature mode, all passing in private containers.
The new credential case uses actual Context acquisition and Scope.caller, proves
archived metadata read versus operate admission, generation fencing, and Context
drop revocation while the callback is still entered. Denied receipts leave the
actor queue empty. Immutable unit executables are individually below 200 MiB;
the four artifacts total 330,342,320 bytes and are retained. No test deletes a
fixture tree or executes a host browser.

The provider ticket issuer is not yet connected to the Node transport adapter.
The private gateway, callback-end cancellation notification, runtime/event and
cleanup capabilities, sealed worker attachment, original outer MCP registration
and all twelve real tool consumers remain the next boundary. In particular,
eight correlated commands, 64 late receipts and the existing 4 MiB CDP frame cap
are foundation resource limits, not final native concurrency/payload parity.

Final all-target Clippy is recorded in
`/var/tmp/o3-server/page-real-owner-strict-2/result.json` for both modes. The
consolidated receipt and final source hashes are in
`/var/tmp/o3-server/page-real-owner-focused-3/proof.json`. The only declaration
change after functional execution moves public reexports before the test module;
all-target strict checks use that final declaration order.

## Exact callback completion

RunContext now exposes a read-only completion receiver. EndAdmission clears the
same active gate and then publishes completion on return, panic or callback
future drop. A human takeover or normal Control close does not publish completion
while that entered callback still drains. The native owner command future selects
this exact terminal notification; an abandoned transport future therefore drops
its existing permit, wakes the actor, and leaves any written response tracked by
the existing late-receipt path. It does not create a ninth correlation slot,
change a native timeout, relabel a ticket, or close the shared browser.

`/var/tmp/o3-server/page-callback-end-units/result.json` records twelve Control
runtime cases and strict checks in both modes. The separate physical receipt
`/var/tmp/o3-server/page-callback-end-physical/result.json` uses immutable
executable SHA `7209487f446458cec5c4fbdd2971fd616b413628d9996c2c44ee366d8299e321`.
The real page logs its started marker before the callback returns, while its
350 ms awaited promise remains outstanding. The retained background caller then
fails with `callback_ended`; the actual later browser response is drained before
individual detach and a clean close receipt. This is a callback-lifetime proof,
not certification of eight simultaneous native timeout/terminate sequences.

## Clean restart and immutable operation class

An ordinary handoff still returns the same live owner binding. Explicit native
owner retirement can be recaptured on the same visible target only after its
clean receipt: the factory allocates a fresh native CDP session while preserving
the target, its real Control and the borrowed legacy observer. A closing owner
without a clean receipt remains retained and blocks recapture, even after a
subsequent attempt releases the recorded sessions. Unknown is not silently reset.
The actual Chromium proof is
`/var/tmp/o3-server/page-reopen-operation-physical/result.json`, executable SHA
`e4b8f9362f98fe1d9b5053725b786718274378e2ff4662413711fe253349f924`.
The attach-delay fixture now waits for an exclusive, owned phase marker rather
than depending on the number of earlier attachments.

A provider admission now records its immutable NativeOperation. Rebinding a
snapshot admission to evaluate or act fails. `validate_receipt_operation` checks
the original class and live authority; the forthcoming sealed worker wire must
use it and match the actual dispatched native tool. The generic owner-library
CDP method is not a wire/MCP authorization surface. CDP Runtime.evaluate cannot
itself distinguish a native snapshot's trusted DOM script from a model's evaluate
operation, so its method name is deliberately not a readonly classifier.

`/var/tmp/o3-server/page-bound-operation-units/result.json` records the actual
Context/Scope.caller regression and ten grant cases in both modes, plus final
all-target strict checks. To avoid new large retained executable copies, the
no-follow source artifact is hashed and gzip-sealed under the compile lock;
only that single executable is expanded into private container tmpfs and checked
against its uncompressed hash before execution. This is not an archive or
filesystem-tree fixture. All compressed artifacts and receipts are retained.

## Whole native handler admission oracle

`/var/tmp/o3-server/page-handler-admission-oracle-3/page-apr4kl7w/proof.json`
evaluates the complete pinned handlers.ts, McpToolAccess.ts and
McpInvocationContext.ts together with the complete native toolkit and actual
Effect MCP registration. Caller, broker and attachment services are explicit
refusing external boundaries; their behavior is not replaced with a copied
policy reducer. The twelve real handler declarations prove that status,
snapshot and waitFor use readsAsCaller and the other nine use actsAsCaller.
The caller-load boundary is reached by writes; reads reach the broker boundary.
This proves declaration routing, not live provider or attachment execution.

Snapshot's Tool.Readonly annotation is false, but its real handler uses
readsAsCaller and strips the deprecated includeImage/save flags. Those two facts
must not be conflated when assigning authority. The actual waitFor schema has
selector/locator/text/urlIncludes/timeoutMs and no expression input. The oracle
rejects expression-only input, strips an excess expression field alongside valid
text, and executes the complete native waitFor function against a private DOM
boundary. Its original JSON.stringify quoting preserves an isolated UTF-16 unit
and prevents the synthetic text from becoming script. No read receipt is
certified for arbitrary evaluate/act dispatch by this oracle.
