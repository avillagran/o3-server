# Security audit validation — 2026-11-02

All changes are local on `t3/implement-client-visible-gaps`. Compatibility statuses
and priorities are unchanged. Fixtures use loopback listeners, temporary profiles,
real `t3-protocol` Connections and production readers. No live inference or devices.

| Finding | Negative control | Verification and limits |
|---|---|---|
| 1. Unsigned webhook inputs | Real task upsert accepted `{{query.text}}`; regression failed. | Body-only prompt/filter/coalescing validation rejects unsigned input; permanent raw-body digest rejects captured delivery even with changed query and expired legacy expiry column. Headers never enter template data. No signed timestamp exists; authenticated sender freshness is unavailable. |
| 2. Replaced rollback workspace | Symlink to another workspace authorized restore; regression failed. | Physical workspace and git-common/worktree directory bindings, thread/project check, intent binding, refusal of missing/changed identity, pinned cwd descriptor. Real Connection refusal leaves target tracked/untracked files intact. Adversarial git-directory replacement races remain unproven. |
| 3. Durable rollback order | NORMAL-synchronous negative control failed (`1`, expected FULL `2`). Uninstrumented boundary test completed successfully where injected failure was expected. | Restored files/directories synced, FULL SQLite receipt commit, then unlink and journal-directory sync. One-shot errors after journal/files/receipt/unlink leave actual state that startup recovery replays; real projection confirms result. Four separately seeded crash-window states also pass. Tests reset process-local flags; no actual power-loss or filesystem fsync-error claim. |
| 4. Unauthenticated admission | Original code blocked behind held writer and returned no 404; regression failed. | Read-only hook lookup before body read; signature verification off Tokio workers before writer admission; 30/IP/s, 60/global/s. Forty sequential invalid loopback requests with held writer: 9.7–11 ms, ten limited. Concurrent saturation and remote-network throughput are unproven. |
| 5. Lost accepted job | Original delete-before-dispatch lost failed work (`0` queued vs expected `1`); regression failed. | Frozen durable claim, stable per-job command/message IDs, receipt deduplication and retained failures. Injected stops after durable claim and dispatch receipt retain the job; replay creates one run through the real projection reader. Claimed work is immediately eligible for startup scheduling. Logical exactly-once dispatch receipts do not prove exactly-once external provider side effects. |
| 6. Usage | Real aggregate returned savings `0`, expected `0.18`; narrow query rejected 100001 undated records; regression also covers 100001 dated turns in the preceding local day. | Both pass after cached-read pricing and exact local-day SQL date/accounting predicates (including skipped DST midnight). Private read-only connection avoids shared reader lock. 50000 matching turns: 1.07 s debug, 682-byte reply. Missing historical accounting remains unavailable. |
| 7. Recorder EOF | Real client saw `ResetWithoutClosingHandshake` after fixture recorder exit. | Recorder reaped/restarted on same socket, second SEMU frame through production reader; 250 ms backoff, three empty restarts. Fixture adb/ffmpeg only. |

Queue and replay storage have count caps plus 8 MiB logical byte budgets;
coalescing keys are limited to 4096 bytes. A real-client regression seeds 130
valid-sized 64 KiB prompts: disabling the byte guard accepted another delivery
(failed), restoring it returns 429 (passes in both configurations). Disk-page/WAL
overhead is additional.
Desktop Shake is still unsupported; its client control is outside this worktree.
Child-scoped cancellation, staged workspace progress/Cancel and tracked clone jobs
are outstanding. No status promotion or claim of feasibility closure is made.

## Final checks

- Full default suite: 251 passed, 19 opt-ins ignored; no-default-features:
  249 passed, 19 opt-ins ignored. Both use `--no-fail-fast`, `nice cargo -j 4`.
- Strict Clippy (`--all-targets -- -D warnings`) passes in both configurations;
  `cargo fmt --check` and `tools/gen-compat` pass. Every status/priority matches
  base `6a53a0d`; only row notes were revised.
- Eight isolated pinned 3991 differentials pass (173.25 seconds). The pinned
  binary accepts unsigned query templates; this implementation rejects them.
  Signed definition, invalid signature, accepted and duplicate outcomes match.
  Pinned telemetry returned no samples; usage and real-device parity remain
  unproven. These are binary observations, not conclusions from newer source.
- Release `tools/bench-rss`: VmHWM **18332 KiB / 18.77 MB**, below 50 MB;
  WebSocket availability **12.48 ms** on a preinitialized synthetic profile.
  Twenty threads, three subscribers, 1000 messages/unary calls; last-100 message
  append median **4.13 ms**, p95 **4.65 ms**, with FULL-synchronous commits.
  See `rss-security-audit.json`. Inference, large rate caches and provider-child
  memory are excluded from this measurement.

No runtime profiles, services or pilot data were inspected. Shared external
journals were left unchanged under this task's worktree-only/no-push constraint;
this document records the verified local handoff.
