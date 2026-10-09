//! The mission domain wired to the journal and the projection.
//!
//! [`Supervisor::execute`] is the only write path for a mission: it reads the
//! mission from the projection, lets the pure domain decide, writes the texts
//! the event names to the blob store, appends the event to the journal and
//! only then applies the durable entry to the projection. A refused command
//! writes nothing.

use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension as _, params};
use serde_json::{Map, Value};
use work_supervision_domain::{
    Budgets, Command, CommitId, Digest32, ExecutorProfile, Mission, MissionEvent, MissionId,
    MissionParts, MissionResult, Refusal, RunId, State, Verdict, apply, decide,
};
use work_supervision_journal::{Event, Journal, JournalError, OpenMode, Timestamp};

use crate::{BlobStore, Store, StoreError};

/// Paths of a Work Supervision root. The root is always a parameter
/// (`--root` or `WS_ROOT`); nothing is placed outside it.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// Layout of the root `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }

    /// The root itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The journal, `journal/journal.v0.jsonl`.
    #[must_use]
    pub fn journal(&self) -> PathBuf {
        self.root.join("journal").join("journal.v0.jsonl")
    }

    /// The projection, `state.sqlite`.
    #[must_use]
    pub fn state(&self) -> PathBuf {
        self.root.join("state.sqlite")
    }

    /// Content-addressed texts, `blobs/`.
    #[must_use]
    pub fn blobs(&self) -> PathBuf {
        self.root.join("blobs")
    }

    /// Content-addressed evidence and archives, `evidence/`.
    #[must_use]
    pub fn evidence(&self) -> PathBuf {
        self.root.join("evidence")
    }

    /// Mission worktrees, `worktrees/<mission>`.
    #[must_use]
    pub fn worktrees(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    /// Run directories, `runs/<run>` (terminal logs).
    #[must_use]
    pub fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// Private configuration, `config.toml`.
    #[must_use]
    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Opens the text blob store.
    ///
    /// # Errors
    ///
    /// [`StoreError::BlobIo`].
    pub fn blob_store(&self) -> Result<BlobStore, StoreError> {
        BlobStore::open(&self.blobs())
    }
}

/// Why the supervisor refused or failed an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorError {
    /// The domain refused the command; nothing was written.
    Refused(Refusal),
    /// The projection or the blob store failed.
    Store(StoreError),
    /// The journal failed.
    Journal(JournalError),
}

