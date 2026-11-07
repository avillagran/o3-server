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

This source foundation does not yet replace production viewer control or agent
admission. It does not enable popup adoption or advertise additional capabilities.
Trusted `Authority.session_id` is the intended automation-owner input; profile
names are not authority. Production integration must preserve that identity through
open reservations, pending targets, popup receipts and cleanup before expanding
agent admission beyond its current owned-tab scope.

Regenerate with `tools/generate-preview-control-fixtures --checkout ... --staged
... --deps ... --node ... --evidence-root ...`. The wrapper requires rootless
Podman, read-only source/dependency mounts and no network. Output uses a new
retained evidence directory. Do not execute the native reference bodies on the
host. The standalone differential test uses the shared private-test sandbox guard.
