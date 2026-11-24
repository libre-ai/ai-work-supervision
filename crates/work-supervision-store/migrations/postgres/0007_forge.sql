-- Work Supervision v0 projection, migration 0007 (PostgreSQL): forge links
-- and forge signals reduced to their references
-- (docs/work-supervision/coordination-v0.md, « Forge signals »).
-- Same columns, nullability and keys as ../sqlite/0007_forge.sql; checked against ../schema.v0.json.

CREATE TABLE forge_links (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  repository TEXT NOT NULL,
  pull BIGINT NOT NULL,
  seq BIGINT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (mission_id, repository, pull)
);

CREATE TABLE forge_signals (
  seq BIGINT NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  repository TEXT NOT NULL,
  pull BIGINT NOT NULL,
  source TEXT NOT NULL,
  signal_id BIGINT NOT NULL,
  head_commit TEXT NOT NULL,
  state TEXT,
  sender TEXT,
  reason TEXT,
  outcome TEXT NOT NULL,
  proposal TEXT,
  proposal_digest TEXT,
  follow_up TEXT,
  at TEXT NOT NULL
);
