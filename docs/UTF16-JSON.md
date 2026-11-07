# UTF-16 inner MCP text

`src/utf16_json.rs` supplies structured `JsonText` values for inner MCP JSON
text. `Utf16` retains JavaScript code units, including an unpaired surrogate
created by native dialog `slice(0, 4000)`. `Object` preserves field insertion
order; `Array` supports nested status envelopes; `Value` embeds ordinary
serde JSON. `stringify()` returns a valid Rust string with ASCII `\ud800`-style
escapes for lone units, so it can safely become the outer MCP `content[].text`.
No placeholder substitution or lossy UTF-16 decoding occurs.

The quoting primitive follows native `JSON.stringify` string behavior: valid
pairs become Unicode scalars, lone units use lowercase hexadecimal escapes,
and control characters use the native short escapes where applicable. This
module does not implement general JavaScript object property enumeration or
number serialization; callers provide field order and ordinary JSON values.
It does not repair other CDP, GUI, or RPC lone-surrogate channels.

The fixture `tests/fixtures/native-utf16-json-4301.json` was generated with
Node 20.19.2 native `JSON.stringify` and `String.prototype.slice` intrinsics.
The generator verifies the actual native dialog clipping expressions in
`apps/server/src/preview/ServerBrowser.ts` from native revision
`4cb6945dc60ed03f567dc5daafd083e52308e82b`. Native sources and generator were
mounted read-only inside the private, network-disabled rootless Podman test
image. The revision was read from the clean native checkout on the host and
passed through `NATIVE_REVISION`; the checkout's worktree git metadata is not
available inside that container.

Regenerate with the native checkout mounted at `/native`, this repository at
`/source`, and `NATIVE_REVISION` set to its verified revision, then run
`node /source/tools/generate-utf16-json-oracle.mjs` in that private container.
The integration target `utf16_json` checks all fixture cases and the outer MCP
JSON round trip. Run it through `tools/test-scheduled-modes --integration
utf16_json --compile-lock /var/tmp/o3-build/o3-parity-compile.lock`.
