//! Pure mission state machine of Work Supervision v0.
//!
//! Single user, no tenant, no role, no third-party approval (ADR-0042 §5 in
//! `libre-ai/project-governance`): the decisions `accept`, `reject` and
//! `abandon` are the owner's. This crate does no I/O, reads no clock and draws
//! no random number; identifiers and instants are given by its caller.
//!
//! [`decide`] turns a command into one event or a [`Refusal`]; [`apply`]
//! folds an event into the mission. The table (`docs/work-supervision/mission-v0.md`):
//!
//! | From | Command | To |
//! | --- | --- | --- |
//! | — | create | draft |
//! | draft | ready | ready |
//! | ready | provision | provisioned |
//! | provisioned, rejected | start-run | running |
//! | running | await-input | waiting-input |
//! | waiting-input | resume-input | running |
//! | running, waiting-input | exit-run | exited |
//! | exited | submit-result | result-submitted |
//! | result-submitted | accept | accepted |
//! | result-submitted | reject | rejected |
//! | draft, ready, result-submitted, rejected | abandon | abandoned |
//! | provisioned, running, waiting-input, exited | cancel | cancelled |
//! | any | note | unchanged |
//!
//! Every other pair is refused (`mission.transition_forbidden`); a command
//! decided against another revision is refused first (`mission.revision_stale`).
//! Semantics ported, without code, from the recovered `apps/missions` domain:
//! closed refusals, optimistic revision, result and evidence required before
//! acceptance, terminal states without exit, reported activity distinct from a
//! validated result.
//!
//! ```
//! use work_supervision_domain::{
//!     Budgets, Command, ExecutorProfile, MissionId, State, apply, decide,
//! };
//!
//! let id = MissionId::from_bytes([7; 16]);
//! let create = Command::Create {
//!     title: "Add the parser".to_owned(),
//!     repository: "sample".to_owned(),
//!     brief: "print hello".to_owned(),
//!     criteria: vec!["tests pass".to_owned()],
//!     budgets: Budgets::new(600, 1 << 20)?,
//!     executor: ExecutorProfile::Fake,
//! };
//! let mission = apply(None, &decide(&id, None, &create, 0)?)?;
//! assert_eq!((mission.state(), mission.revision()), (State::Draft, 1));
//! let ready = apply(Some(mission.clone()), &decide(&id, Some(&mission), &Command::Ready, 1)?)?;
//! assert_eq!(ready.state(), State::Ready);
//! # Ok::<(), work_supervision_domain::Refusal>(())
//! ```

pub mod coordination;
mod ids;
mod model;
pub mod phases;
pub mod scope;

pub use ids::{
    ArtifactId, CheckId, CommitId, Digest32, IdeaId, MissionId, RequestId, RunId, SessionId,
};
pub use model::{
    Budgets, Command, CommandKind, ExecutorProfile, MAX_DURATION_SECONDS, MAX_OUTPUT_BYTES,
    Mission, MissionEvent, MissionParts, MissionResult, Refusal, State, Verdict, branch_of,
    worktree_of,
};

/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Largest brief, in bytes.
pub const MAX_BRIEF_BYTES: usize = 1 << 20;
/// Most acceptance criteria per mission.
pub const MAX_CRITERIA: usize = 32;
/// Longest criterion, reason or summary, in characters.
pub const MAX_LINE_TEXT_CHARS: usize = 4_000;
/// Largest note, in bytes.
pub const MAX_NOTE_BYTES: usize = 64 * 1024;

