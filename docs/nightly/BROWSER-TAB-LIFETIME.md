# Agent browser tab lifetime

Reference: T3 Code nightly 4301, `4cb6945dc60ed03f567dc5daafd083e52308e82b`,
`apps/server/src/preview/ServerBrowser.ts`. Nightly 4314 leaves this source unchanged.

Agent-created tabs survive foreground turn completion and provider-session revocation.
Revocation still prevents subsequent agent commands; it does not implicitly destroy
the user's visible browser. Explicit tab close, thread deletion, runtime failure and
server shutdown retain their owned cleanup paths.

The coordinator sweeps idle headless agent tabs once per minute. A tab is idle only
when its last admitted operation finished strictly more than 30 minutes ago. Any
connected viewer protects it, including a read-only viewer. Desktop visibility also
protects a tab; hidden desktop tabs are reclaimed only when an agent open needs
space at the server limit. Human-created unclaimed tabs have no agent idle expiry.
The old absolute process/viewer age deadlines are removed; authorization and session
expiry checks remain active.

Capacity is eight tabs per trusted agent session and 32 per server, including opening
reservations not already represented by a published tab. A canceled launch retains
its reservation until its owned setup and cleanup settle. Hidden idle reclamation
selects the oldest last-used timestamp with stable admission-order ties, and cannot
bypass an already-full agent budget. Admission validates an agent open URL before
performing any sweep or reclamation.

Activity belongs to actual invocation completion, including failures, rather than
the lifetime of its HTTP observer. It updates every tab owned by that agent in the
thread and an explicitly selected foreign read target. Default status prefers the
latest-created owned tab, otherwise a connected viewer and then recent activity.
Default open reuses a single owned tab; multiple owned tabs require an explicit
selection or `reuseExistingTab=false`. Creator ownership remains immutable.

Reclamation rechecks the exact page identity, activity and viewer count under the
registry locks. A bounded desktop visibility guard covers the close reservation;
it is released before control/channel callbacks. No asynchronous wait occurs while
that guard is held.

## Validation and limits

`tools/generate-browser-lifetime-fixtures.mjs` executes complete native helpers from
the pinned source, producing 271 observations for capacities, pending reservations,
visibility, stable ties, default selection and operation activity. Physical private
pipe tests cover idle boundaries, viewer protection, stale plans, late activity,
budgets, canceled launches and rejection before lifetime effects. Actual MCP tests
cover persistence across turns and revocation, reuse and explicit close. Fixtures
record removal plans without deleting their profile trees.

This checkpoint does not implement the remaining browser tool catalog, native
keyboard chords, popup adoption or downloads. It does not certify the full desktop
engine or emulator video acceptance.
