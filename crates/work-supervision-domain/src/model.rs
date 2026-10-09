use std::fmt;

use crate::{CommitId, Digest32, MissionId, RunId};

/// State of a v0 mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// Described, brief still open.
    Draft,
    /// Brief frozen at its digest; ready to be provisioned.
    Ready,
    /// A worktree exists on `ws/<mission>` at the base commit.
    Provisioned,
    /// An executor runs in the worktree.
    Running,
    /// The executor waits for an input from the owner.
    WaitingInput,
    /// The executor has stopped (normally, by budget, or interrupted).
    Exited,
    /// A commit, evidence and a summary await the owner's decision.
    ResultSubmitted,
    /// Accepted by the owner. Terminal.
    Accepted,
    /// Rejected by the owner; can be resumed with a new run, or abandoned.
    Rejected,
    /// Given up by the owner. Terminal.
    Abandoned,
    /// Stopped by the owner while provisioned or running. Terminal.
    Cancelled,
}

impl State {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 11] = [
        Self::Draft,
        Self::Ready,
        Self::Provisioned,
        Self::Running,
        Self::WaitingInput,
        Self::Exited,
        Self::ResultSubmitted,
        Self::Accepted,
        Self::Rejected,
        Self::Abandoned,
        Self::Cancelled,
    ];

    /// Name used in events, rows and displays.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Provisioned => "provisioned",
            Self::Running => "running",
            Self::WaitingInput => "waiting-input",
            Self::Exited => "exited",
            Self::ResultSubmitted => "result-submitted",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses a state name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == text)
    }

    /// Whether no transition leaves this state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Abandoned | Self::Cancelled)
    }
}

/// Executor profile of a mission.
///
/// **C0 guard**: before the confinement qualification C0 is green, the only
/// profile is the scenario-driven fake agent. Adding a real profile is a code
/// change, reviewed, made after C0 and routed through the confinement (Q9) —
/// never a configuration value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutorProfile {
    /// The scenario-driven fake agent (`ws-fake-agent`).
    Fake,
}

impl ExecutorProfile {
    /// Every profile this build knows.
    pub const ALL: [Self; 1] = [Self::Fake];

    /// Name of the profile in configuration and events.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fake => "fake",
        }
    }

    /// Resolves a profile named in the private configuration.
    ///
    /// # Errors
    ///
    /// [`Refusal::RealAgentForbiddenUntilC0`] for any name but `fake`
    /// (an agent name, an executable path, a variant spelling).
    pub fn from_config_name(name: &str) -> Result<Self, Refusal> {
        match name {
            "fake" => Ok(Self::Fake),
            _ => Err(Refusal::RealAgentForbiddenUntilC0),
        }
    }
}

/// Limits enforced on every run of a mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    max_duration_seconds: u64,
    max_output_bytes: u64,
}

/// Longest accepted run duration: one day.
pub const MAX_DURATION_SECONDS: u64 = 86_400;
/// Largest accepted terminal output of a run: 1 GiB.
pub const MAX_OUTPUT_BYTES: u64 = 1 << 30;

impl Budgets {
    /// Validates both limits: at least 1, at most one day and 1 GiB.
    ///
    /// # Errors
    ///
    /// [`Refusal::FieldInvalid`] (`budgets`).
    pub const fn new(max_duration_seconds: u64, max_output_bytes: u64) -> Result<Self, Refusal> {
        if max_duration_seconds == 0
            || max_duration_seconds > MAX_DURATION_SECONDS
            || max_output_bytes == 0
            || max_output_bytes > MAX_OUTPUT_BYTES
        {
            return Err(Refusal::FieldInvalid { field: "budgets" });
        }
        Ok(Self {
            max_duration_seconds,
            max_output_bytes,
        })
    }

    /// Wall-clock limit of a run, in seconds.
    #[must_use]
    pub const fn max_duration_seconds(&self) -> u64 {
        self.max_duration_seconds
    }

