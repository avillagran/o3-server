# Private captured Page gateway

This is an inactive owner-library foundation. It does not register any public
MCP tools, select a product browser driver, or certify the twelve-tool toolkit.

`NativePage::gateway` binds one nonce-protected loopback WebSocket to the actual
captured tab owner. Only one worker connection may attach. A duplicate binding
is refused. Initialization failure before publication releases that reservation;
a clean Page close still follows the existing clean-only recapture rules.

The sealed initialization capabilities are distinct from invocation receipts:

- The runtime capability permits only the captured grant's existing command
  policy. It is for the pinned worker's initialization and platform events.
- The cleanup capability permits only detach of a session recorded as owned by
  that grant, with the recorded parent route. It is not a borrowed observer or
  shared-profile teardown capability.
- An invocation must present its original tool class and opaque receipt outside
  CDP parameters. The real `Scope.caller` is rechecked for each frame, including
  live provider generation and revocation. A read receipt cannot be relabeled
  as an action or evaluation receipt.

The gateway rejects Origin, wrong Host/path, query strings, duplicate envelope
fields, non-monotonic IDs, malformed metadata, and a second connection. Payload
JSON stays raw through the actor and exterior response; isolated UTF-16
surrogates are not replaced with null or a lossy scalar string.

The actual State/Scope test opens a real private WebSocket, verifies wrong-class
refusal before actor submission, begins an authorized frame on the exact actor
queue, and returns a synthetic raw result containing an isolated surrogate.
The actor result is an explicit fixture boundary, not Chromium acceptance. The
same test independently revokes the actual provider Context while its callback
remains entered, proving that an unwritten old frame cannot begin afterwards.

Shutdown fences the owner, aborts and drains gateway tasks, and awaits the
existing owned-session close receipt. An ambiguous outcome remains unknown.
No shared browser close, new browser context, credential discovery, or model
chosen CDP URL is introduced.

## Remaining activation obligations

The persistent Node Page sealed initialization and invocation-scope consumer
have a private physical proof through the actual actor and Chromium. They
remain unselected by the product broker and public MCP catalog. The same-connection
scope-end fence reserves each prior frame synchronously before spawning its
command task. It waits for actual writer begin or final frame refusal, not its
remote reply. Later requests cannot reserve another frame on that receipt. Native evaluate sends termination
before its callback finally, but does not wait for its response before allowing
a successor; the original producer must remain unchanged.

The four MiB envelope, separate application worker's eight-call queue, and
captured grant's existing eight attachment / 64 session and correlation windows
remain O3 foundation restrictions, not native parity. The invented eight-call
CDP gateway rejection has been removed; all unresolved command tasks remain
owned and are drained at shutdown, with actor queue backpressure. The inherited
pipe and desktop actors now admit exact CDP independently of the legacy
application's eight-call window. A predecessor fence preserves WebSocket
production order until actor enqueue or definitive refusal; it does not wait
for remote responses. Scope-end separately waits for actual writer begin or
refusal. Neither fence relabels an unresolved command as late to hide capacity.

The official pinned Playwright `WebSocketTransport` sets `maxPayload` to
256 MiB. Replacing the smaller foundation envelope requires incremental parsing
and explicit queued-byte ownership; merely raising a length constant or
allocating nine maximum-size buffers is not an acceptable implementation. The original pinned Page evaluate,
SessionControl and real Playwright/Chromium capacity oracle demonstrate a
termination command reaching the wire while eight CDP calls remain outstanding.
Counting a pending command as late does not reduce that real outstanding count.
Do not activate this gateway as the full native toolkit while those differences
or the remaining operation, attachment, recording, event and broker consumers
are unresolved. Scoped operation input still uses a Unicode-scalar Rust Value;
that input boundary must also be replaced before claiming native isolated-UTF16
input parity. Exact raw results already traverse both the CDP and worker lanes.

The provider classifier separates a tool from its permitted Page operations:
`preview_open` may emit `open` then `navigate`; `preview_act` is restricted to the
eight original action names; appearance invokes `setColorScheme`. Optional
status enrichment is a separately constructed owner request, never an
untrusted flag permitting a foreground tool to choose the metadata path.
