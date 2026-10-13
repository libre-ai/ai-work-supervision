//! Phases of a mission and their approved artifacts (`docs/work-supervision/phases-v0.md`).
//!
//! A mission may declare, while it is `draft`, the phases its work goes
//! through before implementation; each phase closes on one written artifact
//! the owner approves. Like the rest of the domain, everything here is pure:
//! the caller reads the projection, passes what a rule needs and records the
//! event it gets back.

use std::fmt;

use crate::coordination::{Actor, Blocker, CoordinationRefusal, MAX_COORDINATION_LINE_CHARS};
use crate::{ArtifactId, Digest32, MissionId, State};

/// Largest artifact, in bytes.
pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

/// A phase, in canonical order: a later phase prevails over an earlier one
/// when their approved artifacts disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// What to find out, apart from the brief.
    ResearchQuestions,
    /// The current state, with `path:line` findings.
    Research,
    /// Options considered and decisions taken.
    Design,
    /// Problem, success measure, user-visible solution.
    Product,
    /// Contracts, schemas, stores.
    SystemDesign,
    /// Call paths, files, types, signatures, test boundaries.
    ProgramDesign,
    /// Vertical slices with their verifications.
    Outline,
}

impl Phase {
    /// Every phase, in canonical order.
    pub const ALL: [Self; 7] = [
        Self::ResearchQuestions,
        Self::Research,
        Self::Design,
        Self::Product,
        Self::SystemDesign,
        Self::ProgramDesign,
        Self::Outline,
    ];

    /// Name used in events, rows and displays.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResearchQuestions => "research-questions",
            Self::Research => "research",
            Self::Design => "design",
            Self::Product => "product",
            Self::SystemDesign => "system-design",
            Self::ProgramDesign => "program-design",
            Self::Outline => "outline",
        }
    }

    /// Parses a name.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`phase`).
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        Self::ALL
            .iter()
            .copied()
            .find(|phase| phase.as_str() == text)
            .ok_or(CoordinationRefusal::FieldInvalid { field: "phase" })
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// State of an artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactState {
    /// Waiting for the owner.
    Submitted,
    /// Approved by the owner.
    Approved,
    /// Returned by the owner; the phase waits for a new submission.
    Returned,
    /// Replaced by a later submission of its phase or of an earlier one.
    Superseded,
}

impl ArtifactState {
    /// Every state.
    pub const ALL: [Self; 4] = [
        Self::Submitted,
        Self::Approved,
        Self::Returned,
        Self::Superseded,
    ];

    /// Name used in events, rows and displays.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Approved => "approved",
            Self::Returned => "returned",
            Self::Superseded => "superseded",
        }
    }

    /// Parses a name.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`artifact_state`).
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == text)
            .ok_or(CoordinationRefusal::FieldInvalid {
                field: "artifact_state",
            })
    }

    /// Whether the artifact is the current one of its phase.
    #[must_use]
    pub const fn is_current(self) -> bool {
        matches!(self, Self::Submitted | Self::Approved)
    }
}

/// Parses and validates a workflow: 1 to 7 phases, in canonical order, each once.
///
/// # Errors
///
/// [`CoordinationRefusal::FieldInvalid`] (`workflow`, or `phase` for an unknown name).
pub fn parse_workflow(names: &[String]) -> Result<Vec<Phase>, CoordinationRefusal> {
    let phases = names
        .iter()
        .map(|name| Phase::parse(name))
        .collect::<Result<Vec<_>, _>>()?;
    let increasing = phases
        .iter()
        .zip(phases.iter().skip(1))
        .all(|(earlier, later)| earlier < later);
    if phases.is_empty() || !increasing {
        return Err(CoordinationRefusal::FieldInvalid { field: "workflow" });
    }
    Ok(phases)
}

/// What the projection holds of a mission's phases, as far as the rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionPhases {
    /// State of the mission (`None`: unknown mission).
    pub state: Option<State>,
    /// Declared workflow (`None`: none declared).
    pub workflow: Option<Vec<Phase>>,
    /// The current artifact (`submitted` or `approved`) of each phase that has one.
    pub current: Vec<(Phase, ArtifactState)>,
}

impl MissionPhases {
    fn approved(&self, phase: Phase) -> bool {
        self.current
            .iter()
            .any(|(current, state)| *current == phase && *state == ArtifactState::Approved)
    }

