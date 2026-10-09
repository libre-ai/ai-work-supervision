-- Work Supervision v0 projection, migration 0002 (SQLite): mission worktrees.
-- Keep in step with ../postgres/0002_worktrees.sql and ../schema.v0.json.

CREATE TABLE worktrees (
  mission_id TEXT NOT NULL PRIMARY KEY REFERENCES missions (id),
  repository TEXT NOT NULL,
  path TEXT NOT NULL,
  branch TEXT NOT NULL,
  base_commit TEXT NOT NULL,
  state TEXT NOT NULL,
  head TEXT,
  delete_branch INTEGER,
  archive_digest TEXT,
  archive_bytes INTEGER,
  intent_seq INTEGER NOT NULL,
  updated_seq INTEGER NOT NULL
) STRICT;
