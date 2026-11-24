//! Write path, projection and reads of forge links and forge signals
//! (`docs/work-supervision/coordination-v0.md`, « Forge signals »).
//!
//! `mission.forge-linked` names a pull request of a mission; it is no
//! transition (no revision, no state), so a terminal mission can be linked and
//! stays terminal. `forge.signal.<outcome>` records one signal reduced to its
//! reference; only `forge.signal.accepted` leads to a mission, which is
//! recorded by its own `mission.created` after it.

use rusqlite::{OptionalExtension as _, Transaction, params};
use serde_json::{Map, Value};
use work_supervision_domain::MissionId;
use work_supervision_domain::coordination::{Actor, CoordinationRefusal};
use work_supervision_domain::forge::{
    Admission, ForgeRepository, ForgeSignal, IgnoreReason, MAX_FORGE_NUMBER, PullNumber,
    SenderEntry, SignalOutcome, SignalSource, SignalState, check_link, route, triggers,
};
use work_supervision_journal::{Event, Timestamp};

use crate::projection::Fields;
use crate::{BlobStore, Store, StoreError, Supervisor, SupervisorError};

/// What [`Supervisor::record_forge_signal`] records.
#[derive(Debug, Clone, Copy)]
pub struct SignalRecord<'a> {
    /// Mission the signal is routed to.
    pub mission: &'a MissionId,
    /// The signal.
    pub signal: &'a ForgeSignal,
    /// The linked pull request it concerns (`route`).
    pub pull: PullNumber,
    /// The entry of the sender list that admitted it, or why it was ignored.
    /// Its sender's login is never recorded.
    pub admission: &'a Admission,
    /// What became of it.
    pub outcome: SignalOutcome,
    /// The proposal (`would-accept`, `accepted`), as text.
    pub proposal: Option<&'a str>,
    /// The follow-up mission to create (`accepted`).
    pub follow_up: Option<&'a MissionId>,
}

/// One recorded signal, as projected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeSignalRow {
    /// Journal sequence number.
    pub seq: i64,
    /// Repository, `owner/name`.
    pub repository: String,
    /// Pull request.
    pub pull: i64,
    /// `review` or `check`.
    pub source: String,
    /// The forge's identifier of the review or check run.
    pub signal: i64,
    /// Commit.
    pub commit: String,
    /// State; `None` when ignored.
    pub state: Option<String>,
    /// The sender-list entry that admitted it; `None` when ignored.
    pub sender: Option<String>,
    /// Why it was ignored; `None` unless ignored.
    pub reason: Option<String>,
    /// `ignored`, `observed`, `would-accept` or `accepted`.
    pub outcome: String,
    /// The proposal text, for `would-accept` and `accepted`.
    pub proposal: Option<String>,
    /// The follow-up mission, for `accepted`.
    pub follow_up: Option<String>,
    /// Instant of the entry.
    pub at: String,
}

fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

fn outcome_invalid() -> SupervisorError {
    CoordinationRefusal::FieldInvalid { field: "outcome" }.into()
}

