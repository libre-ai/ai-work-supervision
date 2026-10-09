//! Write path and reads of the coordination primitives
//! (`docs/work-supervision/coordination-v0.md`).
//!
//! Same order as [`Supervisor::execute`]: the projection gives the current
//! value, the pure domain decides, the texts become blobs, the event is
//! appended to the journal, and only the durable entry reaches the projection.

use rusqlite::{OptionalExtension as _, params};
use serde_json::{Map, Value};
use work_supervision_domain::coordination::{
    Actor, CheckOutcome, CoordinationRefusal, IdeaCommand, IdeaEvent, IdeaState, IdeaView,
    RequestCommand, RequestEvent, RequestState, RequestView, SessionCommand, SessionEvent,
    SessionState, check_argv, check_dependency, check_draft, decide_idea, decide_request,
    decide_session,
};
use work_supervision_domain::scope::{Scope, ScopePath, parse_scope};
use work_supervision_domain::{
    CheckId, CommitId, Digest32, IdeaId, MissionId, RequestId, SessionId, State,
};
use work_supervision_journal::{Entry, Event, Timestamp};

use crate::{BlobStore, Store, StoreError, Supervisor, SupervisorError};

impl From<CoordinationRefusal> for SupervisorError {
    fn from(refusal: CoordinationRefusal) -> Self {
        Self::Coordination(refusal)
    }
}

fn put(blobs: &BlobStore, text: &str) -> Result<Value, StoreError> {
    Ok(Value::String(blobs.put_text(text)?.to_hex()))
}

fn put_optional(blobs: &BlobStore, text: Option<&String>) -> Result<Value, StoreError> {
    text.map_or(Ok(Value::Null), |text| put(blobs, text))
}

fn put_json(blobs: &BlobStore, values: &[&str]) -> Result<Value, StoreError> {
    let text = serde_json::to_string(values).map_err(|_| StoreError::Sqlite)?;
    put(blobs, &text)
}

fn string(text: &str) -> Value {
    Value::String(text.to_owned())
}

fn optional<T: AsRef<str>>(value: Option<T>) -> Value {
    value.map_or(Value::Null, |value| string(value.as_ref()))
}

fn journal_event(kind: &str, data: Map<String, Value>) -> Result<Event, SupervisorError> {
    Ok(Event::new(kind, data)?)
}

fn index(value: usize) -> Value {
    Value::from(u64::try_from(value).unwrap_or(u64::MAX))
}

/// Encodes an idea event (`idea.*`).
fn encode_idea(event: &IdeaEvent, blobs: &BlobStore) -> Result<Event, SupervisorError> {
    let mut data = Map::new();
    match event {
        IdeaEvent::Captured { idea, text, actor } => {
            data.insert("idea".to_owned(), string(idea.as_str()));
            data.insert("text_digest".to_owned(), put(blobs, text)?);
            data.insert("actor".to_owned(), Value::String(actor.to_text()));
        }
        IdeaEvent::Qualified {
            idea,
            repository,
            mission,
            context,
            actor,
        } => {
            data.insert("idea".to_owned(), string(idea.as_str()));
            data.insert("repository".to_owned(), optional(repository.as_deref()));
            data.insert(
                "mission".to_owned(),
                optional(mission.as_ref().map(MissionId::as_str)),
            );
            data.insert(
                "context_digest".to_owned(),
                put_optional(blobs, context.as_ref())?,
            );
            data.insert("actor".to_owned(), Value::String(actor.to_text()));
        }
        IdeaEvent::PromotionIntended { idea, mission } | IdeaEvent::Promoted { idea, mission } => {
            data.insert("idea".to_owned(), string(idea.as_str()));
            data.insert("mission".to_owned(), string(mission.as_str()));
        }
        IdeaEvent::PromotionAborted { idea } => {
            data.insert("idea".to_owned(), string(idea.as_str()));
        }
        IdeaEvent::Dismissed { idea, reason } => {
            data.insert("idea".to_owned(), string(idea.as_str()));
            data.insert("reason_digest".to_owned(), put(blobs, reason)?);
        }
    }
    journal_event(event.kind(), data)
}