impl SupervisorError {
    /// Stable code of the underlying refusal.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Refused(refusal) => refusal.code(),
            Self::Store(error) => error.code(),
            Self::Journal(_) => "journal.invalid",
        }
    }
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Journal(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<StoreError> for SupervisorError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<JournalError> for SupervisorError {
    fn from(error: JournalError) -> Self {
        Self::Journal(error)
    }
}

impl From<Refusal> for SupervisorError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

/// The single writer of a root: journal, projection and blobs together.
#[derive(Debug)]
pub struct Supervisor {
    journal: Journal,
    store: Store,
    blobs: BlobStore,
}

impl Supervisor {
    /// Opens the root: journal (exclusive lock, full verification), blobs,
    /// projection, then catches the projection up with the journal.
    ///
    /// # Errors
    ///
    /// Any journal refusal (a torn tail in [`OpenMode::Strict`], a held
    /// lock), and [`Store::catch_up`]'s refusals.
    pub fn open(layout: &Layout, mode: OpenMode) -> Result<Self, SupervisorError> {
        let journal_dir = layout.root().join("journal");
        std::fs::create_dir_all(&journal_dir).map_err(|_| JournalError::Io)?;
        let (journal, _) = Journal::open(&layout.journal(), mode)?;
        let blobs = layout.blob_store()?;
        let mut store = Store::open(&layout.state())?;
        store.catch_up(&journal, &blobs)?;
        Ok(Self {
            journal,
            store,
            blobs,
        })
    }

    /// Decides `command` on mission `id` against `expected_revision`, and
    /// records the event: blobs, then journal, then projection.
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Refused`] when the domain refuses (nothing is
    /// written), store and journal failures otherwise.
    pub fn execute(
        &mut self,
        id: &MissionId,
        command: &Command,
        expected_revision: u64,
        at: Timestamp,
    ) -> Result<Mission, SupervisorError> {
        let current = self.store.mission(id)?;
        let event = decide(id, current.as_ref(), command, expected_revision)?;
        let next = apply(current, &event)?;
        let encoded = encode(&event, &next, &self.blobs)?;
        self.append(at, encoded)?;
        Ok(next)
    }

    /// Appends an event that is not a mission transition (run, worktree,
    /// journal events of later tranches) and applies it to the projection.
    ///
    /// # Errors
    ///
    /// Journal and projection failures.
    pub fn append(
        &mut self,
        at: Timestamp,
        event: Event,
    ) -> Result<work_supervision_journal::Entry, SupervisorError> {
        let entry = self.journal.append_entry(at, event)?;
        self.store.apply(&entry, &self.blobs)?;
        Ok(entry)
    }

    /// The mission `id` as projected.
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Store`].
    pub fn mission(&self, id: &MissionId) -> Result<Option<Mission>, SupervisorError> {
        Ok(self.store.mission(id)?)
    }

    /// Every mission, in creation order.
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Store`].
    pub fn missions(&self) -> Result<Vec<Mission>, SupervisorError> {
        Ok(self.store.missions()?)
    }

    /// Number of entries in the journal.
    #[must_use]
    pub const fn journal_entries(&self) -> u64 {
        self.journal.entries()
    }

    /// The open journal.
    #[must_use]
    pub const fn journal(&self) -> &Journal {
        &self.journal
    }

    /// The projection.
    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    /// The text blob store.
    #[must_use]
    pub const fn blobs(&self) -> &BlobStore {
        &self.blobs
    }
}

fn put(blobs: &BlobStore, text: &str) -> Result<Value, StoreError> {
    Ok(Value::String(blobs.put_text(text)?.to_hex()))
}

/// Encodes a domain event as its journal event (`docs/work-supervision/events-v0.md`),
/// storing every text it carries as a blob first.
fn encode(
    event: &MissionEvent,
    after: &Mission,
    blobs: &BlobStore,
) -> Result<Event, SupervisorError> {
    let mut data = Map::new();
    data.insert(
        "mission".to_owned(),
        Value::String(event.mission().to_string()),
    );
    if let Some(state) = event.state_after() {
        data.insert("revision".to_owned(), Value::from(after.revision()));
        data.insert("state".to_owned(), Value::String(state.as_str().to_owned()));
    }
    match event {
        MissionEvent::Created {
            title,
            repository,
            brief,
            criteria,
            budgets,
            executor,
            ..
        } => {
            data.insert("title_digest".to_owned(), put(blobs, title)?);
            data.insert("repository".to_owned(), Value::String(repository.clone()));
            data.insert("brief_digest".to_owned(), put(blobs, brief)?);
            let digests = criteria
                .iter()
                .map(|criterion| put(blobs, criterion))
                .collect::<Result<Vec<_>, _>>()?;
            data.insert("criteria_digests".to_owned(), Value::Array(digests));
            data.insert(
                "max_duration_seconds".to_owned(),
                Value::from(budgets.max_duration_seconds()),
            );
            data.insert(
                "max_output_bytes".to_owned(),
                Value::from(budgets.max_output_bytes()),
            );
            data.insert(
                "executor".to_owned(),
                Value::String(executor.as_str().to_owned()),
            );
        }
        MissionEvent::Readied { .. } => {}
        MissionEvent::Provisioned {
            mission,
            base_commit,
        } => {
            data.insert(
                "base_commit".to_owned(),
                Value::String(base_commit.as_str().to_owned()),
            );
            data.insert(
                "branch".to_owned(),
                Value::String(work_supervision_domain::branch_of(mission)),
            );
            data.insert(
                "worktree".to_owned(),
                Value::String(work_supervision_domain::worktree_of(mission)),
            );
        }
        MissionEvent::RunStarted { run, .. }
        | MissionEvent::InputAwaited { run, .. }
        | MissionEvent::InputResumed { run, .. } => {
            data.insert("run".to_owned(), Value::String(run.to_string()));
        }
        MissionEvent::RunExited {
            run, interrupted, ..
        } => {
            data.insert("run".to_owned(), Value::String(run.to_string()));
            data.insert("interrupted".to_owned(), Value::Bool(*interrupted));
        }
        MissionEvent::ResultSubmitted { result, .. } => {
            data.insert(
                "commit".to_owned(),
                Value::String(result.commit().as_str().to_owned()),
            );
            data.insert(
                "evidence_digest".to_owned(),
                Value::String(result.evidence().to_hex()),
            );
            data.insert("summary_digest".to_owned(), put(blobs, result.summary())?);
        }
        MissionEvent::Decided { verdict, .. } => {
            data.insert("reason_digest".to_owned(), put(blobs, verdict.reason())?);
        }
        MissionEvent::Noted { text, .. } => {
            data.insert("note_digest".to_owned(), put(blobs, text)?);
        }
    }
    Ok(Event::new(event.kind(), data)?)
}

const MISSION_COLUMNS: &str =
    "id, title, repository, brief, max_duration_seconds, max_output_bytes,
    executor, state, revision, base_commit, current_run, result_commit, evidence_digest, summary,
    verdict, verdict_reason";

impl Store {
    /// The mission `id` as projected, `None` when it does not exist.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when a row cannot be read back into the domain.
    pub fn mission(&self, id: &MissionId) -> Result<Option<Mission>, StoreError> {
        let row = self
            .connection()
            .query_row(
                &format!("SELECT {MISSION_COLUMNS} FROM missions WHERE id = ?1"),
                params![id.as_str()],
                Row::read,
            )
            .optional()?;
        row.map(|row| self.to_mission(row)).transpose()
    }

    /// Every mission, in creation order.
    ///
    /// # Errors
    ///
    /// As [`Store::mission`].
    pub fn missions(&self) -> Result<Vec<Mission>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {MISSION_COLUMNS} FROM missions ORDER BY created_seq"
        ))?;
        let rows = statement
            .query_map([], Row::read)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(|row| self.to_mission(row)).collect()
    }

    fn to_mission(&self, row: Row) -> Result<Mission, StoreError> {
        let id = MissionId::parse(&row.id).map_err(|_| StoreError::Sqlite)?;
        let mut statement = self
            .connection()
            .prepare("SELECT text FROM mission_criteria WHERE mission_id = ?1 ORDER BY position")?;
        let criteria = statement
            .query_map(params![row.id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let unsigned = |value: i64| u64::try_from(value).map_err(|_| StoreError::Sqlite);
        let state = State::parse(&row.state).ok_or(StoreError::Sqlite)?;
        let result = match (row.result_commit, row.evidence_digest, row.summary) {
            (Some(commit), Some(evidence), Some(summary)) => Some(MissionResult::new(
                CommitId::parse(&commit).map_err(|_| StoreError::Sqlite)?,
                Digest32::parse(&evidence).map_err(|_| StoreError::Sqlite)?,
                summary,
            )),
            _ => None,
        };
        let verdict = match (row.verdict, row.verdict_reason) {
            (Some(verdict), Some(reason)) => Some(Verdict::new(
                State::parse(&verdict).ok_or(StoreError::Sqlite)?,
                reason,
            )),
            _ => None,
        };
        Ok(Mission::from_parts(MissionParts {
            id,
            title: row.title,
            repository: row.repository,
            brief: row.brief,
            criteria,
            budgets: Budgets::new(unsigned(row.max_duration)?, unsigned(row.max_output)?)
                .map_err(|_| StoreError::Sqlite)?,
            executor: ExecutorProfile::from_config_name(&row.executor)
                .map_err(|_| StoreError::Sqlite)?,
            state,
            revision: unsigned(row.revision)?,
            base_commit: row
                .base_commit
                .map(|commit| CommitId::parse(&commit).map_err(|_| StoreError::Sqlite))
                .transpose()?,
            current_run: row
                .current_run
                .map(|run| RunId::parse(&run).map_err(|_| StoreError::Sqlite))
                .transpose()?,
            result,
            verdict,
        }))
    }
}

struct Row {
    id: String,
    title: String,
    repository: String,
    brief: String,
    max_duration: i64,
    max_output: i64,
    executor: String,
    state: String,
    revision: i64,
    base_commit: Option<String>,
    current_run: Option<String>,
    result_commit: Option<String>,
    evidence_digest: Option<String>,
    summary: Option<String>,
    verdict: Option<String>,
    verdict_reason: Option<String>,
}

impl Row {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            repository: row.get(2)?,
            brief: row.get(3)?,
            max_duration: row.get(4)?,
            max_output: row.get(5)?,
            executor: row.get(6)?,
            state: row.get(7)?,
            revision: row.get(8)?,
            base_commit: row.get(9)?,
            current_run: row.get(10)?,
            result_commit: row.get(11)?,
            evidence_digest: row.get(12)?,
            summary: row.get(13)?,
            verdict: row.get(14)?,
            verdict_reason: row.get(15)?,
        })
    }
}