impl Supervisor {
    /// Links the pull request `pull` of `repository` to `mission` (owner).
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Coordination`] (the refusals of
    /// [`work_supervision_domain::forge::check_link`], and those of a session
    /// actor that is not active); store and journal failures.
    pub fn link_forge(
        &mut self,
        mission: &MissionId,
        repository: &ForgeRepository,
        pull: PullNumber,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        self.require_actor(actor)?;
        let exists = self.store().mission(mission)?.is_some();
        let links = self.store().forge_links(mission)?;
        check_link(actor, exists, &links, repository, pull)?;
        let mut data = Map::new();
        data.insert("mission".to_owned(), text(mission.as_str()));
        data.insert("repository".to_owned(), text(repository.as_str()));
        data.insert("pull".to_owned(), Value::from(pull.get()));
        self.append(at, Event::new("mission.forge-linked", data)?)?;
        Ok(())
    }

    /// Records one signal under its outcome. The signal must concern a pull
    /// request linked to the mission; the outcome must fit the signal (only a
    /// triggering state is proposed or accepted, never merely observed), and a
    /// proposal goes with `would-accept` and `accepted` only, a follow-up — a
    /// mission that does not exist yet — with `accepted` only.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::ForgeNotLinked`],
    /// [`CoordinationRefusal::FieldInvalid`] (`outcome`), store and journal
    /// failures. Nothing is written on a refusal.
    pub fn record_forge_signal(
        &mut self,
        record: &SignalRecord<'_>,
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        let signal = record.signal;
        let links = self.store().forge_links(record.mission)?;
        if route(signal, &links)? != record.pull {
            return Err(CoordinationRefusal::ForgeNotLinked.into());
        }
        let triggering = triggers(signal.source, signal.state);
        let ignored = matches!(record.admission, Admission::Ignored(_));
        if ignored != (record.outcome == SignalOutcome::Ignored) {
            return Err(outcome_invalid());
        }
        let shape_ok = match record.outcome {
            SignalOutcome::Ignored => record.proposal.is_none() && record.follow_up.is_none(),
            SignalOutcome::Observed => {
                !triggering && record.proposal.is_none() && record.follow_up.is_none()
            }
            SignalOutcome::WouldAccept => {
                triggering && record.proposal.is_some() && record.follow_up.is_none()
            }
            SignalOutcome::Accepted => triggering && record.proposal.is_some(),
        };
        if !shape_ok {
            return Err(outcome_invalid());
        }
        if record.outcome == SignalOutcome::Accepted {
            let Some(follow_up) = record.follow_up else {
                return Err(outcome_invalid());
            };
            if self.store().mission(follow_up)?.is_some() {
                return Err(outcome_invalid());
            }
        }
        let mut data = Map::new();
        data.insert("mission".to_owned(), text(record.mission.as_str()));
        data.insert("repository".to_owned(), text(signal.repository.as_str()));
        data.insert("pull".to_owned(), Value::from(record.pull.get()));
        data.insert("source".to_owned(), text(signal.source.as_str()));
        data.insert("signal".to_owned(), Value::from(signal.id));
        data.insert("commit".to_owned(), text(signal.commit.as_str()));
        match record.admission {
            Admission::Ignored(reason) => {
                data.insert("reason".to_owned(), text(reason.as_str()));
            }
            Admission::Admitted(entry) => {
                data.insert("state".to_owned(), text(signal.state.as_str()));
                data.insert("sender".to_owned(), text(entry.as_str()));
            }
        }
        if let Some(proposal) = record.proposal {
            data.insert(
                "proposal_digest".to_owned(),
                Value::String(self.blobs().put_text(proposal)?.to_hex()),
            );
        }
        if let Some(follow_up) = record.follow_up {
            data.insert("follow_up".to_owned(), text(follow_up.as_str()));
        }
        self.append(at, Event::new(record.outcome.kind(), data)?)?;
        Ok(())
    }
}

