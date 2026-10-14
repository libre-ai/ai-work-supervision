-- Work Supervision v0 projection, migration 0005 (PostgreSQL): phases and their
-- approved artifacts (docs/work-supervision/phases-v0.md).
-- Same columns, nullability and keys as ../sqlite/0005_phases.sql; checked against ../schema.v0.json.

CREATE TABLE mission_phases (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  position BIGINT NOT NULL,
  phase TEXT NOT NULL,
  PRIMARY KEY (mission_id, position)
);

CREATE TABLE artifacts (
  id TEXT NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  phase TEXT NOT NULL,
  state TEXT NOT NULL,
  content TEXT NOT NULL,
  content_digest TEXT NOT NULL,
  bytes BIGINT NOT NULL,
  submitted_by TEXT NOT NULL,
  submitted_seq BIGINT NOT NULL,
  submitted_at TEXT NOT NULL,
  reason TEXT,
  reason_digest TEXT,
  decided_seq BIGINT,
  decided_at TEXT
);
