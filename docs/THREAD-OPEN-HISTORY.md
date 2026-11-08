# Native thread open history

The complete visit/open source contract is byte-identical in nightly 4380
(`ade473018d32003658b7d38693e083ee2e314bd8`).

Nightly 4367 extends `thread.visit` with optional `opened: boolean` and
`searchQuery: string` (at most 200 UTF-16 units). All visits use the original
Effect DateTime implementation in the admitted JavaScript runtime. Watermarks only move forward and visits never
advance the thread's activity `updatedAt`.

An open runs the complete original `recordThreadOpen` implementation from
`runtime/search/NativeSearchRuntime.cjs`. The source receipt pins upstream
`6417151c093e54dbba0e6c824c69d03983793b9a`, original inputs, and generated
bundle SHA-256 `08a9d39d1fea882ac713d1753bac145b6bbb38b4d2374b0e577136eba7799d3d`.
The original folding, surrogate-safe picked query, 10-minute deduplication,
14-day decay, three picked queries, and 21-day pick window remain together.
The MIT license accompanies the bundle. Rust performs schema admission and
ownership coordination; it does not approximate the ranking/history algorithm.

The server package places the exact admitted Node 26.13.0 executable at
`runtime/node-26.13.0` adjacent to `o3-server`. An application owner can instead
supply `--native-search-node /absolute/path/to/node`. The existing sealed Node
resource factory admits SHA-256
`0adde40d1b7dfa57c8bc83cbc319758c3ec6ebba21363bb2fa752ea6b267ad1b`
and reuses that exact existing private immutable capsule. The install path
passes the admitted source itself as the existing factory destination; a missing
source is rejected before that call. No executable is copied into the profile.
The package installer creates the immutable capsule before startup. There is no PATH discovery, model,
voice enablement, download, per-visit executable copy, or request-selected code.
Without that resource, visits fail explicitly. The original native DateTime
semantics are also required for watermark-only visits. Packaging must include the runtime rather than silently
falling back to an approximate algorithm.

A visit gate serializes all native visit computations. The server reads the actual previous
history, releases SQLite, and invokes an owned bounded pure worker. It then
rechecks the previous history and thread creation identity in the normal receipt
transaction. A conflict fails explicitly; it never blindly retries or overwrites
another open. The same server timestamp feeds the original algorithm and the
committed event's `occurredAt`. Authorization and policy are checked before
computation and again in the commit transaction. Receipt replay does not repeat
an open. Full thread records and shell projection clones persist `opens`,
including `picks`; shared T3 export preserves the complete thread payload.

Private server component validation passed for implementation
`23113f07c85525ee3a56e37ab4d06cf2110dbb2a`. Publication applies only
repository-default rustfmt layout/import ordering to its eight changed Rust
files and adds this validation record; those cosmetic successors are not
represented as a new independent runtime observation:

- Strict minimal and default all-target checks and a normal default library
  build passed using the actual Cargo dependency graph.
- A RAM-linked genuine RPC/SQLite fixture passed original V8 open-history,
  server-clock event timing, receipt replay, deduplication, picked-query folding,
  and shell persistence checks through the existing sealed Node factory.
- All eight synthetic Node-admission regressions passed, including reuse of the
  already-created successful version-probe capsule.
- The earlier watermark-only case passed canonical offset handling, forward-only
  read state, and unchanged activity time. It was not repeated after the
  independent open-worker correction.

The first actual opened-history run failed honestly because the writer remained
open while Node's synchronous request decoder awaited EOF. Tokio pipe shutdown
does not close that descriptor. The correction drops the request writer before
awaiting its response. The same two-second operation/close bounds and required
normal owned cleanup remain unchanged. The failed receipt remains separate;
no expected failure is counted as a passing feature.

The accepted runtime retained one actual container identity to terminal and
verified all 24 reserved executable aliases, 1,070 source inputs and 1,335
actual normal-graph artifacts unchanged. Source and target were read-only;
only the test fixture was linked into private RAM. No Cargo/runtime copying,
profile installation or production executable build occurred during runtime.

This establishes the tested server component only. Client visit integration,
normal release packaging execution, public CI and full nightly parity remain
separate acceptance requirements. The accompanying SDK has its own 129-test,
C ABI and Kotlin/JNI validation and public green CI; those do not substitute
for server/client runtime proof.

Local evidence hashes:

| Evidence | SHA-256 |
| --- | --- |
| Canonical formatter token/literal equivalence | `ac630dc91b871d417a695d193c34f0aad626d6c0945b2ff0e7ad71e8bec4d988` |
| Build3 actual strict/default/normal result | `29e20690bfe4a233c5f3a3051e9dd00b459736a3c1bf62f452bc897e23283f05` |
| Preserved first opened-history failure | `652ab0243c372b40004db4e766d83705c5c9695773be8e18528d5acf8d642fa0` |
| Corrected actual history and admission runtime | `684c2744b6bebe3fbcb0985e0f77d4315babd4bb7fe0aba7a58b6524a11e690d` |

These receipts are retained under `/var/tmp/o3-build/thread-open-4380-server-*`;
public source checkpoints alone do not establish runtime acceptance.

## Building a normal user package

After the server build and release review, the packaging step is:

