#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use work_supervision_domain::{
    Budgets, Command, CommandKind, CommitId, Digest32, ExecutorProfile, Mission, MissionEvent,
    MissionId, Refusal, RunId, State, apply, decide,
};

const ID: &str = "0123456789abcdef0123456789abcdef";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const RUN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EVIDENCE: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn id() -> MissionId {
    MissionId::parse(ID).unwrap()
}

fn create() -> Command {
    Command::Create {
        title: "Add the parser".to_owned(),
        repository: "sample".to_owned(),
        brief: "print hello\n".to_owned(),
        criteria: vec!["tests pass".to_owned()],
        budgets: Budgets::new(600, 1 << 20).unwrap(),
        executor: ExecutorProfile::Fake,
    }
}

/// A valid command of each kind, for the mission `ID`.
fn command(kind: CommandKind) -> Command {
    match kind {
        CommandKind::Create => create(),
        CommandKind::Ready => Command::Ready,
        CommandKind::Provision => Command::Provision {
            base_commit: CommitId::parse(COMMIT).unwrap(),
        },
        CommandKind::StartRun => Command::StartRun {
            run: RunId::parse(RUN).unwrap(),
        },
        CommandKind::AwaitInput => Command::AwaitInput,
        CommandKind::ResumeInput => Command::ResumeInput,
        CommandKind::ExitRun => Command::ExitRun { interrupted: false },
        CommandKind::SubmitResult => Command::SubmitResult {
            commit: Some(CommitId::parse(COMMIT).unwrap()),
            evidence: Some(Digest32::parse(EVIDENCE).unwrap()),
            summary: "did it".to_owned(),
        },
        CommandKind::Accept => Command::Accept {
            reason: "ok".to_owned(),
        },
        CommandKind::Reject => Command::Reject {
            reason: "not yet".to_owned(),
        },
        CommandKind::Abandon => Command::Abandon {
            reason: "dropped".to_owned(),
        },
        CommandKind::Cancel => Command::Cancel {
            reason: "stop".to_owned(),
        },
        CommandKind::Note => Command::Note {
            text: "a note".to_owned(),
        },
    }
}

/// Path of commands from creation to each state.
fn path_to(state: State) -> Vec<CommandKind> {
    use CommandKind::*;
    let mut path = vec![Create];
    let tail: &[CommandKind] = match state {
        State::Draft => &[],
        State::Ready => &[Ready],
        State::Provisioned => &[Ready, Provision],
        State::Running => &[Ready, Provision, StartRun],
        State::WaitingInput => &[Ready, Provision, StartRun, AwaitInput],
        State::Exited => &[Ready, Provision, StartRun, ExitRun],
        State::ResultSubmitted => &[Ready, Provision, StartRun, ExitRun, SubmitResult],
        State::Accepted => &[Ready, Provision, StartRun, ExitRun, SubmitResult, Accept],
        State::Rejected => &[Ready, Provision, StartRun, ExitRun, SubmitResult, Reject],
        State::Abandoned => &[Abandon],
        State::Cancelled => &[Ready, Provision, Cancel],
    };
    path.extend_from_slice(tail);
    path
}

fn mission_in(state: State) -> Mission {
    let mut mission: Option<Mission> = None;
    for kind in path_to(state) {
        let revision = mission.as_ref().map_or(0, Mission::revision);
        let event = decide(&id(), mission.as_ref(), &command(kind), revision).unwrap();
        mission = Some(apply(mission, &event).unwrap());
    }
    let mission = mission.unwrap();
    assert_eq!(mission.state(), state);
    mission
}

/// The v0 transition table, written independently of the implementation:
/// `Some(target)` when the command is allowed in the state, `None` when it is
/// refused with `mission.transition_forbidden`.
fn expected(state: State, kind: CommandKind) -> Option<State> {
    use CommandKind as K;
    use State as S;
    match (state, kind) {
        (_, K::Note) => Some(state),
        (S::Draft, K::Ready) => Some(S::Ready),
        (S::Ready, K::Provision) => Some(S::Provisioned),
        (S::Provisioned | S::Rejected, K::StartRun) => Some(S::Running),
        (S::Running, K::AwaitInput) => Some(S::WaitingInput),
        (S::WaitingInput, K::ResumeInput) => Some(S::Running),
        (S::Running | S::WaitingInput, K::ExitRun) => Some(S::Exited),
        (S::Exited, K::SubmitResult) => Some(S::ResultSubmitted),
        (S::ResultSubmitted, K::Accept) => Some(S::Accepted),
        (S::ResultSubmitted, K::Reject) => Some(S::Rejected),
        (S::Draft | S::Ready | S::ResultSubmitted | S::Rejected, K::Abandon) => Some(S::Abandoned),
        (S::Provisioned | S::Running | S::WaitingInput | S::Exited, K::Cancel) => {
            Some(S::Cancelled)
        }
        _ => None,
    }
}