fn encode_request(event: &RequestEvent, blobs: &BlobStore) -> Result<Event, SupervisorError> {
    let mut data = Map::new();
    match event {
        RequestEvent::Opened {
            request,
            mission,
            question,
            options,
            recommended,
            actor,
        } => {
            data.insert("request".to_owned(), string(request.as_str()));
            data.insert(
                "mission".to_owned(),
                optional(mission.as_ref().map(MissionId::as_str)),
            );
            data.insert("question_digest".to_owned(), put(blobs, question)?);
            let encoded = options
                .iter()
                .map(|option| {
                    let mut entry = Map::new();
                    entry.insert("label_digest".to_owned(), put(blobs, &option.label)?);
                    entry.insert(
                        "consequence_digest".to_owned(),
                        put(blobs, &option.consequence)?,
                    );
                    entry.insert(
                        "reversibility".to_owned(),
                        string(option.reversibility.as_str()),
                    );
                    Ok(Value::Object(entry))
                })
                .collect::<Result<Vec<_>, StoreError>>()?;
            data.insert("options".to_owned(), Value::Array(encoded));
            data.insert(
                "recommended".to_owned(),
                recommended.map_or(Value::Null, index),
            );
            data.insert("actor".to_owned(), Value::String(actor.to_text()));
        }
        RequestEvent::Answered {
            request,
            choice,
            reason,
        } => {
            data.insert("request".to_owned(), string(request.as_str()));
            data.insert("choice".to_owned(), index(*choice));
            data.insert("reason_digest".to_owned(), put(blobs, reason)?);
        }
        RequestEvent::Withdrawn {
            request,
            reason,
            actor,
        } => {
            data.insert("request".to_owned(), string(request.as_str()));
            data.insert("reason_digest".to_owned(), put(blobs, reason)?);
            data.insert("actor".to_owned(), Value::String(actor.to_text()));
        }
    }
    journal_event(event.kind(), data)
}

fn encode_session(event: &SessionEvent, blobs: &BlobStore) -> Result<Event, SupervisorError> {
    let mut data = Map::new();
    match event {
        SessionEvent::Registered {
            session,
            harness,
            repository,
            mission,
            label,
            external,
        } => {
            data.insert("session".to_owned(), string(session.as_str()));
            data.insert("harness".to_owned(), string(harness.as_str()));
            data.insert("repository".to_owned(), optional(repository.as_deref()));
            data.insert(
                "mission".to_owned(),
                optional(mission.as_ref().map(MissionId::as_str)),
            );
            data.insert("label_digest".to_owned(), put(blobs, label)?);
            data.insert(
                "external_digest".to_owned(),
                optional(external.as_ref().map(Digest32::to_hex)),
            );
        }
        SessionEvent::Reported {
            session,
            state,
            note,
        } => {
            data.insert("session".to_owned(), string(session.as_str()));
            data.insert("state".to_owned(), string(state.as_str()));
            data.insert(
                "note_digest".to_owned(),
                put_optional(blobs, note.as_ref())?,
            );
        }
        SessionEvent::Ended {
            session,
            outcome,
            summary,
        } => {
            data.insert("session".to_owned(), string(session.as_str()));
            data.insert("outcome".to_owned(), string(outcome.as_str()));
            data.insert(
                "summary_digest".to_owned(),
                put_optional(blobs, summary.as_ref())?,
            );
        }
    }
    journal_event(event.kind(), data)
}

/// Outcome of one finished check execution, as the run machinery reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckExit {
    /// Output bytes.
    pub bytes: u64,
    /// Digest of the whole output.
    pub digest: String,
    /// Exit code, when the leader exited by itself.
    pub exit_code: Option<i64>,
    /// Signal number, when it was terminated by one.
    pub signal: Option<i64>,
    /// Overrun budget, `duration` or `output`.
    pub budget: Option<&'static str>,
}

