//! Coordination operations of the daemon (`docs/work-supervision/coordination-v0.md`):
//! the actor gate, ideas, decision requests, declared sessions, mission
//! contract refinements, checks, the guards of a run and an acceptance, the
//! report, and their recovery steps.

use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use work_supervision_domain::coordination::{
    Actor, Blocker, Harness, IdeaCommand, ReportedState, RequestCommand, RequestOption,
    Reversibility, SessionCommand, SessionOutcome, accept_blockers, holds_worktree, run_blockers,
};
use work_supervision_domain::scope::{outside, overlaps};
use work_supervision_domain::{
    CheckId, Command, CommitId, Digest32, IdeaId, Mission, MissionId, RequestId, SessionId, State,
};
use work_supervision_pty::{RunBudgets, Session, SpawnSpec};
use work_supervision_store::{CheckExit, RequestRow};

use crate::core::{
    Core, Recovered, Shared, field, is_simulation, lock, mission_field, optional_u64, random_bytes,
};
use crate::{Failure, clock, fault};

/// A declared session silent for longer than this is shown `silent`.
const SILENT_AFTER_SECONDS: u64 = 15 * 60;

/// Operations a declared session may ask for; every other operation that
/// writes is the owner's (`actor.owner_only`).
const SESSION_OPS: [&str; 6] = [
    "idea.capture",
    "idea.qualify",
    "request.open",
    "request.withdraw",
    "session.report",
    "session.end",
];

/// Operations that write nothing, or that any client may ask for.
const OPEN_OPS: [&str; 11] = [
    "status",
    "mission.list",
    "mission.show",
    "mission.report",
    "wait",
    "idea.list",
    "request.list",
    "session.list",
    "session.find",
    "session.register",
    "doctor",
];

/// Every operation of this module.
const OPS: [&str; 20] = [
    "idea.capture",
    "idea.qualify",
    "idea.promote",
    "idea.dismiss",
    "idea.list",
    "request.open",
    "request.answer",
    "request.withdraw",
    "request.list",
    "session.register",
    "session.report",
    "session.end",
    "session.list",
    "session.find",
    "mission.depend",
    "mission.scope",
    "mission.check",
    "mission.report",
    "check.run",
    "decisions.pending",
];

fn invalid() -> Failure {
    Failure::new("request.field_invalid")
}

/// The actor a request declares; `owner` when absent.
fn actor(request: &Value) -> Result<Actor, Failure> {
    match request.get("actor") {
        None | Some(Value::Null) => Ok(Actor::Owner),
        Some(Value::String(text)) => Actor::parse(text).map_err(|_| invalid()),
        Some(_) => Err(invalid()),
    }
}

/// The actor gate: a session actor may only ask for [`SESSION_OPS`] and
/// [`OPEN_OPS`]. The actor is declarative (see the specification).
pub(crate) fn authorize(op: &str, request: &Value) -> Result<(), Failure> {
    match actor(request)? {
        Actor::Owner => Ok(()),
        Actor::Session(_) if SESSION_OPS.contains(&op) || OPEN_OPS.contains(&op) => Ok(()),
        Actor::Session(_) => Err(Failure::new("actor.owner_only")),
    }
}

/// Whether this module handles `op`.
pub(crate) fn handles(op: &str) -> bool {
    OPS.contains(&op)
}

fn optional_text(request: &Value, key: &str) -> Result<Option<String>, Failure> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(invalid()),
    }
}

fn optional_mission(request: &Value, key: &str) -> Result<Option<MissionId>, Failure> {
    optional_text(request, key)?
        .map(|text| MissionId::parse(&text).map_err(|_| invalid()))
        .transpose()
}

fn strings(request: &Value, key: &str) -> Result<Vec<String>, Failure> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid),
        Some(_) => Err(invalid()),
    }
}

fn index(request: &Value, key: &str) -> Result<usize, Failure> {
    request
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(invalid)
}

/// Checks that a repository named by a request is in the private configuration.
fn configured(core: &Core, repository: Option<&str>) -> Result<(), Failure> {
    if let Some(name) = repository {
        core.config.repository(name)?;
    }
    Ok(())
}

