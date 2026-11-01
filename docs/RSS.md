# RSS measurement

Use disk-backed, isolated state; never benchmark an owner's live profile in place:

```
TMPDIR=/var/tmp/o3-build/tmp CARGO_TARGET_DIR=/var/tmp/o3-build/main nice cargo build --release --locked -j 4
O3_DESKTOP_BIN=/var/tmp/o3-build/e2e/desktop-target/debug/omarchy-t3-client nice tools/bench-rss --profile /path/to/imported/profile
```

For public relay plus a native turn, also build `--example bridge_probe` and add
`--bridge-public`; see the public relay workload below.

`--profile` expects `userdata/o3.sqlite`. It opens the source read-only, backs it
up consistently to a private run directory below `/var/tmp/o3-build/real-opt`,
starts the release server with isolated HOME/XDG/provider paths and a private
`t3` symlink, waits for HTTP and embedded bridge readiness, runs the real desktop
`snapshot`, then opens the thread with the most visible items in desktop `watch`.
The run waits for messages and a connected live stream, samples RSS/HWM, terminates the desktop,
and samples again after three seconds. It removes the work copy on exit.
Neither client output nor server errors containing private data are persisted.
Only aggregate counts, memory, timing, binary digest and explicit allocator
settings enter `docs/rss-measurement.json`. `--output` changes that destination.
The decimal budgets (35 MB idle, 50 MB HWM, 500 ms snapshot) are asserted.
No inference is included. Linux VmHWM covers the entire process lifetime,
including first-open index migration. RSS measurements vary with ASLR, bridge
initialization and kernel page residency; a single run is not a universal bound.

For aggregate heap evidence, build with `--features memory-diagnostics`, then add
`--diagnostics`. SIGUSR1 prints only live/peak Rust requested allocation bytes
and SQLite's global live/peak allocation bytes. The counting allocator is
excluded from normal builds. The counters separate Rust requests from SQLite allocations, even when both
use libc (the current default). Earlier measurements below used mimalloc.
These totals do not include allocator metadata, executable pages or stacks.

## Findings

On the imported 23-project, 374-thread history (10,605 messages; 67,250 turn
items; largest thread 8,264 visible items), the instrumented original code took
3,885 ms for the real desktop snapshot. After disconnect, Rust live bytes
returned to 3,530,724 and SQLite to 71,872, while RSS remained 53,352 KiB.
Rust peak requested bytes were 5,279,701; SQLite peak was 8,438,656. This is
allocator retention, not a retained full transcript in Rust.

SQLite EXPLAIN QUERY PLAN showed that the nullable thread predicate selected
the primary-key index on `kind` and a temporary ORDER BY B-tree. Repeating that
query for each shell row sorted historical message bodies. An explicit thread
predicate uses `records_thread(thread_id,kind)` in rowid order. A partial user
message index also eliminates historical body scans for latest-user timestamps.
Shell output already streams with bounded backpressure, one JSON record at a
time; replacing all JSON Values or adding denormalized summaries was unnecessary.
Snapshot readers are already short-lived and have a 512 KiB cache; a new
per-RPC connection pool was unnecessary. The primary cache is now also 512 KiB.

FTS existed in this profile and was **not rebuilt** at startup. Its first-time
migration previously collected all messages, and now streams them. Recovery
runs were not present in this import; startup recovery is not blamed for its
snapshot latency. Startup releases transient cache pages after recovery.

Controlled ablation with only `MIMALLOC_PURGE_DELAY=0` reduced original-code
HWM from 53,352 to 44,384 KiB, but disconnected RSS stayed at 44,044 KiB and
snapshot still took 3,995 ms. Immediate purge is now the default (an explicit
environment override is honored). Completed/cancelled snapshot workers and
closed HTTP/WebSocket connections also collect retired mimalloc pages and
trim freed libc arenas, on the worker that handled the operation.

