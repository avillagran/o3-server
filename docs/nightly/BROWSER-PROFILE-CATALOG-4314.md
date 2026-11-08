# Browser profile report catalog

This source-only prerequisite implements a real in-memory browser profile
catalog and connection-owned report tokens. Full source status and the browser
selector remain unselected. Profiles are never guessed from profile directories
or active browser tabs.

The implementation follows nightly 4314 revision
`83a3674ffd84ffb33de768210c806cb1ac53fd80`:

| Original source | SHA-256 |
| --- | --- |
| `packages/contracts/src/baseSchemas.ts` | `edd4d2c73cc79ce5e50bffd35b9c861fe091de23a1ac792db864d3b71b09a7af` |
| `packages/contracts/src/browserProfile.ts` | `a8995cb24686afddd7dc66a85b2c61019d1ffbd551dac26864fffbc13e9f10e2` |
| `packages/contracts/src/preview.ts` | `d93c13bf3c0510629d61cce4249192cc083b3849d00c7e9f2c4abd70b1d4fb3d` |
| `apps/server/src/preview/ServerBrowser.ts` | `e8b025a1c7de82ba8089c5fa7d4439ee69e1d19a74af7657a26a419a51cdfab6` |

The complete catalog closure is at `ServerBrowser.ts:1526–1588`; the contract
helpers are at `browserProfile.ts:28–59`, and the report schema is at
`preview.ts:214–226`. The original WebSocket layer acquires one scoped reporter
per connection (`ws.ts:3537`) and invokes it through `preview.reportProfiles`
(`ws.ts:3217`). The existing server authorization table already assigns that
method `preview:operate`.

## State and lifetime

The actual `preview::Manager` owns `browser_profile_catalog::Catalog`. A trusted
coordinator can obtain a non-cloneable reporter token; report JSON cannot select
or forge a token. The existing framed RPC connection creates its reporter on its
first accepted report and owns it until connection retirement. Schema decoding
and actual session authorization happen first. The connection guard retires the
report before announcing its own retirement, including error and cancellation
paths.

The catalog implements these original behaviors:

- Real built-ins are always `Default` (`default`, persistent) and `Incognito`
  (`incognito`, ephemeral). Reports cannot rename or replace them. Other IDs
  remain persistent even if a report labels them incognito.
- Live reports form a union. Replacing a profile value preserves the first
  insertion position of that ID. Updating a reporter moves that reporter to the
  end, exactly like the original delete/set sequence on a JavaScript Map.
- The latest report selects its exact matching default ID, falling back to
  `default` when absent. Before the first actual report, status omits
  `defaultProfileId`.
- Retiring a connection rebuilds the union from remaining reports. When the
  last report retires, the last catalog and default remain usable.
- Recording frame rate starts at 30. An explicitly reported 30/60 updates the
  global value; omission and reporter retirement never restore an older rate.

The accepted decoded scalar-Unicode domain uses ECMAScript trimming, UTF-16
length limits, the original ID control-character exclusion, report count 24,
and unknown-field stripping. Names are not subjected to the ID control filter.
The generic Rust `serde_json::Value` ingress still cannot represent lone UTF-16
surrogates accepted by JavaScript; the native oracle includes those cases and
records that existing wire limitation. Exact Effect parse-error prose is not a
claim of this model.

## Prepared checks and integration limits

Six in-memory unit cases cover initial state, union/order/replacement,
retirement and sticky frame rate, built-in protection, decoder boundaries and
coordinator lifetime. The framed RPC case uses real issued State sessions,
real WebSocket framing, two reporting connections, a denied session, malformed
payloads, ignored forged token input, and actual connection retirement. All
fixtures are retained in their private container; no tree cleanup or link
fixture is used.

`tests/preview/browser-profile-catalog-oracle.mjs` prepares a differential
reference using the complete original contract modules and the unmodified
catalog closure. Tokens are acquired/finalized through actual Effect Scope,
including the original release function. It records complete state traces and
native schema cases; it does not reimplement the catalog algorithm. No profile
filesystem, browser, provider process, GUI, or external account is involved.

The whole original oracle has passed in the private rootless container:
`browser-profile-catalog-original-1/result.json`. Its complete proof remains at
`browser-profile-catalog-original-1/browser-profile-catalog-native.json`, SHA-256
`8f63cb0c922bf602081cd352412547047be7786386f603b2a88f5a57edfa882e`.
The separate reference conversion passed without rerunning the oracle or
executing O3 Rust (`reference-result.json`).

The registered Rust differential case consumes
`tests/fixtures/browser-profile-catalog-native-scalar-4314.json` (32,074 bytes,
SHA-256 `0c5408ff8777285d1230807ced96c90db9fd0349d796c3cb611dcb9425df8ea3`).
It compares four complete traces with 26 snapshots and 18 scalar schema cases.
Two additional native-accepted lone-surrogate cases are excluded only from this
scalar corpus because the generic serde JSON ingress cannot represent them.
Their complete inputs/results remain in the original proof; they are not native
rejections or passing Rust cases. The new test source is sealed separately at
SHA-256 `7f7aeddfcc604b75588cf5fa7441a58a269f2bb15a2fdea68bb594578218d025`.

On current server base `1947191`, both minimal/default strict all-target checks
and the minimal libtest build pass in
`browser-profile-catalog-rust-1/build-result.json` (94.38 seconds, 926 source
inputs unchanged). The existing-target libtest is 138,891,792 bytes, SHA-256
`13b97e9cd66935baa06a25e5c620a65de4f5f154d667006339ba1633241f20db`.

`browser-profile-catalog-rust-1/runtime-result.json` passes all eight exact Rust
cases with no ignored cases: six model cases, one differential case covering
the 26 snapshots and 18 scalar schema cases, and the real authenticated framed
RPC case. Runtime is 0.595 seconds in rootless Podman with no network, a
read-only binary and private RAM HOME/config/fixtures. Source and binary remain
unchanged; no host credentials, display, audio or writable host mount is exposed.
Fixtures remain until container exit, with no recursive fixture cleanup.

These receipts accept the in-memory catalog, scalar decoder and connection-owned
RPC lifetime. They do not accept full status, chooser/download behavior, the
actual UI producer, provider/browser execution or the full server suite. The
original oracle was not rerun for this Rust validation.

No new build target, SDK pin or Node/Electron copy has been introduced. The actual
O3 client has profile/default/frame-rate settings controls but does not yet send
`preview.reportProfiles`. That UI producer remains required. This server source
adds the existing native reporting method and connection lifetime; it does not
advertise full status, chooser/download support, or browser activation.
