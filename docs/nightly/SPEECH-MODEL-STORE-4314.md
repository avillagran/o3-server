# Source-backed speech ModelStore — nightly 4314

This checkpoint implements catalog lookup and the ModelStore consumer kernel with
an owned local filesystem backend and real HTTP transport. It does not install a
State/global service, expose an HTTP route/capability, implement the higher
SpeechService selection/unload coordinator, or claim complete speech parity.

## Native source and observations

The complete `SpeechModelStore.ts` at 83a3674ffd84ffb33de768210c806cb1ac53fd80 runs
unchanged inside private Podman using Node 26.13.0 and one canonical Effect 4.1.2
instance. Only its Config service token is adapted. FS/HTTP services are virtual
maps/streams; native startup/exit removal calls never operate on a real tree.

`tools/generate-speech-model-store-fixtures` requires a readonly admitted Node
snapshot, an exact source checkout and existing pinned dependencies. It does not
copy a large cache. The deterministic 50-case corpus covers readiness, verification
cache/identity, marker-write failures, hash changes during verification, retry and
resume, short transfers, status/range/size/SHA errors and numeric header edges.
The generated fixture was reproduced byte-for-byte.

Five range cases append actual LF, CR, CRLF, U+2028 and U+2029 codepoints. The whole
native producer rejects all five; its non-multiline end anchor was not relaxed.
Twelve 1024-bit radix cases show why accumulating `f64` is insufficient: values
just below overflow remain finite and fail the size check; the tie and values
above overflow become Infinity and are ignored by the native finite-length check.
Rust rounds the exact integer once to 53 significant bits with ties-to-even.

The complete original catalog order/default and raw metadata are preserved;
accuracy/speed are filled with zero as in `model.ts`. Requests should choose a
catalog ID; trusted application manifests are a separate internal admission API,
not arbitrary client model URLs or filesystem paths.

## Consumer behavior

A cheap `is_ready` uses regular-file type, byte size and the in-memory corrupt
identity cache, never hashes bytes. `installed_path` checks the source marker
identity (catalog SHA, size, millisecond mtime, device and inode), hashes on first
use and rechecks identity before trusting it. Marker-write failures are ignored
as in the native producer. Worker admission independently verifies/copies model
bytes before actual inference.

Downloads use exact revision URLs, at most three attempts, Range resumption for
transport/disk failures and full restart when the peer answers 200. A resumed 206
must match exact start/end/total. Domain status/range/size errors do not retry.
Progress precedes each sink write; verification begins after transfer attempts.
Only complete expected-size/SHA bytes publish atomically. Cancellation fences
publication, retains the owned transfer and leaves the previous final file intact.
There is no invented backend download deadline; caller/service policies remain
separate from transport and operation cancellation.

## Filesystem safety boundary

The local backend requires an already owned private state directory, opens path
components without following links, and holds directory identities. Transfers
use unique, exclusive staging entries and ownership manifests. Failure,
cancellation and completed staging are retained under `.o3-staging`, with no tree
purge. Unknown legacy native partials are deliberately preserved: a matching UUID
filename is not proof of ownership. Their public model behavior still matches
native observations; the differing native removal plan is explicitly recorded in
the corpus.

Markers never truncate an existing inode. A fresh exclusive private file is
synced and published by directory-FD rename after checking destination identity
and regularity. A hardlinked marker therefore cannot modify another file's bytes.
The physical fixture's other link is inside the same private temporary folder;
it never targets host data or a real HOME/root path. Transfer paths are stored as
owned PathBuf values, with no leaked allocation per lookup.

Explicit removal uses `cleanup::RemovalPlan::prepare_files`: only the two named
non-directory entries, with the containing directory as an identity guard rather
than a removal operation. No children are enumerated and no directory is removed.
The default executor is the real existing removal executor. Acceptance injects
an explicit recording executor and asserts retained fixture/sibling bytes; it
does not claim a physical wipe or substitute a silent no-op in production.

## Focused acceptance

The minimal-feature incremental target compiled on the existing worktree target.
Only its 21,871,616-byte test executable was copied to `/var/tmp`, under the
compile lock. No Node executable, CLI, unrelated DLLs or dependency cache was
copied for these tests. Five private tests pass, including all 50 source traces,
marker hardlink isolation, real files with transfer resume/failure retention,
recorded exact-file removal, cancellation and an actual truncated loopback HTTP
response followed by Range 20 resumption. Targeted strict Clippy also passes.

Evidence is retained under `/var/tmp/o3-model-store-next/`. This focused receipt
is not a full/default-feature matrix or activation of selection, model unload,
settings writes, postprocessing, live voice capture or public speech endpoints.