Original owner measurements at fb87550: idle 42.3 MB, after snapshot 47.3 MB,
largest-thread HWM 58.3 MB, snapshot 3,850 ms; disconnect was not recorded.
The controlled instrumented reproduction above has its own baseline because
allocator scheduling and isolated profile setup change absolute RSS.

## Final production release

Default bridge-enabled release, no counting allocator or allocator environment
overrides; real desktop snapshot and connected live thread stream:

| Metric | Owner baseline fb87550 | Controlled baseline (instrumented) | Final production |
|---|---:|---:|---:|
| Idle RSS, MB | 42.3 | 37.1 | 31.5 |
| After snapshot RSS, MB | 47.3 | 47.3 | 30.5 |
| Largest-thread lifetime HWM, MB | 58.3 | 54.6 | 44.8 |
| Three seconds after disconnect RSS, MB | unmeasured | 54.6 | 31.0 |
| Desktop snapshot, ms | 3,850 | 3,885 | 191 |

MB means 1,000,000 bytes. Raw KiB, measurement time and SHA-256 for the final
release are in `rss-measurement.json`. Final HWM includes first-time creation of
the partial user-message index on the imported database. The benchmark asserts
that disconnected RSS is within 3 MB of idle, in addition to the other budgets.
This run does not exercise provider inference, Android/QML, arbitrary histories,
or long-lived many-client concurrency. The schema index is maintained by SQLite
on inserts/updates/deletes; no shell field or protocol shape changes.

## Client-field follow-up (2026-11-01)

The first repository-identity replacement implementation materialized the full
shell tree. The real-profile regression probe caught a **72.36 MB HWM** despite a
passing synthetic probe. This result was rejected. Replacement events now store
only resolved roots; subscribers open a consistent disk snapshot through the same
bounded writer as initial snapshots. They advance their cursor to that snapshot's
boundary, so a replacement cannot replay older updates afterward. Plan/subagent
shell summaries also query only the required SQL fields.

Final isolated copied-profile probe: **46.24 MB HWM**, idle **32.02 MB**, desktop
snapshot **215.27 ms**, RSS three seconds after disconnect **34.34 MB** (within
3 MB of idle). The largest thread has 8,264 visible items. No owner record contents
are committed. Raw aggregate evidence and executable hash: `rss-measurement.json`.
Synthetic aggregate evidence for the same executable: `rss-synthetic-measurement.json`.
Both probes assert the decimal 50 MB HWM budget; inference/subprocess memory is
excluded from these two probes. Short native turns are validated separately.

## Public relay workload

Build the release `bridge_probe` example as well as the server, then use
`tools/bench-rss --profile /path/to/private-copy --bridge-public`.
This opts into a short real Claude Haiku turn using a private copy of local
Claude credentials. It enables the host's public relay client with `0.0.0.0:0`,
adds a separate Rust QUIC peer, authenticates through bridge pairing, and holds
shell and bounded largest-thread subscriptions open during 100 unary RPCs and
the turn. No gateway or emulator is involved. Public relay selection does not
prove relay-only transport: the local QUIC peer can select a direct path.

The probe still runs desktop snapshot/watch first. Public idle settles for 35
seconds to include relay discovery. Lifetime HWM includes startup, the native
turn and shutdown of clients; the maximum of all HWM samples is asserted.
Disconnect sampling stays at three seconds by default; `--settle-seconds`
records a different interval explicitly. The 50 MB HWM, 35 MB idle, 3 MB return
and 500 ms desktop snapshot budgets are preserved. Provider and client process
RSS are excluded. `--output` selects an aggregate-only artifact. No records,
identifiers, credentials, provider output or bridge addresses are persisted.

The lifecycle bug was in `serve_io`: every HTTP/QUIC connection spawned a
repository identity watcher surviving until `State` shutdown. The listener
also spawned one. Each watcher forced an initial replacement and held project
JSON while resolving Git identities. A polling native turn multiplied retained
futures and periodic 30-second SQL/Git work. There is now one atomically started
watcher per `State`, shared by both entry points, with the outer JSON snapshot
dropped before awaiting resolution. The 24-connection regression test publishes
24 replacements with the old implementation and exactly one with the fix.

