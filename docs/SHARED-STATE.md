# Shared state with T3

**Decision (2026-11-05):** O3 Server and T3 must be fully interchangeable on the same base. Both run on
the same profile (`T3CODE_HOME`, default `~/.t3`), and both read and write the same thread history in
`userdata/statev2.sqlite`. O3 may add features, but they live beside T3's data and never replace it.

This replaces the 2026-10-31 rule "T3's database is never written" (`ARCHITECTURE.md` item 3).

## Where we are

| Layer | Shared today | Notes |
|---|---|---|
| Profile directory | Yes | O3 honors `T3CODE_HOME` and `--base-dir`, defaulting to `~/.t3`. |
| `server-runtime.json`, `environment-id`, `secrets/server-signing-key.bin` | Yes | Same files, so pairing and OmaBridge follow whichever server runs. |
| Settings, keybindings, connection catalog, attachments | Yes | Same files. |
| Thread history | **No** | O3 writes `userdata/o3.sqlite`. `o3-server import` copies T3's projections once, read-only, and local records win. Work done in O3 is invisible to T3. |

Only one server owns a profile at a time (`profile::refuse_live`, the ownership lock and port 3773).

## Target

O3 opens `statev2.sqlite` as its system of record:

1. **Schema gate.** O3 reads `effect_sql_migrations` on start. It writes only when the highest migration
   is one it implements (73 today, `ActiveTurnItemIndexes`). Otherwise it opens the base read-only and
   says so in `server.getConfig`; it never runs or invents migrations of its own on T3 tables.
2. **One log.** Every state change appends to `orchestration_events` with T3's envelope (global
   `sequence`, `event_id`, `aggregate_kind`, `stream_id`, `stream_version`, `event_type`,
   `application_event_version`, command, causation and correlation ids, actor, payload, metadata) and
   records `orchestration_v2_command_receipts`, in the same transaction as the projection rows.
3. **T3's reducers.** Projection rows (`orchestration_v2_projection_*`, `projection_projects`, message
   search, turn item positions, outbox and workflows) are written exactly as T3's reducers write them,
   so T3 can start on the base without a rebuild.
4. **O3 extras.** Data T3 has no table for (usage ledger, background tasks, Claude tasks, MCP claims)
   moves to `o3_*` tables in the same file, created by O3 only, ignored by T3.
5. **Exit from `o3.sqlite`.** A one-time export appends O3-only threads to the shared log, then
   `o3.sqlite` keeps only the extras until they move into `o3_*` tables.

## Phases

1. **Inventory.** List every event type and reducer up to migration 73 from T3's source, mapped to
   O3's `events.rs` kinds. Output: a coverage table like `CONFORMANCE.md`.
2. **Read path.** O3 serves reads straight from `statev2.sqlite` projections (no import), behind a flag.
   Differential tests against T3 on a copy of a real profile.
3. **Write path.** Event append, receipts and reducers, one aggregate at a time (projects, threads,
   messages, turn items, runs, provider sessions, checkpoints, plans, subagents, runtime requests,
   scheduled tasks). Each step is accepted only when T3, started on the result, shows the same
   threads and continues them.
4. **Switch-over test.** Alternate O3 and T3 on one profile copy: create in one, continue in the
   other, both directions, with Claude and Codex.
5. **Migration of existing O3 profiles.** Export O3-only history into the shared log; keep a consistent
   backup of both databases first.

Until phase 3 lands, the safe mode stays: O3 on `~/.t3` with `o3.sqlite`, and `o3-server import` to
pick up threads T3 created.