#[test]
fn every_state_and_command_pair_follows_the_transition_table() {
    let mut covered = 0;
    let mut allowed = 0;
    for state in State::ALL {
        for kind in CommandKind::ALL {
            covered += 1;
            let mission = mission_in(state);
            let outcome = decide(&id(), Some(&mission), &command(kind), mission.revision());
            match (kind, expected(state, kind)) {
                (CommandKind::Create, _) => {
                    assert_eq!(outcome, Err(Refusal::AlreadyExists), "{state:?} × Create");
                }
                (_, None) => {
                    assert_eq!(
                        outcome,
                        Err(Refusal::TransitionForbidden {
                            state,
                            command: kind
                        }),
                        "{state:?} × {kind:?}"
                    );
                }
                (_, Some(target)) => {
                    allowed += 1;
                    let event = outcome
                        .unwrap_or_else(|refusal| panic!("{state:?} × {kind:?}: {refusal:?}"));
                    let after = apply(Some(mission.clone()), &event).unwrap();
                    assert_eq!(after.state(), target, "{state:?} × {kind:?}");
                    let bump = u64::from(kind != CommandKind::Note);
                    assert_eq!(
                        after.revision(),
                        mission.revision() + bump,
                        "{state:?} × {kind:?}"
                    );
                }
            }
        }
    }
    assert_eq!(covered, 11 * 13, "every (state, command) pair is exercised");
    assert_eq!(allowed, 30, "11 notes and 19 transitions are allowed");
}

#[test]
fn terminal_states_have_no_exit_but_notes() {
    for state in State::ALL.into_iter().filter(|state| state.is_terminal()) {
        let mission = mission_in(state);
        for kind in CommandKind::ALL {
            let allowed = decide(&id(), Some(&mission), &command(kind), mission.revision()).is_ok();
            assert_eq!(allowed, kind == CommandKind::Note, "{state:?} × {kind:?}");
        }
    }
    let terminal: Vec<State> = State::ALL.into_iter().filter(|s| s.is_terminal()).collect();
    assert_eq!(
        terminal,
        vec![State::Accepted, State::Abandoned, State::Cancelled]
    );
}

#[test]
fn every_command_on_an_absent_mission_but_create_is_not_found() {
    for kind in CommandKind::ALL {
        let outcome = decide(&id(), None, &command(kind), 0);
        if kind == CommandKind::Create {
            let event = outcome.unwrap();
            let mission = apply(None, &event).unwrap();
            assert_eq!((mission.state(), mission.revision()), (State::Draft, 1));
        } else {
            assert_eq!(outcome, Err(Refusal::NotFound), "{kind:?}");
        }
    }
}

#[test]
fn a_stale_revision_is_refused_before_the_transition_is_examined() {
    let mission = mission_in(State::Ready);
    for stale in [0, mission.revision() - 1, mission.revision() + 1] {
        assert_eq!(
            decide(
                &id(),
                Some(&mission),
                &command(CommandKind::Provision),
                stale
            ),
            Err(Refusal::RevisionStale {
                expected: stale,
                actual: mission.revision()
            })
        );
        // Even a forbidden command reports the stale revision first.
        assert_eq!(
            decide(&id(), Some(&mission), &command(CommandKind::Accept), stale),
            Err(Refusal::RevisionStale {
                expected: stale,
                actual: mission.revision()
            })
        );
    }
    // Creation expects revision 0 (no mission yet).
    assert_eq!(
        decide(&id(), None, &create(), 3),
        Err(Refusal::RevisionStale {
            expected: 3,
            actual: 0
        })
    );
    // Notes are not ordered against transitions and carry no revision.
    assert!(decide(&id(), Some(&mission), &command(CommandKind::Note), 99).is_ok());
}

