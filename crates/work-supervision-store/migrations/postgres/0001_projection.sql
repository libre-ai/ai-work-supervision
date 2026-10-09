-- Work Supervision v0 projection, migration 0001 (PostgreSQL).
-- Same tables, columns, nullability and keys as ../sqlite/0001_projection.sql;
-- SQLite INTEGER is BIGINT here. Single user: no tenant column, no RLS
-- (ADR-0042 §5). Checked against ../schema.v0.json in PGlite.

CREATE TABLE projection_position (
  id BIGINT NOT NULL PRIMARY KEY CHECK (id = 1),
  seq BIGINT NOT NULL CHECK (seq >= 1),
  digest TEXT NOT NULL
);

CREATE TABLE missions (
  id TEXT NOT NULL PRIMARY KEY,
  title TEXT NOT NULL,
  title_digest TEXT NOT NULL,
  repository TEXT NOT NULL,
  brief TEXT NOT NULL,
  brief_digest TEXT NOT NULL,
  max_duration_seconds BIGINT NOT NULL,
  max_output_bytes BIGINT NOT NULL,
  executor TEXT NOT NULL,
  state TEXT NOT NULL,
  revision BIGINT NOT NULL,
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
  created_seq BIGINT NOT NULL,
  created_at TEXT NOT NULL,
  updated_seq BIGINT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE mission_criteria (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  position BIGINT NOT NULL,
  text TEXT NOT NULL,
  text_digest TEXT NOT NULL,
  PRIMARY KEY (mission_id, position)
);

CREATE TABLE mission_notes (
  seq BIGINT NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  text TEXT NOT NULL,
  text_digest TEXT NOT NULL,
  at TEXT NOT NULL
);
