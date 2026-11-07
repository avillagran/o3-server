# Desktop browser channel checkpoint

The reference is nightly 4288, commit
`32a0531748c44b25072d7495ce0b9bd7e9450a87`, with Effect 4.1.2.
`tools/generate-desktop-browser-fixtures.mjs` executes its actual schemas and
complete `DesktopBrowserChannel` constructor with injected configuration, input,
socket-construction and write boundaries. The committed fixture records source
hashes, 346 schema cases, 32 lifecycle traces and 93 native NDJSON byte traces.
With `--endpoints`, it also runs a real native loopback WebSocket roundtrip,
including opaque frames in both directions, a 24-byte endpoint secret and scoped
release. That trace is reference evidence; it does not prove a Rust relay exists.

`desktop_browser` implements validated commands/events, attachment and visibility
state, and incremental NDJSON decoding. Closing the input clears visibility
reports while retaining known attachments, matching the source. Unknown schema
fields are stripped; opaque CDP strings remain unchanged. Decoder failures retain
records already delivered before the invalid line.

`desktop_browser_channel` adds owned asynchronous streams, subscription-before-check
attachment waits, serialized control writes and scope cancellation. Its tests use
in-memory duplex streams and replay native lifecycle fixtures across the real
asynchronous reader. There are no filesystem cleanup operations in these tests.
The Unix descriptor adapter owns nonblocking pipe/socket descriptors directly,
sets close-on-exec and registers them with Tokio without reopening descriptor
paths. Physical tests use anonymous socket pairs and Linux pipes, including a
closed control reader. Regular-file descriptor support is not established.

This remains an integration checkpoint. The single-client secret loopback CDP
relay, endpoint queue ownership, launcher/startup ownership wiring, preview consumers
and actual desktop launcher still need implementation and acceptance. Unowned CDP
frames are ignored, as in the native channel before an endpoint is registered.
Startup continues to reject browser descriptor fields until those consumers exist.
No browser capability is enabled by this module alone.

JavaScript JSON numeric extremes and escaped lone UTF-16 surrogates remain explicit
cross-runtime limits. The fixture does not establish arbitrary JSON equivalence,
real descriptor adoption, WebSocket lifecycle or desktop UI acceptance.
The descriptor tests separately establish the owned Unix adapter. Additional-client
teardown is not established: an exploratory native close probe did not complete
and was stopped. Do not assume the source's one-client guard closes extra sockets.