Aggregate heap evidence distinguishes the remaining suspects:

| Suspect | Evidence / action |
|---|---|
| QUIC receive/GRO buffer | Largest persistent request: 3,014,656 bytes, matching 1472 × 64 GRO segments × 32 UDP batch slots in noq 1.4.0. Kept for normal transport operation. |
| TLS / relay actor | Included in Rust live totals. Relay and certificate verification remain enabled. No per-task TLS attribution is claimed from aggregate counters. |
| Task futures / JSON | Singleton watcher removes repeated project trees and forced replacement snapshots. Large-block live bytes return to their startup level after subscribers leave. |
| SQLite caches | Reader/writer caches remain 512 KiB. The last WebSocket releases their transient pages with `sqlite3_db_release_memory`, including cancellation/error paths. Active subscribers keep caches. |
| Blocking workers / stacks | Two workers and at most two blockers with 512 KiB stacks remain. Blockers expire after one idle second instead of ten, collecting before thread exit. Async futures do not have separate OS stacks. |
| Allocator residency | Disabling mimalloc THP lowered HWM substantially but still missed the 3 MB return budget despite near-idle requested heap. A controlled switch to `System` passed all four budgets. |

The production allocator is now `std::alloc::System`; Rust and bundled SQLite
share libc arenas, so `malloc_trim(0)` reclaims both. GNU/Linux defaults to two
arenas and a fixed 128 KiB mmap threshold: the adaptive threshold otherwise
put later transient buffers into arenas and made the return-to-idle result
vary. Corresponding `MALLOC_ARENA_MAX` / `MALLOC_MMAP_THRESHOLD_` settings
are honored. Mimalloc and its dependency
are removed. Idle runtime workers trim at most once a second per worker; the
main `block_on` thread is covered separately because worker callbacks do not
run there. Linux requests `PR_SET_THP_DISABLE` to avoid automatic huge-page
promotion; this memory policy is inherited by spawned processes. The existing
snapshot completion/cancellation cleanup also remains. Other platforms use
their system allocator; the numerical memory guarantee here is a Linux sample.

`--diagnostics` requires `memory-diagnostics`. It records labeled live/peak Rust
requested bytes, largest request, live/peak requests >=128 KiB, and independent
SQLite live/peak bytes. Linux samples also record anonymous/file RSS, huge-page
residency and thread count. Counters exclude allocator metadata and stacks;
subsystem inferences above are distinguished from exact call-site profiling.

The live pilot is untouched. This isolated local QUIC reproduction does not
certify Android UI, WAN/gateway operation, relay-only transport, long-lived
reconnects or arbitrary concurrent workloads.

Isolated production-release comparison (decimal MB), identical public-mode workload and
three-second disconnect interval; no allocator environment overrides:

| Metric | Before | After |
|---|---:|---:|
| Idle RSS, MB | 45.52 | 15.92 |
| Lifetime HWM, MB | 59.42 | 20.92 |
| Disconnected RSS, MB | 54.37 | 18.19 |
| Desktop snapshot, ms | 270.49 | 264.46 |

All four budgets pass in both final production and instrumented runs. The
instrumented Rust requested heap returns from 3,790,645 to 3,939,604 bytes;
SQLite falls from 752,192 to 245,872 bytes. Large-block live bytes return to
3,153,936. Full aggregate samples, binary digests and allocator ablations are
in [rss-public-measurement.json](rss-public-measurement.json). Historical
mimalloc release tables above describe earlier executables.

Validation: 147 tests pass with all features; 145 without defaults; five
opt-in tests ignored in each suite. Strict all-target/all-feature Clippy,
formatting and compatibility generation pass. Clippy also passes without
default features. The same production executable passes the synthetic
1,000-message / 3-subscriber / 1,000-RPC workload at 15.99 MB HWM.
