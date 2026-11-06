# Nightly 4262 shared-history schema

Reference: `v0.1.8-nightly.20261106.4262`, commit
`c55db3e9c72fdf584f4b54192fb39beb88667b59`, Effect 4.1.2.

O3 accepts exactly migration 80 (`ScheduledTaskThreadMode`) and legacy migration
73 (`ActiveTurnItemIndexes`), with projection schema version 2. It refuses
intermediate or unknown heads and verifies the new columns before creating sync
metadata. T3 remains responsible for creating and migrating its tables.

## Migration review

| Migration | Shared exporter behavior |
| --- | --- |
| 74, attachment placeholders | Preserve T3's own FTS triggers; fixture regression confirms both plain and whitespace-prefixed attachment placeholders remain unindexed. |
| 75, subagent delivery state | Maintain `completion_delivery_state` with the upstream coalescing rule, alongside retained JSON delivery data. Compact updates cannot erase acknowledged delivery. |
| 76, project pin/archive | Read, export and import `pinned_at`/`archived_at`; retain timestamps in project events. Legacy 73 records a rejection for unsupported non-null flags and keeps local intent. |
| 77, boolean settlement repair | T3 runs its repair; O3 already normalizes imported boolean settlement values before exposing them. |
| 78, correlation index removal | No query depends on the removed index; export retains the correlation column. |
| 79, delegated recovery indexes | Preserve the native JSON expression indexes; exporter inserts use the same existing payload columns. |
| 80, scheduled-task thread mode | Require and preserve the native `thread_mode` column. Definitions and native webhook ledgers use an independent row-hash baseline; ingress/dispatch and legacy-work migration remain open. |

The first migration-80 startup revisits local projects once, recovering pin/archive
intent an older exporter could not persist. Successful or already-equal exports
clear previous rejection entries. Subsequent starts do not churn project events.
The schema-only fixture comes from the official runtime database and excludes
authentication, user records, runtime state and FTS shadow contents.

## Acceptance

`tools/test-shared-nightly` runs eight legacy regressions and six new regressions
inside a read-only, credential-free, network-disabled rootless container. The new
cases cover project/subagent reducers, missing-column refusal, intermediate-head
refusal, retained legacy intent, upgrade recovery and FTS placeholder exclusion.
Neither the new tests nor their runner recursively remove host files.

`tools/live-fresh-context --export-database <synthetic-o3.sqlite>
--reference-root <official-4262-package>` runs without credentials or network
access. It exports 81 records, sixteen messages and two fresh runs into migration
80, then exercises the actual T3 binary on the same disposable profile:

1. T3 reads O3's history and fresh markers and changes the thread title and project
   pin/archive flags through authenticated RPC.
2. O3 imports those changes, retains all sixteen messages, edits the title and
   removes the project flags, then exports again.
3. A second T3 startup reads the returned title and unchanged history.

The final database passes the exact reference contracts: 80 public records,
including its project, and 86 events, zero rejections. A separate native-reference
run executes the same eight real Claude/Codex turns as O3 and validates 110 public
records and 264 events. Production and acceptance Clippy checks pass with and
without the default feature set; formatting and diff checks pass.

The validator also checks project rows. A BLOB pin value is rejected with exit
status 1. The reference's `IsoDateTime` contract is a string schema; the validator
intentionally follows it rather than inventing stricter date validation.

This proves shared-history handover at migration 80. Scheduled-task conversation
execution, desktop composer integration, delegated lifecycle behavior and the
remaining platform inventory still require their own acceptance. No live profile
was upgraded and no service was deployed.
