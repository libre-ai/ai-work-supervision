-- Work Supervision v0 projection, migration 0005 (SQLite): phases and their
-- approved artifacts (docs/work-supervision/phases-v0.md).
-- Keep in step with ../postgres/0005_phases.sql and ../schema.v0.json.

CREATE TABLE mission_phases (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  position INTEGER NOT NULL,
  phase TEXT NOT NULL,
  PRIMARY KEY (mission_id, position)
) STRICT;

CREATE TABLE artifacts (
  id TEXT NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  phase TEXT NOT NULL,
  state TEXT NOT NULL,
  content TEXT NOT NULL,
  content_digest TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  submitted_by TEXT NOT NULL,
  submitted_seq INTEGER NOT NULL,
  submitted_at TEXT NOT NULL,
  reason TEXT,
  reason_digest TEXT,
  decided_seq INTEGER,
  decided_at TEXT
) STRICT;