#[test]
fn a_result_without_commit_evidence_or_summary_is_refused_so_acceptance_needs_proof() {
    let mission = mission_in(State::Exited);
    let incomplete = [
        Command::SubmitResult {
            commit: None,
            evidence: Some(Digest32::parse(EVIDENCE).unwrap()),
            summary: "s".to_owned(),
        },
        Command::SubmitResult {
            commit: Some(CommitId::parse(COMMIT).unwrap()),
            evidence: None,
            summary: "s".to_owned(),
        },
        Command::SubmitResult {
            commit: Some(CommitId::parse(COMMIT).unwrap()),
            evidence: Some(Digest32::parse(EVIDENCE).unwrap()),
            summary: "   ".to_owned(),
        },
    ];
    for command in incomplete {
        assert_eq!(
            decide(&id(), Some(&mission), &command, mission.revision()),
            Err(Refusal::ResultIncomplete)
        );
    }
    // Acceptance from exited, without a submitted result, is a forbidden transition.
    assert_eq!(
        decide(
            &id(),
            Some(&mission),
            &command(CommandKind::Accept),
            mission.revision()
        ),
        Err(Refusal::TransitionForbidden {
            state: State::Exited,
            command: CommandKind::Accept
        })
    );
    let accepted = mission_in(State::Accepted);
    let result = accepted.result().unwrap();
    assert_eq!(result.commit().as_str(), COMMIT);
    assert_eq!(result.evidence().to_hex(), EVIDENCE);
}

#[test]
fn resumption_after_rejection_clears_the_previous_result_and_verdict() {
    let rejected = mission_in(State::Rejected);
    assert!(rejected.result().is_some());
    assert!(rejected.verdict().is_some());
    let event = decide(
        &id(),
        Some(&rejected),
        &command(CommandKind::StartRun),
        rejected.revision(),
    )
    .unwrap();
    let running = apply(Some(rejected), &event).unwrap();
    assert_eq!(running.state(), State::Running);
    assert!(running.result().is_none());
    assert!(running.verdict().is_none());
    assert_eq!(running.current_run().map(RunId::as_str), Some(RUN));
}

#[test]
fn provisioning_derives_the_branch_and_worktree_from_the_mission_id() {
    let mission = mission_in(State::Provisioned);
    assert_eq!(
        mission.branch().as_deref(),
        Some(format!("ws/{ID}").as_str())
    );
    assert_eq!(
        mission.worktree().as_deref(),
        Some(format!("worktrees/{ID}").as_str())
    );
    assert_eq!(mission.base_commit().map(CommitId::as_str), Some(COMMIT));
}

#[test]
fn creation_fields_are_validated() {
    let base = || match create() {
        Command::Create {
            title,
            repository,
            brief,
            criteria,
            budgets,
            executor,
        } => (title, repository, brief, criteria, budgets, executor),
        _ => unreachable!(),
    };
    let cases: Vec<(Command, &str)> = vec![
        {
            let (_, r, b, c, bu, e) = base();
            (
                Command::Create {
                    title: " ".to_owned(),
                    repository: r,
                    brief: b,
                    criteria: c,
                    budgets: bu,
                    executor: e,
                },
                "title",
            )
        },
        {
            let (_, r, b, c, bu, e) = base();
            (
                Command::Create {
                    title: "a\u{7}b".to_owned(),
                    repository: r,
                    brief: b,
                    criteria: c,
                    budgets: bu,
                    executor: e,
                },
                "title",
            )
        },
        {
            let (t, _, b, c, bu, e) = base();
            (
                Command::Create {
                    title: t,
                    repository: "../etc".to_owned(),
                    brief: b,
                    criteria: c,
                    budgets: bu,
                    executor: e,
                },
                "repository",
            )
        },
        {
            let (t, r, _, c, bu, e) = base();
            (
                Command::Create {
                    title: t,
                    repository: r,
                    brief: String::new(),
                    criteria: c,
                    budgets: bu,
                    executor: e,
                },
                "brief",
            )
        },
        {
            let (t, r, b, _, bu, e) = base();
            (
                Command::Create {
                    title: t,
                    repository: r,
                    brief: b,
                    criteria: vec![String::new()],
                    budgets: bu,
                    executor: e,
                },
                "criteria",
            )
        },
    ];
    for (command, field) in cases {
        assert_eq!(
            decide(&id(), None, &command, 0),
            Err(Refusal::FieldInvalid { field })
        );
    }
    assert_eq!(
        Budgets::new(0, 1).unwrap_err(),
        Refusal::FieldInvalid { field: "budgets" }
    );
    assert_eq!(
        Budgets::new(1, 0).unwrap_err(),
        Refusal::FieldInvalid { field: "budgets" }
    );
    assert_eq!(
        Budgets::new(86_401, 1).unwrap_err(),
        Refusal::FieldInvalid { field: "budgets" }
    );
    assert!(Budgets::new(86_400, 1 << 30).is_ok());
    assert_eq!(
        Budgets::new(1, (1 << 30) + 1).unwrap_err(),
        Refusal::FieldInvalid { field: "budgets" }
    );
}

