//! Coordination primitives of Work Supervision v0: actors, deferred ideas,
//! decision requests, declared agent sessions, mission contract refinements
//! and the blockers that guard a run and an acceptance.
//!
//! Specified in `docs/work-supervision/coordination-v0.md`. Like the mission
//! state machine, everything here is pure: the caller reads the projection,
//! passes what a rule needs and records the event it gets back.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{CommitId, Digest32, IdeaId, MissionId, RequestId, SessionId, State};

/// Longest idea, note or brief-like text, in bytes.
pub const MAX_IDEA_BYTES: usize = 64 * 1024;
/// Longest question, consequence, reason or summary, in characters.
pub const MAX_COORDINATION_LINE_CHARS: usize = 4_000;
/// Longest option label or session label, in characters.
pub const MAX_LABEL_CHARS: usize = 200;
/// Fewest and most options of a decision request.
pub const REQUEST_OPTIONS: std::ops::RangeInclusive<usize> = 2..=4;
/// Most dependencies of one mission.
pub const MAX_DEPENDENCIES: usize = 32;
/// Most arguments of a check.
pub const MAX_CHECK_ARGUMENTS: usize = 64;
/// Longest check argument, in bytes.
pub const MAX_CHECK_ARGUMENT_BYTES: usize = 4_096;

/// Why a coordination command is refused. Codes are stable; `Display` writes
/// the code (and a field name or an index), never a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinationRefusal {
    /// A text or a value does not satisfy its rule.
    FieldInvalid {
        /// Name of the field.
        field: &'static str,
    },
    /// The operation is the owner's.
    OwnerOnly,
    /// A session may only act for itself, or withdraw what it opened.
    NotItsOwn,
    /// No idea has this identifier.
    IdeaNotFound,
    /// The idea's state does not allow the command.
    IdeaTransitionForbidden {
        /// State of the idea.
        state: IdeaState,
    },
    /// No decision request has this identifier.
    RequestNotFound,
    /// The decision request is answered or withdrawn.
    RequestNotOpen,
    /// The chosen option does not exist.
    RequestChoiceInvalid,
    /// A request cannot be opened on a terminal mission.
    RequestMissionClosed,
    /// No mission has this identifier.
    MissionNotFound,
    /// Contract refinements are declared while the mission is `draft`.
    MissionNotDraft,
    /// No session has this identifier.
    SessionNotFound,
    /// The session has ended.
    SessionEnded,
    /// The dependency would close a cycle.
    DependencyCycle,
    /// The dependency is declared already (or names the mission itself).
    DependencyDuplicate,
    /// The mission has a scope already.
    ScopeAlreadyDeclared,
    /// The criterion has a check already.
    CheckAlreadyDeclared,
    /// The criterion index does not exist.
    CheckCriterionUnknown,
}

impl CoordinationRefusal {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::FieldInvalid { .. } => "coordination.field_invalid",
            Self::OwnerOnly => "actor.owner_only",
            Self::NotItsOwn => "actor.not_its_own",
            Self::IdeaNotFound => "idea.not_found",
            Self::IdeaTransitionForbidden { .. } => "idea.transition_forbidden",
            Self::RequestNotFound => "request.not_found",
            Self::RequestNotOpen => "request.not_open",
            Self::RequestChoiceInvalid => "request.choice_invalid",
            Self::RequestMissionClosed => "request.mission_closed",
            Self::MissionNotFound => "mission.not_found",
            Self::MissionNotDraft => "mission.not_draft",
            Self::SessionNotFound => "session.not_found",
            Self::SessionEnded => "session.ended",
            Self::DependencyCycle => "dependency.cycle",
            Self::DependencyDuplicate => "dependency.duplicate",
            Self::ScopeAlreadyDeclared => "scope.already_declared",
            Self::CheckAlreadyDeclared => "check.already_declared",
            Self::CheckCriterionUnknown => "check.criterion_unknown",
        }
    }
}

impl fmt::Display for CoordinationRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldInvalid { field } => write!(formatter, "{}: {field}", self.code()),
            Self::IdeaTransitionForbidden { state } => {
                write!(formatter, "{}: idea {}", self.code(), state.as_str())
            }
            _ => formatter.write_str(self.code()),
        }
    }
}

