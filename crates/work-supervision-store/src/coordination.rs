//! Projection of the coordination events (`docs/work-supervision/coordination-v0.md`):
//! ideas, decision requests, declared sessions and mission contract
//! refinements. As for worktrees, each event is accepted only from the state
//! that precedes it; anything else is `projection.event_invalid`.

use rusqlite::{OptionalExtension as _, Transaction, params};
use serde_json::Value;
use work_supervision_domain::coordination::{
    Actor, Harness, ReportedState, Reversibility, SessionOutcome,
};
use work_supervision_domain::scope::ScopePath;

use crate::projection::{Fields, is_lower_hex};
use crate::{BlobStore, StoreError};

fn exactly_one(fields: &Fields<'_>, changed: usize) -> Result<(), StoreError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(fields.invalid())
    }
}

fn actor<'a>(fields: &Fields<'a>, key: &str) -> Result<&'a str, StoreError> {
    let text = fields.string(key)?;
    Actor::parse(text).map_err(|_| fields.invalid())?;
    Ok(text)
}

fn mission_exists(transaction: &Transaction<'_>, mission: &str) -> Result<bool, StoreError> {
    let count: i64 = transaction.query_row(
        "SELECT count(*) FROM missions WHERE id = ?1",
        [mission],
        |row| row.get(0),
    )?;
    Ok(count == 1)
}

fn mission_state(
    transaction: &Transaction<'_>,
    mission: &str,
) -> Result<Option<String>, StoreError> {
    Ok(transaction
        .query_row(
            "SELECT state FROM missions WHERE id = ?1",
            [mission],
            |row| row.get(0),
        )
        .optional()?)
}

fn require_mission(
    transaction: &Transaction<'_>,
    fields: &Fields<'_>,
    mission: Option<&str>,
) -> Result<(), StoreError> {
    match mission {
        Some(id) if !mission_exists(transaction, id)? => Err(fields.invalid()),
        _ => Ok(()),
    }
}

/// Reads a blob holding a JSON array of strings.
fn string_array(
    fields: &Fields<'_>,
    key: &str,
    blobs: &BlobStore,
) -> Result<(Vec<String>, String, String), StoreError> {
    let (text, digest) = fields.text(key, blobs)?;
    let values: Vec<String> = serde_json::from_str(&text).map_err(|_| fields.invalid())?;
    Ok((values, text, digest))
}

/// `idea.*`.
pub(crate) fn idea(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let idea = fields.identifier("idea")?;
    let current: Option<(String, i64, Option<String>)> = transaction
        .query_row(
            "SELECT state, ever_qualified, promoting_mission FROM ideas WHERE id = ?1",
            [idea],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let open = current
        .as_ref()
        .is_some_and(|(state, _, _)| state == "captured" || state == "qualified");
    let promoting = current
        .as_ref()
        .is_some_and(|(state, _, _)| state == "promoting");
    let changed = match kind {
        "idea.captured" => {
            if current.is_some() {
                return Err(fields.invalid());
            }
            let (text, digest) = fields.text("text_digest", blobs)?;
            transaction.execute(
                "INSERT INTO ideas (id, state, text, text_digest, captured_by, ever_qualified,
                   created_seq, created_at, updated_seq, updated_at)
                 VALUES (?1, 'captured', ?2, ?3, ?4, 0, ?5, ?6, ?5, ?6)",
                params![idea, text, digest, actor(fields, "actor")?, seq, at],
            )?
        }
        "idea.qualified" if open => {
            actor(fields, "actor")?;
            let repository = fields.optional_string("repository")?;
            let mission = fields.optional_identifier("mission")?;
            require_mission(transaction, fields, mission)?;
            let context = fields.optional_text("context_digest", blobs)?;
            let (context, context_digest) = context.unzip();
            transaction.execute(
                "UPDATE ideas SET state = 'qualified', ever_qualified = 1,
                   repository = coalesce(?2, repository), mission_id = coalesce(?3, mission_id),
                   context = coalesce(?4, context), context_digest = coalesce(?5, context_digest),
                   updated_seq = ?6, updated_at = ?7
                 WHERE id = ?1",
                params![idea, repository, mission, context, context_digest, seq, at],
            )?
        }
        "idea.promotion.intent" if open => transaction.execute(
            "UPDATE ideas SET state = 'promoting', promoting_mission = ?2, updated_seq = ?3,
               updated_at = ?4 WHERE id = ?1",
            params![idea, fields.identifier("mission")?, seq, at],
        )?,
        "idea.promoted" if promoting => {
            let mission = fields.identifier("mission")?;
            let intended = current.as_ref().and_then(|(_, _, promoting)| promoting.as_deref());
            if intended != Some(mission) || !mission_exists(transaction, mission)? {
                return Err(fields.invalid());
            }
            transaction.execute(
                "UPDATE ideas SET state = 'promoted', promoted_mission = ?2,
                   promoting_mission = NULL, updated_seq = ?3, updated_at = ?4 WHERE id = ?1",
                params![idea, mission, seq, at],
            )?
        }
        "idea.promotion.aborted" if promoting => transaction.execute(
            "UPDATE ideas SET state = CASE ever_qualified WHEN 1 THEN 'qualified' ELSE 'captured' END,
               promoting_mission = NULL, updated_seq = ?2, updated_at = ?3 WHERE id = ?1",
            params![idea, seq, at],
        )?,
        "idea.dismissed" if open => {
            let (reason, digest) = fields.text("reason_digest", blobs)?;
            transaction.execute(
                "UPDATE ideas SET state = 'dismissed', dismiss_reason = ?2,
                   dismiss_reason_digest = ?3, updated_seq = ?4, updated_at = ?5 WHERE id = ?1",
                params![idea, reason, digest, seq, at],
            )?
        }
        "idea.qualified"
        | "idea.promotion.intent"
        | "idea.promoted"
        | "idea.promotion.aborted"
        | "idea.dismissed" => return Err(fields.invalid()),
        _ => return Err(StoreError::UnknownKind { seq: fields.seq }),
    };
    exactly_one(fields, changed)
}