    /// Terminal output limit of a run, in bytes.
    #[must_use]
    pub const fn max_output_bytes(&self) -> u64 {
        self.max_output_bytes
    }
}

/// Kind of a command, for refusals and coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandKind {
    /// [`Command::Create`].
    Create,
    /// [`Command::Ready`].
    Ready,
    /// [`Command::Provision`].
    Provision,
    /// [`Command::StartRun`].
    StartRun,
    /// [`Command::AwaitInput`].
    AwaitInput,
    /// [`Command::ResumeInput`].
    ResumeInput,
    /// [`Command::ExitRun`].
    ExitRun,
    /// [`Command::SubmitResult`].
    SubmitResult,
    /// [`Command::Accept`].
    Accept,
    /// [`Command::Reject`].
    Reject,
    /// [`Command::Abandon`].
    Abandon,
    /// [`Command::Cancel`].
    Cancel,
    /// [`Command::Note`].
    Note,
}

impl CommandKind {
    /// Every command kind.
    pub const ALL: [Self; 13] = [
        Self::Create,
        Self::Ready,
        Self::Provision,
        Self::StartRun,
        Self::AwaitInput,
        Self::ResumeInput,
        Self::ExitRun,
        Self::SubmitResult,
        Self::Accept,
        Self::Reject,
        Self::Abandon,
        Self::Cancel,
        Self::Note,
    ];

    /// Name used in refusals.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Ready => "ready",
            Self::Provision => "provision",
            Self::StartRun => "start-run",
            Self::AwaitInput => "await-input",
            Self::ResumeInput => "resume-input",
            Self::ExitRun => "exit-run",
            Self::SubmitResult => "submit-result",
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::Abandon => "abandon",
            Self::Cancel => "cancel",
            Self::Note => "note",
        }
    }
}

/// What the owner, the daemon or the executor asks of a mission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Describe a new mission (state `draft`).
    Create {
        /// Short title, one line.
        title: String,
        /// Repository name from the private configuration.
        repository: String,
        /// Brief given to the executor (the fake agent reads it as its scenario).
        brief: String,
        /// Acceptance criteria, in order.
        criteria: Vec<String>,
        /// Limits of every run.
        budgets: Budgets,
        /// Executor profile; only [`ExecutorProfile::Fake`] before C0.
        executor: ExecutorProfile,
    },
    /// Freeze the brief.
    Ready,
    /// Record the worktree created at `base_commit` on `ws/<mission>`.
    Provision {
        /// Commit the branch was created at.
        base_commit: CommitId,
    },
    /// Record that a run started (from `provisioned`, or `rejected` to resume).
    StartRun {
        /// The new run.
        run: RunId,
    },
    /// The executor waits for input.
    AwaitInput,
    /// Input was given; the executor works again.
    ResumeInput,
    /// The run ended.
    ExitRun {
        /// Whether it ended because its supervisor died, not by itself.
        interrupted: bool,
    },
    /// Submit a result: a commit, the digest of the evidence and a summary.
    SubmitResult {
        /// Commit holding the work; required.
        commit: Option<CommitId>,
        /// Digest of the evidence blob; required.
        evidence: Option<Digest32>,
        /// What was done, for the owner; required, not blank.
        summary: String,
    },
    /// Accept the submitted result.
    Accept {
        /// The owner's reason.
        reason: String,
    },
    /// Reject the submitted result; the mission can be resumed.
    Reject {
        /// The owner's reason.
        reason: String,
    },
    /// Give the mission up.
    Abandon {
        /// The owner's reason.
        reason: String,
    },
    /// Stop a provisioned or running mission.
    Cancel {
        /// The owner's reason.
        reason: String,
    },
    /// Attach a note (any state, no revision).
    Note {
        /// The note.
        text: String,
    },
}