impl Supervisor {
    /// Decides and records an idea command.
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Coordination`] when the domain refuses; store and journal failures.
    pub fn idea(
        &mut self,
        id: &IdeaId,
        command: &IdeaCommand,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<IdeaEvent, SupervisorError> {
        self.require_actor(actor)?;
        let current = self.store().idea_view(id)?;
        let event = decide_idea(id, current.as_ref(), command, actor)?;
        let encoded = encode_idea(&event, self.blobs())?;
        self.append(at, encoded)?;
        Ok(event)
    }

    /// Decides and records a decision-request command.
    ///
    /// # Errors
    ///
    /// As [`Supervisor::idea`].
    pub fn request(
        &mut self,
        id: &RequestId,
        command: &RequestCommand,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<RequestEvent, SupervisorError> {
        self.require_actor(actor)?;
        let current = self.store().request_view(id)?;
        let event = decide_request(id, current.as_ref(), command, actor)?;
        let encoded = encode_request(&event, self.blobs())?;
        self.append(at, encoded)?;
        Ok(event)
    }

    /// Decides and records a session command.
    ///
    /// # Errors
    ///
    /// As [`Supervisor::idea`].
    pub fn session(
        &mut self,
        id: &SessionId,
        command: &SessionCommand,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<SessionEvent, SupervisorError> {
        let current = self.store().session_state(id)?;
        let mission_exists = match command {
            SessionCommand::Register {
                mission: Some(mission),
                ..
            } => self.store().mission(mission)?.is_some(),
            _ => false,
        };
        let event = decide_session(id, current, command, actor, mission_exists)?;
        let encoded = encode_session(&event, self.blobs())?;
        self.append(at, encoded)?;
        Ok(event)
    }

    /// A session actor must name an active session.
    fn require_actor(&self, actor: &Actor) -> Result<(), SupervisorError> {
        let state = match actor {
            Actor::Owner => None,
            Actor::Session(id) => self.store().session_state(id)?,
        };
        Ok(work_supervision_domain::coordination::check_actor_session(
            actor, state,
        )?)
    }

    fn require_draft(&self, mission: &MissionId, actor: &Actor) -> Result<(), SupervisorError> {
        if *actor != Actor::Owner {
            return Err(CoordinationRefusal::OwnerOnly.into());
        }
        let state = self
            .store()
            .mission(mission)?
            .map(|mission| mission.state());
        Ok(check_draft(state)?)
    }

    /// Declares that `mission` waits for `on` (owner, mission in `draft`).
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Coordination`]; store and journal failures.
    pub fn declare_dependency(
        &mut self,
        mission: &MissionId,
        on: &MissionId,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        self.require_draft(mission, actor)?;
        let on_exists = self.store().mission(on)?.is_some();
        check_dependency(mission, on, on_exists, &self.store().dependency_edges()?)?;
        let mut data = Map::new();
        data.insert("mission".to_owned(), string(mission.as_str()));
        data.insert("on".to_owned(), string(on.as_str()));
        self.append(at, journal_event("dependency.declared", data)?)?;
        Ok(())
    }

    /// Declares the path scope of `mission` (owner, mission in `draft`, once).
    ///
    /// # Errors
    ///
    /// As [`Supervisor::declare_dependency`].
    pub fn declare_scope(
        &mut self,
        mission: &MissionId,
        paths: &[String],
        actor: &Actor,
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        self.require_draft(mission, actor)?;
        if self.store().scope(mission)?.is_some() {
            return Err(CoordinationRefusal::ScopeAlreadyDeclared.into());
        }
        let parsed = parse_scope(paths)?;
        let names: Vec<&str> = parsed.iter().map(ScopePath::as_str).collect();
        let mut data = Map::new();
        data.insert("mission".to_owned(), string(mission.as_str()));
        data.insert("paths_digest".to_owned(), put_json(self.blobs(), &names)?);
        self.append(at, journal_event("scope.declared", data)?)?;
        Ok(())
    }

    /// Declares the check of criterion `criterion` of `mission` (owner, `draft`).
    ///
    /// # Errors
    ///
    /// As [`Supervisor::declare_dependency`], plus
    /// [`CoordinationRefusal::CheckCriterionUnknown`] and
    /// [`CoordinationRefusal::CheckAlreadyDeclared`].
    pub fn declare_check(
        &mut self,
        mission: &MissionId,
        criterion: usize,
        argv: &[String],
        actor: &Actor,
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        self.require_draft(mission, actor)?;
        let criteria = self
            .store()
            .mission(mission)?
            .map_or(0, |mission| mission.criteria().len());
        if criterion >= criteria {
            return Err(CoordinationRefusal::CheckCriterionUnknown.into());
        }
        if self
            .store()
            .criterion_checks(mission)?
            .iter()
            .any(|check| check.criterion == criterion)
        {
            return Err(CoordinationRefusal::CheckAlreadyDeclared.into());
        }
        check_argv(argv)?;
        let words: Vec<&str> = argv.iter().map(String::as_str).collect();
        let mut data = Map::new();
        data.insert("mission".to_owned(), string(mission.as_str()));
        data.insert("criterion".to_owned(), index(criterion));
        data.insert("argv_digest".to_owned(), put_json(self.blobs(), &words)?);
        self.append(at, journal_event("check.declared", data)?)?;
        Ok(())
    }

    /// Records the scope check of `mission` at `commit`: `changed` paths, of
    /// which `outside` lie outside the scope.
    ///
    /// # Errors
    ///
    /// Store and journal failures.
    pub fn record_scope_check(
        &mut self,
        mission: &MissionId,
        commit: &CommitId,
        changed: usize,
        outside: &[&str],
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        let mut data = Map::new();
        data.insert("mission".to_owned(), string(mission.as_str()));
        data.insert("commit".to_owned(), string(commit.as_str()));
        data.insert("changed".to_owned(), index(changed));
        data.insert("outside".to_owned(), index(outside.len()));
        data.insert(
            "outside_digest".to_owned(),
            put_json(self.blobs(), outside)?,
        );
        self.append(at, journal_event("scope.checked", data)?)?;
        Ok(())
    }

    /// Records the start of a check execution.
    ///
    /// # Errors
    ///
    /// Store and journal failures.
    pub fn check_started(
        &mut self,
        mission: &MissionId,
        check: &CheckId,
        criterion: usize,
        commit: &CommitId,
        argv_digest: &str,
        at: Timestamp,
    ) -> Result<Entry, SupervisorError> {
        let mut data = Map::new();
        data.insert("mission".to_owned(), string(mission.as_str()));
        data.insert("check".to_owned(), string(check.as_str()));
        data.insert("criterion".to_owned(), index(criterion));
        data.insert("commit".to_owned(), string(commit.as_str()));
        data.insert("argv_digest".to_owned(), string(argv_digest));
        self.append(at, journal_event("check.started", data)?)
    }

    /// Records the end of a check execution.
    ///
    /// # Errors
    ///
    /// Store and journal failures.
    pub fn check_finished(
        &mut self,
        mission: &MissionId,
        check: &CheckId,
        exit: &CheckExit,
        at: Timestamp,
    ) -> Result<Entry, SupervisorError> {
        let mut data = Map::new();
        data.insert("mission".to_owned(), string(mission.as_str()));
        data.insert("check".to_owned(), string(check.as_str()));
        data.insert("bytes".to_owned(), Value::from(exit.bytes));
        data.insert("digest".to_owned(), string(&exit.digest));
        data.insert(
            "exit_code".to_owned(),
            exit.exit_code.map_or(Value::Null, Value::from),
        );
        data.insert(
            "signal".to_owned(),
            exit.signal.map_or(Value::Null, Value::from),
        );
        data.insert("budget".to_owned(), optional(exit.budget));
        self.append(at, journal_event("check.finished", data)?)
    }

    /// Records that a check execution died with its daemon.
    ///
    /// # Errors
    ///
    /// Store and journal failures.
    pub fn check_interrupted(
        &mut self,
        mission: &str,
        check: &str,
        at: Timestamp,
    ) -> Result<Entry, SupervisorError> {
        let mut data = Map::new();
        data.insert("mission".to_owned(), string(mission));
        data.insert("check".to_owned(), string(check));
        self.append(at, journal_event("check.interrupted", data)?)
    }
}