/// `request.*`.
pub(crate) fn request(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let request = fields.identifier("request")?;
    let current: Option<String> = transaction
        .query_row(
            "SELECT state FROM requests WHERE id = ?1",
            [request],
            |row| row.get(0),
        )
        .optional()?;
    let open = current.as_deref() == Some("open");
    let changed = match kind {
        "request.opened" => {
            if current.is_some() {
                return Err(fields.invalid());
            }
            let mission = fields.optional_identifier("mission")?;
            require_mission(transaction, fields, mission)?;
            let (question, question_digest) = fields.text("question_digest", blobs)?;
            let options = fields
                .data
                .get("options")
                .and_then(Value::as_array)
                .filter(|options| (2..=4).contains(&options.len()))
                .ok_or_else(|| fields.invalid())?;
            let recommended = fields.optional_integer("recommended")?;
            let count = i64::try_from(options.len()).map_err(|_| fields.invalid())?;
            if recommended.is_some_and(|index| index < 0 || index >= count) {
                return Err(fields.invalid());
            }
            transaction.execute(
                "INSERT INTO requests (id, mission_id, state, question, question_digest,
                   recommended, opened_by, created_seq, created_at, updated_seq, updated_at)
                 VALUES (?1, ?2, 'open', ?3, ?4, ?5, ?6, ?7, ?8, ?7, ?8)",
                params![
                    request,
                    mission,
                    question,
                    question_digest,
                    recommended,
                    actor(fields, "actor")?,
                    seq,
                    at
                ],
            )?;
            for (position, option) in options.iter().enumerate() {
                let Some(option) = option.as_object() else {
                    return Err(fields.invalid());
                };
                let option_fields = Fields {
                    data: option,
                    seq: fields.seq,
                };
                let (label, label_digest) = option_fields.text("label_digest", blobs)?;
                let (consequence, consequence_digest) =
                    option_fields.text("consequence_digest", blobs)?;
                let reversibility = option_fields.string("reversibility")?;
                Reversibility::parse(reversibility).map_err(|_| fields.invalid())?;
                transaction.execute(
                    "INSERT INTO request_options (request_id, position, label, label_digest,
                       consequence, consequence_digest, reversibility)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        request,
                        i64::try_from(position).map_err(|_| fields.invalid())?,
                        label,
                        label_digest,
                        consequence,
                        consequence_digest,
                        reversibility
                    ],
                )?;
            }
            1
        }
        "request.answered" if open => {
            let choice = fields.integer("choice")?;
            let options: i64 = transaction.query_row(
                "SELECT count(*) FROM request_options WHERE request_id = ?1",
                [request],
                |row| row.get(0),
            )?;
            if choice >= options {
                return Err(fields.invalid());
            }
            let (reason, digest) = fields.text("reason_digest", blobs)?;
            transaction.execute(
                "UPDATE requests SET state = 'answered', choice = ?2, reason = ?3,
                   reason_digest = ?4, closed_by = 'owner', updated_seq = ?5, updated_at = ?6
                 WHERE id = ?1",
                params![request, choice, reason, digest, seq, at],
            )?
        }
        "request.withdrawn" if open => {
            let (reason, digest) = fields.text("reason_digest", blobs)?;
            transaction.execute(
                "UPDATE requests SET state = 'withdrawn', reason = ?2, reason_digest = ?3,
                   closed_by = ?4, updated_seq = ?5, updated_at = ?6 WHERE id = ?1",
                params![request, reason, digest, actor(fields, "actor")?, seq, at],
            )?
        }
        "request.answered" | "request.withdrawn" => return Err(fields.invalid()),
        _ => return Err(StoreError::UnknownKind { seq: fields.seq }),
    };
    exactly_one(fields, changed)
}

