# Hub reset credits and native API boundary

This leaf executes the nightly 4288 hub API (`32a0531748c44b25072d7495ce0b9bd7e9450a87`) and ports its schema validation, management requests, account selection and credit redemption. The source hashes and exact oracle coverage are recorded in `cliproxy-intake.json`.

`provider.consumeResetCredit` accepts the native input union. A valid provider-instance input wins union decoding before a source input, matching the contract's member order. Source IDs retain their spelling; account and credit IDs use the native trim/nonblank checks. Instance requests currently return the native missing, disabled or unsupported-driver errors. They do not acquire a synthetic redemption implementation.

Source redemption reads the configured management key through the shared resolver. An unreadable reference refuses the operation before any HTTP request. The hub must contain the named, enabled Codex account; the server never switches accounts. The redemption request ID is the native SHA-1 UUIDv5 over the workspace/account ID and credit ID. The actual provider URL is sent as data to the configured hub, rather than fetched directly by this client.

The four native outcomes are preserved. A failed cooldown-clear request after a successful redemption returns the native warning with the successful outcome. Redemption and source refresh hold the same per-server mutex. The source is reread before publishing its replacement snapshot, so a concurrent stale refresh cannot resurrect a redeemed credit. The server now includes a validated `nextCreditId` in the usage-source wire event, allowing an authorized client to request redemption. The existing client quota reducer and UI action require their own integration proof.

Management URLs preserve subpaths, strip the panel's `management.html` suffix and discard the old query/fragment as the native relative URL resolution does. Errors expose a safe category or HTTP status, never response bodies or credentials. Requests have the native 15-second deadline. Additional PEM roots from `NODE_EXTRA_CA_CERTS` are added to the default trust roots. JSON HTTP decoding handles the UTF-8 BOM and replacement decoding at the outer response boundary; inner API response strings still use strict JSON parsing, matching their separate native schema stage.

## Source and physical proof

`tools/generate-cliproxy-fixtures.mjs` executes the complete actual API module with Effect 4.1.2, real contract/API schemas, the real Digest implementation and a controlled HTTP transport. The only provider import narrowed from its service module is the complete pure `codexPlanLabel` function. The actual Codex, Claude and shared usage helper implementations remain in the source closure. The clock fixes observation time; the timeout case advances its lower-level sleep service rather than changing the source deadline.

The closed API schema compiler contains 66 nodes. It fails on unsupported AST tags, refinements, transforms, tuple shapes and regular-expression flags. Its corpus covers 189 root-schema cases, 1,254 cases spanning every node, 16 native UUIDs, 11 numeric formatting cases, 13 safe error cases and 43 native API traces. Private HTTP comparisons exercise all consume/credit traces; the empty full-account trace is retained as oracle material without claiming full account projection parity.

All generators and tests run in rootless Podman with read-only source/runtime mounts, private HOME and temporary storage, and networking disabled. Synthetic `op` is copied into a private directory. Synthetic hub servers never forward the provider URL, touch a real account or redeem a real credit. The actual scoped RPC test proves that a read-only client cannot redeem, input validation happens before HTTP, successful redemption preserves a cooldown warning, and private management keys do not reach the result. The consumer test also proves shared-lock refresh serialization and the post-redemption snapshot.

The focused validation contains 19 passing tests per build mode, including the real client/server RPC and quota reducer, with strict all-target Clippy in both modes.

## Remaining parity boundaries

The native hub account projection is implemented in [CLIPROXY-PROJECTION.md](CLIPROXY-PROJECTION.md). Provider-instance redemption drivers remain separate unfinished consumers. Expiry selection currently handles RFC 3339 and RFC 2822 strings; the other loose date forms accepted by JavaScript `Date.parse` are not certified. Runtime-specific TLS/transport error codes still need cross-runtime comparison. The HTTP body limit is one MiB; the source decoder has no equivalent explicit limit. As elsewhere in the Rust JSON boundary, isolated UTF-16 surrogate escape sequences cannot be represented as Rust strings and are rejected. These limits prevent a complete nightly parity claim.