#[test]
fn identifiers_are_lowercase_hexadecimal_of_the_exact_length() {
    assert!(MissionId::parse(ID).is_ok());
    assert!(MissionId::parse(&ID.to_uppercase()).is_err());
    assert!(MissionId::parse(&ID[..31]).is_err());
    assert!(RunId::parse("g".repeat(32).as_str()).is_err());
    assert!(CommitId::parse(COMMIT).is_ok());
    assert!(CommitId::parse(&"a".repeat(64)).is_ok());
    assert!(CommitId::parse(&"a".repeat(41)).is_err());
    assert!(Digest32::parse(EVIDENCE).is_ok());
    assert!(Digest32::parse(&EVIDENCE[..63]).is_err());
    assert_eq!(MissionId::from_bytes([0xab; 16]).as_str(), "ab".repeat(16));
}

#[test]
fn the_c0_guard_admits_only_the_fake_executor() {
    assert_eq!(
        ExecutorProfile::from_config_name("fake"),
        Ok(ExecutorProfile::Fake)
    );
    for real in [
        "claude",
        "codex",
        "pi",
        "/usr/local/bin/claude",
        "Fake",
        "",
        "fake ",
    ] {
        let refusal = ExecutorProfile::from_config_name(real).unwrap_err();
        assert_eq!(refusal, Refusal::RealAgentForbiddenUntilC0);
        assert_eq!(refusal.code(), "agent.real_forbidden_until_c0");
    }
    assert_eq!(
        ExecutorProfile::ALL,
        [ExecutorProfile::Fake],
        "one profile only before C0"
    );
}

#[test]
fn refusal_codes_are_stable_and_carry_no_content() {
    let refusals = [
        (Refusal::NotFound, "mission.not_found"),
        (Refusal::AlreadyExists, "mission.already_exists"),
        (
            Refusal::RevisionStale {
                expected: 1,
                actual: 2,
            },
            "mission.revision_stale",
        ),
        (
            Refusal::TransitionForbidden {
                state: State::Draft,
                command: CommandKind::Accept,
            },
            "mission.transition_forbidden",
        ),
        (Refusal::ResultIncomplete, "mission.result_incomplete"),
        (
            Refusal::FieldInvalid { field: "title" },
            "mission.field_invalid",
        ),
        (
            Refusal::RealAgentForbiddenUntilC0,
            "agent.real_forbidden_until_c0",
        ),
    ];
    for (refusal, code) in refusals {
        assert_eq!(refusal.code(), code);
        assert!(refusal.to_string().starts_with(code));
    }
}

#[test]
fn events_name_the_mission_state_after_them() {
    let draft = mission_in(State::Draft);
    let event = decide(&id(), Some(&draft), &command(CommandKind::Ready), 1).unwrap();
    assert!(matches!(event, MissionEvent::Readied { .. }));
    assert_eq!(event.kind(), "mission.readied");
    assert_eq!(event.state_after(), Some(State::Ready));
    let note = decide(&id(), Some(&draft), &command(CommandKind::Note), 1).unwrap();
    assert_eq!(note.kind(), "mission.noted");
    assert_eq!(note.state_after(), None);
    assert_eq!(State::parse("waiting-input"), Some(State::WaitingInput));
    assert_eq!(State::WaitingInput.as_str(), "waiting-input");
    for state in State::ALL {
        assert_eq!(State::parse(state.as_str()), Some(state));
    }
}
