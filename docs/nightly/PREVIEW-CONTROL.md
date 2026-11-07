# Browser control and popup source foundation

The reference is the complete `SessionControl.ts` class and `adoptPopup` function
from nightly 4288 (`32a0531748c44b25072d7495ce0b9bd7e9450a87`). The generator
executes those source bodies with actual canonical Effect 4.1.2 and contract error
schemas. Only the monotonic clock, deferred callback work, page close,
publication and viewer services are synthetic. Source and staging hashes are
retained with the fixture.

The fixture contains 45 complete control traces and 112 popup-adoption traces.
Control traces observe ownership reservation before waiting, immutable agent
identity, generation invalidation, exact deadline equality, independent read
observations, pending-navigation versus handoff-only barriers, adjacent queued
input coalescing, repeated close and native error precedence. Popup traces record
open options, same-page receipts before publication, inherited opener/profile/
automation owner, failure cleanup and the controller-only notification policy.
They do not start browsers, install tools, use host profiles or remove files.

`preview_control::Kernel` is an inert effect kernel. Synchronous submission methods
reserve ownership immediately and return an action ID. `drain(now)` emits owned
callback, completion and generation effects; completion and tracked-work settlement
feed back into the kernel. A dropped result receiver cannot cancel queued work.
Adjacent latest-input submissions share the same waiting action ID; a running
callback and intervening queued work retain their order. The driver, rather than
an RPC future, must own callbacks and drain cleanup.

The production integration supplies trusted `Authority.session_id` as the
immutable automation owner; profile names are not authority. Local and desktop
pages hold separate controls and immutable transport receipts. Viewer takeover,
release, disconnect, ordinary input and adjacent latest-input work use the native
queue. Agent callbacks recheck authority, current page identity and modal state
when they actually start. Agent admission remains restricted to its existing
owned-tab scope; popup adoption and broader native read scopes are not enabled.

Regenerate with `tools/generate-preview-control-fixtures --checkout ... --staged
... --deps ... --node ... --evidence-root ...`. The wrapper requires rootless
Podman, read-only source/dependency mounts and no network. Output uses a new
retained evidence directory. Do not execute the native reference bodies on the
host. The standalone differential test uses the shared private-test sandbox guard.

## Owned runtime adapter

The runtime adapter owns queued callbacks and tracked futures in a task set.
Callers receive cloneable result observations; dropping or timing out one does not
cancel its callback. Reservations and snapshot publication happen synchronously
before the returned observation is polled. The monotonic deadline clock is sampled
when a valid queued action can start, preserving the native guard/error precedence.

The source `close` operation waits for its queue tail. The adapter's separate
lifecycle `shutdown` additionally drains every owned callback and tracking task.
The page owner must settle its transport and navigation watches before waiting
for shutdown. Dropping the final runtime owner closes admission while retaining
already-running work until it settles. This is an internal ownership guarantee,
not a new wire operation or permission grant.

Nine private Tokio tests cover immediate takeover and stale-queue rejection,
dropped result receivers, independent observations, pending-navigation versus
handoff barriers, adjacent latest-input aliasing, close/shutdown cancellation,
expired queued work and final-owner disposal. The tests also compile and execute
result/shutdown futures as owned `Send` tasks. A rejected callback that owns its
last control is disposed outside the driver's mutex, preventing destructor
re-entry from deadlocking shutdown.

Actor cleanup stops admission, fails pending transport requests, closes page
watches, and drains owned callbacks before publishing cleanup acknowledgement.
An acknowledged process with unfinished callbacks still retains its closing
receipt and profile. Reconnection creates a fresh native control with generation
zero and the same immutable automation owner; captured old pages cannot address
the replacement even if the frontend reuses an opaque session identifier.

Status renders exact UTF-16 dialog units into inner MCP `content.text`, including
a surrogate pair split at the native 4,000-unit boundary. Raw structured content
remains a compatibility projection, and initially unpaired incoming CDP strings
remain a separate decoder gap. Existing foreground-turn cleanup is unchanged;
the native 30-minute idle retention and one-minute sweep require a separate
lifetime consumer. Navigation commit/handoff tracking, complete viewer input
mapping, popup adoption and full toolkit admission remain explicit gaps.