impl std::error::Error for CoordinationRefusal {}

fn line(text: &str, max_chars: usize, field: &'static str) -> Result<String, CoordinationRefusal> {
    if text.trim().is_empty()
        || text.chars().count() > max_chars
        || text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        Err(CoordinationRefusal::FieldInvalid { field })
    } else {
        Ok(text.to_owned())
    }
}

fn label(text: &str, field: &'static str) -> Result<String, CoordinationRefusal> {
    if text.chars().any(char::is_control) {
        return Err(CoordinationRefusal::FieldInvalid { field });
    }
    line(text, MAX_LABEL_CHARS, field)
}

fn long_text(text: &str, field: &'static str) -> Result<String, CoordinationRefusal> {
    if text.trim().is_empty() || text.len() > MAX_IDEA_BYTES {
        Err(CoordinationRefusal::FieldInvalid { field })
    } else {
        Ok(text.to_owned())
    }
}

/// Who asks: the owner, or an agent session declared through the bridge.
///
/// Declarative, not authenticated (see the specification): it records what a
/// client said, and the rules below are enforced on that declaration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Actor {
    /// The owner.
    Owner,
    /// A declared agent session.
    Session(SessionId),
}

impl Actor {
    /// `owner` or `session:<id>`.
    #[must_use]
    pub fn to_text(&self) -> String {
        match self {
            Self::Owner => "owner".to_owned(),
            Self::Session(id) => format!("session:{id}"),
        }
    }

    /// Parses [`Actor::to_text`]'s form.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`actor`).
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        let invalid = CoordinationRefusal::FieldInvalid { field: "actor" };
        if text == "owner" {
            return Ok(Self::Owner);
        }
        let id = text.strip_prefix("session:").ok_or(invalid)?;
        SessionId::parse(id).map(Self::Session).map_err(|_| invalid)
    }

    fn require_owner(&self) -> Result<(), CoordinationRefusal> {
        match self {
            Self::Owner => Ok(()),
            Self::Session(_) => Err(CoordinationRefusal::OwnerOnly),
        }
    }
}

/// Requires that a session actor names a known, active session.
///
/// # Errors
///
/// [`CoordinationRefusal::SessionNotFound`] and [`CoordinationRefusal::SessionEnded`].
pub fn check_actor_session(
    actor: &Actor,
    state: Option<SessionState>,
) -> Result<(), CoordinationRefusal> {
    match (actor, state) {
        (Actor::Owner, _) | (Actor::Session(_), Some(SessionState::Active)) => Ok(()),
        (Actor::Session(_), None) => Err(CoordinationRefusal::SessionNotFound),
        (Actor::Session(_), Some(SessionState::Ended)) => Err(CoordinationRefusal::SessionEnded),
    }
}

macro_rules! named_enum {
    ($(#[$meta:meta])* $name:ident, $field:expr, { $($(#[$vmeta:meta])* $variant:ident => $text:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Name used in events, rows and displays.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            /// Parses a name.
            ///
            /// # Errors
            ///
            /// [`CoordinationRefusal::FieldInvalid`] for any other text.
            pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|value| value.as_str() == text)
                    .ok_or(CoordinationRefusal::FieldInvalid { field: $field })
            }
        }
    };
}

named_enum!(
    /// Agent harness a declared session runs in. A harness is not an executor
    /// profile: naming one here launches nothing (C0 guard, `mission-v0.md`).
    Harness, "harness", {
        /// Anthropic's Claude Code.
        ClaudeCode => "claude-code",
        /// OpenAI's Codex CLI.
        Codex => "codex",
        /// Pi coding agent.
        Pi => "pi",
        /// Any other harness.
        Other => "other",
    }
);

named_enum!(
    /// State of a deferred idea.
    IdeaState, "idea_state", {
        /// Recorded, not yet qualified.
        Captured => "captured",
        /// Attached to a repository or a mission, or given context.
        Qualified => "qualified",
        /// Promotion to a mission intended, not confirmed.
        Promoting => "promoting",
        /// Became a mission. Terminal.
        Promoted => "promoted",
        /// Given up. Terminal.
        Dismissed => "dismissed",
    }
);