impl Store {
    /// The pull requests linked to `mission`, in the order they were linked.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn forge_links(
        &self,
        mission: &MissionId,
    ) -> Result<Vec<(ForgeRepository, PullNumber)>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT repository, pull FROM forge_links WHERE mission_id = ?1 ORDER BY seq",
        )?;
        let rows = statement
            .query_map([mission.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(repository, pull)| {
                Ok((
                    ForgeRepository::parse(&repository).map_err(|_| StoreError::Sqlite)?,
                    u64::try_from(pull)
                        .ok()
                        .and_then(|pull| PullNumber::new(pull).ok())
                        .ok_or(StoreError::Sqlite)?,
                ))
            })
            .collect()
    }

    /// Whether `signal` was recorded for `mission` under `outcome` already:
    /// the same repository, source, identifier and — unless ignored, which
    /// keeps no state — state.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn forge_signal_recorded(
        &self,
        mission: &MissionId,
        signal: &ForgeSignal,
        outcome: SignalOutcome,
    ) -> Result<bool, StoreError> {
        let state = (outcome != SignalOutcome::Ignored).then(|| signal.state.as_str());
        let id = i64::try_from(signal.id).map_err(|_| StoreError::Sqlite)?;
        let count: i64 = self.connection().query_row(
            "SELECT count(*) FROM forge_signals WHERE mission_id = ?1 AND repository = ?2
               AND source = ?3 AND signal_id = ?4 AND state IS ?5 AND outcome = ?6",
            params![
                mission.as_str(),
                signal.repository.as_str(),
                signal.source.as_str(),
                id,
                state,
                outcome.as_str()
            ],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// The signals recorded for `mission`, oldest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn forge_signals_of(&self, mission: &MissionId) -> Result<Vec<ForgeSignalRow>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT seq, repository, pull, source, signal_id, head_commit, state, sender, reason,
               outcome, proposal, follow_up, at
             FROM forge_signals WHERE mission_id = ?1 ORDER BY seq",
        )?;
        let rows = statement
            .query_map([mission.as_str()], |row| {
                Ok(ForgeSignalRow {
                    seq: row.get(0)?,
                    repository: row.get(1)?,
                    pull: row.get(2)?,
                    source: row.get(3)?,
                    signal: row.get(4)?,
                    commit: row.get(5)?,
                    state: row.get(6)?,
                    sender: row.get(7)?,
                    reason: row.get(8)?,
                    outcome: row.get(9)?,
                    proposal: row.get(10)?,
                    follow_up: row.get(11)?,
                    at: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every accepted signal: the original mission, the follow-up it names
    /// and the proposal it is created from (recovery).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn forge_follow_ups(&self) -> Result<Vec<(MissionId, MissionId, String)>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT mission_id, follow_up, proposal FROM forge_signals
             WHERE outcome = 'accepted' ORDER BY seq",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(mission, follow_up, proposal)| {
                let parse = |id: &str| MissionId::parse(id).map_err(|_| StoreError::Sqlite);
                Ok((
                    parse(&mission)?,
                    parse(follow_up.as_deref().ok_or(StoreError::Sqlite)?)?,
                    proposal.ok_or(StoreError::Sqlite)?,
                ))
            })
            .collect()
    }
}

/// Keys a `mission.forge-linked` may carry.
const LINK_KEYS: [&str; 3] = ["mission", "repository", "pull"];

/// Keys a `forge.signal.*` may carry; which of them each outcome requires
/// or forbids is checked by [`signal`].
const SIGNAL_KEYS: [&str; 11] = [
    "mission",
    "repository",
    "pull",
    "source",
    "signal",
    "commit",
    "state",
    "sender",
    "reason",
    "proposal_digest",
    "follow_up",
];

/// Refuses a key outside `allowed`: a forge event carries references only,
/// and an extra key is where a forge text or a login would slip in.
fn only_keys(fields: &Fields<'_>, allowed: &[&str]) -> Result<(), StoreError> {
    if fields
        .data
        .keys()
        .all(|key| allowed.contains(&key.as_str()))
    {
        Ok(())
    } else {
        Err(fields.invalid())
    }
}

/// A field holding an integer from 1 to `MAX_FORGE_NUMBER`.
fn forge_number(fields: &Fields<'_>, key: &str) -> Result<i64, StoreError> {
    let value = fields.integer(key)?;
    let in_range = u64::try_from(value).is_ok_and(|value| (1..=MAX_FORGE_NUMBER).contains(&value));
    if in_range {
        Ok(value)
    } else {
        Err(fields.invalid())
    }
}

/// A repository field, in its canonical lowercase `owner/name` form.
fn repository<'a>(fields: &Fields<'a>) -> Result<&'a str, StoreError> {
    let text = fields.string("repository")?;
    match ForgeRepository::parse(text) {
        Ok(parsed) if parsed.as_str() == text => Ok(text),
        _ => Err(fields.invalid()),
    }
}