```sh
tools/package-native-search --server /absolute/admitted/o3-server \
  --node /absolute/admitted/node --output /absolute/new/package
```

The existing artifacts are read without following links, the exact Node digest
is checked, and an exclusive user-owned package receives `o3-server` plus
`runtime/node-26.13.0`. The immutable executable and private directory modes meet
the same existing resource factory's reuse admission. Starting that packaged
server requires no search-specific flag or speech bundle. The normal service
installer uses the packaged server's executable path, so its adjacent runtime
is discovered identically. A package receipt records both hashes. Nothing is
downloaded, launched, overwritten, or recursively removed. Partial output is
retained on failure. This release packaging action must not be used to create
additional executable/runtime copies in test acceptance workflows; tests mount
and reuse an already admitted capsule.

## CI resource reuse

The genuine disposable CI job exports the immutable capsule already created by
its successful Node version probe. It checks the existing capsule's ownership,
mode, link count, byte length, and digest before exporting it. The original
setup-node executable remains the acquisition receipt; the runtime path points
to the same retained probe capsule. No second snapshot or profile copy is needed.

Stored open history follows the original Effect Struct decoder: undeclared
properties are stripped at both object levels, invalid history becomes null,
and valid empty queries remain valid. Forward watermarks compare epoch
milliseconds and persist the canonical UTC millisecond timestamp, matching the
original DateTime event encoder.

## First public CI observation

Run `42520902690` on `23fd209` passed Node capsule admission, formatting and
filesystem policy in both feature modes. The library suites passed 477 minimal and 479 default cases with zero
failures and 17 ignored per mode. Both jobs then reached an existing dynamic
command-loop fixture that omitted the original required `visitedAt` field for `thread.visit`
and failed with `Invalid visit time`. The correction supplies that timestamp
only for the visit command, retains every existing assertion, and additionally
requires the exact persisted timestamp. Production validation and timestamps
are unchanged. The failed workflow is retained and is not rerun unchanged.


## Native DateTime fidelity validation

The initial green CI implementation accepted only RFC 3339 visit strings. The
4380 command actually accepts strings through Effect 4.1.2 `DateTime.make`.
The private whole-package oracle found native date-only, zone-free, normalized
calendar overflow, TimeClip-boundary, long padded-string and trusted-timezone
cases outside that subset. Its original receipts are retained.

The follow-up source embeds the exact 66-module original Effect graph and its
MIT notice. A sealed in-memory loader checks the compiled packet digest and
module hashes; a fresh admitted Node worker owns one graph instance. Neither
source text, expected digest nor timezone comes from an RPC request. The normal
runtime installation captures the application's timezone once. The request
budget remains the transport's 8 MiB, separate from the immutable source envelope.
All visits use this capability, including visits without `opened`. Node remains
an ordinary packaged runtime dependency; speech/model installation is unrelated.
The package command includes both original license notices beside the executable.

Projection lookup/deletion refusal precedes date evaluation. An invalid previous
projection date maps to a projection error; an invalid requested date maps to a
dispatch error before clock/history reads. Valid stale/equal timestamps still
read the real native server clock, update optional opens, and emit an event.
Only a strictly newer watermark replaces `lastVisitedAt`. The canonical native
string persists directly, including dates outside Rust timestamp ranges.
Computation happens outside SQLite; commit rechecks generation, raw previous
opens and watermark under the existing command transaction before applying it.

The generated graph/adapter matched all 74 admitted strings in each of three
trusted zones against the prior complete-package oracle, with 138 original
history calls and tamper/module-escape refusals. The follow-up then passed strict minimal/default all-target checks and a
normal DEFAULT library build. Its complete actual Cargo graph (1,335 files and
four native output directories) supplied a single RAM-only genuine server
fixture. Five actual RPC cases passed: forward-only legacy visits, complete
original open history/server-event clock/replay/picks, and native date-domain
transactions under UTC, America/Santiago and Asia/Kolkata. The timezone-sensitive
`99-01-01` result differed as the original engine requires. These cases also
validated date-only/zone-free strings, normalized calendar overflow, the expanded
TimeClip boundary, non-string and invalid-input refusal without mutation, an
accepted 140,010-character string, stale-watermark opens, and deleted-thread
refusal before invalid-date evaluation. Each real worker retained the original
two-second computation and confirmed graceful retirement bounds.

The same physical container retired with exit zero; all source, complete graph,
native outputs and 24 protected executable aliases remained unchanged. No
Cargo invocation, persistent executable link or runtime copy occurred in that
RPC gate. Its result SHA-256 is
`23acb94824039f95e544e5091882d98790cf741086d535bd87eab62b9869ef52`.
The compile result is
`db198f86851ecea7fa2eb7d6c64014b0faaaa94b6c689f913eb80e4843483352`.
This is server visit/date/history acceptance, not full nightly or client acceptance.
No new release, default merge or install is claimed.

The wire decoder admits strings, not numbers or other DateTime input types.
The Rust boundary checks this before entering the native helper. The complete
command retains its 8 MiB limit, with a separate 64 KiB allowance for bounded
stored projection/history fields inside the owned worker request. The source
packet is outside both budgets. The fresh worker has a 128 MiB V8 heap cap so
native graph loading and a full-size public string share an explicit envelope;
the existing two-second computation and retirement deadlines are unchanged.