/// `session.*`.
pub(crate) fn session(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let session = fields.identifier("session")?;
    let current: Option<String> = transaction
        .query_row(
            "SELECT state FROM sessions WHERE id = ?1",
            [session],
            |row| row.get(0),
        )
        .optional()?;
    let active = current.as_deref() == Some("active");
    let changed = match kind {
        "session.registered" => {
            if current.is_some() {
                return Err(fields.invalid());
            }
            let harness = fields.string("harness")?;
            Harness::parse(harness).map_err(|_| fields.invalid())?;
            let mission = fields.optional_identifier("mission")?;
            require_mission(transaction, fields, mission)?;
            let external = fields.optional_string("external_digest")?;
            if external.is_some_and(|digest| !is_lower_hex(digest, 64)) {
                return Err(fields.invalid());
            }
            let (label, label_digest) = fields.text("label_digest", blobs)?;
            transaction.execute(
                "INSERT INTO sessions (id, harness, state, repository, mission_id, label,
                   label_digest, external_digest, created_seq, created_at, updated_seq, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?8, ?9)",
                params![
                    session,
                    harness,
                    fields.optional_string("repository")?,
                    mission,
                    label,
                    label_digest,
                    external,
                    seq,
                    at
                ],
            )?
        }
        "session.reported" if active => {
            let state = fields.string("state")?;
            ReportedState::parse(state).map_err(|_| fields.invalid())?;
            let (note, note_digest) = fields.optional_text("note_digest", blobs)?.unzip();
            transaction.execute(
                "UPDATE sessions SET reported_state = ?2, note = ?3, note_digest = ?4,
                   updated_seq = ?5, updated_at = ?6 WHERE id = ?1",
                params![session, state, note, note_digest, seq, at],
            )?
        }
        "session.ended" if active => {
            let outcome = fields.string("outcome")?;
            SessionOutcome::parse(outcome).map_err(|_| fields.invalid())?;
            let (summary, summary_digest) = fields.optional_text("summary_digest", blobs)?.unzip();
            transaction.execute(
                "UPDATE sessions SET state = 'ended', outcome = ?2, summary = ?3,
                   summary_digest = ?4, updated_seq = ?5, updated_at = ?6 WHERE id = ?1",
                params![session, outcome, summary, summary_digest, seq, at],
            )?
        }
        "session.reported" | "session.ended" => return Err(fields.invalid()),
        _ => return Err(StoreError::UnknownKind { seq: fields.seq }),
    };
    exactly_one(fields, changed)
}