// ------------------------------------------------------------------ reads ---

/// A projected idea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeaRow {
    /// Identifier.
    pub id: String,
    /// State.
    pub state: String,
    /// Text.
    pub text: String,
    /// Who captured it (`owner`, `session:<id>`).
    pub captured_by: String,
    /// Repository it was attached to.
    pub repository: Option<String>,
    /// Mission it was attached to.
    pub mission: Option<String>,
    /// Context.
    pub context: Option<String>,
    /// Mission it became.
    pub promoted_mission: Option<String>,
    /// Pending promotion's mission.
    pub promoting_mission: Option<String>,
    /// Why it was dismissed.
    pub dismiss_reason: Option<String>,
    /// Capture instant.
    pub created_at: String,
}

const IDEA_COLUMNS: &str = "id, state, text, captured_by, repository, mission_id, context,
    promoted_mission, promoting_mission, dismiss_reason, created_at";

impl IdeaRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            state: row.get(1)?,
            text: row.get(2)?,
            captured_by: row.get(3)?,
            repository: row.get(4)?,
            mission: row.get(5)?,
            context: row.get(6)?,
            promoted_mission: row.get(7)?,
            promoting_mission: row.get(8)?,
            dismiss_reason: row.get(9)?,
            created_at: row.get(10)?,
        })
    }
}

