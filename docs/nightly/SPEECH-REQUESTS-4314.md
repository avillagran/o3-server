# Speech request schemas and framing

Reference: T3 Code nightly4314, `83a3674ffd84ffb33de768210c806cb1ac53fd80`.

The closed native schema program appends the actual `SpeechTranscriptionOptions`
and `EnvironmentSpeechPostProcessingRequest` roots. It has 666 nodes; the prior
650 retain their IDs and behavior. The existing settings, sparse, provider and
managed goldens remain byte-identical. The shared interpreter still enforces
JavaScript safe-integer bounds, UTF-16 string lengths, the native dictionary
constraints and schema defaults. There are 72 whole speech-schema observations
and 18,648 individual node observations.

Binary requests contain a four-byte little-endian metadata length, at most
128,000 UTF-8 metadata bytes, and at least one trailing PCM byte. The decoder
validates UTF-8 before JSON and strips exactly one leading UTF-8 BOM. A repeated,
embedded leading-whitespace or trailing BOM is not JSON whitespace. A BOM inside
a language value remains part of that value. Thirteen rejected and five accepted
native whole-parser observations cover these boundaries, malformed UTF-8, pooled
views, maximum metadata and missing PCM. The reproducible private generator
executes the actual shared function and verifies its whole source manifest.

Envelope decoding deliberately does not validate PCM. One trailing PCM byte
passes the native envelope parser and fails the separate owned PCM decoder.
`BinaryRequest::owned_pcm` provides that explicit ownership and validation step
before asynchronous inference. The envelope borrows only the received view.

Typed schema constructors preserve native decoded defaults and drop unknown
fields. Schema acceptance and decoded/encoded values are compared directly with
Effect 4.1.2. Complete human-readable schema diagnostics are not reproduced by
the internal `InvalidOptions` error. Fixed parser errors match the pinned parser.
Unicode folding follows the compiled Rust tables; full Node Unicode 17 locale
behavior and raw lone-surrogate JSON remain qualified.

This is request preparation, not an HTTP service or engine activation. A future
HTTP consumer must enforce authentication, native operation scopes and the total
body limit before buffering; an owned worker must receive validated PCM. No
routes, model downloads, transcription calls, microphone access or capabilities
are enabled here. Fixtures use private synthetic data and are retained without
filesystem-tree deletion.