fn mission_exists(transaction: &Transaction<'_>, mission: &str) -> Result<bool, StoreError> {
    Ok(transaction
        .query_row(
            "SELECT 1 FROM missions WHERE id = ?1",
            [mission],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// `mission.forge-linked`.
pub(crate) fn link(
    transaction: &Transaction<'_>,
    fields: &Fields<'_>,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    only_keys(fields, &LINK_KEYS)?;
    let mission = fields.mission()?;
    let repository = repository(fields)?;
    let pull = forge_number(fields, "pull")?;
    let linked = transaction
        .query_row(
            "SELECT 1 FROM forge_links WHERE mission_id = ?1 AND repository = ?2 AND pull = ?3",
            params![mission, repository, pull],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if linked || !mission_exists(transaction, mission)? {
        return Err(fields.invalid());
    }
    transaction.execute(
        "INSERT INTO forge_links (mission_id, repository, pull, seq, at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![mission, repository, pull, seq, at],
    )?;
    Ok(())
}

/// `forge.signal.*`.
pub(crate) fn signal(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let Some(outcome) = SignalOutcome::ALL
        .iter()
        .copied()
        .find(|outcome| outcome.kind() == kind)
    else {
        return Err(StoreError::UnknownKind { seq: fields.seq });
    };
    only_keys(fields, &SIGNAL_KEYS)?;
    let mission = fields.mission()?;
    let repository = repository(fields)?;
    let pull = forge_number(fields, "pull")?;
    let source = SignalSource::parse(fields.string("source")?).map_err(|_| fields.invalid())?;
    let id = forge_number(fields, "signal")?;
    let commit = fields.commit("commit")?;
    let linked = transaction
        .query_row(
            "SELECT 1 FROM forge_links WHERE mission_id = ?1 AND repository = ?2 AND pull = ?3",
            params![mission, repository, pull],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !linked {
        return Err(fields.invalid());
    }
    let (state, sender, reason) = if outcome == SignalOutcome::Ignored {
        // An ignored signal keeps its reference and a closed reason only:
        // nothing of its sender.
        if fields.data.contains_key("state") || fields.data.contains_key("sender") {
            return Err(fields.invalid());
        }
        let reason = IgnoreReason::parse(fields.string("reason")?).map_err(|_| fields.invalid())?;
        (None, None, Some(reason.as_str()))
    } else {
        if fields.data.contains_key("reason") {
            return Err(fields.invalid());
        }
        let sender = fields.string("sender")?;
        if SenderEntry::parse(sender).is_err() {
            return Err(fields.invalid());
        }
        let state = SignalState::parse(fields.string("state")?).map_err(|_| fields.invalid())?;
        let triggering = triggers(source, state);
        let fits = state.belongs_to(source)
            && match outcome {
                SignalOutcome::Observed => !triggering,
                SignalOutcome::WouldAccept | SignalOutcome::Accepted => triggering,
                SignalOutcome::Ignored => false,
            };
        if !fits {
            return Err(fields.invalid());
        }
        (Some(state.as_str()), Some(sender), None)
    };
    let proposal = match outcome {
        SignalOutcome::WouldAccept | SignalOutcome::Accepted => {
            Some(fields.text("proposal_digest", blobs)?)
        }
        SignalOutcome::Ignored | SignalOutcome::Observed => {
            if fields.data.contains_key("proposal_digest") {
                return Err(fields.invalid());
            }
            None
        }
    };
    let follow_up = if outcome == SignalOutcome::Accepted {
        let follow_up = fields.identifier("follow_up")?;
        // The follow-up is named before it is created: it must not exist yet.
        if follow_up == mission || mission_exists(transaction, follow_up)? {
            return Err(fields.invalid());
        }
        Some(follow_up)
    } else {
        if fields.data.contains_key("follow_up") {
            return Err(fields.invalid());
        }
        None
    };
    let (proposal, proposal_digest) = proposal.unzip();
    transaction.execute(
        "INSERT INTO forge_signals (seq, mission_id, repository, pull, source, signal_id,
           head_commit, state, sender, reason, outcome, proposal, proposal_digest, follow_up, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            seq,
            mission,
            repository,
            pull,
            source.as_str(),
            id,
            commit,
            state,
            sender,
            reason,
            outcome.as_str(),
            proposal,
            proposal_digest,
            follow_up,
            at
        ],
    )?;
    Ok(())
}