/// One option of a projected request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionRow {
    /// Label.
    pub label: String,
    /// Consequence.
    pub consequence: String,
    /// `reversible`, `costly` or `irreversible`.
    pub reversibility: String,
}

/// A projected decision request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRow {
    /// Identifier.
    pub id: String,
    /// Mission it conditions.
    pub mission: Option<String>,
    /// `open`, `answered` or `withdrawn`.
    pub state: String,
    /// Question.
    pub question: String,
    /// Options, in order.
    pub options: Vec<OptionRow>,
    /// Recommended option.
    pub recommended: Option<i64>,
    /// Opener (`owner`, `session:<id>`).
    pub opened_by: String,
    /// Chosen option.
    pub choice: Option<i64>,
    /// Reason of the answer or the withdrawal.
    pub reason: Option<String>,
    /// Who closed it.
    pub closed_by: Option<String>,
    /// Opening instant.
    pub created_at: String,
}

/// A projected agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// Identifier.
    pub id: String,
    /// Harness.
    pub harness: String,
    /// `active` or `ended`.
    pub state: String,
    /// Repository.
    pub repository: Option<String>,
    /// Mission.
    pub mission: Option<String>,
    /// Label.
    pub label: String,
    /// Last reported state.
    pub reported_state: Option<String>,
    /// Last note.
    pub note: Option<String>,
    /// Outcome, once ended.
    pub outcome: Option<String>,
    /// Summary, once ended.
    pub summary: Option<String>,
    /// Registration instant.
    pub created_at: String,
    /// Instant of the last event.
    pub updated_at: String,
}

const SESSION_COLUMNS: &str = "id, harness, state, repository, mission_id, label, reported_state,
    note, outcome, summary, created_at, updated_at";

impl SessionRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            harness: row.get(1)?,
            state: row.get(2)?,
            repository: row.get(3)?,
            mission: row.get(4)?,
            label: row.get(5)?,
            reported_state: row.get(6)?,
            note: row.get(7)?,
            outcome: row.get(8)?,
            summary: row.get(9)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
        })
    }
}

