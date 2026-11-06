# Headless nightly parity

The active target is T3 Code headless nightly **4262**, commit
`c55db3e9c72fdf584f4b54192fb39beb88667b59`, published on 2026-11-06.
[Upstream release](https://github.com/pingdotgg/t3code/releases/tag/v0.1.8-nightly.20261106.4262).
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
methods: `orchestration.getThreadTitles` and `sourceControl.star`.

The reference's shared database has migration head 80. This candidate accepts exactly
80 and legacy 73 after isolated native-runtime handover, reducer/export/import and
contract validation; see [the acceptance](nightly/SHARED-SCHEMA-80.md). Live services
have not been upgraded by this work. Intermediate and future heads remain refused.
This establishes shared history, not synchronization of task definitions or complete
platform parity. Preserving a historical status of `done` does not bypass acceptance.

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

Validation details belong in `CONFORMANCE.md` and the per-block intake files. A green
suite and an RPC count alone do not establish complete headless behavior.

## Scheduled conversation modes

The candidate advertises `scheduledTaskThreadModes` and implements `new_thread`,
`continue` and `fresh_session`, including first-run binding, stale-form saves,
project changes, queued bound sends, task provenance and guarded completion.
[Evidence and remaining limitations](nightly/SCHEDULED-TASK-MODES.md) distinguish
native runtime checks from source-function and peer regressions. Task definitions
still live in O3's private store; bidirectional task/webhook storage and the rest
of the service lifecycle remain open. This block is not certification of every
scheduled-task method in the inventory.
