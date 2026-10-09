-- Work Supervision v0 projection, migration 0003 (PostgreSQL): runs of the executor.
-- Same columns, nullability and keys as ../sqlite/0003_runs.sql; checked against ../schema.v0.json.

CREATE TABLE runs (
  run_id TEXT NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  state TEXT NOT NULL,
  argv_digest TEXT NOT NULL,
  cwd TEXT NOT NULL,
  output_bytes BIGINT NOT NULL,
  output_digest TEXT,
  inputs BIGINT NOT NULL,
  input_bytes BIGINT NOT NULL,
  exit_code BIGINT,
  signal BIGINT,
  budget TEXT,
  escalated BIGINT,
  started_seq BIGINT NOT NULL,
  updated_seq BIGINT NOT NULL
);