/// A declared criterion check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionCheck {
    /// Criterion index.
    pub criterion: usize,
    /// Argument vector.
    pub argv: Vec<String>,
    /// Digest of the argument vector's blob.
    pub argv_digest: String,
}

/// A projected check execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRunRow {
    /// Identifier.
    pub check: String,
    /// Criterion index.
    pub criterion: i64,
    /// Commit it ran at.
    pub commit: String,
    /// `running`, `finished` or `interrupted`.
    pub state: String,
    /// Output bytes.
    pub output_bytes: Option<i64>,
    /// Exit code.
    pub exit_code: Option<i64>,
    /// Signal.
    pub signal: Option<i64>,
    /// Overrun budget.
    pub budget: Option<String>,
}

impl CheckRunRow {
    /// Whether the execution passed: finished, exit 0, no signal, no overrun.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.state == "finished"
            && self.exit_code == Some(0)
            && self.signal.is_none()
            && self.budget.is_none()
    }
}

/// A projected scope check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeCheckRow {
    /// Commit.
    pub commit: String,
    /// Changed paths.
    pub changed: i64,
    /// Paths outside the scope.
    pub outside: Vec<String>,
}

/// One entry of a mission's timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRow {
    /// Journal sequence number.
    pub seq: i64,
    /// Event kind.
    pub kind: String,
    /// Instant.
    pub at: String,
}

fn parse_state(text: &str) -> Result<State, StoreError> {
    State::parse(text).ok_or(StoreError::Sqlite)
}