named_enum!(
    /// Reversibility of an option of a decision request.
    Reversibility, "reversibility", {
        /// Undone at no significant cost.
        Reversible => "reversible",
        /// Undone, at a cost.
        Costly => "costly",
        /// Cannot be undone.
        Irreversible => "irreversible",
    }
);

named_enum!(
    /// State of a decision request.
    RequestState, "request_state", {
        /// Waiting for the owner's answer.
        Open => "open",
        /// Answered by the owner. Terminal.
        Answered => "answered",
        /// Withdrawn. Terminal.
        Withdrawn => "withdrawn",
    }
);

named_enum!(
    /// State of a declared agent session.
    SessionState, "session_state", {
        /// Registered and not ended.
        Active => "active",
        /// Ended. Terminal.
        Ended => "ended",
    }
);

named_enum!(
    /// What a session reports it is doing; never a verified result.
    ReportedState, "reported_state", {
        /// Working.
        Working => "working",
        /// Waiting for an input or an approval of its user.
        WaitingInput => "waiting-input",
        /// Cannot go on without something else.
        Blocked => "blocked",
        /// Between two tasks.
        Idle => "idle",
    }
);

named_enum!(
    /// How a session says it ended.
    SessionOutcome, "outcome", {
        /// Finished its work.
        Completed => "completed",
        /// Stopped on a failure.
        Failed => "failed",
        /// Stopped without finishing.
        Abandoned => "abandoned",
    }
);

// ----------------------------------------------------------------- ideas ---

/// What the projection holds of an idea, as far as its rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeaView {
    /// Current state.
    pub state: IdeaState,
    /// Whether it was ever qualified (where an aborted promotion returns).
    pub ever_qualified: bool,
    /// Mission named by a pending promotion intent.
    pub promoting: Option<MissionId>,
}

/// What is asked of an idea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdeaCommand {
    /// Record a new idea.
    Capture {
        /// The idea.
        text: String,
    },
    /// Attach it to a repository and/or a mission, or give it context.
    Qualify {
        /// Repository name of the private configuration.
        repository: Option<String>,
        /// Mission it concerns.
        mission: Option<MissionId>,
        /// Context.
        context: Option<String>,
    },
    /// Intend its promotion to the new mission `mission`.
    PromoteIntent {
        /// Identifier the new mission will have.
        mission: MissionId,
    },
    /// Confirm the promotion once the mission exists.
    ConfirmPromotion,
    /// Abort a promotion whose mission was never created.
    AbortPromotion,
    /// Give it up.
    Dismiss {
        /// Why.
        reason: String,
    },
}

/// What a decided idea command records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdeaEvent {
    /// `idea.captured`.
    Captured {
        /// Idea.
        idea: IdeaId,
        /// Text.
        text: String,
        /// Who captured it.
        actor: Actor,
    },
    /// `idea.qualified`.
    Qualified {
        /// Idea.
        idea: IdeaId,
        /// Repository.
        repository: Option<String>,
        /// Mission.
        mission: Option<MissionId>,
        /// Context.
        context: Option<String>,
        /// Who qualified it.
        actor: Actor,
    },
    /// `idea.promotion.intent`.
    PromotionIntended {
        /// Idea.
        idea: IdeaId,
        /// New mission.
        mission: MissionId,
    },
    /// `idea.promoted`.
    Promoted {
        /// Idea.
        idea: IdeaId,
        /// The mission it became.
        mission: MissionId,
    },
    /// `idea.promotion.aborted`.
    PromotionAborted {
        /// Idea.
        idea: IdeaId,
    },
    /// `idea.dismissed`.
    Dismissed {
        /// Idea.
        idea: IdeaId,
        /// Why.
        reason: String,
    },
}

impl IdeaEvent {
    /// Journal kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Captured { .. } => "idea.captured",
            Self::Qualified { .. } => "idea.qualified",
            Self::PromotionIntended { .. } => "idea.promotion.intent",
            Self::Promoted { .. } => "idea.promoted",
            Self::PromotionAborted { .. } => "idea.promotion.aborted",
            Self::Dismissed { .. } => "idea.dismissed",
        }
    }
}

/// `[a-z0-9][a-z0-9._-]{0,63}`, the rule of a repository name.
fn is_repository_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && name.len() <= 64
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

