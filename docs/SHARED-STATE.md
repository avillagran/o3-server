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
   O3's first import carried no runs, so O3 may have numbered runs (and provider turns) from 1 again;
   where one collides with a T3 ordinal, O3's records of that thread move after T3's last, with the
   checkpoints that name them.
6. **Shape.** T3's decoders reject a whole thread, and its startup, on one value outside its schema,
   so the exporter maps O3-only values to T3's nearest (`disconnected` sessions are `stopped`, a bare
   cost is a USD amount, a null message context is absent, an unknown runtime mode asks for
   approval) and fills the fields O3's own checkpoints lack from their run and thread. Taking a T3
   row in merges it over O3's record, so O3-only fields stay.

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
- `tools/validate-t3-shared.ts` decodes every projection row, and the events O3 wrote, with T3's
  contract schemas. Run it on a copy after a T3 nightly bump, together with the acceptance test,
  before moving `MIGRATION_HEAD`.
