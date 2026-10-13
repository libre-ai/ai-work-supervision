-- Work Supervision v0 projection, migration 0006 (PostgreSQL): an approved
-- outline split into missions linked by their dependencies, and the ground of
-- an idea dismissal (docs/work-supervision/coordination-v0.md).
-- Same columns, nullability and keys as ../sqlite/0006_outline_splits.sql; checked against ../schema.v0.json.

-- NULL for a dismissal journalled before grounds existed.
ALTER TABLE ideas ADD COLUMN dismiss_ground TEXT;

CREATE TABLE outline_splits (
  artifact_id TEXT NOT NULL PRIMARY KEY REFERENCES artifacts (id),
  mission_id TEXT NOT NULL REFERENCES missions (id),
  state TEXT NOT NULL,
  plan TEXT NOT NULL,
  plan_digest TEXT NOT NULL,
  intent_seq BIGINT NOT NULL,
  intent_at TEXT NOT NULL,
  decided_seq BIGINT,
  decided_at TEXT
);

-- The mission identifiers are fixed by the intent, before the missions exist.
CREATE TABLE outline_missions (
  artifact_id TEXT NOT NULL REFERENCES outline_splits (artifact_id),
  position BIGINT NOT NULL,
  mission_id TEXT NOT NULL,
  PRIMARY KEY (artifact_id, position)
);
