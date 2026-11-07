# Managed client and reachability rules

`src/managed_limits.rs` ports the pure authentication, client, Connect and bind
decisions from nightly 4262. The source oracle executes the actual
`managedLimits.ts` functions, `auth/utils.ts::loopbackBindAddress` and shared
hostname normalization with Effect 4.1.2. It generates 9,216 cases covering
client combinations, Connect overrides, authentication modes, host literals,
Tailscale enablement and background-service diagnostics.
All three covered source files are unchanged in nightly 4275 (`6577c6b6`).

An installed policy forbids unauthenticated operation. Client limits distinguish
the desktop, local browsers and remote clients. Connect requires both its own
permission and remote-client permission. When remote clients are prohibited,
the bind decision accepts only native loopback literals or normalized localhost;
it rejects DNS names and mapped IPv6 forms that the native bind check rejects.
The native refusal messages and branch precedence are retained.

This module is a foundation, not completed enforcement. The production readiness
gate still rejects unfinished client and Connect policy families. Enabling them
also requires pre-bind invocation, current-session revocation, pairing and cloud
link restrictions, and complete Tailscale Serve route inspection, selective
closure and reinspection. A pure successful bind check cannot establish that an
existing tunnel no longer exposes the server. Those physical consumers remain
part of the full parity goal.

Run the affected tests through `tools/test-scheduled-modes --unit-filter
managed_limits` in each feature mode. The runner compiles on the host and
executes only in rootless Podman with read-only inputs and private state.