/// `dependency.*`, `scope.*` and `check.*`: the refinements of a mission's
/// contract and the evidence that verifies it.
pub(crate) fn contract(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
) -> Result<(), StoreError> {
    let mission = fields.mission()?;
    let state = mission_state(transaction, mission)?.ok_or_else(|| fields.invalid())?;
    let draft = state == "draft";
    let count = |sql: &str, parameters: &[&dyn rusqlite::ToSql]| -> Result<i64, StoreError> {
        Ok(transaction.query_row(sql, parameters, |row| row.get(0))?)
    };
    let changed = match kind {
        "dependency.declared" if draft => {
            let on = fields.identifier("on")?;
            let duplicate = count(
                "SELECT count(*) FROM mission_dependencies WHERE mission_id = ?1 AND on_mission = ?2",
                &[&mission, &on],
            )?;
            if on == mission || duplicate != 0 || !mission_exists(transaction, on)? {
                return Err(fields.invalid());
            }
            transaction.execute(
                "INSERT INTO mission_dependencies (mission_id, on_mission, seq) VALUES (?1, ?2, ?3)",
                params![mission, on, seq],
            )?
        }
        "scope.declared" if draft => {
            let declared = count(
                "SELECT count(*) FROM mission_scopes WHERE mission_id = ?1",
                &[&mission],
            )?;
            let (paths, _, _) = string_array(fields, "paths_digest", blobs)?;
            let parsed = work_supervision_domain::scope::parse_scope(&paths)
                .map_err(|_| fields.invalid())?;
            if declared != 0 || parsed.len() != paths.len() {
                return Err(fields.invalid());
            }
            for (position, path) in parsed.iter().map(ScopePath::as_str).enumerate() {
                transaction.execute(
                    "INSERT INTO mission_scopes (mission_id, position, path) VALUES (?1, ?2, ?3)",
                    params![
                        mission,
                        i64::try_from(position).map_err(|_| fields.invalid())?,
                        path
                    ],
                )?;
            }
            1
        }
        "scope.checked" => {
            let commit = fields.commit("commit")?;
            let changed = fields.integer("changed")?;
            let outside = fields.integer("outside")?;
            let (paths, text, digest) = string_array(fields, "outside_digest", blobs)?;
            let listed = i64::try_from(paths.len()).map_err(|_| fields.invalid())?;
            if listed != outside || outside > changed {
                return Err(fields.invalid());
            }
            transaction.execute(
                "INSERT INTO scope_checks (mission_id, commit_id, changed, outside, outside_digest,
                   outside_paths, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (mission_id, commit_id) DO UPDATE SET changed = excluded.changed,
                   outside = excluded.outside, outside_digest = excluded.outside_digest,
                   outside_paths = excluded.outside_paths, seq = excluded.seq",
                params![mission, commit, changed, outside, digest, text, seq],
            )?
        }
        "check.declared" if draft => {
            let criterion = fields.integer("criterion")?;
            let criteria = count(
                "SELECT count(*) FROM mission_criteria WHERE mission_id = ?1",
                &[&mission],
            )?;
            let declared = count(
                "SELECT count(*) FROM criterion_checks WHERE mission_id = ?1 AND criterion = ?2",
                &[&mission, &criterion],
            )?;
            let (argv, text, digest) = string_array(fields, "argv_digest", blobs)?;
            if criterion >= criteria
                || declared != 0
                || work_supervision_domain::coordination::check_argv(&argv).is_err()
            {
                return Err(fields.invalid());
            }
            transaction.execute(
                "INSERT INTO criterion_checks (mission_id, criterion, argv, argv_digest, seq)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![mission, criterion, text, digest, seq],
            )?
        }
        "check.started" => {
            let check = fields.identifier("check")?;
            let criterion = fields.integer("criterion")?;
            let argv_digest = fields.digest("argv_digest")?.to_hex();
            let declared: Option<String> = transaction
                .query_row(
                    "SELECT argv_digest FROM criterion_checks WHERE mission_id = ?1 AND criterion = ?2",
                    params![mission, criterion],
                    |row| row.get(0),
                )
                .optional()?;
            let exists = count(
                "SELECT count(*) FROM check_runs WHERE check_id = ?1",
                &[&check],
            )?;
            if declared.as_deref() != Some(argv_digest.as_str()) || exists != 0 {
                return Err(fields.invalid());
            }
            transaction.execute(
                "INSERT INTO check_runs (check_id, mission_id, criterion, commit_id, argv_digest,
                   state, started_seq, updated_seq) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?6)",
                params![
                    check,
                    mission,
                    criterion,
                    fields.commit("commit")?,
                    argv_digest,
                    seq
                ],
            )?
        }
        "check.finished" => {
            let check = fields.identifier("check")?;
            let budget = match fields.data.get("budget") {
                Some(Value::Null) => None,
                Some(Value::String(name)) if name == "duration" || name == "output" => {
                    Some(name.as_str())
                }
                _ => return Err(fields.invalid()),
            };
            transaction.execute(
                "UPDATE check_runs SET state = 'finished', output_bytes = ?3, output_digest = ?4,
                   exit_code = ?5, signal = ?6, budget = ?7, updated_seq = ?8
                 WHERE check_id = ?1 AND mission_id = ?2 AND state = 'running'",
                params![
                    check,
                    mission,
                    fields.integer("bytes")?,
                    fields.digest("digest")?.to_hex(),
                    fields.optional_integer("exit_code")?,
                    fields.optional_integer("signal")?,
                    budget,
                    seq
                ],
            )?
        }
        "check.interrupted" => transaction.execute(
            "UPDATE check_runs SET state = 'interrupted', updated_seq = ?3
             WHERE check_id = ?1 AND mission_id = ?2 AND state = 'running'",
            params![fields.identifier("check")?, mission, seq],
        )?,
        "dependency.declared" | "scope.declared" | "check.declared" => {
            return Err(fields.invalid());
        }
        _ => return Err(StoreError::UnknownKind { seq: fields.seq }),
    };
    exactly_one(fields, changed)
}
