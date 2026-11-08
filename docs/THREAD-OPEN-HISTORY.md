# Native thread open history

The complete visit/open source contract is byte-identical in nightly 4380
(`ade473018d32003658b7d38693e083ee2e314bd8`).

Nightly 4367 extends `thread.visit` with optional `opened: boolean` and
`searchQuery: string` (at most 200 UTF-16 units). A watermark-only visit does
not need a JavaScript runtime. Watermarks only move forward and visits never
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
Without that resource, an open fails explicitly; watermark-only legacy visits
remain available. Packaging must include the runtime rather than silently
falling back to an approximate algorithm.

A visit gate serializes open computations. The server reads the actual previous
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
