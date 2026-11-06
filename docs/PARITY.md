# Headless nightly parity

The active target is T3 Code headless nightly **4275**, commit
`6577c6b6185167c0e502fa01f9e8e8f2dee6ff25`, published on 2026-11-06.
[Upstream release](https://github.com/pingdotgg/t3code/releases/tag/v0.1.8-nightly.20261106.4275).
The implementation is not yet a complete 1:1 replacement.

The scope includes every headless RPC, command, HTTP/CLI surface, authentication
flow, stream lifecycle, provider driver, native continuation, shared-state transition
and observable error/cancellation behavior. Methods without a current client caller
remain in scope. Legacy aliases are tracked separately and do not count as current
nightly coverage. Pure web/mobile/desktop presentation is outside this headless target;
server behavior supporting those clients remains in scope.

## Reference and evidence

[`nightly/parity-4262.json`](nightly/parity-4262.json) inventories 211 discovered
public method names and records existing method statuses as historical evidence,
not certification against this nightly. The source adds two previously untracked
methods: `orchestration.getThreadTitles` and `sourceControl.star`. The title method
is now tracked and implemented; the star method remains untracked.

[`nightly/drift-4275.json`](nightly/drift-4275.json) records the next intake. Goal,
thread-read and RPC authorization source is unchanged from 4262. Device discovery,
Antigravity identification and CLI/service-launch changes require their own comparison;
existing 4262 runtime evidence does not certify those surfaces.

The reference's shared database has migration head 80. Live O3 profiles are accepted
through migration 73. Supporting migrations 74–80 requires isolated native-T3 migration,
reducer/export/import and schema validation before widening the live-profile gate.
The existing migration guard remains the protection against opening an incompatible
profile. Preserving a historical status of `done` does not bypass this acceptance.

For each implementation block:

1. Identify the exact source functions, schemas and lifecycle, including errors and races.
2. Run pure upstream functions when possible and retain reproducible golden cases.
3. Drive the real compatible client through the server and native protocol peers.
4. Exercise installed native dependencies for behavior a peer cannot establish.
5. Check restart, Stop, stale/duplicate events, ownership and resource release.
6. Record evidence and remaining gaps; only claim parity for the behavior actually checked.

## Current execution block

The native-goal block replaces Codex slash-command prompts with native goal APIs,
retains automatic turns within their owning app run, pauses uncontinued goals after
the upstream five-second grace, and stops late orphan turns. Control-only turns have
no native rollback reference. Claude goal acknowledgements and synthetic Stop-hook
feedback update the provider-thread goal; child output and failed turns cannot mark
a root goal complete. Golden fixtures execute the nightly's actual parser and goal
normalizers. This block does not close the broader idle-wake/child-continuation,
provider-driver or shared-migration backlog.

The thread-read block adds title batches, active family rows and exact-path worktree
membership. An isolated official nightly 4262 runtime agrees on selection, ordering,
Unicode trimming and 31 malformed-input wire defects. Families reuse the common shell
projection, whose complete field fidelity remains a separate task. The full headless
goal remains active after either block.

Validation details belong in `CONFORMANCE.md` and the per-block intake files. A green
suite and an RPC count alone do not establish complete headless behavior.