/// Decides `command` on the mission `id`, whose current value is `current`.
///
/// `expected_revision` is the revision the command was decided against: `0`
/// for a creation, the mission's revision otherwise. Notes are not ordered
/// against transitions and ignore it.
///
/// # Errors
///
/// [`Refusal::NotFound`], [`Refusal::AlreadyExists`],
/// [`Refusal::RevisionStale`], [`Refusal::TransitionForbidden`],
/// [`Refusal::ResultIncomplete`] and [`Refusal::FieldInvalid`].
pub fn decide(
    id: &MissionId,
    current: Option<&Mission>,
    command: &Command,
    expected_revision: u64,
) -> Result<MissionEvent, Refusal> {
    let Some(mission) = current else {
        return create(id, command, expected_revision);
    };
    let kind = command.kind();
    if kind == CommandKind::Create {
        return Err(Refusal::AlreadyExists);
    }
    if kind != CommandKind::Note && expected_revision != mission.revision {
        return Err(Refusal::RevisionStale {
            expected: expected_revision,
            actual: mission.revision,
        });
    }
    let forbidden = Refusal::TransitionForbidden {
        state: mission.state,
        command: kind,
    };
    let mission_id = mission.id.clone();
    let current_run = || mission.current_run.clone().ok_or(forbidden);
    let event = match (mission.state, command) {
        (_, Command::Note { text }) => MissionEvent::Noted {
            mission: mission_id,
            text: checked_text(text, MAX_NOTE_BYTES, "note")?,
        },
        (State::Draft, Command::Ready) => MissionEvent::Readied {
            mission: mission_id,
        },
        (State::Ready, Command::Provision { base_commit }) => MissionEvent::Provisioned {
            mission: mission_id,
            base_commit: base_commit.clone(),
        },
        (State::Provisioned | State::Rejected, Command::StartRun { run }) => {
            MissionEvent::RunStarted {
                mission: mission_id,
                run: run.clone(),
            }
        }
        (State::Running, Command::AwaitInput) => MissionEvent::InputAwaited {
            mission: mission_id,
            run: current_run()?,
        },
        (State::WaitingInput, Command::ResumeInput) => MissionEvent::InputResumed {
            mission: mission_id,
            run: current_run()?,
        },
        (State::Running | State::WaitingInput, Command::ExitRun { interrupted }) => {
            MissionEvent::RunExited {
                mission: mission_id,
                run: current_run()?,
                interrupted: *interrupted,
            }
        }
        (
            State::Exited,
            Command::SubmitResult {
                commit,
                evidence,
                summary,
            },
        ) => {
            let (Some(commit), Some(evidence)) = (commit, evidence) else {
                return Err(Refusal::ResultIncomplete);
            };
            if summary.trim().is_empty() {
                return Err(Refusal::ResultIncomplete);
            }
            MissionEvent::ResultSubmitted {
                mission: mission_id,
                result: MissionResult {
                    commit: commit.clone(),
                    evidence: *evidence,
                    summary: checked_line_text(summary, "summary")?,
                },
            }
        }
        (State::ResultSubmitted, Command::Accept { reason }) => {
            decided(mission_id, State::Accepted, reason)?
        }
        (State::ResultSubmitted, Command::Reject { reason }) => {
            decided(mission_id, State::Rejected, reason)?
        }
        (
            State::Draft | State::Ready | State::ResultSubmitted | State::Rejected,
            Command::Abandon { reason },
        ) => decided(mission_id, State::Abandoned, reason)?,
        (
            State::Provisioned | State::Running | State::WaitingInput | State::Exited,
            Command::Cancel { reason },
        ) => decided(mission_id, State::Cancelled, reason)?,
        _ => return Err(forbidden),
    };
    Ok(event)
}

