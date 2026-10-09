//! Application of one journal entry to the projection tables.
//!
//! The catalogue of event kinds and their data is specified in
//! `docs/work-supervision/events-v0.md`. The projection does not re-run the
//! mission state machine (the journal is the authority, the domain decided
//! before appending); it checks that each entry is well-formed and consistent
//! with the row it updates — known mission, revision exactly one above the
//! stored one, state matching the kind — and refuses otherwise.

use rusqlite::{OptionalExtension as _, Transaction, params};
use serde_json::{Map, Value};
use work_supervision_journal::{Digest, Entry};

use crate::{BlobStore, StoreError};

struct Fields<'a> {
    data: &'a Map<String, Value>,
    seq: u64,
}

impl<'a> Fields<'a> {
    const fn invalid(&self) -> StoreError {
        StoreError::EventInvalid { seq: self.seq }
    }

    fn string(&self, key: &str) -> Result<&'a str, StoreError> {
        self.data
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| self.invalid())
    }

    fn integer(&self, key: &str) -> Result<i64, StoreError> {
        self.data
            .get(key)
            .and_then(Value::as_i64)
            .filter(|value| *value >= 0)
            .ok_or_else(|| self.invalid())
    }

    fn boolean(&self, key: &str) -> Result<bool, StoreError> {
        self.data
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| self.invalid())
    }

    fn mission(&self) -> Result<&'a str, StoreError> {
        let id = self.string("mission")?;
        if is_lower_hex(id, 32) {
            Ok(id)
        } else {
            Err(self.invalid())
        }
    }

    fn digest(&self, key: &str) -> Result<Digest, StoreError> {
        Digest::from_hex(self.string(key)?).ok_or_else(|| self.invalid())
    }

    fn commit(&self, key: &str) -> Result<&'a str, StoreError> {
        let commit = self.string(key)?;
        if is_lower_hex(commit, 40) || is_lower_hex(commit, 64) {
            Ok(commit)
        } else {
            Err(self.invalid())
        }
    }

    /// Reads the text a digest field names; a missing blob is its own refusal.
    fn text(&self, key: &str, blobs: &BlobStore) -> Result<(String, String), StoreError> {
        let digest = self.digest(key)?;
        match blobs.get_text(&digest) {
            Ok(text) => Ok((text, digest.to_hex())),
            Err(StoreError::BlobIo) if !blobs.contains(&digest) => {
                Err(StoreError::BlobMissing { seq: self.seq })
            }
            Err(error) => Err(error),
        }
    }
}

pub(crate) fn is_lower_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Applies `entry` inside `transaction`; the caller updates the position.
pub(crate) fn apply(
    transaction: &Transaction<'_>,
    entry: &Entry,
    blobs: &BlobStore,
) -> Result<(), StoreError> {
    let seq = entry.seq();
    let fields = Fields {
        data: entry.event().data(),
        seq,
    };
    let seq_value = i64::try_from(seq).map_err(|_| fields.invalid())?;
    let at = entry.at().as_str();
    match entry.event().kind() {
        "journal.recovered" => Ok(()),
        "mission.created" => created(transaction, &fields, blobs, seq_value, at),
        "mission.noted" => {
            let mission = fields.mission()?;
            let (text, digest) = fields.text("note_digest", blobs)?;
            let inserted = transaction.execute(
                "INSERT INTO mission_notes (seq, mission_id, text, text_digest, at)
                 SELECT ?1, id, ?3, ?4, ?5 FROM missions WHERE id = ?2",
                params![seq_value, mission, text, digest, at],
            )?;
            if inserted == 1 {
                Ok(())
            } else {
                Err(fields.invalid())
            }
        }
        kind if kind.starts_with("worktree.") => worktree(transaction, kind, &fields, seq_value),
        kind if kind.starts_with("run.") => run(transaction, kind, &fields, seq_value),
        kind => transition(transaction, kind, &fields, blobs, seq_value, at),
    }
}

