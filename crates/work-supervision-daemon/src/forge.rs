//! Forge signals (`docs/work-supervision/coordination-v0.md`, « Forge
//! signals »): the owner links a mission to its pull requests, then passes
//! the forge's own answer about their reviews and check runs, read by `gh api`
//! outside Work Supervision, to `ws forge ingest`.
//!
//! Work Supervision opens no connection and holds no forge secret: the input
//! is exactly as authentic as the owner's own `gh`. [`parse_output`] keeps a
//! reference of each review or check run — never a body, a comment, a title,
//! a check name, a summary or any other text written on the forge — and runs
//! in the client, so that text does not even cross the socket. The sender's
//! login (a check run's: its application's slug) crosses it to be compared
//! with the owner's `[forge.senders]` list, and is never recorded. The daemon
//! checks every reference again, routes it to a pull request linked to the
//! mission, and records it under one outcome: ignored (with a closed reason,
//! nothing of the sender), observed, proposed (`observe` mode) or accepted
//! (`active` mode: a draft follow-up mission, never a change to the
//! original); an admitted one names the list entry that admitted it.

use serde_json::{Map, Value, json};
use work_supervision_domain::coordination::Actor;
use work_supervision_domain::forge::{
    ForgeLogin, ForgeRepository, ForgeSignal, PullNumber, SignalOutcome, SignalSource, SignalState,
    admit, follow_up, outcome, route,
};
use work_supervision_domain::{Budgets, Command, CommitId, Mission, MissionId};
use work_supervision_store::{SignalRecord, SupervisorError};

use crate::coordination::{actor, invalid};
use crate::core::{
    Core, DEFAULT_DURATION_SECONDS, DEFAULT_OUTPUT_BYTES, Recovered, Shared, field, lock,
    mission_field, random_bytes,
};
use crate::{Failure, clock, fault};

/// Most signals one ingestion may carry.
pub const MAX_SIGNALS: usize = 256;

/// Every operation of this module; both are the owner's.
const OPS: [&str; 2] = ["forge.link", "forge.ingest"];

/// The only forge API origin whose URLs are read; any other refuses the input.
const API_REPOS: &str = "https://api.github.com/repos/";

/// Keys of one signal in a `forge.ingest` request, and no other.
const SIGNAL_KEYS: [&str; 7] = [
    "source",
    "id",
    "state",
    "commit",
    "repository",
    "pulls",
    "sender",
];

/// Whether this module handles `op`.
pub(crate) fn handles(op: &str) -> bool {
    OPS.contains(&op)
}

fn unknown_form() -> Failure {
    Failure::new("forge.input_unknown")
}

fn invalid_input() -> Failure {
    Failure::new("forge.input_invalid")
}

/// `owner/name` and the rest of a forge API URL under [`API_REPOS`].
fn api_path(url: &str) -> Option<(ForgeRepository, &str)> {
    let rest = url.strip_prefix(API_REPOS)?;
    let mut parts = rest.splitn(3, '/');
    let (owner, name, tail) = (parts.next()?, parts.next()?, parts.next()?);
    let repository = ForgeRepository::parse(&format!("{owner}/{name}")).ok()?;
    Some((repository, tail))
}

fn forge_number(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64)
}

fn text_of<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn review_state(text: &str) -> Option<SignalState> {
    Some(match text {
        "APPROVED" => SignalState::Approved,
        "CHANGES_REQUESTED" => SignalState::ChangesRequested,
        "COMMENTED" => SignalState::Commented,
        "DISMISSED" => SignalState::Dismissed,
        "PENDING" => SignalState::Pending,
        _ => return None,
    })
}

