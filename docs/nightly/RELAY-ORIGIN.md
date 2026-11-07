# Managed relay webhook URLs

Scheduled-task responses derive their public webhook URL from the native private
secret files `cloud-relay-url.bin` and `cloud-endpoint-runtime-config.bin` under
the profile's `userdata/secrets`. List, upsert, enablement and token-rotation
responses use the current metadata. The derived URL is not stored in a task
snapshot. Read-only task viewers still receive no webhook endpoint metadata.

The implementation follows nightly 4262's `relayHookBaseUrl`, runtime-config
decoder and server origin provider. It derives the endpoint key from the final
hyphen-separated tunnel-name component, which must be 16 lowercase hexadecimal
characters. The environment ID is not the endpoint key. Missing or invalid
configuration yields a null URL while preserving all files and the direct path.
Connector credentials never enter task responses.

`tools/generate-relay-origin-fixtures.mjs` executes the actual pinned native URL
function and configuration schemas with Effect 4.1.2. It records 80 URL cases
and 1,026 configuration cases, including optional fields, invalid types, trimming
and Unicode whitespace. The RPC integration fixture uses private synthetic
metadata, checks token rotation and read-only redaction, then verifies an invalid
configuration remains intact. It contacts no relay and starts no inference.
All five recorded source files are unchanged through nightly 4275 (`6577c6b6`).

This adds endpoint presentation for existing native metadata. Connect provisioning,
managed tunnel lifecycle, signed relay-delivery verification and trusted relay
receive-time/replay admission remain separate implementation work. A displayed
relay URL does not certify those paths or global relay parity.