impl Command {
    /// Kind of the command.
    #[must_use]
    pub const fn kind(&self) -> CommandKind {
        match self {
            Self::Create { .. } => CommandKind::Create,
            Self::Ready => CommandKind::Ready,
            Self::Provision { .. } => CommandKind::Provision,
            Self::StartRun { .. } => CommandKind::StartRun,
            Self::AwaitInput => CommandKind::AwaitInput,
            Self::ResumeInput => CommandKind::ResumeInput,
            Self::ExitRun { .. } => CommandKind::ExitRun,
            Self::SubmitResult { .. } => CommandKind::SubmitResult,
            Self::Accept { .. } => CommandKind::Accept,
            Self::Reject { .. } => CommandKind::Reject,
            Self::Abandon { .. } => CommandKind::Abandon,
            Self::Cancel { .. } => CommandKind::Cancel,
            Self::Note { .. } => CommandKind::Note,
        }
    }
}

/// Why a command is refused. Codes are stable; `Display` carries no content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No mission has this identifier.
    NotFound,
    /// A mission with this identifier exists already.
    AlreadyExists,
    /// The command was decided against another revision.
    RevisionStale {
        /// Revision the command expected.
        expected: u64,
        /// Revision of the mission.
        actual: u64,
    },
    /// The transition is not in the v0 table.
    TransitionForbidden {
        /// State of the mission.
        state: State,
        /// Command refused.
        command: CommandKind,
    },
    /// A result lacks its commit, its evidence or its summary.
    ResultIncomplete,
    /// A field does not satisfy its rule.
    FieldInvalid {
        /// Name of the field.
        field: &'static str,
    },
    /// A real agent was named before the C0 confinement qualification.
    RealAgentForbiddenUntilC0,
}

impl Refusal {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "mission.not_found",
            Self::AlreadyExists => "mission.already_exists",
            Self::RevisionStale { .. } => "mission.revision_stale",
            Self::TransitionForbidden { .. } => "mission.transition_forbidden",
            Self::ResultIncomplete => "mission.result_incomplete",
            Self::FieldInvalid { .. } => "mission.field_invalid",
            Self::RealAgentForbiddenUntilC0 => "agent.real_forbidden_until_c0",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RevisionStale { expected, actual } => write!(
                formatter,
                "{}: expected revision {expected}, mission at {actual}",
                self.code()
            ),
            Self::TransitionForbidden { state, command } => write!(
                formatter,
                "{}: {} in state {}",
                self.code(),
                command.as_str(),
                state.as_str()
            ),
            Self::FieldInvalid { field } => write!(formatter, "{}: {field}", self.code()),
            Self::NotFound
            | Self::AlreadyExists
            | Self::ResultIncomplete
            | Self::RealAgentForbiddenUntilC0 => formatter.write_str(self.code()),
        }
    }
}

impl std::error::Error for Refusal {}

/// A submitted result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionResult {
    pub(crate) commit: CommitId,
    pub(crate) evidence: Digest32,
    pub(crate) summary: String,
}

impl MissionResult {
    /// Builds a result read back from a projection.
    #[must_use]
    pub const fn new(commit: CommitId, evidence: Digest32, summary: String) -> Self {
        Self {
            commit,
            evidence,
            summary,
        }
    }

    /// Commit holding the work.
    #[must_use]
    pub const fn commit(&self) -> &CommitId {
        &self.commit
    }

    /// Digest of the evidence blob.
    #[must_use]
    pub const fn evidence(&self) -> &Digest32 {
        &self.evidence
    }

    /// Summary of the work.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// The owner's decision on a mission and its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub(crate) state: State,
    pub(crate) reason: String,
}

impl Verdict {
    /// Builds a verdict read back from a projection.
    #[must_use]
    pub const fn new(state: State, reason: String) -> Self {
        Self { state, reason }
    }

    /// State the decision led to (`accepted`, `rejected`, `abandoned`, `cancelled`).
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    /// The owner's reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// A v0 mission: single user, no tenant, no role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mission {
    pub(crate) id: MissionId,
    pub(crate) title: String,
    pub(crate) repository: String,
    pub(crate) brief: String,
    pub(crate) criteria: Vec<String>,
    pub(crate) budgets: Budgets,
    pub(crate) executor: ExecutorProfile,
    pub(crate) state: State,
    pub(crate) revision: u64,
    pub(crate) base_commit: Option<CommitId>,
    pub(crate) current_run: Option<RunId>,
    pub(crate) result: Option<MissionResult>,
    pub(crate) verdict: Option<Verdict>,
}