/// One element of `gh api repos/<o>/<r>/pulls/<n>/reviews`.
fn review(item: &Value) -> Result<ForgeSignal, Failure> {
    let object = item.as_object().ok_or_else(invalid_input)?;
    let id = forge_number(object.get("id")).ok_or_else(invalid_input)?;
    let state = text_of(object, "state")
        .and_then(review_state)
        .ok_or_else(invalid_input)?;
    let commit = text_of(object, "commit_id")
        .and_then(|text| CommitId::parse(text).ok())
        .ok_or_else(invalid_input)?;
    let (repository, tail) = text_of(object, "pull_request_url")
        .and_then(api_path)
        .ok_or_else(invalid_input)?;
    let pull = tail
        .strip_prefix("pulls/")
        .and_then(|number| number.parse::<u64>().ok())
        .and_then(|number| PullNumber::new(number).ok())
        .ok_or_else(invalid_input)?;
    let sender = object
        .get("user")
        .and_then(|user| user.get("login"))
        .and_then(Value::as_str)
        .and_then(|login| ForgeLogin::parse(login).ok())
        .ok_or_else(invalid_input)?;
    ForgeSignal::new(
        SignalSource::Review,
        id,
        state,
        commit,
        repository,
        vec![pull],
        sender,
    )
    .map_err(|_| invalid_input())
}

/// The state of a check run: its conclusion once `completed`, `pending` before.
fn check_state(status: &str, conclusion: Option<&Value>) -> Option<SignalState> {
    match status {
        "queued" | "in_progress" | "waiting" | "requested" | "pending" => {
            Some(SignalState::Pending)
        }
        "completed" => {
            let state = SignalState::parse(conclusion?.as_str()?).ok()?;
            state
                .belongs_to(SignalSource::Check)
                .then_some(state)
                .filter(|state| *state != SignalState::Pending)
        }
        _ => None,
    }
}

/// One element of `check_runs` in `gh api repos/<o>/<r>/commits/<sha>/check-runs`.
fn check_run(item: &Value) -> Result<ForgeSignal, Failure> {
    let object = item.as_object().ok_or_else(invalid_input)?;
    let id = forge_number(object.get("id")).ok_or_else(invalid_input)?;
    let state = text_of(object, "status")
        .and_then(|status| check_state(status, object.get("conclusion")))
        .ok_or_else(invalid_input)?;
    let commit = text_of(object, "head_sha")
        .and_then(|text| CommitId::parse(text).ok())
        .ok_or_else(invalid_input)?;
    let (repository, tail) = text_of(object, "url")
        .and_then(api_path)
        .ok_or_else(invalid_input)?;
    if tail != format!("check-runs/{id}") {
        return Err(invalid_input());
    }
    let sender = object
        .get("app")
        .and_then(|app| app.get("slug"))
        .and_then(Value::as_str)
        .and_then(|slug| ForgeLogin::parse(&format!("{slug}[bot]")).ok())
        .ok_or_else(invalid_input)?;
    // The pull requests of this repository whose head the run ran on; one of
    // a fork or of another repository cannot be linked here.
    let mut pulls = Vec::new();
    for entry in object
        .get("pull_requests")
        .and_then(Value::as_array)
        .ok_or_else(invalid_input)?
    {
        let number = forge_number(entry.get("number")).ok_or_else(invalid_input)?;
        let (owner, tail) = entry
            .get("url")
            .and_then(Value::as_str)
            .and_then(api_path)
            .ok_or_else(invalid_input)?;
        if owner == repository && tail == format!("pulls/{number}") {
            pulls.push(PullNumber::new(number).map_err(|_| invalid_input())?);
        }
    }
    if pulls.is_empty() {
        // A run on no pull request of its repository cannot be routed.
        return Err(Failure::new("forge.not_linked"));
    }
    ForgeSignal::new(
        SignalSource::Check,
        id,
        state,
        commit,
        repository,
        pulls,
        sender,
    )
    .map_err(|_| invalid_input())
}

/// Reads one element of a forge output into its reference.
type ElementReader = fn(&Value) -> Result<ForgeSignal, Failure>;

