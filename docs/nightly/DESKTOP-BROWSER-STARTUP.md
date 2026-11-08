# Launcher-owned browser startup and foreground activity

Reference: `v0.1.8-nightly.20261106.4288`, commit
`32a0531748c44b25072d7495ce0b9bd7e9450a87`.

The CLI takes ownership of the launcher's `desktopBrowserFd` input and
`desktopBrowserControlFd` output before profile discovery or provider subprocesses.
Both descriptors must be dedicated pipes or sockets, distinct from each other,
stdio, and the bootstrap descriptor. They receive `FD_CLOEXEC` immediately.
This deliberately rejects incomplete descriptor pairs and regular files; the native
schema alone does not establish these ownership constraints.

The temporary bootstrap-reading runtime never registers the browser channels.
Only the final server runtime creates their asynchronous adapters and installs the
preview coordinator, before serving authenticated RPCs. `serverBrowser` reflects
an installed desktop channel or a discovered local browser executable. Shutdown
joins preview setup and coordinator tasks before dropping the owned driver.
Telemetry, resource-monitor and OTLP bootstrap fields retain their existing gates.
No installed service or shared profile is changed by this implementation.

Foreground activity uses a token per executing provider run. Beginning the first
live token publishes `active: true`; ending the last publishes `false`. Explicit
end is idempotent, and dropping a guard also ends its token. Thinking counts as
activity. Terminal events end activity before checkpoint capture and provider
reaping, so those operations cannot retain focus. Overlapping attempts cannot
clear a successor's token. The desktop driver forwards these transitions in order.

`tools/generate-preview-activity-fixtures.mjs` executes the complete pinned native
`PreviewAgentActivity` service with Effect 4.1.2. Its 32 traces contain 1,420
operations, including overlapping turns, repeated endings and independent threads.
The Rust differential test compares every event and both threads' active states.
Additional tests cover RAII drop and a disconnected subscriber. These pure checks
are distinct from provider lifecycle and real launcher transport validation.

The actual CLI integration uses private inherited socket pairs, an independent
bootstrap stream, ephemeral loopback HTTP, launcher authentication and
`server.getConfig`; it also verifies shutdown closes the control stream. Admission
regressions reject incomplete, aliased and unavailable channels before profile
creation. Run all runtime checks in the private Podman harness. This document does
not assert full client activation, maximum-message parity, hardware coverage, or
complete nightly parity.
