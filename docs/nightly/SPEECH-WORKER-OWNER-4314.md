# Owned native speech worker — nightly4314

This foundation runs the complete pinned native speech factory behind an owned
Rust process lifetime. It does not register HTTP routes, a speech engine/catalog,
a model downloader, cloud providers, microphone capture, or a voice capability.
Actual `transcribe-cpp0.4.1`, model/VAD acquisition and real inference remain
unaccepted. Physical fixtures use an explicitly synthetic engine.

## Resource and authority boundary

The original resource provenance and full MIT notice are documented in
[SPEECH-NATIVE-RESOURCE-4314.md](SPEECH-NATIVE-RESOURCE-4314.md). The whole generated
factory and audio functions remain unchanged. The application supplies a
`WorkerSpec`, never a deserialized MCP/HTTP engine URL or model path.

`SealedFile` opens every path component without following symbolic links, checks
regular-file identity and an expected SHA-256/size receipt, and retains the file
lease. `NodeRuntime::capture` creates a fresh private executable snapshot shared
across workers. An explicit `EnginePackage` manifest lists the entry and every
transitive package file; only those files are copied into a new private graph.
Model and optional VAD bytes are independently verified and copied. Existing
paths are never overwritten. Staging and runtime directories are retained, not
recursively removed. Long hashing/copying runs on Tokio's blocking pool.

A generated Node26.13 preload permits module resolution only to builtins, the
exact bundled native data URL, and graph-listed files. It is inherited by the
native child through an explicit `NODE_OPTIONS`; neither process searches PATH
for its runtime. The environment is cleared, then the owner supplies private
HOME/TMPDIR and working directory, empty PATH, the sealed preload, and
`ELECTRON_RUN_AS_NODE`. Destination ancestors must be owned by the application
user or root; writable non-sticky ancestors and writable destination parents
are refused before creating a snapshot. The native
factory can additionally set its source-defined CPU backend variable.

This is a trusted package-receipt boundary, not an operating-system sandbox for
malicious admitted native addons. OS shared-library dependencies and a complete
real engine installation receipt still need separate acceptance. A manifest
with missing dependencies fails closed instead of loading ambient Node packages.

## Requests and stream identity

One reservation covers serialization, pipe writes, and replies. A concurrent
caller receives `Busy`; large PCM is encoded only after acquiring that reservation.
There is no audio replay or unbounded request backlog. Buffered PCM retains its
native19,200,000-byte maximum; each live feed is at most64,000 bytes. The private
NDJSON record budget is32MiB **including LF**, accommodating base64-encoded full
buffered PCM rather than inheriting the browser's16MiB limit.

The reader preserves partial records across cancelled reads and checks its budget
before appending bytes. Owner/command identity and reply type are checked before
publication. Stream handles contain private worker identity and a generation;
finished, reset, foreign, and stale generations cannot send audio into another
turn. Cancellation during a request terminates the lifetime instead of reusing a
possibly partial input record. Begin reservations exist before IPC publication.

Discovery is one-shot. The original factory calls `child.kill()` after receiving
devices without awaiting child exit; repeated calls could otherwise accumulate
children. Rust closes/fences/reaps the discovery lifetime before returning a
successful result. A rejected `Busy` discovery never stops the active operation.
Discovery does not require a model file.

## Process lifetime and receipts

The bridge starts a new session/process group. Its leader stays unreaped while
requests run; no `try_wait` or early `wait` can release its PID for reuse. Close
first requests graceful termination, observes bounded output closure, then sends
the final group SIGKILL fence **before** polling leader wait. Drop also fences
before releasing an unreaped child. No negative-PID signal is sent after wait
has begun.

`Cleanup::Confirmed` means the group fence and leader reap were confirmed.
`graceful` additionally requires observed closure and a successful bridge exit.
An unconfirmed signal/wait remains `Unknown`, even if other observations suggest
exit. Runtime startup failures retain a typed `StartupFailure` containing that
cleanup receipt inside the returned I/O error; an error is never silently treated
as confirmed cleanup. Normal native disposal reaps the factory's child. Following a forced bridge
crash, Rust fences remaining group members; orphan reaping belongs to the OS or
container init and is not falsely reported as Rust reaping every descendant.

The operation, startup and close deadlines are owner inputs. They do not redefine
the application HTTP/client deadlines. Full speech service, streaming admission,
engine acquisition and application shutdown integration are later consumers.
Rust string transport also does not claim preservation of arbitrary lone UTF16
surrogates in native engine output.

## Focused validation

Private acceptance covers actual Rust→admitted Node→whole factory execution with
synthetic inference, full19.2MB PCM, stream generations, hung-child cancellation,
Busy discovery, exit/disconnect without replay, bridge crash, dependency/source
snapshot isolation, and refusal of an existing excluded module. Fresh test child
processes measure an actual512KiB Tokio worker stack, avoiding glibc stack-cache
reuse. Additional physical pipe tests exercise exact32MiB/LF accounting and
cancelled partial reads. An explicit process-control fault injection withholds a
fence receipt after performing real owned cleanup and requires `Unknown`.

The final acceptance consists of seven owner integration cases and three focused
frame/fence tests per feature mode, plus the seven protocol-only regression
observations. The negative startup executable is an explicitly synthetic private
fixture, not a Node compatibility shim.

The retained proof directory is
`/home/node/BuildCache/o3-server-speech-worker-4314`. Final both-mode results and
strict-check hashes are recorded there; repository documentation does not imply
real engine or maximum aggregate production-memory acceptance from these fixtures.
