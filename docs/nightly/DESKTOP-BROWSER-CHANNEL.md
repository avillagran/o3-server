# Desktop browser channel checkpoint

The reference is nightly 4288, commit
`32a0531748c44b25072d7495ce0b9bd7e9450a87`, with Effect 4.1.2.
`tools/generate-desktop-browser-fixtures.mjs` executes its actual schemas and
complete `DesktopBrowserChannel` constructor with injected configuration, input,
socket-construction and write boundaries. The committed fixture records source
hashes, 346 schema cases, 32 lifecycle traces and 93 native NDJSON byte traces.

`desktop_browser` implements validated commands/events, attachment and visibility
state, and incremental NDJSON decoding. Closing the input clears visibility
reports while retaining known attachments, matching the source. Unknown schema
fields are stripped; opaque CDP strings remain unchanged. Decoder failures retain
records already delivered before the invalid line.

`desktop_browser_channel` adds owned asynchronous streams, subscription-before-check
attachment waits, serialized control writes and scope cancellation. Its tests use
in-memory duplex streams and replay native lifecycle fixtures across the real
asynchronous reader. There are no filesystem cleanup operations in these tests.

This remains an integration checkpoint. The single-client secret loopback CDP
relay, endpoint queue ownership, launcher descriptor adoption, preview consumers
and actual desktop launcher still need implementation and acceptance. Unowned CDP
frames are ignored, as in the native channel before an endpoint is registered.
Startup continues to reject browser descriptor fields until those consumers exist.
No browser capability is enabled by this module alone.

JavaScript JSON numeric extremes and escaped lone UTF-16 surrogates remain explicit
cross-runtime limits. The fixture does not establish arbitrary JSON equivalence,
real descriptor adoption, WebSocket lifecycle or desktop UI acceptance.
