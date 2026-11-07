# Preview kernel and owned runtime coordination

The coordinator uses `preview_manager::Kernel` as the single source of session
snapshots, server epoch and revisions. Its event ring stores finalized native
events without incrementing their revisions again. Server tab IDs include the
server epoch; base ID allocation precedes URL normalization. Native manager
acceptance is recorded in `PREVIEW-MANAGER-KERNEL.md`.

A server open publishes its snapshot and reserves setup ownership in one locked
transaction, then notifies subscribers before waiting for the desktop. The wait
uses the native ten-second timeout (`ServerBrowser.ts:766`). A connected desktop
adopts its existing page without initial navigation. If no page attaches, an
available desktop channel causes sticky headless marking before local fallback.
A late attachment cannot replace that running headless page. Local agent contexts
without an explicit profile use an isolated agent profile.

Setup runs in an owned background task with owned profile root, agent registry and
browser handles. Protocol2 tracks preview-open interruption separately and cancels a pending call
when its WebSocket response receiver closes. Other RPC handling is unchanged.
RPC cancellation removes the logical session and signals its
reservation. A local CDP operation already creating a target finishes inside that
owned task, checks its generation and closes stale results. Closing a pending tab
removes the snapshot and signals cancellation before awaiting any browser actor;
it does not wait for the desktop timeout. Thread deletion and profile clearing
additionally join matching setup/cleanup tasks before reporting completion.
Shutdown cancels setup, joins setup and desktop monitoring, stops actors and joins
completion monitors. Completion monitors use weak browser references to avoid
keeping an otherwise dropped browser alive.

Unexpected desktop detachment ends its actor connection while retaining its kernel
session. An attachment monitor reconnects that same session through a fresh scoped
endpoint (`ServerBrowser.ts:2764`). Explicit close and thread cleanup remove the
session, fencing later attachment events. Local process completion closes its
kernel session. Client status reports cannot claim server control; trusted CDP
navigation updates use the internal controlled path. Reveal requests allocate a
fresh UUID after session lookup, preserve sticky headless state and publish the
native force flag.

`install_desktop(Arc<State>, Channel)` returns an owned `DesktopDriver`. Availability
is false by default. Startup retains the driver and awaits preview shutdown and
then driver shutdown. A dedicated subscription forwards exact token-based agent
activity edges in order, independent of attachment work. Desktop CDP mouse commands
forward their coordinates and move/click phase through the channel; this is scoped
to actual mouse commands and does not claim the complete native locator-action
implementation.

Physical tests use the real channel, loopback WebSockets, synthetic CDP streams and
an owned local pipe peer. They cover publication order, pending close, actual HTTP/protocol2 disconnect and Interrupt,
caller cancellation during target creation, retained/reconnected desktop sessions, status
control refusal, activity edges, pointer forwarding, reveal UUIDs and the actual
ten-second sticky fallback. Existing preview, browser, agent and deletion tests
provide adjacent regression coverage. All fixtures run in private containers and
retain their trees. No SDK, real frontend browser or live desktop service is used.
Startup descriptor admission, capability publication and actual CLI wire proofs
are tracked by their separate startup acceptance.

Desktop live page notifications are now projected by the consumer described in
`PREVIEW-NAVIGATION.md`. Local inherited-pipe notifications and the remaining native
network/download/dialog consumers retain their separate acceptance boundaries.