    /// Declared phases without an approved artifact, in workflow order.
    #[must_use]
    pub fn unapproved(&self) -> Vec<Phase> {
        self.workflow
            .iter()
            .flatten()
            .copied()
            .filter(|phase| !self.approved(*phase))
            .collect()
    }

    /// The governing phase: the latest declared phase with an approved artifact.
    #[must_use]
    pub fn governing(&self) -> Option<Phase> {
        self.workflow
            .iter()
            .flatten()
            .copied()
            .filter(|phase| self.approved(*phase))
            .max()
    }
}

/// What the projection holds of one artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactView {
    /// Mission.
    pub mission: MissionId,
    /// State of the mission.
    pub mission_state: State,
    /// State of the artifact.
    pub state: ArtifactState,
    /// SHA-256 of its content.
    pub digest: Digest32,
}

/// What the owner decides on a submitted artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactDecision {
    /// Approve the content whose SHA-256 is `digest`.
    Approve {
        /// SHA-256 of the content the owner read.
        digest: Digest32,
        /// Why, optionally.
        reason: Option<String>,
    },
    /// Return it for a new submission.
    Return {
        /// Why.
        reason: String,
    },
}

/// What a decided phase command records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseEvent {
    /// `workflow.declared`.
    WorkflowDeclared {
        /// Mission.
        mission: MissionId,
        /// Phases, in canonical order.
        phases: Vec<Phase>,
    },
    /// `artifact.submitted`.
    Submitted {
        /// Mission.
        mission: MissionId,
        /// Artifact.
        artifact: ArtifactId,
        /// Phase.
        phase: Phase,
        /// Content.
        content: String,
        /// Who submitted it.
        actor: Actor,
    },
    /// `artifact.approved`.
    Approved {
        /// Mission.
        mission: MissionId,
        /// Artifact.
        artifact: ArtifactId,
        /// SHA-256 of the approved content.
        digest: Digest32,
        /// Why.
        reason: Option<String>,
    },
    /// `artifact.returned`.
    Returned {
        /// Mission.
        mission: MissionId,
        /// Artifact.
        artifact: ArtifactId,
        /// Why.
        reason: String,
    },
}

impl PhaseEvent {
    /// Journal kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::WorkflowDeclared { .. } => "workflow.declared",
            Self::Submitted { .. } => "artifact.submitted",
            Self::Approved { .. } => "artifact.approved",
            Self::Returned { .. } => "artifact.returned",
        }
    }
}

fn reason(text: &str) -> Result<String, CoordinationRefusal> {
    if text.trim().is_empty()
        || text.chars().count() > MAX_COORDINATION_LINE_CHARS
        || text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        Err(CoordinationRefusal::FieldInvalid { field: "reason" })
    } else {
        Ok(text.to_owned())
    }
}

/// Decides a workflow declaration (owner, mission in `draft`, once).
///
/// # Errors
///
/// [`CoordinationRefusal::OwnerOnly`], [`CoordinationRefusal::MissionNotFound`],
/// [`CoordinationRefusal::MissionNotDraft`],
/// [`CoordinationRefusal::WorkflowAlreadyDeclared`] and
/// [`CoordinationRefusal::FieldInvalid`].
pub fn decide_workflow(
    mission: &MissionId,
    current: &MissionPhases,
    names: &[String],
    actor: &Actor,
) -> Result<PhaseEvent, CoordinationRefusal> {
    if *actor != Actor::Owner {
        return Err(CoordinationRefusal::OwnerOnly);
    }
    crate::coordination::check_draft(current.state)?;
    if current.workflow.is_some() {
        return Err(CoordinationRefusal::WorkflowAlreadyDeclared);
    }
    Ok(PhaseEvent::WorkflowDeclared {
        mission: mission.clone(),
        phases: parse_workflow(names)?,
    })
}

