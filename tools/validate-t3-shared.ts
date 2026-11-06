// Decodes O3 records or T3 projection rows (and, with AFTER_SEQUENCE, the events after it) with
// T3's own contract schemas, and groups what T3 would reject. Run it on a copy after a T3 nightly
// bump, before trusting the shared base (docs/SHARED-STATE.md).
//
//   T3_CONTRACTS=<t3code checkout>/packages/contracts node tools/validate-t3-shared.ts <db> [limit]
//
// The contracts package needs `effect` at the version T3 pins, resolvable from its directory.
import { DatabaseSync } from "node:sqlite";
import { createRequire } from "node:module";
const contracts = process.env.T3_CONTRACTS;
if (!contracts) throw new Error("T3_CONTRACTS must name T3's packages/contracts directory");
const C: any = await import(`${contracts}/src/index.ts`);
const Schema: any = await import(createRequire(`${contracts}/package.json`).resolve("effect/Schema"));

const schemas: Record<string, { schema: any; table: string; id: string }> = {
  thread: { schema: C.OrchestrationV2AppThreadJson, table: "orchestration_v2_projection_threads", id: "thread_id" },
  runs: { schema: C.OrchestrationV2RunJson, table: "orchestration_v2_projection_runs", id: "run_id" },
  attempts: { schema: C.OrchestrationV2RunAttemptJson, table: "orchestration_v2_projection_run_attempts", id: "attempt_id" },
  nodes: { schema: C.OrchestrationV2ExecutionNodeJson, table: "orchestration_v2_projection_nodes", id: "node_id" },
  subagents: { schema: C.OrchestrationV2SubagentJson, table: "orchestration_v2_projection_subagents", id: "subagent_id" },
  providerSessions: { schema: C.OrchestrationV2ProviderSessionJson, table: "orchestration_v2_projection_provider_sessions", id: "provider_session_id" },
  providerThreads: { schema: C.OrchestrationV2ProviderThreadJson, table: "orchestration_v2_projection_provider_threads", id: "provider_thread_id" },
  providerTurns: { schema: C.OrchestrationV2ProviderTurnJson, table: "orchestration_v2_projection_provider_turns", id: "provider_turn_id" },
  runtimeRequests: { schema: C.OrchestrationV2RuntimeRequestJson, table: "orchestration_v2_projection_runtime_requests", id: "runtime_request_id" },
  messages: { schema: C.OrchestrationV2ConversationMessageJson, table: "orchestration_v2_projection_messages", id: "message_id" },
  plans: { schema: C.OrchestrationV2PlanArtifact, table: "orchestration_v2_projection_plans", id: "plan_id" },
  turnItems: { schema: C.OrchestrationV2TurnItemJson, table: "orchestration_v2_projection_turn_items", id: "turn_item_id" },
  checkpointScopes: { schema: C.OrchestrationV2CheckpointScopeJson, table: "orchestration_v2_projection_checkpoint_scopes", id: "scope_id" },
  checkpoints: { schema: C.OrchestrationV2CheckpointJson, table: "orchestration_v2_projection_checkpoints", id: "checkpoint_id" },
  contextHandoffs: { schema: C.OrchestrationV2ContextHandoffJson, table: "orchestration_v2_projection_context_handoffs", id: "context_handoff_id" },
  contextTransfers: { schema: C.OrchestrationV2ContextTransferJson, table: "orchestration_v2_projection_context_transfers", id: "context_transfer_id" },
};

const [path, limitArg] = process.argv.slice(2);
const db = new DatabaseSync(path, { readOnly: true });
const isO3 = db.prepare("SELECT count(*) AS n FROM sqlite_master WHERE name='records'").get().n === 1;
const limit = Number(limitArg ?? 0);
const groups = new Map<string, { count: number; example: string }>();
for (const [kind, { schema, table, id }] of Object.entries(schemas)) {
  const decode = Schema.decodeUnknownExit(Schema.fromJsonString(schema));
  const sql = isO3
    ? `SELECT id, body FROM records WHERE kind='${kind}'${limit ? ` LIMIT ${limit}` : ""}`
    : `SELECT ${id} AS id, payload_json AS body FROM ${table}${limit ? ` LIMIT ${limit}` : ""}`;
  let total = 0;
  let bad = 0;
  for (const row of db.prepare(sql).iterate() as Iterable<{ id: string; body: string }>) {
    total++;
    const exit = decode(row.body);
    if (exit._tag === "Success") continue;
    bad++;
    const text = String(exit.cause).replace(/\s+/g, " ");
    const key = `${kind}: ${text.slice(0, 220)}`;
    const group = groups.get(key) ?? { count: 0, example: row.id };
    group.count++;
    groups.set(key, group);
  }
  console.log(`${kind}: ${total} rows, ${bad} rejected`);
}
// Events O3 wrote (after AFTER_SEQUENCE), decoded the way T3's event store reads them back.
const after = Number(process.env.AFTER_SEQUENCE ?? -1);
if (!isO3 && after >= 0) {
  const decodeThread = Schema.decodeUnknownExit(C.OrchestrationV2DomainEventJson);
  const decodeProject = Schema.decodeUnknownExit(C.ApplicationProjectEvent);
  let total = 0;
  let bad = 0;
  const rows = db
    .prepare("SELECT * FROM orchestration_events WHERE sequence>? ORDER BY sequence")
    .iterate(after) as Iterable<Record<string, any>>;
  for (const row of rows) {
    total++;
    const metadata = JSON.parse(row.metadata_json);
    const payload = JSON.parse(row.payload_json);
    const exit =
      row.aggregate_kind === "project"
        ? decodeProject({ sequence: row.sequence, eventId: row.event_id, type: row.event_type, aggregateKind: "project", aggregateId: row.stream_id, occurredAt: row.occurred_at, commandId: row.command_id, causationEventId: row.causation_event_id, correlationId: row.correlation_id, payload, metadata })
        : decodeThread({ id: row.event_id, threadId: row.stream_id, type: row.event_type, occurredAt: row.occurred_at, payload, ...metadata });
    if (exit._tag === "Success") continue;
    bad++;
    const key = `event ${row.event_type}: ${String(exit.cause).replace(/\s+/g, " ").slice(0, 220)}`;
    const group = groups.get(key) ?? { count: 0, example: String(row.sequence) };
    group.count++;
    groups.set(key, group);
  }
  console.log(`events after ${after}: ${total} rows, ${bad} rejected`);
}
for (const [key, { count, example }] of [...groups].sort((a, b) => b[1].count - a[1].count)) {
  console.log(`${count}\t${key}\t(e.g. ${example})`);
}
