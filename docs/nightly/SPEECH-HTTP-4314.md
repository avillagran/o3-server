# Local speech HTTP and application ownership (4314)

The source boundary is nightly 4314 (`83a3674ffd84ffb33de768210c806cb1ac53fd80`), including the complete local HTTP controllers and media-stream handler. Cloud speech, microphone capture, consent, and transcript postprocessing are separate consumers.

`State` owns an optional local speech service. The standalone server admits it only when both application-owner flags, `--speech-node` and `--speech-bundle`, are supplied. They require absolute paths. No RPC, settings field, model identifier, URL, or PATH lookup can select executable code. Admission verifies the exact Node 26.13.0 digest and all 140 regular engine-package files against a compiled allowlist derived independently from the four pinned archives. The model store remains responsible for catalog size/hash verification before inference. Startup neither downloads a model nor starts inference. An invalid graph leaves the other server routes available and local speech unavailable.

The private Node snapshot is reused only when its bytes, exclusive ownership, executable mode, and immutability match. Unknown, writable, linked, or partial destinations are retained and refused. Every worker load independently seals its complete package and model graph again. Shutdown signals the service and awaits the actual worker cleanup receipt. An unknown receipt remains an error; a later successful close does not erase it.

## Current authority

HTTP status, model listing, preparation, and buffered transcription require `orchestration:operate`. Model/settings mutations require `settings:write`. This follows the native controller order: decode the request before evaluating its required scope. The settings writer checks current authority again under the existing settings lock before any persistence effects. Read-only sessions cannot use an operation-scoped settings adapter to write. Only bearer sessions are advertised; proof-bound sessions are refused instead of silently downgraded. MCP identities are not voice credentials.

Media WebSocket admission uses the `media` audience and the last duplicate `wsTicket` query value. An invalid ticket never falls back to a valid bearer; a blank ticket does. After upgrade, this implementation additionally checks the original session against current persisted authority before commands, output, and during active work. The native pinned handler authenticates only at upgrade; continuous revocation is an explicit O3 enforcement improvement. Revoking an already running feed fences its owner before reporting authorization loss. Revoking or disconnecting during preparation cancels the owned startup job and awaits its cleanup, rather than discarding a future with an unknown process behind it.

The scope retains the reservation through `finish` until close. A reset may retain a clean shared engine, while a failed or uncertain reset disposes it and keeps uncertainty sticky. The transport uses the native command/event tags without exposing causes, credentials, or PCM in errors and logs.

## Evidence boundaries

The whole original HTTP router and actual Effect framework produce 111 admission observations: 96 pre-domain rows are compared against the actual O3 connection service; the remaining 15 compare the admitted domain arguments. Those 15 native calls deliberately use an already observed native service error and are not claimed as real engine response equivalence.

Physical private tests cover actual HTTP control/PCM handling, media handshakes, ticket precedence and audience, MCP exclusion, open-session revocation, revocation during an already running hung native feed, and private settings authority. The service tests use the whole original worker factory with a clearly identified synthetic ASR engine. Separate CLI acceptance exercises the sealed real CPU engine and verified native Tiny model with synthetic silence; transcript byte counts do not establish recognition accuracy.

A failed model download HTTP 500 is already a typed native download error and is not retried. Transport and disk retry policies remain unchanged. Removing the sole installed model preserves the selected identifier when no other installed candidate exists, as in `SpeechService.ts`'s complete `removeModel` body; it does not invent a default-model replacement.

Defensive O3 bounds are explicit: control bodies 128,000 bytes, buffered PCM plus framing bounded by 19,200,000 PCM bytes and the native options bound, five-minute HTTP upload collection, WebSocket frames/messages 256 KiB, and ten-minute socket lifetime. The native application command bounds remain 128,000 text bytes and 64,000 PCM bytes. These transport safety bounds and overflow rejection do not establish parity with the native Node WebSocket implementation's full transport maximum. No aggregate maximum-memory or unpaired-surrogate JSON parity is claimed here.

The frozen checkpoint has nine focused physical cases in each feature mode (five HTTP/media, two current-authority/settings, one native admission projection, one immutable-cache case), plus both-mode all-target Clippy and CLI checks. The final default-feature CLI receipt additionally proves eight actual CPU/startup conditions, including positive and negative capability admission. Its source fence covers all Rust modules, runtime resources, and the producer/acceptance scripts. Earlier failing fixture expectations and the rejected producer edit remain separate evidence; no successful receipt is attributed to a different source snapshot.

Reproduce the real CPU acceptance with already acquired, verified inputs; it performs no download:

```sh
CARGO_TARGET_DIR=/absolute/worktree-specific-target \
  tools/test-speech-http-runtime \
  --node /absolute/admitted/node \
  --inputs /absolute/verified-engine-acquisition \
  --evidence-root /absolute/private-evidence \
  --default-features
```

The runner asserts rootless Podman, snapshots the CLI and runtime libraries under the shared compile lock, verifies the explicit Node digest and private runtime version, and retains its private profile and evidence. It never executes the CLI, engine, model, or acceptance process on the host. The startup flags are optional; existing unconfigured servers do not acquire or advertise a local speech engine.

## Media fixture phase boundaries

The media authority/flow fixtures prepare the same authenticated state/session/model through the actual HTTP controller before starting their ten-second per-event witness. They require preparation's 204 response and assert that WebSocket readiness reuses exactly one CPU worker. Preparation keeps the existing thirty-second control-request budget; native process, inference, cleanup, and media event deadlines are unchanged.

An instrumented default-parallel five-case run under two CPUs measured cold first-ready phases around 8.2 seconds, while normal subsequent events took milliseconds and hung-feed revocation took about one second. This establishes that cold admission consumed most of the former event witness locally. The retained failed CI log did not identify which shared `next` call expired, so the exact CI failure cause remains unproven. Each event now carries an explicit phase label and elapsed timing. Cold Worker and real CPU CLI acceptance remain separate receipts; preparing the media fixture does not reclassify them or simulate a ready event.
