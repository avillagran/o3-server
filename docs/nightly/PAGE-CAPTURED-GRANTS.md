# Captured Page grants

This policy is an unregistered transport foundation. It is compiled by
`tests/native_page_grants.rs`; the product actor does not yet instantiate it.
It does not activate tools, authenticate an MCP caller, or grant a browser
endpoint to a worker.

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

The next acceptance boundary is an actual actor-owned grant factory and event
route, using the existing raw command lane and completion observers. It must
retain all pending callbacks through shutdown, detach late granted sessions,
refuse foreign targets at the transport boundary, and preserve the borrowed
legacy session. Actual provider authority, persistent Page operations, and the
outer twelve-tool MCP dispatch remain separate outstanding consumers. The
existing 4 MiB CDP frame cap also remains an explicit parity gap.