/// Executes one coordination request.
pub(crate) fn handle(shared: &Shared, op: &str, request: &Value) -> Result<Value, Failure> {
    let actor = actor(request)?;
    match op {
        "idea.capture" => {
            let id = IdeaId::from_bytes(random_bytes()?);
            let command = IdeaCommand::Capture {
                text: field(request, "text")?.to_owned(),
            };
            lock(shared)?
                .supervisor
                .idea(&id, &command, &actor, clock::now()?)?;
            Ok(json!({ "idea": id.as_str() }))
        }
        "idea.qualify" => {
            let id = IdeaId::parse(field(request, "idea")?).map_err(|_| invalid())?;
            let repository = optional_text(request, "repository")?;
            let mission = optional_mission(request, "mission")?;
            let mut core = lock(shared)?;
            configured(&core, repository.as_deref())?;
            if let Some(mission) = &mission {
                core.mission(mission)?;
            }
            let command = IdeaCommand::Qualify {
                repository,
                mission,
                context: optional_text(request, "context")?,
            };
            core.supervisor.idea(&id, &command, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "idea.promote" => promote(shared, request),
        "idea.dismiss" => {
            let id = IdeaId::parse(field(request, "idea")?).map_err(|_| invalid())?;
            let command = IdeaCommand::Dismiss {
                reason: field(request, "reason")?.to_owned(),
            };
            lock(shared)?
                .supervisor
                .idea(&id, &command, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "idea.list" => {
            let core = lock(shared)?;
            let ideas = core.supervisor.store().ideas()?;
            Ok(Value::Array(
                ideas
                    .into_iter()
                    .map(|idea| {
                        json!({
                            "id": idea.id,
                            "state": idea.state,
                            "text": idea.text,
                            "captured_by": idea.captured_by,
                            "repository": idea.repository,
                            "mission": idea.mission,
                            "context": idea.context,
                            "promoted_mission": idea.promoted_mission,
                            "dismiss_reason": idea.dismiss_reason,
                            "captured_at": idea.created_at,
                        })
                    })
                    .collect(),
            ))
        }
        "request.open" => open_request(shared, request, &actor),
        "request.answer" => {
            let id = RequestId::parse(field(request, "request")?).map_err(|_| invalid())?;
            let command = RequestCommand::Answer {
                choice: index(request, "choice")?,
                reason: field(request, "reason")?.to_owned(),
            };
            lock(shared)?
                .supervisor
                .request(&id, &command, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "request.withdraw" => {
            let id = RequestId::parse(field(request, "request")?).map_err(|_| invalid())?;
            let command = RequestCommand::Withdraw {
                reason: field(request, "reason")?.to_owned(),
            };
            lock(shared)?
                .supervisor
                .request(&id, &command, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "request.list" | "decisions.pending" => {
            let open_only = op == "decisions.pending"
                || request.get("open_only").and_then(Value::as_bool) == Some(true);
            let core = lock(shared)?;
            let rows = core.supervisor.store().requests(open_only)?;
            Ok(Value::Array(rows.iter().map(request_json).collect()))
        }
        "session.register" => register(shared, request),
        "session.report" => {
            let id = SessionId::parse(field(request, "session")?).map_err(|_| invalid())?;
            let command = SessionCommand::Report {
                state: ReportedState::parse(field(request, "state")?).map_err(|_| invalid())?,
                note: optional_text(request, "note")?,
            };
            lock(shared)?
                .supervisor
                .session(&id, &command, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "session.end" => {
            let id = SessionId::parse(field(request, "session")?).map_err(|_| invalid())?;
            let command = SessionCommand::End {
                outcome: SessionOutcome::parse(field(request, "outcome")?)
                    .map_err(|_| invalid())?,
                summary: optional_text(request, "summary")?,
            };
            lock(shared)?
                .supervisor
                .session(&id, &command, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "session.list" => {
            let core = lock(shared)?;
            let now = clock::epoch_seconds(clock::now()?.as_str()).unwrap_or(0);
            let rows = core.supervisor.store().sessions()?;
            Ok(Value::Array(
                rows.into_iter()
                    .map(|row| {
                        let age = clock::epoch_seconds(&row.updated_at)
                            .map(|then| now.saturating_sub(then));
                        let silent = row.state == "active"
                            && age.is_some_and(|age| age > SILENT_AFTER_SECONDS);
                        json!({
                            "id": row.id,
                            "harness": row.harness,
                            "state": row.state,
                            "silent": silent,
                            "seconds_since_report": age,
                            "repository": row.repository,
                            "mission": row.mission,
                            "label": row.label,
                            "reported_state": row.reported_state,
                            "note": row.note,
                            "outcome": row.outcome,
                            "summary": row.summary,
                            "registered_at": row.created_at,
                            "updated_at": row.updated_at,
                        })
                    })
                    .collect(),
            ))
        }
        "session.find" => {
            let harness = Harness::parse(field(request, "harness")?).map_err(|_| invalid())?;
            let external =
                Digest32::parse(field(request, "external_digest")?).map_err(|_| invalid())?;
            let core = lock(shared)?;
            let row = core
                .supervisor
                .store()
                .session_by_external(harness.as_str(), &external)?;
            Ok(json!({ "session": row.map(|row| row.id) }))
        }
        "mission.depend" => {
            let id = mission_field(request)?;
            let on = MissionId::parse(field(request, "on")?).map_err(|_| invalid())?;
            lock(shared)?
                .supervisor
                .declare_dependency(&id, &on, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "mission.scope" => {
            let id = mission_field(request)?;
            let paths = strings(request, "paths")?;
            lock(shared)?
                .supervisor
                .declare_scope(&id, &paths, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "mission.check" => {
            let id = mission_field(request)?;
            let argv = strings(request, "argv")?;
            lock(shared)?.supervisor.declare_check(
                &id,
                index(request, "criterion")?,
                &argv,
                &actor,
                clock::now()?,
            )?;
            Ok(json!({}))
        }
        "mission.report" => {
            let id = mission_field(request)?;
            let core = lock(shared)?;
            let mission = core.mission(&id)?;
            report(&core, &mission)
        }
        "check.run" => run_checks(shared, &mission_field(request)?),
        _ => Err(Failure::new("request.unknown_op")),
    }
}

fn promote(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let id = IdeaId::parse(field(request, "idea")?).map_err(|_| invalid())?;
    let title = field(request, "title")?.to_owned();
    let criteria = strings(request, "criteria")?;
    let mut core = lock(shared)?;
    let row = core
        .supervisor
        .store()
        .ideas()?
        .into_iter()
        .find(|idea| idea.id == id.as_str())
        .ok_or(Failure::new("idea.not_found"))?;
    let repository = optional_text(request, "repository")?
        .or(row.repository.clone())
        .ok_or(Failure::new("idea.repository_required"))?;
    core.config.repository(&repository)?;
    let brief = optional_text(request, "brief")?.unwrap_or(row.text);
    let budgets = work_supervision_domain::Budgets::new(
        optional_u64(request, "max_duration_seconds", 600)?,
        optional_u64(request, "max_output_bytes", 10 << 20)?,
    )?;
    let create = Command::Create {
        title,
        repository,
        brief,
        criteria,
        budgets,
        executor: core.config.executor(),
    };
    // Validate the mission before writing the intent: a refused creation must
    // not leave an idea promoting toward a mission that cannot exist.
    let mission = MissionId::from_bytes(random_bytes()?);
    work_supervision_domain::decide(&mission, None, &create, 0)?;
    core.supervisor.idea(
        &id,
        &IdeaCommand::PromoteIntent {
            mission: mission.clone(),
        },
        &Actor::Owner,
        clock::now()?,
    )?;
    fault::hit("idea-promotion-intended");
    core.execute(&mission, &create)?;
    fault::hit("idea-mission-created");
    core.supervisor.idea(
        &id,
        &IdeaCommand::ConfirmPromotion,
        &Actor::Owner,
        clock::now()?,
    )?;
    Ok(json!({ "mission": mission.as_str() }))
}

fn open_request(shared: &Shared, request: &Value, actor: &Actor) -> Result<Value, Failure> {
    let options = match request.get("options") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                Ok(RequestOption {
                    label: field(item, "label")?.to_owned(),
                    consequence: field(item, "consequence")?.to_owned(),
                    reversibility: Reversibility::parse(field(item, "reversibility")?)
                        .map_err(|_| invalid())?,
                })
            })
            .collect::<Result<Vec<_>, Failure>>()?,
        _ => return Err(invalid()),
    };
    let recommended = match request.get("recommended") {
        None | Some(Value::Null) => None,
        Some(_) => Some(index(request, "recommended")?),
    };
    let mission = optional_mission(request, "mission")?;
    let mut core = lock(shared)?;
    let mission = match mission {
        Some(id) => {
            let state = core.supervisor.mission(&id)?.map(|mission| mission.state());
            Some((id, state))
        }
        None => None,
    };
    let id = RequestId::from_bytes(random_bytes()?);
    let command = RequestCommand::Open {
        mission,
        question: field(request, "question")?.to_owned(),
        options,
        recommended,
    };
    core.supervisor
        .request(&id, &command, actor, clock::now()?)?;
    Ok(json!({ "request": id.as_str() }))
}

fn register(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let harness = Harness::parse(field(request, "harness")?).map_err(|_| invalid())?;
    let repository = optional_text(request, "repository")?;
    let external = optional_text(request, "external_digest")?
        .map(|text| Digest32::parse(&text).map_err(|_| invalid()))
        .transpose()?;
    let mut core = lock(shared)?;
    configured(&core, repository.as_deref())?;
    let id = SessionId::from_bytes(random_bytes()?);
    let command = SessionCommand::Register {
        harness,
        repository,
        mission: optional_mission(request, "mission")?,
        label: field(request, "label")?.to_owned(),
        external,
    };
    core.supervisor
        .session(&id, &command, &Actor::Owner, clock::now()?)?;
    Ok(json!({ "session": id.as_str() }))
}

fn request_json(row: &RequestRow) -> Value {
    json!({
        "id": row.id,
        "mission": row.mission,
        "state": row.state,
        "question": row.question,
        "options": row.options.iter().map(|option| json!({
            "label": option.label,
            "consequence": option.consequence,
            "reversibility": option.reversibility,
        })).collect::<Vec<_>>(),
        "recommended": row.recommended,
        "opened_by": row.opened_by,
        "choice": row.choice,
        "reason": row.reason,
        "closed_by": row.closed_by,
        "opened_at": row.created_at,
    })
}

// ------------------------------------------------------------------ guards ---

/// Missions of the same repository holding a worktree whose scope overlaps.
fn conflicts(core: &Core, mission: &Mission) -> Result<Vec<MissionId>, Failure> {
    let store = core.supervisor.store();
    let own = store.scope(mission.id())?;
    let mut found = Vec::new();
    for other in core.supervisor.missions()? {
        if other.id() == mission.id()
            || other.repository() != mission.repository()
            || !holds_worktree(other.state())
        {
            continue;
        }
        if overlaps(&own, &store.scope(other.id())?) {
            found.push(other.id().clone());
        }
    }
    Ok(found)
}

fn run_blockers_of(core: &Core, mission: &Mission) -> Result<Vec<Blocker>, Failure> {
    let store = core.supervisor.store();
    Ok(run_blockers(
        &store.dependencies_of(mission.id())?,
        &store.open_requests_of(mission.id())?,
        &conflicts(core, mission)?,
    ))
}

/// What blocks an acceptance, reading only what is recorded. A declared
/// scope without a recorded check at the submitted commit blocks it
/// (`scope.unchecked`): nothing proves the changes stayed inside.
fn accept_blockers_of(core: &Core, mission: &Mission) -> Result<Vec<&'static str>, Failure> {
    let store = core.supervisor.store();
    let submitted = mission.result().map(|result| result.commit().clone());
    let scope_check = match &submitted {
        Some(commit) => store.scope_check(mission.id(), commit.as_str())?,
        None => None,
    };
    let declared: Vec<usize> = store
        .criterion_checks(mission.id())?
        .iter()
        .map(|check| check.criterion)
        .collect();
    let blockers = accept_blockers(
        &store.open_requests_of(mission.id())?,
        core.checks.contains_key(mission.id().as_str()),
        &declared,
        &store.check_outcomes(mission.id())?,
        submitted.as_ref(),
        scope_check
            .as_ref()
            .and_then(|check| u64::try_from(check.outside.len()).ok()),
    );
    let mut codes: Vec<&'static str> = blockers.iter().map(Blocker::code).collect();
    if store.scope(mission.id())?.is_some() && scope_check.is_none() {
        codes.push("scope.unchecked");
    }
    Ok(codes)
}

/// Refuses a run of `mission` with the first blocker's code.
pub(crate) fn guard_run(core: &Core, mission: &Mission) -> Result<(), Failure> {
    if !matches!(
        mission.state(),
        State::Ready | State::Provisioned | State::Rejected
    ) {
        return Ok(());
    }
    match run_blockers_of(core, mission)?.first() {
        Some(blocker) => Err(Failure::new(blocker.code())),
        None => Ok(()),
    }
}

/// Refuses an acceptance of `mission` with the first blocker's code, after
/// recording a scope check a crash left missing.
pub(crate) fn guard_accept(core: &mut Core, mission: &Mission) -> Result<(), Failure> {
    if mission.state() != State::ResultSubmitted {
        return Ok(());
    }
    if let Some(result) = mission.result() {
        // The branch kept on acceptance must be the commit the checks and the
        // scope check examined: a commit made after the submission (by a check,
        // or by code a check ran) would be kept unverified.
        if core.worktree_state(mission.id())?.as_deref() == Some("created") {
            let head = core
                .worktrees
                .head_of(&core.worktrees.path_of(mission.id().as_str()))?;
            if head != *result.commit() {
                return Err(Failure::new("worktree.head_moved"));
            }
        }
        let recorded = core
            .supervisor
            .store()
            .scope_check(mission.id(), result.commit().as_str())?;
        if recorded.is_none() {
            // A failure leaves no record: a declared scope then blocks with
            // `scope.unchecked` below, an undeclared one has nothing to check.
            let _ = record_scope_check(core, mission.id());
        }
    }
    match accept_blockers_of(core, mission)?.first() {
        Some(code) => Err(Failure::new(code)),
        None => Ok(()),
    }
}

/// The current blockers of a mission, by code, for displays.
pub(crate) fn blocker_codes(core: &Core, mission: &Mission) -> Result<Vec<&'static str>, Failure> {
    match mission.state() {
        State::Draft | State::Ready | State::Provisioned | State::Rejected => {
            Ok(run_blockers_of(core, mission)?
                .iter()
                .map(Blocker::code)
                .collect())
        }
        State::ResultSubmitted => accept_blockers_of(core, mission),
        State::Running | State::WaitingInput | State::Exited => Ok(core
            .supervisor
            .store()
            .open_requests_of(mission.id())?
            .iter()
            .map(|_| "request.pending")
            .collect()),
        State::Accepted | State::Abandoned | State::Cancelled => Ok(Vec::new()),
    }
}

/// Records the scope check of the submitted result of mission `id`; returns
/// the number of paths outside the scope, `None` when the worktree is gone.
pub(crate) fn record_scope_check(core: &mut Core, id: &MissionId) -> Result<Option<u64>, Failure> {
    let mission = core.mission(id)?;
    let (Some(result), Some(base)) = (mission.result(), mission.base_commit()) else {
        return Ok(None);
    };
    if core.worktree_state(id)?.as_deref() != Some("created") {
        return Ok(None);
    }
    let changed = core.worktrees.changed_paths(id, base, result.commit())?;
    let scope = core.supervisor.store().scope(id)?;
    let outside_paths = outside(&scope, &changed);
    core.supervisor.record_scope_check(
        id,
        result.commit(),
        changed.len(),
        &outside_paths,
        clock::now()?,
    )?;
    Ok(u64::try_from(outside_paths.len()).ok())
}

// ------------------------------------------------------------------ checks ---

fn run_checks(shared: &Shared, id: &MissionId) -> Result<Value, Failure> {
    let mut core = lock(shared)?;
    let mission = core.mission(id)?;
    if mission.state() != State::ResultSubmitted {
        return Err(Failure::new("check.not_submitted"));
    }
    if core.checks.contains_key(id.as_str()) {
        return Err(Failure::new("check.running"));
    }
    let declared = core.supervisor.store().criterion_checks(id)?;
    if declared.is_empty() {
        return Err(Failure::new("check.none_declared"));
    }
    let commit = mission
        .result()
        .map(|result| result.commit().clone())
        .ok_or(Failure::new("check.not_submitted"))?;
    let at_commit = core.worktree_state(id)?.as_deref() == Some("created")
        && core.worktrees.is_clean(id)?
        && core
            .worktrees
            .head_of(&core.worktrees.path_of(id.as_str()))?
            == commit;
    if !at_commit {
        return Err(Failure::new("check.worktree_changed"));
    }
    core.checks.insert(id.as_str().to_owned(), None);
    let count = declared.len();
    let thread_shared = Arc::clone(shared);
    let mission_id = id.clone();
    let thread = std::thread::spawn(move || {
        for check in declared {
            if let Err(failure) = run_one_check(
                &thread_shared,
                &mission_id,
                &commit,
                check.criterion,
                &check.argv,
                &check.argv_digest,
            ) {
                // The remaining checks are not run: their criteria stay
                // unverified, which blocks the acceptance.
                eprintln!("wsd: check refused: {failure}");
                crate::core::anchor(&thread_shared);
                break;
            }
            crate::core::anchor(&thread_shared);
        }
        if let Ok(mut core) = lock(&thread_shared) {
            core.checks.remove(mission_id.as_str());
        }
    });
    if let Some(slot) = core.checks.get_mut(id.as_str()) {
        *slot = Some(thread);
    }
    Ok(json!({ "checks": count }))
}

/// Runs one check: journals its start, executes it in a fresh terminal in the
/// worktree under the mission's budgets, journals its end.
fn run_one_check(
    shared: &Shared,
    mission: &MissionId,
    commit: &CommitId,
    criterion: usize,
    argv: &[String],
    argv_digest: &str,
) -> Result<(), Failure> {
    let check = CheckId::from_bytes(random_bytes()?);
    let session = {
        let mut core = lock(shared)?;
        // Every check runs on the submitted commit, clean: a previous check
        // (or the code it ran) may have committed or written in the worktree.
        let at_commit = core.worktree_state(mission)?.as_deref() == Some("created")
            && core.worktrees.is_clean(mission)?
            && core
                .worktrees
                .head_of(&core.worktrees.path_of(mission.as_str()))?
                == *commit;
        if !at_commit {
            return Err(Failure::new("check.worktree_changed"));
        }
        // Everything that can fail is prepared before `check.started`, so the
        // journal never holds a start without its end but for a crash.
        let directory = core.layout.runs().join(check.as_str());
        let home = directory.join("home");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&home)
            .map_err(|_| Failure::new("run.io"))?;
        let budgets = core.mission(mission)?.budgets();
        let (program, arguments) = argv.split_first().ok_or(Failure::new("check.invalid"))?;
        core.supervisor.check_started(
            mission,
            &check,
            criterion,
            commit,
            argv_digest,
            clock::now()?,
        )?;
        fault::hit("check-started");
        let spec = SpawnSpec::new(
            PathBuf::from(program),
            arguments.iter().map(Into::into).collect(),
            core.worktrees.path_of(mission.as_str()),
            home,
            directory.join("pty.log"),
            RunBudgets::new(
                Duration::from_secs(budgets.max_duration_seconds()),
                budgets.max_output_bytes(),
                Duration::from_millis(core.config.grace_ms()),
            ),
        )
        .with_path(core.config.path().into());
        Session::spawn(spec)
    };
    let exit = match session {
        Ok(session) => match session.wait_with(|_| {}) {
            Ok(exit) => CheckExit {
                bytes: exit.output_bytes(),
                digest: exit.output_digest().to_owned(),
                exit_code: exit.exit_code().map(i64::from),
                signal: exit.signal().map(i64::from),
                budget: exit
                    .budget_exceeded()
                    .map(work_supervision_pty::Budget::as_str),
            },
            Err(_) => not_executed(),
        },
        Err(_) => not_executed(),
    };
    lock(shared)?
        .supervisor
        .check_finished(mission, &check, &exit, clock::now()?)?;
    Ok(())
}

/// A check whose program could not be started or supervised: no exit code,
/// no output; it never passes.
fn not_executed() -> CheckExit {
    CheckExit {
        bytes: 0,
        digest: Digest32::from_bytes(Sha256::digest([]).into()).to_hex(),
        exit_code: None,
        signal: None,
        budget: None,
    }
}

// ---------------------------------------------------------- show, report ---

fn check_runs_json(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    Ok(Value::Array(
        core.supervisor
            .store()
            .check_runs(mission.id())?
            .into_iter()
            .map(|row| {
                json!({
                    "check": row.check,
                    "criterion": row.criterion,
                    "commit": row.commit,
                    "state": row.state,
                    "exit_code": row.exit_code,
                    "signal": row.signal,
                    "budget": row.budget,
                    "passed": row.passed(),
                })
            })
            .collect(),
    ))
}

/// What `mission.show` adds: refinements, requests, checks and blockers.
pub(crate) fn mission_extras(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    let store = core.supervisor.store();
    let scope = store.scope(mission.id())?;
    Ok(json!({
        "dependencies": store.dependencies_of(mission.id())?.iter().map(|(id, state)| json!({
            "mission": id.as_str(), "state": state.as_str(),
        })).collect::<Vec<_>>(),
        "scope": scope.map(|paths| paths.iter().map(|path| path.as_str().to_owned()).collect::<Vec<_>>()),
        "checks": store.criterion_checks(mission.id())?.iter().map(|check| json!({
            "criterion": check.criterion, "argv": check.argv,
        })).collect::<Vec<_>>(),
        "check_runs": check_runs_json(core, mission)?,
        "requests": store.requests_of(mission.id().as_str())?.iter().map(request_json).collect::<Vec<_>>(),
        "blockers": blocker_codes(core, mission)?,
        "checks_running": core.checks.contains_key(mission.id().as_str()),
    }))
}

/// Whether the last finished execution of the check of `criterion` passed at
/// the submitted commit: the rule of `accept_blockers`, applied to rows.
fn verified(
    rows: &[work_supervision_store::CheckRunRow],
    criterion: usize,
    submitted: Option<&str>,
) -> bool {
    rows.iter()
        .rev()
        .find(|row| {
            row.state == "finished" && usize::try_from(row.criterion).ok() == Some(criterion)
        })
        .is_some_and(|row| Some(row.commit.as_str()) == submitted && row.passed())
}

/// The self-standing report of a mission (`ws report`): what was asked, what
/// was done, the evidence, the gaps, the decisions and the timeline.
pub(crate) fn report(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    let store = core.supervisor.store();
    let id = mission.id();
    let checks = store.criterion_checks(id)?;
    let runs = store.runs_of(id.as_str())?;
    let check_runs = store.check_runs(id)?;
    let submitted = mission
        .result()
        .map(|result| result.commit().as_str().to_owned());
    let scope = store.scope(id)?;
    let scope_check = match &submitted {
        Some(commit) => store.scope_check(id, commit)?,
        None => None,
    };
    let requests = store.requests_of(id.as_str())?;
    let criteria: Vec<Value> = mission
        .criteria()
        .iter()
        .enumerate()
        .map(|(position, text)| {
            let check = checks.iter().find(|check| check.criterion == position);
            let last = check_runs
                .iter()
                .rev()
                .find(|row| usize::try_from(row.criterion).ok() == Some(position));
            json!({
                "index": position,
                "text": text,
                "check": check.map(|check| json!({ "argv": check.argv })),
                "verified": check.map(|_| verified(&check_runs, position, submitted.as_deref())),
                "last_execution": last.map(|row| json!({
                    "commit": row.commit,
                    "state": row.state,
                    "exit_code": row.exit_code,
                    "signal": row.signal,
                    "budget": row.budget,
                    "passed": row.passed(),
                    "at_submitted_commit": Some(&row.commit) == submitted.as_ref(),
                })),
            })
        })
        .collect();
    let blockers = blocker_codes(core, mission)?;
    let mut gaps: Vec<&str> = Vec::new();
    if is_simulation(mission) {
        gaps.push("simulation");
    }
    if mission.result().is_none() {
        gaps.push("result.missing");
    }
    if checks.len() < mission.criteria().len() {
        gaps.push("criteria.unchecked");
    }
    // Same rule as the acceptance guard: the last finished execution decides.
    let unverified = checks
        .iter()
        .any(|check| !verified(&check_runs, check.criterion, submitted.as_deref()));
    if unverified {
        gaps.push("criteria.unverified");
    }
    if scope.is_none() {
        gaps.push("scope.undeclared");
    }
    if scope_check
        .as_ref()
        .is_some_and(|check| !check.outside.is_empty())
    {
        gaps.push("scope.violated");
    }
    if runs.iter().any(|run| run.budget.is_some()) {
        gaps.push("run.budget_exceeded");
    }
    if runs.iter().any(|run| run.state == "interrupted") {
        gaps.push("run.interrupted");
    }
    if requests.iter().any(|request| request.state == "open") {
        gaps.push("request.pending");
    }
    let sessions: Vec<Value> = store
        .sessions()?
        .into_iter()
        .filter(|session| session.mission.as_deref() == Some(id.as_str()))
        .map(|session| {
            json!({
                "id": session.id,
                "harness": session.harness,
                "state": session.state,
                "reported_state": session.reported_state,
                "outcome": session.outcome,
                "summary": session.summary,
            })
        })
        .collect();
    let notes: Vec<Value> = store
        .notes_of(id.as_str())?
        .into_iter()
        .map(|(text, at)| json!({ "text": text, "at": at }))
        .collect();
    let mut report = Map::new();
    report.insert(
        "schema".to_owned(),
        json!("libre-ai.work-supervision.report.v0"),
    );
    report.insert(
        "mission".to_owned(),
        json!({
            "id": id.as_str(),
            "title": mission.title(),
            "repository": mission.repository(),
            "state": mission.state().as_str(),
            "executor": mission.executor().as_str(),
            "simulation": is_simulation(mission),
        }),
    );
    report.insert(
        "intent".to_owned(),
        json!({
            "brief": mission.brief(),
            "criteria": criteria,
            "dependencies": store.dependencies_of(id)?.iter().map(|(on, state)| json!({
                "mission": on.as_str(), "state": state.as_str(),
            })).collect::<Vec<_>>(),
            "scope": scope.map(|paths| paths.iter().map(|path| path.as_str().to_owned()).collect::<Vec<_>>()),
        }),
    );
    report.insert(
        "result".to_owned(),
        mission.result().map_or(Value::Null, |result| {
            json!({
                "commit": result.commit().as_str(),
                "evidence_digest": result.evidence().to_hex(),
                "summary": result.summary(),
            })
        }),
    );
    report.insert(
        "verdict".to_owned(),
        mission.verdict().map_or(
            Value::Null,
            |verdict| json!({ "state": verdict.state().as_str(), "reason": verdict.reason() }),
        ),
    );
    report.insert(
        "evidence".to_owned(),
        json!({
            "scope_check": scope_check.map(|check| json!({
                "commit": check.commit,
                "changed": check.changed,
                "outside": check.outside,
            })),
            "check_executions": check_runs_json(core, mission)?,
        }),
    );
    report.insert(
        "runs".to_owned(),
        Value::Array(
            runs.iter()
                .map(|run| {
                    json!({
                        "run": run.run,
                        "state": run.state,
                        "exit_code": run.exit_code,
                        "signal": run.signal,
                        "budget": run.budget,
                        "output_bytes": run.output_bytes,
                        "inputs": run.inputs,
                    })
                })
                .collect(),
        ),
    );
    report.insert(
        "decisions".to_owned(),
        Value::Array(requests.iter().map(request_json).collect()),
    );
    report.insert("sessions".to_owned(), Value::Array(sessions));
    report.insert("notes".to_owned(), Value::Array(notes));
    report.insert("blockers".to_owned(), json!(blockers));
    report.insert("gaps".to_owned(), json!(gaps));
    report.insert(
        "timeline".to_owned(),
        Value::Array(
            store
                .timeline(id)?
                .into_iter()
                .map(|row| json!({ "seq": row.seq, "at": row.at, "kind": row.kind }))
                .collect(),
        ),
    );
    Ok(Value::Object(report))
}

// ---------------------------------------------------------------- recovery ---

/// Recovery steps of the coordination primitives: checks left running are
/// interrupted, promotions left pending are confirmed or aborted, a submitted
/// result whose scope check a crash skipped is checked.
pub(crate) fn recover(core: &mut Core, recovered: &mut Recovered) -> Result<(), Failure> {
    for (mission, check) in core.supervisor.store().running_checks()? {
        if core.checks.contains_key(&mission) {
            continue;
        }
        core.supervisor
            .check_interrupted(&mission, &check, clock::now()?)?;
        recovered.checks_interrupted += 1;
    }
    for idea in core.supervisor.store().promoting_ideas()? {
        let id = IdeaId::parse(&idea.id).map_err(|_| Failure::new("projection.sqlite"))?;
        let exists = match idea.promoting_mission.as_deref().map(MissionId::parse) {
            Some(Ok(mission)) => core.supervisor.mission(&mission)?.is_some(),
            _ => false,
        };
        let command = if exists {
            recovered.promotions_confirmed += 1;
            IdeaCommand::ConfirmPromotion
        } else {
            recovered.promotions_aborted += 1;
            IdeaCommand::AbortPromotion
        };
        core.supervisor
            .idea(&id, &command, &Actor::Owner, clock::now()?)?;
    }
    for mission in core.supervisor.missions()? {
        let Some(result) = mission.result() else {
            continue;
        };
        if mission.state() != State::ResultSubmitted
            || core
                .supervisor
                .store()
                .scope_check(mission.id(), result.commit().as_str())?
                .is_some()
        {
            continue;
        }
        // A scope check that fails here is counted, never propagated: it must
        // not keep the daemon from starting. Acceptance then reports
        // `scope.unchecked` for a declared scope.
        match record_scope_check(core, mission.id()) {
            Ok(Some(_)) => recovered.scope_checks += 1,
            Ok(None) => {}
            Err(_) => recovered.scope_check_failures += 1,
        }
    }
    Ok(())
}
