# Source-control authentication credentials

The implementation is pinned to T3 Code nightly revision `32a0531748c44b25072d7495ce0b9bd7e9450a87` (4288). Authentication discovery now consumes the schema-declared GitCafe and Bitbucket settings through the existing private-slot materializer and external secret resolver. This does not enable the remaining source-control action families.

## Native evidence

`tools/generate-source-credentials-fixtures.mjs` loads the complete pinned `GitCafeCredentials.ts` and `BitbucketApi.ts` modules with Effect 4.1.2. Settings, process, HTTP, filesystem, and clock services are synthetic; the native methods themselves execute. The fixture records the transitive source hashes and 58 GitCafe plus 80 Bitbucket observations. Rust replays results, fresh settings reads, process requests, HTTP authorization, cache expiry, negative-cache behavior, LRU eviction, concurrent deduplication, environment host scoping, credential priority, and unreadable-reference refusal.

GitCafe saves successful environment/CLI credentials for five minutes, missing CLI and signed-out failures for ten seconds, and transient failures for no time. Saved settings bypass that cache. The credential helper receives the native protocol on stdin capped at 64 KiB; stdout is capped at 64 KiB and stderr at 256 KiB by the existing O3 runner. The token never appears in argv. A 401 invalidates the cached credential. Existing documented GitCafe API CLI discovery remains available when the native credential helper supplies no credential. A selected credential's HTTP failure and an unreadable saved reference never select a different account. The legacy `discover()` entry point retains its previous contract; the RPC uses state-aware discovery.

Bitbucket preserves native saved-access-token, saved-email/API-token, and `T3CODE_BITBUCKET_*` environment priority. An unreadable saved access-token reference refuses fallback; a valid access token does not require reading an unused API-token reference. The older `BITBUCKET_ACCESS_TOKEN` alias remains accepted only when the native access-token environment variable is absent. Admission, HTTP authorization, and error projection use the native independent fresh-read boundaries.

## Physical boundary

The real `server.discoverSourceControl` RPC uses per-state credential caches and the existing settings transaction lock. It releases that lock before external reference resolution or HTTP. HTTP uses four concurrent slots, no automatic redirects, a 30-second request timeout, and an 8 MiB response cap. Synthetic loopback HTTP and a private executable verify Bearer/Basic authorization, helper stdin and argv, cache reuse, reference-only configuration output, and refusal without HTTP or CLI fallback. Separate private process tests verify input limits, closed pipes, deadlines, cancellation, and child cleanup. Tests retain their private fixture directories for container teardown and perform no recursive deletion.

## Remaining boundaries

This is authentication discovery, not a port of all GitCafe/Bitbucket API operations. The generic native rate-limit ledger, redirect/retry behavior for other API methods, project actions, and direct instance APIs remain outside this boundary. GitCafe principal parsing retains the existing O3 actor/handle projection. The O3 process-output caps are an explicit transport boundary for oversized helper output. TLS implementation differences and JavaScript JSON versus serde behavior (including isolated UTF-16 surrogates and numeric limits) remain explicit transport boundaries. No actual account, vault, host credential helper, or public HTTP endpoint is used by these proofs. No global readiness gate or signing/session identity changes are included.
