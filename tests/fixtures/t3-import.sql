-- Appendix D: migrations 005 and 055, verbatim table DDL.
CREATE TABLE IF NOT EXISTS projection_projects (
  project_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  workspace_root TEXT NOT NULL,
  default_model TEXT,
  scripts_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);
CREATE TABLE orchestration_v2_projection_threads (
  thread_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  title TEXT NOT NULL,
  default_provider TEXT NOT NULL,
  runtime_mode TEXT NOT NULL,
  interaction_mode TEXT NOT NULL,
  active_provider_thread_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT,
  deleted_at TEXT,
  payload_json TEXT NOT NULL
);
CREATE TABLE orchestration_v2_projection_messages (
  message_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  role TEXT NOT NULL,
  streaming INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
CREATE TABLE orchestration_v2_projection_turn_items (
  turn_item_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  provider_thread_id TEXT,
  provider_turn_id TEXT,
  parent_item_id TEXT,
  ordinal INTEGER NOT NULL,
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);