fn created(
    transaction: &Transaction<'_>,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let mission = fields.mission()?;
    if fields.integer("revision")? != 1 || fields.string("state")? != "draft" {
        return Err(fields.invalid());
    }
    let (title, title_digest) = fields.text("title_digest", blobs)?;
    let (brief, brief_digest) = fields.text("brief_digest", blobs)?;
    let repository = fields.string("repository")?;
    let executor = fields.string("executor")?;
    let max_duration = fields.integer("max_duration_seconds")?;
    let max_output = fields.integer("max_output_bytes")?;
    let criteria = fields
        .data
        .get("criteria_digests")
        .and_then(Value::as_array)
        .ok_or_else(|| fields.invalid())?;
    let exists: i64 = transaction.query_row(
        "SELECT count(*) FROM missions WHERE id = ?1",
        [mission],
        |row| row.get(0),
    )?;
    if exists != 0 {
        return Err(fields.invalid());
    }
    transaction.execute(
        "INSERT INTO missions (id, title, title_digest, repository, brief, brief_digest,
           max_duration_seconds, max_output_bytes, executor, state, revision,
           created_seq, created_at, updated_seq, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'draft', 1, ?10, ?11, ?10, ?11)",
        params![
            mission,
            title,
            title_digest,
            repository,
            brief,
            brief_digest,
            max_duration,
            max_output,
            executor,
            seq,
            at
        ],
    )?;
    for (position, criterion) in criteria.iter().enumerate() {
        let digest = criterion
            .as_str()
            .and_then(Digest::from_hex)
            .ok_or_else(|| fields.invalid())?;
        let text = match blobs.get_text(&digest) {
            Ok(text) => text,
            Err(StoreError::BlobIo) if !blobs.contains(&digest) => {
                return Err(StoreError::BlobMissing { seq: fields.seq });
            }
            Err(error) => return Err(error),
        };
        let position = i64::try_from(position).map_err(|_| fields.invalid())?;
        transaction.execute(
            "INSERT INTO mission_criteria (mission_id, position, text, text_digest) VALUES (?1, ?2, ?3, ?4)",
            params![mission, position, text, digest.to_hex()],
        )?;
    }
    Ok(())
}

/// State a transition kind leads to.
fn target_state(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "mission.readied" => "ready",
        "mission.provisioned" => "provisioned",
        "mission.run-started" | "mission.input-resumed" => "running",
        "mission.input-awaited" => "waiting-input",
        "mission.run-exited" => "exited",
        "mission.result-submitted" => "result-submitted",
        "mission.accepted" => "accepted",
        "mission.rejected" => "rejected",
        "mission.abandoned" => "abandoned",
        "mission.cancelled" => "cancelled",
        _ => return None,
    })
}

fn transition(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let Some(state) = target_state(kind) else {
        return Err(StoreError::UnknownKind { seq: fields.seq });
    };
    let mission = fields.mission()?;
    let revision = fields.integer("revision")?;
    if fields.string("state")? != state || revision < 2 {
        return Err(fields.invalid());
    }
    let updated = transaction.execute(
        "UPDATE missions SET state = ?1, revision = ?2, updated_seq = ?3, updated_at = ?4
         WHERE id = ?5 AND revision = ?2 - 1",
        params![state, revision, seq, at, mission],
    )?;
    if updated != 1 {
        return Err(fields.invalid());
    }
    match kind {
        "mission.provisioned" => {
            let base = fields.commit("base_commit")?;
            let branch = fields.string("branch")?;
            let worktree = fields.string("worktree")?;
            transaction.execute(
                "UPDATE missions SET base_commit = ?1, branch = ?2, worktree = ?3 WHERE id = ?4",
                params![base, branch, worktree, mission],
            )?;
        }
        "mission.run-started" => {
            let run = fields.string("run")?;
            transaction.execute(
                "UPDATE missions SET current_run = ?1, result_commit = NULL, evidence_digest = NULL,
                   summary = NULL, summary_digest = NULL, verdict = NULL, verdict_reason = NULL,
                   verdict_reason_digest = NULL
                 WHERE id = ?2",
                params![run, mission],
            )?;
        }
        "mission.input-awaited" | "mission.input-resumed" => {
            let run = fields.string("run")?;
            check_current_run(transaction, fields, mission, run)?;
        }
        "mission.run-exited" => {
            let run = fields.string("run")?;
            fields.boolean("interrupted")?;
            check_current_run(transaction, fields, mission, run)?;
        }
        "mission.result-submitted" => {
            let commit = fields.commit("commit")?;
            let evidence = fields.digest("evidence_digest")?;
            let (summary, summary_digest) = fields.text("summary_digest", blobs)?;
            transaction.execute(
                "UPDATE missions SET result_commit = ?1, evidence_digest = ?2, summary = ?3,
                   summary_digest = ?4
                 WHERE id = ?5",
                params![commit, evidence.to_hex(), summary, summary_digest, mission],
            )?;
        }
        "mission.readied" => {}
        _ => {
            // accepted, rejected, abandoned, cancelled: the owner's verdict and its reason.
            let (reason, reason_digest) = fields.text("reason_digest", blobs)?;
            transaction.execute(
                "UPDATE missions SET verdict = ?1, verdict_reason = ?2, verdict_reason_digest = ?3
                 WHERE id = ?4",
                params![state, reason, reason_digest, mission],
            )?;
        }
    }
    Ok(())
}