/// Every field of a mission, to rebuild one from a projection row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionParts {
    /// Identifier.
    pub id: MissionId,
    /// Title.
    pub title: String,
    /// Repository name.
    pub repository: String,
    /// Brief.
    pub brief: String,
    /// Acceptance criteria.
    pub criteria: Vec<String>,
    /// Budgets.
    pub budgets: Budgets,
    /// Executor profile.
    pub executor: ExecutorProfile,
    /// State.
    pub state: State,
    /// Revision.
    pub revision: u64,
    /// Base commit, once provisioned.
    pub base_commit: Option<CommitId>,
    /// Current or last run.
    pub current_run: Option<RunId>,
    /// Submitted result.
    pub result: Option<MissionResult>,
    /// Owner's decision.
    pub verdict: Option<Verdict>,
}

impl Mission {
    /// Rebuilds a mission from its parts (projection rows).
    #[must_use]
    pub fn from_parts(parts: MissionParts) -> Self {
        Self {
            id: parts.id,
            title: parts.title,
            repository: parts.repository,
            brief: parts.brief,
            criteria: parts.criteria,
            budgets: parts.budgets,
            executor: parts.executor,
            state: parts.state,
            revision: parts.revision,
            base_commit: parts.base_commit,
            current_run: parts.current_run,
            result: parts.result,
            verdict: parts.verdict,
        }
    }

    /// Identifier.
    #[must_use]
    pub const fn id(&self) -> &MissionId {
        &self.id
    }

    /// Title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Repository name from the private configuration.
    #[must_use]
    pub fn repository(&self) -> &str {
        &self.repository
    }

    /// Brief.
    #[must_use]
    pub fn brief(&self) -> &str {
        &self.brief
    }

    /// Acceptance criteria.
    #[must_use]
    pub fn criteria(&self) -> &[String] {
        &self.criteria
    }

    /// Budgets of every run.
    #[must_use]
    pub const fn budgets(&self) -> Budgets {
        self.budgets
    }

    /// Executor profile.
    #[must_use]
    pub const fn executor(&self) -> ExecutorProfile {
        self.executor
    }

    /// Current state.
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    /// Revision, incremented by every transition (not by notes).
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Commit the worktree branch was created at.
    #[must_use]
    pub const fn base_commit(&self) -> Option<&CommitId> {
        self.base_commit.as_ref()
    }

    /// Branch of the worktree, `ws/<mission>`, once provisioned.
    #[must_use]
    pub fn branch(&self) -> Option<String> {
        self.base_commit.as_ref().map(|_| branch_of(&self.id))
    }

    /// Worktree path relative to the root, `worktrees/<mission>`, once provisioned.
    #[must_use]
    pub fn worktree(&self) -> Option<String> {
        self.base_commit.as_ref().map(|_| worktree_of(&self.id))
    }

    /// Current or last run.
    #[must_use]
    pub const fn current_run(&self) -> Option<&RunId> {
        self.current_run.as_ref()
    }

    /// Submitted result, cleared when a new run starts.
    #[must_use]
    pub const fn result(&self) -> Option<&MissionResult> {
        self.result.as_ref()
    }

    /// The owner's decision, cleared when a new run starts.
    #[must_use]
    pub const fn verdict(&self) -> Option<&Verdict> {
        self.verdict.as_ref()
    }
}

/// Branch of a mission's worktree.
#[must_use]
pub fn branch_of(id: &MissionId) -> String {
    format!("ws/{id}")
}

/// Worktree path of a mission, relative to the root.
#[must_use]
pub fn worktree_of(id: &MissionId) -> String {
    format!("worktrees/{id}")
}

