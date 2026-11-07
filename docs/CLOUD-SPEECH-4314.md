# Cloud speech foundation (nightly 4314)

This increment ports the complete `CloudSpeech.ts` service from native revision
`83a3674ffd84ffb33de768210c806cb1ac53fd80`, with authenticated server routes and
an independently owned text worker. It does not enable a cloud capability or add
production startup configuration. Transcript postprocessing has a source-derived
prompt/text resource and oracle, but its structured provider execution and HTTP
consumer remain unfinished.

## Source and execution boundaries

`tools/generate-cloud-speech-fixtures` runs the original pinned modules with one
canonical Effect 4.1.2 installation. Explicit settings, driver, upstream fetch and
logical filesystem boundaries replace external effects. The cloud corpus has 72
whole-service scenarios and 85 observations. The separate postprocessing corpus
has 14 scoped-service scenarios and 12 exact UTF-16 prompt vectors. Original
filesystem cleanup is not executed by these reference fixtures.

The real text actor uses the admitted immutable Node 26.13.0 binary, an empty
environment, a private home, bounded queue/reservation, request correlation and
owned process-group retirement. It has no ASR engine, model, VAD or arbitrary
module authority. The resource preserves the original dictionary, filler and
prompt functions rather than replacing their Unicode behavior with Rust rules.
ASR operations are rejected in text mode. A prepare call receives a live worker
acknowledgement, not just an earlier process-start receipt.

Provider requests use fixed native origins and paths, reject redirects, omit
ambient proxy authentication, and never retry uploads. The trusted Rust-only
loopback transport is a private acceptance boundary, not a caller-provided URL.
Error bodies are not exposed. The current response body safety bound is 8 MiB;
this is an O3 bound, not a demonstrated native maximum. Logical multipart field
ordering and WAV bytes are covered; exhaustive native encoder-byte equivalence,
maximum aggregate memory and catalog expiry clock edges are not claimed.

## Fresh secret authority and intentional security deviation

A named secret is invalidated before lookup. The real resolver then obtains a
fresh effective provider snapshot and retains the canonical kind and decoded
configuration from the exact plan used to read the value. Immediately before
upload, the request checks its current authority and reads a coherent final
credential/provider snapshot under the settings writer lock.

The original whole-source resolver oracle demonstrates that changing a provider
account during a blocked lookup can still upload the old key when the reference
itself is unchanged. O3 deliberately rejects this case with the existing
`key-changed` error, without sending PCM, fallback or replay. This is a security
deviation, not exact parity at that edge. Label changes and configuration fields
ignored by the actual driver decoder do not revoke the operation. Changing the
account before the next lookup uses the new decoded account normally.

The retained native evidence is `cloud-secret-authority-v1/fixtures.json` in the
private evidence directory. Its three cases distinguish a change before lookup,
a change during lookup and a change to the credential itself. No real vault,
credentials, provider network or paid inference is used.

## Current acceptance

The frozen `cloud-current-runtime-dfc1a4a79cb04adb817ce25c03e7ccfd/result.json`
records both minimal and default modes. Each immutable binary passes both the
72-scenario whole-service differential and the authenticated HTTP fixture. The
HTTP child uses an actual measured 512 KiB Tokio worker stack, the real settings
private store/resolver, the real text worker and real loopback HTTP transport.
It checks all three provider endpoints, WAV/file ordering, router ZDR, cached
catalog access, read/operator/writer permissions, vault relock, the rejected
held-account race, accepted label/unknown-field changes and a fresh-account
lookup. Provider keys and PCM are not logged. The original public resolver cache
path is also exercised twice before the cloud invalidation path.

These are synthetic provider responses and PCM, not a transcription accuracy or
live cloud-provider acceptance claim. Existing local CPU HTTP/WS acceptance is
separate. No microphone, user profile, real credential or live service is used.
The cloud capability remains absent until startup ownership, complete provider
postprocessing, cancellation/shutdown and current-authority consumers are ready.
