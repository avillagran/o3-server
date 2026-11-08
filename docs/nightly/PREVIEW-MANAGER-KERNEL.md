# Native preview manager state kernel

`preview_manager::Kernel` ports the complete state transitions in nightly 4288
`apps/server/src/preview/Manager.ts`, pinned to
`32a0531748c44b25072d7495ce0b9bd7e9450a87` (source SHA-256
`54cca40642c36da824e03390e72153d66c072e629403fef0eccdb47c77b765da`).
It has no filesystem, browser, process, network or asynchronous effects.

The coordinator supplies the server epoch, allocates a base tab ID before calling
`open`, and supplies lazy clock and reveal UUID functions. Allocation precedes URL
validation, including failed opens. Server IDs append the epoch. The callback sees
the new snapshot before state and event commit; a callback panic commits neither.
Inputs are already decoded native contract objects. Callers serialize access and
publish drained events in order. Each event already
contains its final revision and epoch, so an outer event ring must not increment
these again. Read-only accessors expose individual snapshots, insertion order,
epoch and revision.

The ordered map preserves JavaScript composite NUL-key collisions and replacement
positions. Thread listings sort stably by ISO timestamps; close-all emits one
revision per matching session in map order. Missing close is a no-op. Repeated
headless marking is sticky and consumes no clock value. Missing lookups consume
neither clock nor reveal UUID. Server status reports require explicit control.
Resize and adjustment preserve existing session fields and emit native request
payloads, including empty adjustments.

The fixture generator executes the actual complete Effect manager, replacing only
clock and random services. The 43 retained traces (588 operations) compare every return/error,
event, before-publish snapshot and thread listing. Separate pure checks cover
composite-key collisions, callback rollback and lazy input consumption. The
fixture records source hashes and comes from
`tools/generate-preview-manager-fixtures.mjs`.

This checkpoint supplies the state kernel only. Browser ownership, actor commands,
RPC orchestration, event subscription delivery and startup activation remain
coordinator responsibilities and require their own acceptance tests. No capability
is enabled by adding this module.
