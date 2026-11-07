# Native speech factory and bounded transport resource

Reference: T3 Code nightly4314, `83a3674ffd84ffb33de768210c806cb1ac53fd80`.

The generated native resource contains the complete `speech/native.ts` and
`speech/audio.ts` runtime after TypeScript erasure and replacement of the one
audio import with its whole factory. No native factory body is patched. The
generator imports both original and generated modules and requires identical
`Function.prototype.toString` bodies for all three exported operations. The
manifest records source hashes, function hashes, Node 26.13.0 and the original
transcribe-cpp 0.4.1 dependency. The complete MIT copyright and permission notice
remain in the resource and the separate license files. Type erasure intentionally
preserves whitespace holes; the generated file has a narrowly scoped whitespace
attribute so its checked function bodies remain byte-identical to Node's original
loader output.

The private transport adapter imports checked native bytes directly and always
passes the owner's explicit engine file URL; the native ambient package fallback
is unreachable. Model, engine and optional VAD files come from a hashed startup
specification, never command-frame paths or MCP arguments. Input files are
opened without following their final links, checked against the owner's size
and SHA-256 receipts, and retained for the lifetime of this worker. A production
Rust producer must additionally supply its complete owned no-follow path and
package-installation receipts. This adapter alone does not establish package
origin or complete a model installation.

Private NDJSON records are bounded at 32 MiB including the final LF. PCM is
canonical base64, decoded only from the received view into finite Float32 samples
within [-1,1]. Whole buffered input up to 19,200,000 bytes is admitted; the browser
transport's smaller bound is not reused. An oversized record is rejected, never
truncated. This is a bounded adapter, not a claim that every maximum-size output
or aggregate queue boundary has been measured.

Private physical fixtures run the actual native factory with an explicitly
synthetic engine. Seven observations cover one sample, all 4,800,000 samples of
the five-minute input, two separate streaming turns with revision reset, native
automatic CPU fallback, native exit, IPC disconnect without replay, and abort of
a synchronous hung native child followed by its actual reap. The child receives
only the cleared private environment and native IPC variables. No real
transcribe-cpp inference, model download, microphone, upload or hardware probe
is involved. Fixture files remain private and retained.

The Rust process-group owner, retained-leader fencing, recovery of uncertain
cleanup and application service integration are subsequent work. No HTTP route,
MCP tool, UI selector or voice capability is activated by this resource.
