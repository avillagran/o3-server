# Shared state with T3

**Decision (2026-11-05):** O3 Server and T3 must be fully interchangeable on the same base. Both run on
the same profile (`T3CODE_HOME`, default `~/.t3`) and the same thread history in
`userdata/statev2.sqlite`. O3 may add features, but they live beside T3's data and never replace it.

This replaces the 2026-10-31 rule "T3's database is never written" (`ARCHITECTURE.md` item 3).

## What is shared

| Layer | How |
|---|---|
| Profile directory | O3 honors `T3CODE_HOME` and `--base-dir`, defaulting to `~/.t3`. |
| `server-runtime.json`, `environment-id`, `secrets/server-signing-key.bin` | Same files, so pairing and OmaBridge follow whichever server runs. |
| Settings, keybindings, connection catalog, attachments | Same files. |
| Thread history | `src/t3_shared.rs`: O3 writes every change into `statev2.sqlite` and takes in what T3 wrote. |

Only one server owns a profile at a time (`profile::refuse_live`, the ownership lock and port 3773).
That is what lets the two directions below run without coordination.

## How O3 writes T3's base

T3 keeps an event log (`orchestration_events`) and projections its reducer derives from it. T3 does
not rebuild projections at start: it trusts them, after checking that the
`orchestration_v2_projection_metadata` high-water mark equals the newest agent event and that every
projected thread has a `thread.created` event. So O3 writes both, exactly as T3's event sink does:

1. **Schema gate.** O3 writes only when the newest row in `effect_sql_migrations` is migration 73,
   `ActiveTurnItemIndexes` (`t3_shared::MIGRATION_HEAD`), and the projection schema is version 2.
   Any other base is left untouched and O3 logs why; a newer T3 needs a matching O3.
2. **Marks.** Triggers on O3's `records` table mark every changed record of a shared kind in
   `t3_dirty`, with a version so a record rewritten during an export stays marked.
3. **Export.** A background thread drains the marks about every 250 ms, and once more when the server
   stops. For each record whose T3 row differs, it appends one T3 v2 event (`thread.created`,
   `run.updated`, `turn-item.updated`, `project.meta-updated` and so on, with T3's envelope and
   metadata) and upserts the projection row with T3's reducer SQL, including its merge rules
   (recorded run fields, subagent completion delivery, provider turn token usage, the active
   provider thread on the thread row, turn item positions and the thread's `updatedAt`). The
   high-water mark and O3's cursor move in the same transaction. A record T3's columns cannot hold
   is skipped and listed in `t3_rejects`.
4. **Catch-up.** At start, O3 reads T3's events after its cursor (`o3_sync.o3SeenSequence`) and takes
   in the current T3 row of every record they touched, emitting client events so open clients see
   the change.
5. **Adoption.** The first time O3 meets a T3 base, it takes in every T3 row it lacks (its own records
   win) and marks all of its records, so the first export writes what T3 lacks or holds differently.

A profile without `statev2.sqlite` keeps the marks until T3 creates its database; O3 does not create
T3's schema.

## What stays O3's own

O3-only data with no T3 table (usage ledger, background and Claude tasks, MCP claims, control outbox,
`visibleTurnItems`, receipts) stays in `userdata/o3.sqlite` beside `statev2.sqlite`.

## Verification

- `tests/t3_shared.rs` builds T3's schema at migration 73 (`tests/fixtures/t3-statev2-73.sql`) and
  covers both directions, idempotent re-export, adoption and the schema gate.
- `tests/shared_acceptance.rs` (opt-in) runs O3 on a copy of a real profile, then the real T3 runtime
  on the same copy, and reads every live thread through T3's own decoders.