/// Decides an idea command.
///
/// # Errors
///
/// [`CoordinationRefusal::IdeaNotFound`], [`CoordinationRefusal::OwnerOnly`]
/// (promote, dismiss), [`CoordinationRefusal::IdeaTransitionForbidden`] and
/// [`CoordinationRefusal::FieldInvalid`].
pub fn decide_idea(
    id: &IdeaId,
    current: Option<&IdeaView>,
    command: &IdeaCommand,
    actor: &Actor,
) -> Result<IdeaEvent, CoordinationRefusal> {
    let idea = id.clone();
    let Some(view) = current else {
        return match command {
            IdeaCommand::Capture { text } => Ok(IdeaEvent::Captured {
                idea,
                text: long_text(text, "idea")?,
                actor: actor.clone(),
            }),
            _ => Err(CoordinationRefusal::IdeaNotFound),
        };
    };
    let forbidden = CoordinationRefusal::IdeaTransitionForbidden { state: view.state };
    let open = matches!(view.state, IdeaState::Captured | IdeaState::Qualified);
    match command {
        IdeaCommand::Capture { .. } => Err(forbidden),
        IdeaCommand::Qualify {
            repository,
            mission,
            context,
        } => {
            if !open {
                return Err(forbidden);
            }
            if repository.is_none() && mission.is_none() && context.is_none() {
                return Err(CoordinationRefusal::FieldInvalid { field: "qualify" });
            }
            if repository
                .as_deref()
                .is_some_and(|name| !is_repository_name(name))
            {
                return Err(CoordinationRefusal::FieldInvalid {
                    field: "repository",
                });
            }
            Ok(IdeaEvent::Qualified {
                idea,
                repository: repository.clone(),
                mission: mission.clone(),
                context: context
                    .as_deref()
                    .map(|text| long_text(text, "context"))
                    .transpose()?,
                actor: actor.clone(),
            })
        }
        IdeaCommand::PromoteIntent { mission } => {
            actor.require_owner()?;
            if !open {
                return Err(forbidden);
            }
            Ok(IdeaEvent::PromotionIntended {
                idea,
                mission: mission.clone(),
            })
        }
        IdeaCommand::ConfirmPromotion => match (&view.state, &view.promoting) {
            (IdeaState::Promoting, Some(mission)) => Ok(IdeaEvent::Promoted {
                idea,
                mission: mission.clone(),
            }),
            _ => Err(forbidden),
        },
        IdeaCommand::AbortPromotion => {
            if view.state == IdeaState::Promoting {
                Ok(IdeaEvent::PromotionAborted { idea })
            } else {
                Err(forbidden)
            }
        }
        IdeaCommand::Dismiss { reason } => {
            actor.require_owner()?;
            if !open {
                return Err(forbidden);
            }
            Ok(IdeaEvent::Dismissed {
                idea,
                reason: line(reason, MAX_COORDINATION_LINE_CHARS, "reason")?,
            })
        }
    }
}

/// Folds an idea event into its view.
#[must_use]
pub fn apply_idea(current: Option<IdeaView>, event: &IdeaEvent) -> IdeaView {
    let mut view = current.unwrap_or(IdeaView {
        state: IdeaState::Captured,
        ever_qualified: false,
        promoting: None,
    });
    match event {
        IdeaEvent::Captured { .. } => {}
        IdeaEvent::Qualified { .. } => {
            view.state = IdeaState::Qualified;
            view.ever_qualified = true;
        }
        IdeaEvent::PromotionIntended { mission, .. } => {
            view.state = IdeaState::Promoting;
            view.promoting = Some(mission.clone());
        }
        IdeaEvent::Promoted { .. } => view.state = IdeaState::Promoted,
        IdeaEvent::PromotionAborted { .. } => {
            view.state = if view.ever_qualified {
                IdeaState::Qualified
            } else {
                IdeaState::Captured
            };
            view.promoting = None;
        }
        IdeaEvent::Dismissed { .. } => view.state = IdeaState::Dismissed,
    }
    view
}

// ------------------------------------------------------ decision requests ---

/// One option of a decision request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOption {
    /// Short label.
    pub label: String,
    /// What choosing it entails.
    pub consequence: String,
    /// Whether it can be undone.
    pub reversibility: Reversibility,
}

