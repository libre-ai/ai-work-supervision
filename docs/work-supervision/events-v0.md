# Work Supervision v0 — event catalogue and projection

The journal (`journal-v0.md`) carries one event per entry. This catalogue lists
the event kinds of Work Supervision v0 and the data each one carries, and how
the SQLite projection (`crates/work-supervision-store`) applies them. Kinds
outside this catalogue are refused by the projection
(`projection.unknown_kind`): a journal written by a newer version is never
silently half-read.

## Rules common to every event

- **No free text in the journal.** Titles, briefs, criteria, notes, summaries
  and reasons may contain personal data; an append-only hash chain cannot
  forget. They are stored as content-addressed blobs (`<root>/blobs/<sha256>`)
  and the event carries their digest (`*_digest`, 64 lowercase hex characters).
  A blob is written and synchronised before the entry that names it.
- `mission` is the mission identifier: 32 lowercase hexadecimal characters
  (128 random bits).
- Transition events carry `revision`, the mission revision **after** the
  event, and `state`, the state it leads to. The projection refuses an event
  whose revision is not exactly one above the stored one, or whose state is
  not the one its kind leads to (`projection.event_invalid`).
- Commits are 40 or 64 lowercase hexadecimal characters.

## Mission events

| Kind | State after | Data besides `mission`, `revision`, `state` |
| --- | --- | --- |
| `mission.created` | `draft` (revision 1) | `title_digest`, `repository` (name in the private configuration), `brief_digest`, `criteria_digests` (array), `max_duration_seconds`, `max_output_bytes`, `executor` |
| `mission.readied` | `ready` | — (the brief is frozen at its digest) |
| `mission.provisioned` | `provisioned` | `base_commit`, `branch` (`ws/<mission>`), `worktree` (path relative to the root) |
| `mission.run-started` | `running` | `run`; clears the previous result and verdict (resumption after a rejection) |
| `mission.input-awaited` | `waiting-input` | `run` (must be the current run) |
| `mission.input-resumed` | `running` | `run` (must be the current run) |
| `mission.run-exited` | `exited` | `run` (must be the current run), `interrupted` (boolean) |
| `mission.result-submitted` | `result-submitted` | `commit`, `evidence_digest`, `summary_digest` |
| `mission.accepted` | `accepted` | `reason_digest` |
| `mission.rejected` | `rejected` | `reason_digest` |
| `mission.abandoned` | `abandoned` | `reason_digest` |
| `mission.cancelled` | `cancelled` | `reason_digest` |
| `mission.noted` | unchanged, no revision | `note_digest` |

## Worktree events

| Kind | Worktree state after | Data besides `mission` |
| --- | --- | --- |
| `worktree.create.intent` | `creating` (first intent, or after `aborted`) | `repository`, `path` (`worktrees/<mission>`), `branch` (`ws/<mission>`), `base_commit` |
| `worktree.created` | `created` (from `creating`) | `head` (observed) |
| `worktree.create.aborted` | `aborted` (from `creating`) | — |
| `worktree.remove.intent` | `releasing` (from `created`) | `delete_branch` (boolean) |
| `worktree.archived` | unchanged (`releasing`) | `archive_digest` (evidence blob), `archive_bytes` |
| `worktree.removed` | `removed` (from `releasing`) | — |

A step from any other state is refused (`projection.event_invalid`). Projected
in table `worktrees` (migration 0002). Lifecycle: `worktree-v0.md`.

`journal.recovered` (written by the journal itself after a torn tail is
quarantined) changes nothing in the projection.

## Projection

- Tables: `missions`, `mission_criteria`, `mission_notes`, `worktrees`,
  `projection_position`. Migrations: `crates/work-supervision-store/migrations/`
  (`sqlite/` and `postgres/`, same numbers), both checked against
  `migrations/schema.v0.json` — SQLite by the crate's tests, PostgreSQL in
  PGlite (`crates/work-supervision-store/postgres-check`).
- **Journal first, projection second.** An entry is applied once it is
  durable; the entry and the new position are written in one SQLite
  transaction. On open, the projection replays the journal: entries after its
  position are applied, the entry at its position must have the same digest
  (`projection.diverged`), and a position beyond the journal's head is
  refused (`projection.ahead`).
- **Rebuild.** `Store::rebuild` builds a new database from the journal and the
  blobs alone, under a temporary name renamed into place on success; it never
  overwrites a database (`projection.target_exists`). `Store::dump` is the
  canonical byte form — tables by name, rows by primary key, one JSON array
  per row — in which a rebuilt projection equals the live one.
- Errors name a code and a sequence number, never a text, digest or path.