fn check_current_run(
    transaction: &Transaction<'_>,
    fields: &Fields<'_>,
    mission: &str,
    run: &str,
) -> Result<(), StoreError> {
    let current: Option<String> = transaction.query_row(
        "SELECT current_run FROM missions WHERE id = ?1",
        [mission],
        |row| row.get(0),
    )?;
    if current.as_deref() == Some(run) {
        Ok(())
    } else {
        Err(fields.invalid())
    }
}

/// `worktree.*` events (`docs/work-supervision/worktree-v0.md`): each step of
/// the lifecycle is accepted only from the state that precedes it.
fn worktree(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    seq: i64,
) -> Result<(), StoreError> {
    let mission = fields.mission()?;
    let current: Option<String> = transaction
        .query_row(
            "SELECT state FROM worktrees WHERE mission_id = ?1",
            [mission],
            |row| row.get(0),
        )
        .optional()?;
    let current = current.as_deref();
    let updated = match kind {
        "worktree.create.intent" => {
            // A first intent, or a new attempt after an aborted one.
            if !matches!(current, None | Some("aborted")) {
                return Err(fields.invalid());
            }
            let path = fields.string("path")?;
            let branch = fields.string("branch")?;
            if path != format!("worktrees/{mission}") || branch != format!("ws/{mission}") {
                return Err(fields.invalid());
            }
            transaction.execute(
                "INSERT INTO worktrees (mission_id, repository, path, branch, base_commit, state,
                   intent_seq, updated_seq)
                 SELECT id, ?2, ?3, ?4, ?5, 'creating', ?6, ?6 FROM missions WHERE id = ?1
                 ON CONFLICT (mission_id) DO UPDATE SET repository = excluded.repository,
                   base_commit = excluded.base_commit, state = 'creating', head = NULL,
                   delete_branch = NULL, archive_digest = NULL, archive_bytes = NULL,
                   intent_seq = excluded.intent_seq, updated_seq = excluded.updated_seq",
                params![
                    mission,
                    fields.string("repository")?,
                    path,
                    branch,
                    fields.commit("base_commit")?,
                    seq
                ],
            )?
        }
        "worktree.created" if current == Some("creating") => transaction.execute(
            "UPDATE worktrees SET state = 'created', head = ?2, updated_seq = ?3 WHERE mission_id = ?1",
            params![mission, fields.commit("head")?, seq],
        )?,
        "worktree.create.aborted" if current == Some("creating") => transaction.execute(
            "UPDATE worktrees SET state = 'aborted', updated_seq = ?2 WHERE mission_id = ?1",
            params![mission, seq],
        )?,
        "worktree.remove.intent" if current == Some("created") => transaction.execute(
            "UPDATE worktrees SET state = 'releasing', delete_branch = ?2, updated_seq = ?3
             WHERE mission_id = ?1",
            params![mission, i64::from(fields.boolean("delete_branch")?), seq],
        )?,
        "worktree.archived" if current == Some("releasing") => transaction.execute(
            "UPDATE worktrees SET archive_digest = ?2, archive_bytes = ?3, updated_seq = ?4
             WHERE mission_id = ?1",
            params![
                mission,
                fields.digest("archive_digest")?.to_hex(),
                fields.integer("archive_bytes")?,
                seq
            ],
        )?,
        "worktree.removed" if current == Some("releasing") => transaction.execute(
            "UPDATE worktrees SET state = 'removed', updated_seq = ?2 WHERE mission_id = ?1",
            params![mission, seq],
        )?,
        "worktree.created"
        | "worktree.create.aborted"
        | "worktree.remove.intent"
        | "worktree.archived"
        | "worktree.removed" => return Err(fields.invalid()),
        _ => return Err(StoreError::UnknownKind { seq: fields.seq }),
    };
    if updated == 1 {
        Ok(())
    } else {
        Err(fields.invalid())
    }
}