/// What the projection holds of a request, as far as its rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestView {
    /// Current state.
    pub state: RequestState,
    /// Who opened it.
    pub opener: Actor,
    /// Number of options.
    pub options: usize,
}

/// What is asked of a decision request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestCommand {
    /// Ask a question.
    Open {
        /// Mission it conditions, with its state (`None` state: unknown mission).
        mission: Option<(MissionId, Option<State>)>,
        /// The question.
        question: String,
        /// Two to four options.
        options: Vec<RequestOption>,
        /// Index of the recommended option.
        recommended: Option<usize>,
    },
    /// Answer it (owner).
    Answer {
        /// Index of the chosen option.
        choice: usize,
        /// Why.
        reason: String,
    },
    /// Withdraw it (owner, or the session that opened it).
    Withdraw {
        /// Why.
        reason: String,
    },
}

/// What a decided request command records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestEvent {
    /// `request.opened`.
    Opened {
        /// Request.
        request: RequestId,
        /// Mission.
        mission: Option<MissionId>,
        /// Question.
        question: String,
        /// Options.
        options: Vec<RequestOption>,
        /// Recommended option.
        recommended: Option<usize>,
        /// Opener.
        actor: Actor,
    },
    /// `request.answered`.
    Answered {
        /// Request.
        request: RequestId,
        /// Chosen option.
        choice: usize,
        /// Why.
        reason: String,
    },
    /// `request.withdrawn`.
    Withdrawn {
        /// Request.
        request: RequestId,
        /// Why.
        reason: String,
        /// Who withdrew it.
        actor: Actor,
    },
}

impl RequestEvent {
    /// Journal kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Opened { .. } => "request.opened",
            Self::Answered { .. } => "request.answered",
            Self::Withdrawn { .. } => "request.withdrawn",
        }
    }
}

/// Decides a decision-request command.
///
/// # Errors
///
/// [`CoordinationRefusal::RequestNotFound`], [`CoordinationRefusal::RequestNotOpen`],
/// [`CoordinationRefusal::RequestChoiceInvalid`], [`CoordinationRefusal::RequestMissionClosed`],
/// [`CoordinationRefusal::MissionNotFound`], [`CoordinationRefusal::OwnerOnly`],
/// [`CoordinationRefusal::NotItsOwn`] and [`CoordinationRefusal::FieldInvalid`].
pub fn decide_request(
    id: &RequestId,
    current: Option<&RequestView>,
    command: &RequestCommand,
    actor: &Actor,
) -> Result<RequestEvent, CoordinationRefusal> {
    let request = id.clone();
    match (current, command) {
        (
            None,
            RequestCommand::Open {
                mission,
                question,
                options,
                recommended,
            },
        ) => {
            let mission = match mission {
                None => None,
                Some((_, None)) => return Err(CoordinationRefusal::MissionNotFound),
                Some((_, Some(state))) if state.is_terminal() => {
                    return Err(CoordinationRefusal::RequestMissionClosed);
                }
                Some((id, Some(_))) => Some(id.clone()),
            };
            if !REQUEST_OPTIONS.contains(&options.len()) {
                return Err(CoordinationRefusal::FieldInvalid { field: "options" });
            }
            let options = options
                .iter()
                .map(|option| {
                    Ok(RequestOption {
                        label: label(&option.label, "label")?,
                        consequence: line(
                            &option.consequence,
                            MAX_COORDINATION_LINE_CHARS,
                            "consequence",
                        )?,
                        reversibility: option.reversibility,
                    })
                })
                .collect::<Result<Vec<_>, CoordinationRefusal>>()?;
            let labels: BTreeSet<&str> = options.iter().map(|o| o.label.as_str()).collect();
            if labels.len() != options.len() {
                return Err(CoordinationRefusal::FieldInvalid { field: "options" });
            }
            if recommended.is_some_and(|index| index >= options.len()) {
                return Err(CoordinationRefusal::FieldInvalid {
                    field: "recommended",
                });
            }
            Ok(RequestEvent::Opened {
                request,
                mission,
                question: line(question, MAX_COORDINATION_LINE_CHARS, "question")?,
                options,
                recommended: *recommended,
                actor: actor.clone(),
            })
        }
        (None, _) => Err(CoordinationRefusal::RequestNotFound),
        (Some(_), RequestCommand::Open { .. }) => Err(CoordinationRefusal::RequestNotOpen),
        (Some(view), _) if view.state != RequestState::Open => {
            Err(CoordinationRefusal::RequestNotOpen)
        }
        (Some(view), RequestCommand::Answer { choice, reason }) => {
            actor.require_owner()?;
            if *choice >= view.options {
                return Err(CoordinationRefusal::RequestChoiceInvalid);
            }
            Ok(RequestEvent::Answered {
                request,
                choice: *choice,
                reason: line(reason, MAX_COORDINATION_LINE_CHARS, "reason")?,
            })
        }
        (Some(view), RequestCommand::Withdraw { reason }) => {
            if *actor != Actor::Owner && *actor != view.opener {
                return Err(CoordinationRefusal::NotItsOwn);
            }
            Ok(RequestEvent::Withdrawn {
                request,
                reason: line(reason, MAX_COORDINATION_LINE_CHARS, "reason")?,
                actor: actor.clone(),
            })
        }
    }
}

