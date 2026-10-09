-- Work Supervision v0 projection, migration 0002 (PostgreSQL): mission worktrees.
-- Same columns, nullability and keys as ../sqlite/0002_worktrees.sql.

CREATE TABLE worktrees (
  mission_id TEXT NOT NULL PRIMARY KEY REFERENCES missions (id),
  repository TEXT NOT NULL,
  path TEXT NOT NULL,
  branch TEXT NOT NULL,
  base_commit TEXT NOT NULL,
  state TEXT NOT NULL,
  head TEXT,
  delete_branch BIGINT,
  archive_digest TEXT,
  archive_bytes BIGINT,
  intent_seq BIGINT NOT NULL,
  updated_seq BIGINT NOT NULL
);