/// `run.*` events: a run is `running` from `run.started` until `run.exited`
/// or `run.interrupted`; checkpoints never go backwards.
fn run(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    seq: i64,
) -> Result<(), StoreError> {
    let mission = fields.mission()?;
    let run = fields.string("run")?;
    if !is_lower_hex(run, 32) {
        return Err(fields.invalid());
    }
    let current: Option<(String, String, i64)> = transaction
        .query_row(
            "SELECT mission_id, state, output_bytes FROM runs WHERE run_id = ?1",
            [run],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if kind == "run.started" {
        if current.is_some() {
            return Err(fields.invalid());
        }
        let inserted = transaction.execute(
            "INSERT INTO runs (run_id, mission_id, state, argv_digest, cwd, output_bytes, inputs,
               input_bytes, started_seq, updated_seq)
             SELECT ?1, id, 'running', ?3, ?4, 0, 0, 0, ?5, ?5 FROM missions WHERE id = ?2",
            params![
                run,
                mission,
                fields.digest("argv_digest")?.to_hex(),
                fields.string("cwd")?,
                seq
            ],
        )?;
        return if inserted == 1 {
            Ok(())
        } else {
            Err(fields.invalid())
        };
    }
    let Some((owner, state, output_bytes)) = current else {
        return Err(fields.invalid());
    };
    if owner != mission || state != "running" {
        return Err(fields.invalid());
    }
    let optional_integer = |key: &str| -> Result<Option<i64>, StoreError> {
        match fields.data.get(key) {
            Some(Value::Null) => Ok(None),
            Some(value) => value.as_i64().map(Some).ok_or_else(|| fields.invalid()),
            None => Err(fields.invalid()),
        }
    };
    let updated = match kind {
        "run.output.checkpoint" => {
            let bytes = fields.integer("bytes")?;
            if bytes < output_bytes {
                return Err(fields.invalid());
            }
            transaction.execute(
                "UPDATE runs SET output_bytes = ?2, output_digest = ?3, updated_seq = ?4 WHERE run_id = ?1",
                params![run, bytes, fields.digest("digest")?.to_hex(), seq],
            )?
        }
        "run.input" => {
            fields.digest("digest")?;
            transaction.execute(
                "UPDATE runs SET inputs = inputs + 1, input_bytes = input_bytes + ?2, updated_seq = ?3
                 WHERE run_id = ?1",
                params![run, fields.integer("bytes")?, seq],
            )?
        }
        "run.exited" => {
            let bytes = fields.integer("bytes")?;
            if bytes < output_bytes {
                return Err(fields.invalid());
            }
            let budget = match fields.data.get("budget") {
                Some(Value::Null) => None,
                Some(Value::String(name)) if name == "duration" || name == "output" => {
                    Some(name.clone())
                }
                _ => return Err(fields.invalid()),
            };
            transaction.execute(
                "UPDATE runs SET state = 'exited', output_bytes = ?2, output_digest = ?3, exit_code = ?4,
                   signal = ?5, budget = ?6, escalated = ?7, updated_seq = ?8
                 WHERE run_id = ?1",
                params![
                    run,
                    bytes,
                    fields.digest("digest")?.to_hex(),
                    optional_integer("exit_code")?,
                    optional_integer("signal")?,
                    budget,
                    i64::from(fields.boolean("escalated")?),
                    seq
                ],
            )?
        }
        "run.interrupted" => transaction.execute(
            "UPDATE runs SET state = 'interrupted', updated_seq = ?2 WHERE run_id = ?1",
            params![run, seq],
        )?,
        _ => return Err(StoreError::UnknownKind { seq: fields.seq }),
    };
    if updated == 1 {
        Ok(())
    } else {
        Err(fields.invalid())
    }
}