// ------------------------------------------------------------- sessions ---

/// What is asked of a declared session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    /// Declare a session.
    Register {
        /// Its harness.
        harness: Harness,
        /// Repository it works in, when known.
        repository: Option<String>,
        /// Mission it works on, when known.
        mission: Option<MissionId>,
        /// Short label.
        label: String,
        /// SHA-256 of the harness's own session identifier.
        external: Option<Digest32>,
    },
    /// Report what it is doing.
    Report {
        /// Reported state.
        state: ReportedState,
        /// Optional note.
        note: Option<String>,
    },
    /// Declare its end.
    End {
        /// How it ended.
        outcome: SessionOutcome,
        /// Optional summary.
        summary: Option<String>,
    },
}

/// What a decided session command records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// `session.registered`.
    Registered {
        /// Session.
        session: SessionId,
        /// Harness.
        harness: Harness,
        /// Repository.
        repository: Option<String>,
        /// Mission.
        mission: Option<MissionId>,
        /// Label.
        label: String,
        /// Digest of the harness's identifier.
        external: Option<Digest32>,
    },
    /// `session.reported`.
    Reported {
        /// Session.
        session: SessionId,
        /// State.
        state: ReportedState,
        /// Note.
        note: Option<String>,
    },
    /// `session.ended`.
    Ended {
        /// Session.
        session: SessionId,
        /// Outcome.
        outcome: SessionOutcome,
        /// Summary.
        summary: Option<String>,
    },
}

impl SessionEvent {
    /// Journal kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Registered { .. } => "session.registered",
            Self::Reported { .. } => "session.reported",
            Self::Ended { .. } => "session.ended",
        }
    }
}

/// Decides a session command. A session reports and ends for itself; the
/// owner may also end a session (a silent one, for instance).
///
/// # Errors
///
/// [`CoordinationRefusal::SessionNotFound`], [`CoordinationRefusal::SessionEnded`],
/// [`CoordinationRefusal::NotItsOwn`], [`CoordinationRefusal::MissionNotFound`]
/// and [`CoordinationRefusal::FieldInvalid`].
pub fn decide_session(
    id: &SessionId,
    current: Option<SessionState>,
    command: &SessionCommand,
    actor: &Actor,
    mission_exists: bool,
) -> Result<SessionEvent, CoordinationRefusal> {
    let session = id.clone();
    match (current, command) {
        (
            None,
            SessionCommand::Register {
                harness,
                repository,
                mission,
                label: text,
                external,
            },
        ) => {
            if repository
                .as_deref()
                .is_some_and(|name| !is_repository_name(name))
            {
                return Err(CoordinationRefusal::FieldInvalid {
                    field: "repository",
                });
            }
            if mission.is_some() && !mission_exists {
                return Err(CoordinationRefusal::MissionNotFound);
            }
            Ok(SessionEvent::Registered {
                session,
                harness: *harness,
                repository: repository.clone(),
                mission: mission.clone(),
                label: label(text, "label")?,
                external: *external,
            })
        }
        (None, _) => Err(CoordinationRefusal::SessionNotFound),
        (Some(_), SessionCommand::Register { .. }) => {
            Err(CoordinationRefusal::FieldInvalid { field: "session" })
        }
        (Some(SessionState::Ended), _) => Err(CoordinationRefusal::SessionEnded),
        (Some(SessionState::Active), command) => {
            let own = *actor == Actor::Session(id.clone());
            let ending_by_owner =
                *actor == Actor::Owner && matches!(command, SessionCommand::End { .. });
            if !own && !ending_by_owner {
                return Err(CoordinationRefusal::NotItsOwn);
            }
            let optional = |text: &Option<String>, field| {
                text.as_deref()
                    .map(|text| line(text, MAX_COORDINATION_LINE_CHARS, field))
                    .transpose()
            };
            match command {
                SessionCommand::Report { state, note } => Ok(SessionEvent::Reported {
                    session,
                    state: *state,
                    note: optional(note, "note")?,
                }),
                SessionCommand::End { outcome, summary } => Ok(SessionEvent::Ended {
                    session,
                    outcome: *outcome,
                    summary: optional(summary, "summary")?,
                }),
                SessionCommand::Register { .. } => {
                    Err(CoordinationRefusal::FieldInvalid { field: "session" })
                }
            }
        }
    }
}