/// Reads the output of `gh api repos/<o>/<r>/pulls/<n>/reviews` (an array of
/// reviews) or of `gh api repos/<o>/<r>/commits/<sha>/check-runs` (an object
/// with `total_count` and `check_runs`) and keeps the reference of each
/// element. Every text of the input is dropped here.
///
/// # Errors
///
/// `forge.input_unknown` for any other form; `forge.input_invalid` for an
/// element missing a field, holding a state outside the closed vocabulary or
/// a URL outside the forge API, or for more than [`MAX_SIGNALS`] elements;
/// `forge.input_incomplete` for a page of check runs smaller than their total
/// (a later page would be missing); `forge.not_linked` for a check run on no
/// pull request of its repository.
pub fn parse_output(input: &Value) -> Result<Vec<ForgeSignal>, Failure> {
    let (items, read): (&Vec<Value>, ElementReader) = match input {
        Value::Array(items)
            if items.iter().all(|item| {
                item.get("pull_request_url").is_some() && item.get("commit_id").is_some()
            }) =>
        {
            (items, review)
        }
        Value::Object(object) => {
            let (Some(total), Some(Value::Array(items))) = (
                object.get("total_count").and_then(Value::as_u64),
                object.get("check_runs"),
            ) else {
                return Err(unknown_form());
            };
            if u64::try_from(items.len()).ok() != Some(total) {
                return Err(Failure::new("forge.input_incomplete"));
            }
            (items, check_run)
        }
        _ => return Err(unknown_form()),
    };
    if items.len() > MAX_SIGNALS {
        return Err(invalid_input());
    }
    items.iter().map(read).collect()
}

fn signal_json(signal: &ForgeSignal) -> Value {
    json!({
        "source": signal.source.as_str(),
        "id": signal.id,
        "state": signal.state.as_str(),
        "commit": signal.commit.as_str(),
        "repository": signal.repository.as_str(),
        "pulls": signal.pulls.iter().map(|pull| pull.get()).collect::<Vec<_>>(),
        "sender": signal.sender.as_str(),
    })
}

/// The `forge.ingest` request for `mission` built from a forge output: the
/// references only ([`parse_output`]).
///
/// # Errors
///
/// As [`parse_output`].
pub fn ingest_request(mission: &str, input: &Value) -> Result<Value, Failure> {
    let signals = parse_output(input)?;
    Ok(json!({
        "op": "forge.ingest",
        "mission": mission,
        "signals": signals.iter().map(signal_json).collect::<Vec<_>>(),
    }))
}