fn create(
    id: &MissionId,
    command: &Command,
    expected_revision: u64,
) -> Result<MissionEvent, Refusal> {
    let Command::Create {
        title,
        repository,
        brief,
        criteria,
        budgets,
        executor,
    } = command
    else {
        return Err(Refusal::NotFound);
    };
    if expected_revision != 0 {
        return Err(Refusal::RevisionStale {
            expected: expected_revision,
            actual: 0,
        });
    }
    let title_ok = !title.trim().is_empty()
        && title.chars().count() <= MAX_TITLE_CHARS
        && !title.chars().any(char::is_control);
    if !title_ok {
        return Err(Refusal::FieldInvalid { field: "title" });
    }
    if !is_repository_name(repository) {
        return Err(Refusal::FieldInvalid {
            field: "repository",
        });
    }
    if brief.trim().is_empty() || brief.len() > MAX_BRIEF_BYTES {
        return Err(Refusal::FieldInvalid { field: "brief" });
    }
    let criteria_ok = criteria.len() <= MAX_CRITERIA
        && criteria.iter().all(|criterion| {
            !criterion.trim().is_empty() && criterion.chars().count() <= MAX_LINE_TEXT_CHARS
        });
    if !criteria_ok {
        return Err(Refusal::FieldInvalid { field: "criteria" });
    }
    Ok(MissionEvent::Created {
        mission: id.clone(),
        title: title.clone(),
        repository: repository.clone(),
        brief: brief.clone(),
        criteria: criteria.clone(),
        budgets: *budgets,
        executor: *executor,
    })
}

/// `[a-z0-9][a-z0-9._-]{0,63}`: a configuration key, never a path.
fn is_repository_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && name.len() <= 64
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

fn checked_text(text: &str, max_bytes: usize, field: &'static str) -> Result<String, Refusal> {
    if text.trim().is_empty() || text.len() > max_bytes {
        Err(Refusal::FieldInvalid { field })
    } else {
        Ok(text.to_owned())
    }
}

fn checked_line_text(text: &str, field: &'static str) -> Result<String, Refusal> {
    if text.trim().is_empty() || text.chars().count() > MAX_LINE_TEXT_CHARS {
        Err(Refusal::FieldInvalid { field })
    } else {
        Ok(text.to_owned())
    }
}

fn decided(mission: MissionId, state: State, reason: &str) -> Result<MissionEvent, Refusal> {
    Ok(MissionEvent::Decided {
        mission,
        verdict: Verdict {
            state,
            reason: checked_line_text(reason, "reason")?,
        },
    })
}

/// Folds `event` into `current`.
///
/// # Errors
///
/// [`Refusal::AlreadyExists`] when a creation meets an existing mission,
/// [`Refusal::NotFound`] when another event meets none: an event never applies
/// to a mission it does not belong to.
pub fn apply(current: Option<Mission>, event: &MissionEvent) -> Result<Mission, Refusal> {
    if let MissionEvent::Created {
        mission,
        title,
        repository,
        brief,
        criteria,
        budgets,
        executor,
    } = event
    {
        if current.is_some() {
            return Err(Refusal::AlreadyExists);
        }
        return Ok(Mission {
            id: mission.clone(),
            title: title.clone(),
            repository: repository.clone(),
            brief: brief.clone(),
            criteria: criteria.clone(),
            budgets: *budgets,
            executor: *executor,
            state: State::Draft,
            revision: 1,
            base_commit: None,
            current_run: None,
            result: None,
            verdict: None,
        });
    }
    let Some(mut mission) = current else {
        return Err(Refusal::NotFound);
    };
    if mission.id != *event.mission() {
        return Err(Refusal::NotFound);
    }
    match event {
        MissionEvent::Noted { .. } => return Ok(mission),
        MissionEvent::Provisioned { base_commit, .. } => {
            mission.base_commit = Some(base_commit.clone());
        }
        MissionEvent::RunStarted { run, .. } => {
            mission.current_run = Some(run.clone());
            mission.result = None;
            mission.verdict = None;
        }
        MissionEvent::ResultSubmitted { result, .. } => {
            mission.result = Some(result.clone());
        }
        MissionEvent::Decided { verdict, .. } => {
            mission.verdict = Some(verdict.clone());
        }
        MissionEvent::Created { .. }
        | MissionEvent::Readied { .. }
        | MissionEvent::InputAwaited { .. }
        | MissionEvent::InputResumed { .. }
        | MissionEvent::RunExited { .. } => {}
    }
    if let Some(state) = event.state_after() {
        mission.state = state;
    }
    mission.revision += 1;
    Ok(mission)
}
