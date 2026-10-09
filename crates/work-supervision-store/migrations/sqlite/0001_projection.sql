-- Work Supervision v0 projection, migration 0001 (SQLite).
-- The journal is the authority; every row here is rebuilt from it.
-- Keep in step with ../postgres/0001_projection.sql and ../schema.v0.json.

CREATE TABLE projection_position (
  id INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
  seq INTEGER NOT NULL CHECK (seq >= 1),
  digest TEXT NOT NULL
) STRICT;

CREATE TABLE missions (
  id TEXT NOT NULL PRIMARY KEY,
  title TEXT NOT NULL,
  title_digest TEXT NOT NULL,
  repository TEXT NOT NULL,
  brief TEXT NOT NULL,
  brief_digest TEXT NOT NULL,
  max_duration_seconds INTEGER NOT NULL,
  max_output_bytes INTEGER NOT NULL,
  executor TEXT NOT NULL,
  state TEXT NOT NULL,
  revision INTEGER NOT NULL,
  base_commit TEXT,
  branch TEXT,
  worktree TEXT,
  current_run TEXT,
  result_commit TEXT,
  evidence_digest TEXT,
  summary TEXT,
  summary_digest TEXT,
  verdict TEXT,
  verdict_reason TEXT,
  verdict_reason_digest TEXT,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_seq INTEGER NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE mission_criteria (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  position INTEGER NOT NULL,
  text TEXT NOT NULL,
  text_digest TEXT NOT NULL,
  PRIMARY KEY (mission_id, position)
) STRICT;

CREATE TABLE mission_notes (
  seq INTEGER NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  text TEXT NOT NULL,
  text_digest TEXT NOT NULL,
  at TEXT NOT NULL
) STRICT;
