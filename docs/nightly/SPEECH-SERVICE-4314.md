# Native speech control lifecycle (4314)

Source: `83a3674ffd84ffb33de768210c806cb1ac53fd80`, complete
`apps/server/src/speech/SpeechService.ts`. The service is a standalone backend
consumer of the verified ModelStore and owned Worker. No State, HTTP route,
capability, microphone, or live profile is connected by this checkpoint.

## Resources and authority

`Service` shares engine/store ownership. `bind` supplies a request/session-specific
`Settings` facade. Its reads must obtain current effective settings, and its patch
must check current authority and policy at commit. A previous read is never an
authorization receipt. Selection and replacement patches occur after disposal and
must therefore revalidate authority after that await. No ordinary settings cache or
captured privileged writer is installed here. Future transport admission must also
authorize each resource operation; this internal facade is not an HTTP permission
boundary.

Prepare, selection, removal, download and shutdown run in owned jobs. Cancelling a
waiting caller does not drop an in-flight worker creation or shared transfer.
Reservations precede spawning. Downloads for the same model share one result;
another model and exclusive operations are refused while the transfer is active.
Wrong-model cancellation is a no-op. Shutdown marks closing before waiting,
cancels a transfer, awaits the reserved operation and discovery, and serializes
worker disposal. A load completing after closing is disposed without publication.
The final service lease also initiates closing. Unknown cleanup remains unknown
and blocks later resource use; another successful empty close cannot erase it.

`WorkerEngine` re-admits each file of the complete trusted package graph and the
selected model's catalog digest before the real Worker snapshots them. Its Node
runtime is explicitly admitted. There is no PATH discovery, caller executable,
model URL, or arbitrary wire path. GPU discovery is serialized and cached forever
on success or one minute on failure, matching the native cache policy.

## Source-proven control behavior

- An unknown saved model falls back to the native default; selecting a known,
  uninstalled model is allowed. Unknown IDs fail without disposing a loaded model.
- Selection disposes before its settings patch. Removing the selected model uses
  the first other installed catalog entry, preserving catalog order.
- Prepare reuses a matching loaded model and acceleration. Its final unload
  deadline is at least five minutes, including when the configured timeout is
  two minutes. Changed unload settings reschedule from the current clock.
- `never` cancels the deadline; shutdown still disposes. Busy timer expiry
  reschedules through the configured timeout, preserving the native minimum rule.
- Status and ordered model-list projections retain their native distinction:
  `ready` means the selected model file is ready, not that an engine or public
  endpoint has been admitted. Benchmarks appear only when requested.

## Reproducible evidence

`tools/generate-speech-service-fixtures` imports the entire untouched native
service under Node 26.13.0 and one canonical Effect 4.1.2 installation. Only its
settings, store/config, and engine dependency bindings are explicit virtual
fixtures. Eight complete traces cover selection, missing IDs, prepare/unload,
download deduplication/cancellation, selected-model removal, never-unload and
Silero recording/installation deadlines and backoff.
The internal loaded-model return object is not a fabricated HTTP response.

`tests/speech_service.rs` compares public values and lifecycle observations from
those traces and adds explicit revoked-authority, cancelled-caller, pending-load
shutdown and unknown-cleanup regressions. Virtual resources and in-memory file
operations are identified as such. The separate opt-in
`examples/speech_service_acceptance.rs` uses the actual Local ModelStore,
admitted complete transcribe-cpp 0.4.1 graph, tiny catalog model and real Worker
inside a private container; it performs no microphone capture or network request.

## Remaining consumers

The Silero descriptor is the exact native internal helper, excluded from the
selectable catalog. A model download concurrently acquires it through the actual
Store. Its installation timeout is two minutes, and failure is optional. Recording
acquisition uses fifteen seconds and a ten-minute retry backoff; installation
bypasses that backoff and success resets it. Cancellation interrupts both owned
store operations. The whole native service independently confirms those rules.
The recording policy is a standalone kernel until its audio consumer is wired.

This checkpoint does not implement the entire native SpeechService. Late sealed
VAD attachment to an already prepared worker, transcription/stream option
resolution, automatic CPU recovery, active-transcription accounting, and their
engine/VAD lifetimes remain to be connected and proven. No real Silero model or
VAD inference has been acquired or claimed by these virtual deadline fixtures. The existing worker and pure options/PCM
leaves do not by themselves close those orchestration gaps. No claim of full
speech or HTTP parity follows from control tests or the earlier inference proof.
The ModelStore's retained staging/foreign-file safety differences remain as
documented in `SPEECH-MODEL-STORE-4314.md`.