// ------------------------------------------------ contract refinements ---

/// Contract refinements are declared while the mission is `draft`.
///
/// # Errors
///
/// [`CoordinationRefusal::MissionNotFound`], [`CoordinationRefusal::MissionNotDraft`].
pub fn check_draft(state: Option<State>) -> Result<(), CoordinationRefusal> {
    match state {
        None => Err(CoordinationRefusal::MissionNotFound),
        Some(State::Draft) => Ok(()),
        Some(_) => Err(CoordinationRefusal::MissionNotDraft),
    }
}

/// Checks that `mission` may depend on `on`, given every declared edge
/// (`(mission, on)` pairs) and whether `on` exists.
///
/// # Errors
///
/// [`CoordinationRefusal::MissionNotFound`], [`CoordinationRefusal::DependencyDuplicate`]
/// (including a self-dependency), [`CoordinationRefusal::DependencyCycle`] and
/// [`CoordinationRefusal::FieldInvalid`] (`dependencies`, beyond [`MAX_DEPENDENCIES`]).
pub fn check_dependency(
    mission: &MissionId,
    on: &MissionId,
    on_exists: bool,
    edges: &[(MissionId, MissionId)],
) -> Result<(), CoordinationRefusal> {
    if !on_exists {
        return Err(CoordinationRefusal::MissionNotFound);
    }
    if mission == on || edges.iter().any(|(from, to)| from == mission && to == on) {
        return Err(CoordinationRefusal::DependencyDuplicate);
    }
    if edges.iter().filter(|(from, _)| from == mission).count() >= MAX_DEPENDENCIES {
        return Err(CoordinationRefusal::FieldInvalid {
            field: "dependencies",
        });
    }
    if reaches(edges, on, mission) {
        return Err(CoordinationRefusal::DependencyCycle);
    }
    Ok(())
}

/// Whether `target` is reachable from `start` along the edges.
fn reaches(edges: &[(MissionId, MissionId)], start: &MissionId, target: &MissionId) -> bool {
    let mut next: BTreeMap<&MissionId, Vec<&MissionId>> = BTreeMap::new();
    for (from, to) in edges {
        next.entry(from).or_default().push(to);
    }
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if seen.insert(node) {
            stack.extend(next.get(node).into_iter().flatten().copied());
        }
    }
    false
}

/// Validates the argument vector of a check.
///
/// # Errors
///
/// [`CoordinationRefusal::FieldInvalid`] (`check`): 1 to [`MAX_CHECK_ARGUMENTS`]
/// arguments, each at most [`MAX_CHECK_ARGUMENT_BYTES`] without NUL, the
/// program not blank.
pub fn check_argv(argv: &[String]) -> Result<(), CoordinationRefusal> {
    let invalid = CoordinationRefusal::FieldInvalid { field: "check" };
    let program_ok = argv
        .first()
        .is_some_and(|program| !program.trim().is_empty());
    if !program_ok
        || argv.len() > MAX_CHECK_ARGUMENTS
        || argv
            .iter()
            .any(|argument| argument.len() > MAX_CHECK_ARGUMENT_BYTES || argument.contains('\0'))
    {
        return Err(invalid);
    }
    Ok(())
}