impl Store {
    /// The idea `id` as its rules need it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn idea_view(&self, id: &IdeaId) -> Result<Option<IdeaView>, StoreError> {
        let row: Option<(String, i64, Option<String>)> = self
            .connection()
            .query_row(
                "SELECT state, ever_qualified, promoting_mission FROM ideas WHERE id = ?1",
                [id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        row.map(|(state, ever_qualified, promoting)| {
            Ok(IdeaView {
                state: IdeaState::parse(&state).map_err(|_| StoreError::Sqlite)?,
                ever_qualified: ever_qualified != 0,
                promoting: promoting
                    .map(|id| MissionId::parse(&id).map_err(|_| StoreError::Sqlite))
                    .transpose()?,
            })
        })
        .transpose()
    }

    /// Every idea, newest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn ideas(&self) -> Result<Vec<IdeaRow>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {IDEA_COLUMNS} FROM ideas ORDER BY created_seq DESC"
        ))?;
        let rows = statement
            .query_map([], IdeaRow::read)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Ideas left `promoting` (recovery).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn promoting_ideas(&self) -> Result<Vec<IdeaRow>, StoreError> {
        Ok(self
            .ideas()?
            .into_iter()
            .filter(|idea| idea.state == "promoting")
            .collect())
    }

    /// The request `id` as its rules need it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn request_view(&self, id: &RequestId) -> Result<Option<RequestView>, StoreError> {
        let Some(row) = self.request(id.as_str())? else {
            return Ok(None);
        };
        Ok(Some(RequestView {
            state: RequestState::parse(&row.state).map_err(|_| StoreError::Sqlite)?,
            opener: Actor::parse(&row.opened_by).map_err(|_| StoreError::Sqlite)?,
            options: row.options.len(),
        }))
    }

    /// One projected request.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn request(&self, id: &str) -> Result<Option<RequestRow>, StoreError> {
        Ok(self.requests_where("id = ?1", id)?.into_iter().next())
    }

    /// Every request, newest first; `open_only` keeps the open ones.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn requests(&self, open_only: bool) -> Result<Vec<RequestRow>, StoreError> {
        if open_only {
            self.requests_where("state = ?1", "open")
        } else {
            self.requests_where("?1 = ?1", "")
        }
    }

    /// Requests attached to `mission`, newest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn requests_of(&self, mission: &str) -> Result<Vec<RequestRow>, StoreError> {
        self.requests_where("mission_id = ?1", mission)
    }

    fn requests_where(&self, filter: &str, value: &str) -> Result<Vec<RequestRow>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT id, mission_id, state, question, recommended, opened_by, choice, reason,
               closed_by, created_at FROM requests WHERE {filter} ORDER BY created_seq DESC"
        ))?;
        let mut rows = statement
            .query_map([value], |row| {
                Ok(RequestRow {
                    id: row.get(0)?,
                    mission: row.get(1)?,
                    state: row.get(2)?,
                    question: row.get(3)?,
                    options: Vec::new(),
                    recommended: row.get(4)?,
                    opened_by: row.get(5)?,
                    choice: row.get(6)?,
                    reason: row.get(7)?,
                    closed_by: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut options = self.connection().prepare(
            "SELECT label, consequence, reversibility FROM request_options
             WHERE request_id = ?1 ORDER BY position",
        )?;
        for row in &mut rows {
            row.options = options
                .query_map([row.id.as_str()], |option| {
                    Ok(OptionRow {
                        label: option.get(0)?,
                        consequence: option.get(1)?,
                        reversibility: option.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }
        Ok(rows)
    }

    /// Open requests attached to `mission`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn open_requests_of(&self, mission: &MissionId) -> Result<Vec<RequestId>, StoreError> {
        self.requests_of(mission.as_str())?
            .into_iter()
            .filter(|row| row.state == "open")
            .map(|row| RequestId::parse(&row.id).map_err(|_| StoreError::Sqlite))
            .collect()
    }

    /// State of the session `id`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn session_state(&self, id: &SessionId) -> Result<Option<SessionState>, StoreError> {
        let state: Option<String> = self
            .connection()
            .query_row(
                "SELECT state FROM sessions WHERE id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        state
            .map(|state| SessionState::parse(&state).map_err(|_| StoreError::Sqlite))
            .transpose()
    }

    /// Every session, newest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn sessions(&self) -> Result<Vec<SessionRow>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions ORDER BY created_seq DESC"
        ))?;
        let rows = statement
            .query_map([], SessionRow::read)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The active session registered with the harness identifier digest `external`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn session_by_external(
        &self,
        harness: &str,
        external: &Digest32,
    ) -> Result<Option<SessionRow>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions
             WHERE harness = ?1 AND external_digest = ?2 AND state = 'active'
             ORDER BY created_seq DESC LIMIT 1"
        ))?;
        Ok(statement
            .query_row(params![harness, external.to_hex()], SessionRow::read)
            .optional()?)
    }

    /// Every declared dependency edge, `(mission, on)`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn dependency_edges(&self) -> Result<Vec<(MissionId, MissionId)>, StoreError> {
        let mut statement = self
            .connection()
            .prepare("SELECT mission_id, on_mission FROM mission_dependencies ORDER BY seq")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(from, to)| {
                Ok((
                    MissionId::parse(&from).map_err(|_| StoreError::Sqlite)?,
                    MissionId::parse(&to).map_err(|_| StoreError::Sqlite)?,
                ))
            })
            .collect()
    }

    /// The dependencies of `mission`, with their current state.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn dependencies_of(
        &self,
        mission: &MissionId,
    ) -> Result<Vec<(MissionId, State)>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT d.on_mission, m.state FROM mission_dependencies d
             JOIN missions m ON m.id = d.on_mission WHERE d.mission_id = ?1 ORDER BY d.seq",
        )?;
        let rows = statement
            .query_map([mission.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, state)| {
                Ok((
                    MissionId::parse(&id).map_err(|_| StoreError::Sqlite)?,
                    parse_state(&state)?,
                ))
            })
            .collect()
    }

    /// The declared scope of `mission`; `None` when undeclared (whole repository).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn scope(&self, mission: &MissionId) -> Result<Scope, StoreError> {
        let mut statement = self
            .connection()
            .prepare("SELECT path FROM mission_scopes WHERE mission_id = ?1 ORDER BY position")?;
        let paths = statement
            .query_map([mission.as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if paths.is_empty() {
            return Ok(None);
        }
        paths
            .iter()
            .map(|path| ScopePath::parse(path).map_err(|_| StoreError::Sqlite))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// The declared checks of `mission`, by criterion.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn criterion_checks(&self, mission: &MissionId) -> Result<Vec<CriterionCheck>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT criterion, argv, argv_digest FROM criterion_checks
             WHERE mission_id = ?1 ORDER BY criterion",
        )?;
        let rows = statement
            .query_map([mission.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(criterion, argv, argv_digest)| {
                Ok(CriterionCheck {
                    criterion: usize::try_from(criterion).map_err(|_| StoreError::Sqlite)?,
                    argv: serde_json::from_str(&argv).map_err(|_| StoreError::Sqlite)?,
                    argv_digest,
                })
            })
            .collect()
    }

    /// Every check execution of `mission`, oldest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn check_runs(&self, mission: &MissionId) -> Result<Vec<CheckRunRow>, StoreError> {
        self.check_runs_where("mission_id = ?1", mission.as_str())
    }

    /// Check executions still `running` (recovery).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn running_checks(&self) -> Result<Vec<(String, String)>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT mission_id, check_id FROM check_runs WHERE state = 'running' ORDER BY started_seq",
        )?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn check_runs_where(&self, filter: &str, value: &str) -> Result<Vec<CheckRunRow>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT check_id, criterion, commit_id, state, output_bytes, exit_code, signal, budget
             FROM check_runs WHERE {filter} ORDER BY started_seq"
        ))?;
        let rows = statement
            .query_map([value], |row| {
                Ok(CheckRunRow {
                    check: row.get(0)?,
                    criterion: row.get(1)?,
                    commit: row.get(2)?,
                    state: row.get(3)?,
                    output_bytes: row.get(4)?,
                    exit_code: row.get(5)?,
                    signal: row.get(6)?,
                    budget: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The last finished execution of each criterion's check, as outcomes.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn check_outcomes(&self, mission: &MissionId) -> Result<Vec<CheckOutcome>, StoreError> {
        let mut last: std::collections::BTreeMap<i64, CheckRunRow> =
            std::collections::BTreeMap::new();
        for row in self.check_runs(mission)? {
            if row.state == "finished" {
                last.insert(row.criterion, row);
            }
        }
        last.into_values()
            .map(|row| {
                Ok(CheckOutcome {
                    criterion: usize::try_from(row.criterion).map_err(|_| StoreError::Sqlite)?,
                    commit: CommitId::parse(&row.commit).map_err(|_| StoreError::Sqlite)?,
                    passed: row.passed(),
                })
            })
            .collect()
    }

    /// The scope check of `mission` at `commit`, if recorded.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn scope_check(
        &self,
        mission: &MissionId,
        commit: &str,
    ) -> Result<Option<ScopeCheckRow>, StoreError> {
        let row: Option<(i64, String)> = self
            .connection()
            .query_row(
                "SELECT changed, outside_paths FROM scope_checks
                 WHERE mission_id = ?1 AND commit_id = ?2",
                params![mission.as_str(), commit],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(changed, paths)| {
            Ok(ScopeCheckRow {
                commit: commit.to_owned(),
                changed,
                outside: serde_json::from_str(&paths).map_err(|_| StoreError::Sqlite)?,
            })
        })
        .transpose()
    }

    /// The timeline of `mission`: every journal entry that names it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn timeline(&self, mission: &MissionId) -> Result<Vec<TimelineRow>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT seq, kind, at FROM mission_events WHERE mission_id = ?1 ORDER BY seq",
        )?;
        let rows = statement
            .query_map([mission.as_str()], |row| {
                Ok(TimelineRow {
                    seq: row.get(0)?,
                    kind: row.get(1)?,
                    at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
