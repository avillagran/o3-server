# RSS measurement

Use disk-backed, isolated state; never benchmark an owner's live profile in place:

```
TMPDIR=/var/tmp/o3-build/tmp CARGO_TARGET_DIR=/var/tmp/o3-build/main nice cargo build --release --locked -j 4
O3_DESKTOP_BIN=/var/tmp/o3-build/e2e/desktop-target/debug/omarchy-t3-client nice tools/bench-rss --profile /path/to/imported/profile
```

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
excluded from normal builds. SQLite uses libc separately from Rust's mimalloc.
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