/// What a decided command changes, written to the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissionEvent {
    /// `mission.created`.
    Created {
        /// The new mission.
        mission: MissionId,
        /// Title.
        title: String,
        /// Repository name.
        repository: String,
        /// Brief.
        brief: String,
        /// Criteria.
        criteria: Vec<String>,
        /// Budgets.
        budgets: Budgets,
        /// Executor profile.
        executor: ExecutorProfile,
    },
    /// `mission.readied`.
    Readied {
        /// Mission.
        mission: MissionId,
    },
    /// `mission.provisioned`.
    Provisioned {
        /// Mission.
        mission: MissionId,
        /// Base commit.
        base_commit: CommitId,
    },
    /// `mission.run-started`.
    RunStarted {
        /// Mission.
        mission: MissionId,
        /// Run.
        run: RunId,
    },
    /// `mission.input-awaited`.
    InputAwaited {
        /// Mission.
        mission: MissionId,
        /// Current run.
        run: RunId,
    },
    /// `mission.input-resumed`.
    InputResumed {
        /// Mission.
        mission: MissionId,
        /// Current run.
        run: RunId,
    },
    /// `mission.run-exited`.
    RunExited {
        /// Mission.
        mission: MissionId,
        /// Current run.
        run: RunId,
        /// Whether the run was interrupted by the death of its supervisor.
        interrupted: bool,
    },
    /// `mission.result-submitted`.
    ResultSubmitted {
        /// Mission.
        mission: MissionId,
        /// The result.
        result: MissionResult,
    },
    /// `mission.accepted`, `mission.rejected`, `mission.abandoned` or `mission.cancelled`.
    Decided {
        /// Mission.
        mission: MissionId,
        /// The decision.
        verdict: Verdict,
    },
    /// `mission.noted`.
    Noted {
        /// Mission.
        mission: MissionId,
        /// The note.
        text: String,
    },
}

impl MissionEvent {
    /// Journal kind of the event.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Created { .. } => "mission.created",
            Self::Readied { .. } => "mission.readied",
            Self::Provisioned { .. } => "mission.provisioned",
            Self::RunStarted { .. } => "mission.run-started",
            Self::InputAwaited { .. } => "mission.input-awaited",
            Self::InputResumed { .. } => "mission.input-resumed",
            Self::RunExited { .. } => "mission.run-exited",
            Self::ResultSubmitted { .. } => "mission.result-submitted",
            Self::Decided { verdict, .. } => match verdict.state {
                State::Accepted => "mission.accepted",
                State::Rejected => "mission.rejected",
                State::Abandoned => "mission.abandoned",
                _ => "mission.cancelled",
            },
            Self::Noted { .. } => "mission.noted",
        }
    }

    /// Mission the event concerns.
    #[must_use]
    pub const fn mission(&self) -> &MissionId {
        match self {
            Self::Created { mission, .. }
            | Self::Readied { mission }
            | Self::Provisioned { mission, .. }
            | Self::RunStarted { mission, .. }
            | Self::InputAwaited { mission, .. }
            | Self::InputResumed { mission, .. }
            | Self::RunExited { mission, .. }
            | Self::ResultSubmitted { mission, .. }
            | Self::Decided { mission, .. }
            | Self::Noted { mission, .. } => mission,
        }
    }

    /// State the mission is in after the event; `None` for a note.
    #[must_use]
    pub const fn state_after(&self) -> Option<State> {
        Some(match self {
            Self::Created { .. } => State::Draft,
            Self::Readied { .. } => State::Ready,
            Self::Provisioned { .. } => State::Provisioned,
            Self::RunStarted { .. } | Self::InputResumed { .. } => State::Running,
            Self::InputAwaited { .. } => State::WaitingInput,
            Self::RunExited { .. } => State::Exited,
            Self::ResultSubmitted { .. } => State::ResultSubmitted,
            Self::Decided { verdict, .. } => verdict.state,
            Self::Noted { .. } => return None,
        })
    }
}
