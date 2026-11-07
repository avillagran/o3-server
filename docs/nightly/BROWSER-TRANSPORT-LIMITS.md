# Browser command ownership and transport limits

Local and desktop CDP actors correlate up to eight pending command IDs. A blocked
evaluation does not prevent a dialog answer from being sent. Replies may arrive
out of order; a mismatched opaque session cannot satisfy a known command. Each
active command has a five-second deadline, bounded additionally by its original
six-second caller deadline. An expired queued command is never issued. Ordinary
command timeout or rejection does not discard another pending command.

Owned writer tasks have eight queue slots and a five-second write bound. Reads,
notifications, cancellation and command deadlines remain independently pollable.
Actor finalization aborts and joins the writer; local finalization also joins its
reader, reaps the child/process scope and uses the existing captured-identity
profile-removal executor. No generic stack-limit increase is introduced.

Setup owns a newly created local target before it is admitted to the tab map.
Failure closes that target while preserving a live sibling in the same process.
A timed-out target creation has no reliable target ID, so it fences the connection
and reaps the owned local process. Rollback stops a fresh process, or a shared
process if its target cannot be closed. Private acceptance verifies the retained
sibling, exact close-target commands and eventual child reaping.

For desktop replies, pinned Playwright 1.60.0
[WebSocketTransport](https://github.com/microsoft/playwright/blob/v1.60.0/packages/playwright-core/src/server/transport.ts)
sets a 256 MiB receive-byte limit. The actor uses that bound for messages and
frames. The endpoint's 100 MiB receive limit is the opposite direction: backend
requests into the relay. The native `CdpRelay.ts` estimator separately allows
64 Mi serialized UTF-16 units and conservatively accounts for escaping. These
limits count different things; a Unicode reply can exceed 100 MiB in UTF-8 while
remaining within the native value budget.

The production private event decoder admits at most 256 MiB minus one JSON
payload bytes, reserving LF for the client's 256 MiB total-record encoder limit.
CR/LF delimiters are not retained in the decoder. The default pure decoder stays
unbounded to preserve the native decoding oracle. Overflow is checked before
appending a byte, closes existing owned endpoints, and prevents replacement
endpoints on that failed input. Normal stream half-close behavior is unchanged.

The pinned source oracle executes the native value estimator for ASCII, Unicode,
astral and escaped strings. Shared-string arrays exercise logical 63/64/65 Mi
budget outcomes without serializing or allocating a giant wire record. Physical
proof sends bounded 5 MiB ASCII, 6 MiB UTF-8 BMP and astral replies through actual
private NDJSON, loopback WebSocket and actor ownership. A small configured-limit
proof verifies overflow and endpoint fencing. These tests do not establish full
maximum-size or aggregate-memory parity. FD relay queues do not gain aggregate
byte admission in this increment. The local inherited-pipe reader retains its
existing 4 MiB bound; the desktop receive change does not upgrade that separate
limit.

Held-evaluation tests cover both an inherited pipe and a desktop WebSocket.
Dialog state, native automation status/tool results and viewer broadcasts remain
the next consumer increment. No browser capability is enabled by this transport
foundation alone.