// --------------------------------------------------------------- blockers ---

/// Something a mission waits for before a run or an acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// A dependency was abandoned or cancelled.
    DependencyUnsatisfiable(MissionId),
    /// A dependency is not accepted yet.
    DependencyPending(MissionId),
    /// A decision request on the mission is open.
    RequestPending(RequestId),
    /// Another active mission of the repository has an overlapping scope.
    ScopeConflict(MissionId),
    /// Checks of the mission are running.
    CheckRunning,
    /// The criterion's check did not pass at the submitted commit.
    CriteriaUnverified(usize),
    /// Changed files lie outside the declared scope.
    ScopeViolated(u64),
}

impl Blocker {
    /// Stable code, also the refusal of the guarded operation.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DependencyUnsatisfiable(_) => "dependency.unsatisfiable",
            Self::DependencyPending(_) => "dependency.pending",
            Self::RequestPending(_) => "request.pending",
            Self::ScopeConflict(_) => "scope.conflict",
            Self::CheckRunning => "check.running",
            Self::CriteriaUnverified(_) => "criteria.unverified",
            Self::ScopeViolated(_) => "scope.violated",
        }
    }
}

/// Whether a mission in `state` holds a worktree, for scope conflicts.
#[must_use]
pub const fn holds_worktree(state: State) -> bool {
    matches!(
        state,
        State::Provisioned
            | State::Running
            | State::WaitingInput
            | State::Exited
            | State::ResultSubmitted
            | State::Rejected
    )
}

/// What guards a run, in the order of the specification: unsatisfiable and
/// pending dependencies, open requests, scope conflicts.
#[must_use]
pub fn run_blockers(
    dependencies: &[(MissionId, State)],
    open_requests: &[RequestId],
    conflicts: &[MissionId],
) -> Vec<Blocker> {
    let mut blockers: Vec<Blocker> = dependencies
        .iter()
        .filter(|(_, state)| matches!(state, State::Abandoned | State::Cancelled))
        .map(|(id, _)| Blocker::DependencyUnsatisfiable(id.clone()))
        .collect();
    blockers.extend(
        dependencies
            .iter()
            .filter(|(_, state)| {
                !matches!(state, State::Accepted | State::Abandoned | State::Cancelled)
            })
            .map(|(id, _)| Blocker::DependencyPending(id.clone())),
    );
    blockers.extend(open_requests.iter().cloned().map(Blocker::RequestPending));
    blockers.extend(conflicts.iter().cloned().map(Blocker::ScopeConflict));
    blockers
}

/// The last finished execution of a criterion's check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// Criterion index.
    pub criterion: usize,
    /// Commit it ran at.
    pub commit: CommitId,
    /// Exit 0, no signal, no overrun budget.
    pub passed: bool,
}

/// What guards an acceptance, in the order of the specification: open
/// requests, running checks, unverified criteria, scope violation.
///
/// `declared` lists the criteria carrying a check; `last` the last finished
/// execution of each; `outside` the outside count of the scope check at the
/// submitted commit, when there is one.
#[must_use]
pub fn accept_blockers(
    open_requests: &[RequestId],
    checks_running: bool,
    declared: &[usize],
    last: &[CheckOutcome],
    submitted: Option<&CommitId>,
    outside: Option<u64>,
) -> Vec<Blocker> {
    let mut blockers: Vec<Blocker> = open_requests
        .iter()
        .cloned()
        .map(Blocker::RequestPending)
        .collect();
    if checks_running {
        blockers.push(Blocker::CheckRunning);
    }
    for criterion in declared {
        let verified = last.iter().any(|outcome| {
            outcome.criterion == *criterion && Some(&outcome.commit) == submitted && outcome.passed
        });
        if !verified {
            blockers.push(Blocker::CriteriaUnverified(*criterion));
        }
    }
    if let Some(count) = outside.filter(|count| *count > 0) {
        blockers.push(Blocker::ScopeViolated(count));
    }
    blockers
}
