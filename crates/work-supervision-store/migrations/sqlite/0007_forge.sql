-- Work Supervision v0 projection, migration 0007 (SQLite): forge links and
-- forge signals reduced to their references (docs/work-supervision/coordination-v0.md,
-- « Forge signals »).
-- Keep in step with ../postgres/0007_forge.sql and ../schema.v0.json.

-- The pull requests the owner linked to a mission; `repository` is the
-- lowercase `owner/name` of the forge.
CREATE TABLE forge_links (
  mission_id TEXT NOT NULL REFERENCES missions (id),
  repository TEXT NOT NULL,
  pull INTEGER NOT NULL,
  seq INTEGER NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (mission_id, repository, pull)
) STRICT;

-- One row per recorded signal. An ignored signal keeps its reference and
-- `reason` only: `state` and `sender` are NULL. An admitted one keeps
-- `sender`, the owner's name of the sender-list entry that admitted it,
-- never a login. `proposal` is NULL for an ignored or observed signal,
-- `follow_up` unless accepted. No text written on the forge is ever stored:
-- `proposal` is the fixed template filled with the reference.
CREATE TABLE forge_signals (
  seq INTEGER NOT NULL PRIMARY KEY,
  mission_id TEXT NOT NULL REFERENCES missions (id),
  repository TEXT NOT NULL,
  pull INTEGER NOT NULL,
  source TEXT NOT NULL,
  signal_id INTEGER NOT NULL,
  head_commit TEXT NOT NULL,
  state TEXT,
  sender TEXT,
  reason TEXT,
  outcome TEXT NOT NULL,
  proposal TEXT,
  proposal_digest TEXT,
  follow_up TEXT,
  at TEXT NOT NULL
) STRICT;