/// Decides an artifact submission, in the refusal order of the specification.
///
/// # Errors
///
/// [`CoordinationRefusal::MissionNotFound`], [`CoordinationRefusal::ArtifactMissionDraft`],
/// [`CoordinationRefusal::ArtifactMissionClosed`], [`CoordinationRefusal::WorkflowUndeclared`],
/// [`CoordinationRefusal::PhaseUndeclared`], [`CoordinationRefusal::PhasePreviousUnapproved`]
/// and [`CoordinationRefusal::FieldInvalid`] (`content`).
pub fn decide_submission(
    mission: &MissionId,
    artifact: &ArtifactId,
    current: &MissionPhases,
    phase: Phase,
    content: &str,
    actor: &Actor,
) -> Result<PhaseEvent, CoordinationRefusal> {
    match current.state {
        None => return Err(CoordinationRefusal::MissionNotFound),
        Some(State::Draft) => return Err(CoordinationRefusal::ArtifactMissionDraft),
        Some(state) if state.is_terminal() => {
            return Err(CoordinationRefusal::ArtifactMissionClosed);
        }
        Some(_) => {}
    }
    let Some(workflow) = &current.workflow else {
        return Err(CoordinationRefusal::WorkflowUndeclared);
    };
    if !workflow.contains(&phase) {
        return Err(CoordinationRefusal::PhaseUndeclared);
    }
    if workflow
        .iter()
        .filter(|earlier| **earlier < phase)
        .any(|earlier| !current.approved(*earlier))
    {
        return Err(CoordinationRefusal::PhasePreviousUnapproved);
    }
    if content.trim().is_empty() || content.len() > MAX_ARTIFACT_BYTES || content.contains('\0') {
        return Err(CoordinationRefusal::FieldInvalid { field: "content" });
    }
    Ok(PhaseEvent::Submitted {
        mission: mission.clone(),
        artifact: artifact.clone(),
        phase,
        content: content.to_owned(),
        actor: actor.clone(),
    })
}

/// Decides the owner's approval or return of a submitted artifact.
///
/// # Errors
///
/// [`CoordinationRefusal::OwnerOnly`], [`CoordinationRefusal::ArtifactNotFound`],
/// [`CoordinationRefusal::ArtifactMissionClosed`], [`CoordinationRefusal::ArtifactNotPending`],
/// [`CoordinationRefusal::ArtifactDigestMismatch`] and [`CoordinationRefusal::FieldInvalid`].
pub fn decide_artifact(
    artifact: &ArtifactId,
    current: Option<&ArtifactView>,
    decision: &ArtifactDecision,
    actor: &Actor,
) -> Result<PhaseEvent, CoordinationRefusal> {
    if *actor != Actor::Owner {
        return Err(CoordinationRefusal::OwnerOnly);
    }
    let Some(view) = current else {
        return Err(CoordinationRefusal::ArtifactNotFound);
    };
    if view.mission_state.is_terminal() {
        return Err(CoordinationRefusal::ArtifactMissionClosed);
    }
    if view.state != ArtifactState::Submitted {
        return Err(CoordinationRefusal::ArtifactNotPending);
    }
    match decision {
        ArtifactDecision::Approve {
            digest,
            reason: why,
        } => {
            // The owner approves the text they read, not whatever the
            // identifier holds: a different digest is refused.
            if *digest != view.digest {
                return Err(CoordinationRefusal::ArtifactDigestMismatch);
            }
            Ok(PhaseEvent::Approved {
                mission: view.mission.clone(),
                artifact: artifact.clone(),
                digest: view.digest,
                reason: why.as_deref().map(reason).transpose()?,
            })
        }
        ArtifactDecision::Return { reason: why } => Ok(PhaseEvent::Returned {
            mission: view.mission.clone(),
            artifact: artifact.clone(),
            reason: reason(why)?,
        }),
    }
}

/// Inserts one `phase.unapproved` blocker per phase of `unapproved` after the
/// dependency and request blockers of `blockers`, the place the specification
/// gives them in the run and acceptance guards.
#[must_use]
pub fn with_phase_blockers(mut blockers: Vec<Blocker>, unapproved: &[Phase]) -> Vec<Blocker> {
    let position = blockers
        .iter()
        .position(|blocker| {
            !matches!(
                blocker,
                Blocker::DependencyUnsatisfiable(_)
                    | Blocker::DependencyPending(_)
                    | Blocker::RequestPending(_)
            )
        })
        .unwrap_or(blockers.len());
    blockers.splice(
        position..position,
        unapproved.iter().copied().map(Blocker::PhaseUnapproved),
    );
    blockers
}
