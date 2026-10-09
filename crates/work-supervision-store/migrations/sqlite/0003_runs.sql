-- Work Supervision v0 projection, migration 0003 (SQLite): runs of the executor.
-- Keep in step with ../postgres/0003_runs.sql and ../schema.v0.json.

CREATE TABLE runs (
  run_id TEXT NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  state TEXT NOT NULL,
  argv_digest TEXT NOT NULL,
  cwd TEXT NOT NULL,
  output_bytes INTEGER NOT NULL,
  output_digest TEXT,
  inputs INTEGER NOT NULL,
  input_bytes INTEGER NOT NULL,
  exit_code INTEGER,
  signal INTEGER,
  budget TEXT,
  escalated INTEGER,
  started_seq INTEGER NOT NULL,
  updated_seq INTEGER NOT NULL
) STRICT;
