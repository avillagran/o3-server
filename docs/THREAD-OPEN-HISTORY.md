# Native thread open history

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

Validation is pending: focused original-algorithm, actual transaction/replay,
readiness/resource ownership, SDK round-trip, and client visit tests must run in
private rootless containers. Source checkpoints are not runtime acceptance.

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