/// A signal of a request, read strictly: exactly [`SIGNAL_KEYS`], each in its
/// closed form. The client is declarative; the daemon checks again.
fn signal_of(value: &Value) -> Result<ForgeSignal, Failure> {
    let object = value.as_object().ok_or_else(invalid)?;
    if object.len() != SIGNAL_KEYS.len() || !SIGNAL_KEYS.iter().all(|key| object.contains_key(*key))
    {
        return Err(invalid());
    }
    let text = |key: &str| text_of(object, key).ok_or_else(invalid);
    let pulls = object
        .get("pulls")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
        .iter()
        .map(|pull| {
            pull.as_u64()
                .and_then(|pull| PullNumber::new(pull).ok())
                .ok_or_else(invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    ForgeSignal::new(
        SignalSource::parse(text("source")?).map_err(|_| invalid())?,
        forge_number(object.get("id")).ok_or_else(invalid)?,
        SignalState::parse(text("state")?).map_err(|_| invalid())?,
        CommitId::parse(text("commit")?).map_err(|_| invalid())?,
        ForgeRepository::parse(text("repository")?).map_err(|_| invalid())?,
        pulls,
        ForgeLogin::parse(text("sender")?).map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())
}

/// The mission a triggering signal proposes: in the original's repository,
/// depending on it, with the fixed follow-up template, no criterion and the
/// default budgets. Its text is what `would-accept` records and what
/// `accepted` creates.
fn proposal(original: &Mission, signal: &ForgeSignal, pull: PullNumber) -> Result<String, Failure> {
    let template = follow_up(original.id(), signal, pull);
    serde_json::to_string(&json!({
        "repository": original.repository(),
        "title": template.title,
        "brief": template.brief,
        "criteria": [],
        "depends_on": original.id().as_str(),
        "max_duration_seconds": DEFAULT_DURATION_SECONDS,
        "max_output_bytes": DEFAULT_OUTPUT_BYTES,
    }))
    .map_err(|_| Failure::new("journal.invalid"))
}

/// The creation command and the mission it depends on, read back from a
/// proposal.
fn creation(core: &Core, text: &str) -> Result<(Command, MissionId), Failure> {
    let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    let number = |key: &str| value.get(key).and_then(Value::as_u64).ok_or_else(invalid);
    let criteria = value
        .get("criteria")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
        .collect::<Result<Vec<_>, _>>()?;
    let create = Command::Create {
        title: field(&value, "title")?.to_owned(),
        repository: field(&value, "repository")?.to_owned(),
        brief: field(&value, "brief")?.to_owned(),
        criteria,
        budgets: Budgets::new(number("max_duration_seconds")?, number("max_output_bytes")?)?,
        executor: core.config.executor(),
    };
    let on = MissionId::parse(field(&value, "depends_on")?).map_err(|_| invalid())?;
    Ok((create, on))
}

/// Creates the follow-up if it does not exist yet, then declares its
/// dependency on the original if it is not declared yet. Idempotent, so the
/// same steps finish an acceptance a crash interrupted.
fn complete(core: &mut Core, follow_up: &MissionId, text: &str) -> Result<(), Failure> {
    let (create, on) = creation(core, text)?;
    if core.supervisor.mission(follow_up)?.is_none() {
        core.execute(follow_up, &create)?;
    }
    fault::hit("forge-follow-up-created");
    let declared = core
        .supervisor
        .store()
        .dependency_edges()?
        .iter()
        .any(|(mission, target)| mission == follow_up && *target == on);
    if !declared {
        core.supervisor
            .declare_dependency(follow_up, &on, &Actor::Owner, clock::now()?)?;
    }
    Ok(())
}

/// Finishes every accepted signal whose follow-up a crash left without its
/// mission or its dependency. A failure is counted, never propagated: it must
/// not keep the daemon from starting.
pub(crate) fn recover(core: &mut Core, recovered: &mut Recovered) -> Result<(), Failure> {
    for (original, follow_up, text) in core.supervisor.store().forge_follow_ups()? {
        let created = core.supervisor.mission(&follow_up)?.is_some();
        let declared = core
            .supervisor
            .store()
            .dependency_edges()?
            .iter()
            .any(|(mission, target)| *mission == follow_up && *target == original);
        if created && declared {
            continue;
        }
        if complete(core, &follow_up, &text).is_ok() {
            recovered.forge_follow_ups_completed += 1;
        } else {
            recovered.forge_follow_ups_failed += 1;
        }
    }
    Ok(())
}

/// What `mission.show` adds: the linked pull requests and the recorded signals.
pub(crate) fn mission_json(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    let store = core.supervisor.store();
    let links: Vec<Value> = store
        .forge_links(mission.id())?
        .iter()
        .map(|(repository, pull)| json!({ "repository": repository.as_str(), "pull": pull.get() }))
        .collect();
    let signals: Vec<Value> = store
        .forge_signals_of(mission.id())?
        .into_iter()
        .map(|row| {
            json!({
                "repository": row.repository, "pull": row.pull, "source": row.source,
                "signal": row.signal, "commit": row.commit, "state": row.state,
                "sender": row.sender, "reason": row.reason,
                "outcome": row.outcome, "follow_up": row.follow_up, "at": row.at,
            })
        })
        .collect();
    Ok(json!({ "links": links, "signals": signals }))
}

fn require_owner(request: &Value) -> Result<(), Failure> {
    if actor(request)? == Actor::Owner {
        Ok(())
    } else {
        Err(Failure::new("actor.owner_only"))
    }
}

/// Executes one forge request.
pub(crate) fn handle(shared: &Shared, op: &str, request: &Value) -> Result<Value, Failure> {
    match op {
        "forge.link" => {
            let mission = mission_field(request)?;
            let repository =
                ForgeRepository::parse(field(request, "repository")?).map_err(|_| invalid())?;
            let pull = request
                .get("pull")
                .and_then(Value::as_u64)
                .and_then(|pull| PullNumber::new(pull).ok())
                .ok_or_else(invalid)?;
            let actor = actor(request)?;
            lock(shared)?.supervisor.link_forge(
                &mission,
                &repository,
                pull,
                &actor,
                clock::now()?,
            )?;
            Ok(json!({ "repository": repository.as_str(), "pull": pull.get() }))
        }
        "forge.ingest" => ingest(shared, request),
        _ => Err(Failure::new("request.unknown_op")),
    }
}

/// `forge.ingest`: `mission`, `signals`. Every signal must concern a pull
/// request linked to the mission, or nothing is recorded; a signal recorded
/// under the same outcome before is counted as a duplicate and skipped.
fn ingest(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    require_owner(request)?;
    let mission = mission_field(request)?;
    let signals = match request.get("signals") {
        Some(Value::Array(items)) if items.len() <= MAX_SIGNALS => {
            items.iter().map(signal_of).collect::<Result<Vec<_>, _>>()?
        }
        _ => return Err(invalid()),
    };
    let mut core = lock(shared)?;
    let original = core.mission(&mission)?;
    let links = core.supervisor.store().forge_links(&mission)?;
    let pulls = signals
        .iter()
        .map(|signal| route(signal, &links))
        .collect::<Result<Vec<_>, _>>()
        .map_err(SupervisorError::from)?;
    let mode = core.config.forge_mode();
    let senders = core.config.forge_senders().to_vec();
    let mut counts: Map<String, Value> = SignalOutcome::ALL
        .iter()
        .map(|outcome| (outcome.as_str().to_owned(), Value::from(0_u64)))
        .collect();
    let mut duplicates = 0_u64;
    let mut follow_ups = Vec::new();
    for (signal, pull) in signals.iter().zip(pulls) {
        // The login is compared here and goes no further: what is recorded
        // is the entry that admitted it, or a closed reason.
        let admission = admit(signal, &senders);
        let decided = outcome(signal, &admission, mode);
        if core
            .supervisor
            .store()
            .forge_signal_recorded(&mission, signal, decided)?
        {
            duplicates += 1;
            continue;
        }
        let proposed = match decided {
            SignalOutcome::WouldAccept | SignalOutcome::Accepted => {
                Some(proposal(&original, signal, pull)?)
            }
            SignalOutcome::Ignored | SignalOutcome::Observed => None,
        };
        let follow_up = if decided == SignalOutcome::Accepted {
            let id = MissionId::from_bytes(random_bytes()?);
            // The mission is validated before the intent is written: an
            // accepted signal never names a mission that cannot exist.
            let (create, _) = creation(&core, proposed.as_deref().ok_or_else(invalid)?)?;
            work_supervision_domain::decide(&id, None, &create, 0)?;
            Some(id)
        } else {
            None
        };
        core.supervisor.record_forge_signal(
            &SignalRecord {
                mission: &mission,
                signal,
                pull,
                admission: &admission,
                outcome: decided,
                proposal: proposed.as_deref(),
                follow_up: follow_up.as_ref(),
            },
            clock::now()?,
        )?;
        if let (Some(id), Some(text)) = (&follow_up, &proposed) {
            fault::hit("forge-follow-up-intended");
            complete(&mut core, id, text)?;
            follow_ups.push(Value::String(id.as_str().to_owned()));
        }
        if let Some(count) = counts.get_mut(decided.as_str()) {
            *count = Value::from(count.as_u64().unwrap_or(0) + 1);
        }
    }
    Ok(json!({
        "mode": mode.as_str(),
        "signals": signals.len(),
        "outcomes": counts,
        "duplicates": duplicates,
        "follow_ups": follow_ups,
    }))
}

#[cfg(test)]
mod tests;
