# Provider maintenance and browser follow-up — 2026-11-02

This is an implementation record, not a status promotion or a claim of complete
T3 browser equivalence. No external network, inference, real installer update, real browser
profile, service access, push or merge was used.

## Maintenance evidence

`tests/update_mechanisms.rs` uses a real Connection, lib-t3-rust's server config
reader and provider-instance projection. A user-owned Homebrew formula fixture
with an installation receipt first returned canUpdate=false (failed assertion),
then passed after the resolver change. It and Claude's native versioned symlink
layout now execute only their original updater, and a missing receipt returns to
manual advice. Reading config never executes an update. Root-owned installs stay
manual even when the server itself runs as root.

The additional-driver fixture checks original-prefix npm argv for OpenCode, Pi
and Grok. Scoped and unscoped packages are recognized by package.json identity;
OpenCode's v2 package is recognized as well. OpenCode's native `.opencode/bin`
layout supports its `upgrade` command. Native Claude requires the versioned
`.local/share/claude/versions` target and matching `.local/bin/claude` symlink.
Neither native resolver invokes a shell or downloads a fresh installer.

Homebrew formula recognition requires the original prefix's brew, a core formula
receipt matching the provider, and writable user ownership of the keg hierarchy,
receipt, binary and brew. npm resolution takes precedence inside a Homebrew Node
keg. Casks and custom taps whose receipts are not recognized remain manual;
this is a conservative implementation limit, not proof they are infeasible.
Standalone binaries without an identifying native layout remain manual. Cursor,
Muse, Antigravity and registry agents receive method-specific manual advice;
fx has generic original-installer advice. Execution adapters for the additional
drivers remain unsupported even when maintenance is available.

All plans use the existing serialized, bounded, cancellable execution path with
installer output suppressed and a bounded post-update version check. A successful
version probe does not establish registry-latest equivalence. Tests never ran a
real package manager, native updater or global install. Newer upstream resolver
and driver sources informed package/command names; these are source-only facts.

## Broker and viewer evidence

`tests/agent_browser.rs` first failed the real config-reader assertion that
agentBrowserAccess could be true. It now passes with a fixture browser, and turns
false when the global setting is disabled. Deterministic native Claude/Codex peers
consume the actual injected MCP configuration through a real Connection-driven
run. They initialize/list/call tools, reject a wrong bearer, reject file navigation
and a foreign tab, navigate/evaluate, then run again with a project override that
withholds browser access. Session completion removes snapshots and reaps the
fixture process before deleting its private ephemeral directory.

Each run gets a fresh random bearer and loopback-only HTTP MCP listener. No token
is written into settings or history. The listener rejects browser Origin headers,
bounds connections, body size, request duration, operation duration and lifetime,
and checks global/project access on each call. Owned tabs are limited to four per
run and eight server-wide. The MCP endpoint and outstanding connections are
aborted on run completion; normal and dropped guards schedule owned-tab cleanup.
Claude processes that used a broker are not reused with an obsolete MCP endpoint.
Codex receives per-thread MCP config, including an explicit disabled entry when
access is withheld. Native fixture acceptance is not a real CLI compatibility test.

Implemented tool subset: status, open, navigate, close, resize, evaluate, textual
snapshot, CSS click/type and key press. Arbitrary HTTP(S), including localhost and
private development URLs, follows newer T3 source and its preview normalizer;
local network prohibition was not inferred where upstream allows development
URLs. Explicit file/FTP navigation is refused and CDP is configured to block those URL patterns.
Actual Chromium subresource/redirect/file confinement is unproven by the fixture.
Downloads are denied. Browser data directories are mode 0700. CDP uses inherited
pipes and exposes no debug TCP listener.

Persistent-profile tabs now share a process with distinct attached CDP sessions;
closing one tab keeps the context alive until the last tab closes. Agent contexts
are ephemeral and isolated from user profiles and other runs. The lifecycle test
uses the real lib preview reader and confirms process survival/reaping. Viewer
input has one human controller per tab; agent mutation checks human control.
The stream supports history navigation; list/viewer polling reconciles actual CDP
history/title/readyState into snapshots. Appearance, zoom, cookie/cache clearing
and hard reload reach CDP rather than changing metadata only.

## Remaining gaps

This does not finish the full requested browser contract. Semantic/ARIA locators,
frames, select/drag/hover/wait, dialogs, uploads, clipboard/probe, downloads and
recording tools are not implemented. The textual snapshot is not T3's accessibility
snapshot. Open/profile/reuse/reveal semantics are a subset; agent-created profiles
are private ephemeral contexts rather than selectable saved-login profiles.
Human takeover races during an already-started command, crash recovery and complete
unsolicited CDP load/download/dialog events require more work. Navigation state is
polled, not a complete event-driven mirror. No real Chromium rendering, sandbox
startup, real CLI MCP handshake or Chromium child RSS was established. None of
these gaps has been demonstrated infeasible. The fixture verifies the implemented
subset only; the capability advertises that subset, not all upstream tools.

Pinned binary differentials exercise existing reader contracts and do not prove
external maintenance or MCP/browser equivalence. Validation and RSS measurements
are recorded in the dated CONFORMANCE section and accompanying JSON.
