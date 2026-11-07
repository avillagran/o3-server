# Agent access with a desktop browser driver

The browser access gate accepts either an installed local browser executable or
the coordinator's installed desktop driver. This matches O3's existing
`serverBrowser` availability decision: the native desktop transport can provide
the page without an additional Chromium installation on the server host.

The driver comes from the trusted startup channel, not a URL, capability flag or
debugging endpoint supplied by a remote RPC caller. Global and project agent
access settings, current session authority, creator ownership and control queues
remain required. Removing the driver with no local executable removes availability.
The desktop actor still adopts the existing owned page; it does not create a
browser target, launch a local process or allocate a local profile.

The private integration test starts the actual Codex provider adapter with a
source-owned synthetic CLI, obtains its real scoped MCP credential, lists and
calls the tools over authenticated HTTP, and completes open/navigation/evaluate/
snapshot/status through the desktop channel and loopback WebSocket actor. It
checks availability before and after driver installation/removal, settings denial,
absence of local profile storage, and absence of target creation or browser close.
The existing peer also checks anonymous rejection and thread mode boundaries.

Its CDP page and frontend attachment discovery are synthetic. This proves the
provider/MCP/desktop routing and admission boundary, not real browser rendering,
clipboard handling, native keyboard chords or the complete agent tool catalog.
Actual GUI and browser Pool acceptance remains a separate client checkpoint.
