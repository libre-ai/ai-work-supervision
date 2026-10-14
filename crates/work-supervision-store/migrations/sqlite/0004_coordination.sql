-- Work Supervision v0 projection, migration 0004 (SQLite): coordination
-- primitives (docs/work-supervision/coordination-v0.md).
-- Keep in step with ../postgres/0004_coordination.sql and ../schema.v0.json.

CREATE TABLE ideas (
  id TEXT NOT NULL PRIMARY KEY,
  state TEXT NOT NULL,
  text TEXT NOT NULL,
  text_digest TEXT NOT NULL,
  captured_by TEXT NOT NULL,
  repository TEXT,
  mission_id TEXT,
  context TEXT,
  context_digest TEXT,
  ever_qualified INTEGER NOT NULL,
  promoting_mission TEXT,
  promoted_mission TEXT,
  dismiss_reason TEXT,
  dismiss_reason_digest TEXT,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_seq INTEGER NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE requests (
  id TEXT NOT NULL PRIMARY KEY,
  mission_id TEXT REFERENCES missions (id),
  state TEXT NOT NULL,
  question TEXT NOT NULL,
  question_digest TEXT NOT NULL,
  recommended INTEGER,
  opened_by TEXT NOT NULL,
  choice INTEGER,
  reason TEXT,
  reason_digest TEXT,
  closed_by TEXT,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_seq INTEGER NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE request_options (
  request_id TEXT NOT NULL REFERENCES requests (id),
  position INTEGER NOT NULL,
  label TEXT NOT NULL,
  label_digest TEXT NOT NULL,
  consequence TEXT NOT NULL,
  consequence_digest TEXT NOT NULL,
  reversibility TEXT NOT NULL,
  PRIMARY KEY (request_id, position)
) STRICT;

CREATE TABLE mission_dependencies (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  on_mission TEXT NOT NULL REFERENCES missions (id),
  seq INTEGER NOT NULL,
  PRIMARY KEY (mission_id, on_mission)
) STRICT;

CREATE TABLE mission_scopes (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  position INTEGER NOT NULL,
  path TEXT NOT NULL,
  PRIMARY KEY (mission_id, position)
) STRICT;

CREATE TABLE scope_checks (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  commit_id TEXT NOT NULL,
  changed INTEGER NOT NULL,
  outside INTEGER NOT NULL,
  outside_digest TEXT NOT NULL,
  outside_paths TEXT NOT NULL,
  seq INTEGER NOT NULL,
  PRIMARY KEY (mission_id, commit_id)
) STRICT;

CREATE TABLE criterion_checks (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  criterion INTEGER NOT NULL,
  argv TEXT NOT NULL,
  argv_digest TEXT NOT NULL,
  seq INTEGER NOT NULL,
  PRIMARY KEY (mission_id, criterion)
) STRICT;

CREATE TABLE check_runs (
  check_id TEXT NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  criterion INTEGER NOT NULL,
  commit_id TEXT NOT NULL,
  argv_digest TEXT NOT NULL,
  state TEXT NOT NULL,
  output_bytes INTEGER,
  output_digest TEXT,
  exit_code INTEGER,
  signal INTEGER,
  budget TEXT,
  started_seq INTEGER NOT NULL,
  updated_seq INTEGER NOT NULL
) STRICT;

CREATE TABLE sessions (
  id TEXT NOT NULL PRIMARY KEY,
  harness TEXT NOT NULL,
  state TEXT NOT NULL,
  repository TEXT,
  mission_id TEXT REFERENCES missions (id),
  label TEXT NOT NULL,
  label_digest TEXT NOT NULL,
  external_digest TEXT,
  reported_state TEXT,
  note TEXT,
  note_digest TEXT,
  outcome TEXT,
  summary TEXT,
  summary_digest TEXT,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_seq INTEGER NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE mission_events (
  seq INTEGER NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  kind TEXT NOT NULL,
  at TEXT NOT NULL
) STRICT;
