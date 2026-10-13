#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use work_supervision_domain::coordination::{
    Actor, Blocker, CheckOutcome, CoordinationRefusal, Harness, IdeaCommand, IdeaEvent, IdeaState,
    IdeaView, ReportedState, RequestCommand, RequestEvent, RequestOption, RequestState,
    RequestView, Reversibility, SessionCommand, SessionEvent, SessionOutcome, SessionState,
    accept_blockers, apply_idea, check_actor_session, check_argv, check_dependency, check_draft,
    decide_idea, decide_request, decide_session, holds_worktree, run_blockers,
};
use work_supervision_domain::scope::{ScopePath, outside, overlaps, parse_scope};
use work_supervision_domain::{CommitId, IdeaId, MissionId, RequestId, SessionId, State};

fn mission(n: u8) -> MissionId {
    MissionId::from_bytes([n; 16])
}

fn idea() -> IdeaId {
    IdeaId::from_bytes([1; 16])
}

fn request_id(n: u8) -> RequestId {
    RequestId::from_bytes([n; 16])
}

fn session(n: u8) -> SessionId {
    SessionId::from_bytes([n; 16])
}

fn commit(c: char) -> CommitId {
    CommitId::parse(&c.to_string().repeat(40)).unwrap()
}

fn option(label: &str, reversibility: Reversibility) -> RequestOption {
    RequestOption {
        label: label.to_owned(),
        consequence: format!("consequence of {label}"),
        reversibility,
    }
}

// ------------------------------------------------------------------ actor ---

#[test]
fn actors_round_trip_and_reject_other_spellings() {
    let owner = Actor::Owner;
    let agent = Actor::Session(session(9));
    for actor in [owner, agent] {
        assert_eq!(Actor::parse(&actor.to_text()).unwrap(), actor);
    }
    for bad in [
        "",
        "Owner",
        "session:",
        "session:XYZ",
        "agent",
        "session:0909",
    ] {
        assert_eq!(
            Actor::parse(bad).unwrap_err().code(),
            "coordination.field_invalid",
            "{bad}"
        );
    }
}

#[test]
fn a_session_actor_must_name_an_active_session() {
    let agent = Actor::Session(session(2));
    assert!(check_actor_session(&Actor::Owner, None).is_ok());
    assert!(check_actor_session(&agent, Some(SessionState::Active)).is_ok());
    assert_eq!(
        check_actor_session(&agent, None),
        Err(CoordinationRefusal::SessionNotFound)
    );
    assert_eq!(
        check_actor_session(&agent, Some(SessionState::Ended)),
        Err(CoordinationRefusal::SessionEnded)
    );
}

#[test]
fn harness_names_are_closed_and_none_is_an_executor() {
    let names: Vec<&str> = Harness::ALL.iter().map(|h| h.as_str()).collect();
    assert_eq!(names, ["claude-code", "codex", "pi", "other"]);
    assert!(Harness::parse("claude").is_err());
    for name in names {
        // A harness named in a session never becomes an executor profile.
        assert_eq!(
            work_supervision_domain::ExecutorProfile::from_config_name(name)
                .unwrap_err()
                .code(),
            "agent.real_forbidden_until_c0"
        );
    }
}

// ------------------------------------------------------------------ ideas ---

#[test]
fn an_idea_is_captured_qualified_promoted_through_an_intent() {
    let session_actor = Actor::Session(session(3));
    let captured = decide_idea(
        &idea(),
        None,
        &IdeaCommand::Capture {
            text: "cache the parser output".to_owned(),
        },
        &session_actor,
    )
    .unwrap();
    assert_eq!(captured.kind(), "idea.captured");
    let view = apply_idea(None, &captured);
    assert_eq!(view.state, IdeaState::Captured);

    let qualified = decide_idea(
        &idea(),
        Some(&view),
        &IdeaCommand::Qualify {
            repository: Some("sample".to_owned()),
            mission: None,
            context: None,
        },
        &session_actor,
    )
    .unwrap();
    let view = apply_idea(Some(view), &qualified);
    assert_eq!(
        (view.state, view.ever_qualified),
        (IdeaState::Qualified, true)
    );

    // Promotion is the owner's.
    assert_eq!(
        decide_idea(
            &idea(),
            Some(&view),
            &IdeaCommand::PromoteIntent {
                mission: mission(5)
            },
            &session_actor
        ),
        Err(CoordinationRefusal::OwnerOnly)
    );
    let intent = decide_idea(
        &idea(),
        Some(&view),
        &IdeaCommand::PromoteIntent {
            mission: mission(5),
        },
        &Actor::Owner,
    )
    .unwrap();
    assert_eq!(intent.kind(), "idea.promotion.intent");
    let view = apply_idea(Some(view), &intent);
    assert_eq!(view.state, IdeaState::Promoting);
    let confirmed = decide_idea(
        &idea(),
        Some(&view),
        &IdeaCommand::ConfirmPromotion,
        &Actor::Owner,
    )
    .unwrap();
    assert_eq!(
        confirmed,
        IdeaEvent::Promoted {
            idea: idea(),
            mission: mission(5)
        }
    );
    let view = apply_idea(Some(view), &confirmed);
    assert_eq!(view.state, IdeaState::Promoted);
    // A promoted idea is closed.
    for command in [
        IdeaCommand::Dismiss {
            reason: "x".to_owned(),
        },
        IdeaCommand::AbortPromotion,
        IdeaCommand::PromoteIntent {
            mission: mission(6),
        },
    ] {
        assert_eq!(
            decide_idea(&idea(), Some(&view), &command, &Actor::Owner)
                .unwrap_err()
                .code(),
            "idea.transition_forbidden"
        );
    }
}

#[test]
fn an_aborted_promotion_returns_to_where_the_idea_was() {
    for ever_qualified in [false, true] {
        let view = IdeaView {
            state: IdeaState::Promoting,
            ever_qualified,
            promoting: Some(mission(1)),
        };
        let aborted = decide_idea(
            &idea(),
            Some(&view),
            &IdeaCommand::AbortPromotion,
            &Actor::Owner,
        )
        .unwrap();
        let after = apply_idea(Some(view), &aborted);
        let expected = if ever_qualified {
            IdeaState::Qualified
        } else {
            IdeaState::Captured
        };
        assert_eq!((after.state, after.promoting), (expected, None));
    }
}

#[test]
fn idea_refusals_are_closed() {
    assert_eq!(
        decide_idea(&idea(), None, &IdeaCommand::AbortPromotion, &Actor::Owner),
        Err(CoordinationRefusal::IdeaNotFound)
    );
    for text in ["", "   ", &"x".repeat(64 * 1024 + 1)] {
        assert_eq!(
            decide_idea(
                &idea(),
                None,
                &IdeaCommand::Capture {
                    text: text.to_owned()
                },
                &Actor::Owner
            )
            .unwrap_err()
            .code(),
            "coordination.field_invalid"
        );
    }
    let view = IdeaView {
        state: IdeaState::Captured,
        ever_qualified: false,
        promoting: None,
    };
    let qualify = |repository: Option<&str>| IdeaCommand::Qualify {
        repository: repository.map(str::to_owned),
        mission: None,
        context: None,
    };
    // Qualifying with nothing, or with an invalid repository name.
    for command in [
        qualify(None),
        qualify(Some("../etc")),
        qualify(Some("Sample")),
    ] {
        assert!(decide_idea(&idea(), Some(&view), &command, &Actor::Owner).is_err());
    }
    // Confirming without an intent.
    assert!(
        decide_idea(
            &idea(),
            Some(&view),
            &IdeaCommand::ConfirmPromotion,
            &Actor::Owner
        )
        .is_err()
    );
    assert_eq!(
        decide_idea(
            &idea(),
            Some(&view),
            &IdeaCommand::Dismiss {
                reason: "no".to_owned()
            },
            &Actor::Session(session(1))
        ),
        Err(CoordinationRefusal::OwnerOnly)
    );
}

// --------------------------------------------------------------- requests ---

fn open(options: Vec<RequestOption>, recommended: Option<usize>) -> RequestCommand {
    RequestCommand::Open {
        mission: Some((mission(1), Some(State::Running))),
        question: "Which storage?".to_owned(),
        options,
        recommended,
    }
}

#[test]
fn a_request_is_opened_by_a_session_and_answered_by_the_owner_only() {
    let agent = Actor::Session(session(4));
    let opened = decide_request(
        &request_id(1),
        None,
        &open(
            vec![
                option("SQLite", Reversibility::Reversible),
                option("PostgreSQL", Reversibility::Costly),
            ],
            Some(0),
        ),
        &agent,
    )
    .unwrap();
    let RequestEvent::Opened {
        mission: Some(on),
        options,
        recommended,
        ..
    } = &opened
    else {
        panic!("expected an opened request")
    };
    assert_eq!((on, options.len(), *recommended), (&mission(1), 2, Some(0)));

    let view = RequestView {
        state: RequestState::Open,
        opener: agent.clone(),
        options: 2,
    };
    let answer = |choice| RequestCommand::Answer {
        choice,
        reason: "fits v0".to_owned(),
    };
    assert_eq!(
        decide_request(&request_id(1), Some(&view), &answer(0), &agent),
        Err(CoordinationRefusal::OwnerOnly)
    );
    assert_eq!(
        decide_request(&request_id(1), Some(&view), &answer(2), &Actor::Owner),
        Err(CoordinationRefusal::RequestChoiceInvalid)
    );
    let answered = decide_request(&request_id(1), Some(&view), &answer(1), &Actor::Owner).unwrap();
    assert_eq!(answered.kind(), "request.answered");

    let closed = RequestView {
        state: RequestState::Answered,
        ..view
    };
    assert_eq!(
        decide_request(&request_id(1), Some(&closed), &answer(0), &Actor::Owner),
        Err(CoordinationRefusal::RequestNotOpen)
    );
}

#[test]
fn only_the_owner_or_the_opener_withdraws_a_request() {
    let view = RequestView {
        state: RequestState::Open,
        opener: Actor::Session(session(4)),
        options: 2,
    };
    let withdraw = RequestCommand::Withdraw {
        reason: "obsolete".to_owned(),
    };
    for (actor, allowed) in [
        (Actor::Owner, true),
        (Actor::Session(session(4)), true),
        (Actor::Session(session(5)), false),
    ] {
        let outcome = decide_request(&request_id(1), Some(&view), &withdraw, &actor);
        assert_eq!(outcome.is_ok(), allowed, "{actor:?}");
    }
}

#[test]
fn request_shape_is_checked() {
    let two = || {
        vec![
            option("A", Reversibility::Reversible),
            option("B", Reversibility::Irreversible),
        ]
    };
    let cases = [
        open(vec![option("A", Reversibility::Reversible)], None),
        open(
            (0..5)
                .map(|n| option(&n.to_string(), Reversibility::Reversible))
                .collect(),
            None,
        ),
        open(
            vec![
                option("A", Reversibility::Reversible),
                option("A", Reversibility::Costly),
            ],
            None,
        ),
        open(two(), Some(2)),
        open(
            vec![
                option("A\nB", Reversibility::Reversible),
                option("C", Reversibility::Costly),
            ],
            None,
        ),
    ];
    for command in cases {
        assert_eq!(
            decide_request(&request_id(1), None, &command, &Actor::Owner)
                .unwrap_err()
                .code(),
            "coordination.field_invalid",
            "{command:?}"
        );
    }
    let on = |state| RequestCommand::Open {
        mission: Some((mission(1), state)),
        question: "?".to_owned(),
        options: two(),
        recommended: None,
    };
    assert_eq!(
        decide_request(&request_id(1), None, &on(None), &Actor::Owner),
        Err(CoordinationRefusal::MissionNotFound)
    );
    assert_eq!(
        decide_request(
            &request_id(1),
            None,
            &on(Some(State::Accepted)),
            &Actor::Owner
        ),
        Err(CoordinationRefusal::RequestMissionClosed)
    );
    assert_eq!(
        decide_request(
            &request_id(1),
            None,
            &RequestCommand::Withdraw {
                reason: "x".to_owned()
            },
            &Actor::Owner
        ),
        Err(CoordinationRefusal::RequestNotFound)
    );
}

// --------------------------------------------------------------- sessions ---

fn register(mission: Option<MissionId>) -> SessionCommand {
    SessionCommand::Register {
        harness: Harness::ClaudeCode,
        repository: Some("sample".to_owned()),
        mission,
        label: "refactor the parser".to_owned(),
        external: None,
    }
}

#[test]
fn a_session_registers_reports_for_itself_and_ends() {
    let id = session(7);
    let me = Actor::Session(id.clone());
    let registered = decide_session(&id, None, &register(None), &Actor::Owner, false).unwrap();
    assert!(matches!(
        registered,
        SessionEvent::Registered {
            harness: Harness::ClaudeCode,
            ..
        }
    ));
    let report = SessionCommand::Report {
        state: ReportedState::WaitingInput,
        note: None,
    };
    assert!(decide_session(&id, Some(SessionState::Active), &report, &me, false).is_ok());
    assert_eq!(
        decide_session(
            &id,
            Some(SessionState::Active),
            &report,
            &Actor::Session(session(8)),
            false
        ),
        Err(CoordinationRefusal::NotItsOwn)
    );
    // The owner does not report for a session, but may end a silent one.
    assert_eq!(
        decide_session(
            &id,
            Some(SessionState::Active),
            &report,
            &Actor::Owner,
            false
        ),
        Err(CoordinationRefusal::NotItsOwn)
    );
    let end = SessionCommand::End {
        outcome: SessionOutcome::Abandoned,
        summary: None,
    };
    assert!(decide_session(&id, Some(SessionState::Active), &end, &Actor::Owner, false).is_ok());
    assert_eq!(
        decide_session(&id, Some(SessionState::Ended), &report, &me, false),
        Err(CoordinationRefusal::SessionEnded)
    );
    assert_eq!(
        decide_session(&id, None, &report, &me, false),
        Err(CoordinationRefusal::SessionNotFound)
    );
}

#[test]
fn a_session_on_an_unknown_mission_is_refused() {
    assert_eq!(
        decide_session(
            &session(1),
            None,
            &register(Some(mission(3))),
            &Actor::Owner,
            false
        ),
        Err(CoordinationRefusal::MissionNotFound)
    );
    assert!(
        decide_session(
            &session(1),
            None,
            &register(Some(mission(3))),
            &Actor::Owner,
            true
        )
        .is_ok()
    );
}

// ---------------------------------------------------- contract refinements ---

#[test]
fn refinements_are_declared_in_draft_only() {
    assert!(check_draft(Some(State::Draft)).is_ok());
    assert_eq!(check_draft(None), Err(CoordinationRefusal::MissionNotFound));
    for state in State::ALL.into_iter().filter(|s| *s != State::Draft) {
        assert_eq!(
            check_draft(Some(state)),
            Err(CoordinationRefusal::MissionNotDraft)
        );
    }
}

#[test]
fn dependencies_refuse_self_duplicates_unknown_missions_and_cycles() {
    let edges = vec![(mission(2), mission(3)), (mission(3), mission(4))];
    assert!(check_dependency(&mission(1), &mission(2), true, &edges).is_ok());
    assert_eq!(
        check_dependency(&mission(1), &mission(9), false, &edges),
        Err(CoordinationRefusal::MissionNotFound)
    );
    assert_eq!(
        check_dependency(&mission(1), &mission(1), true, &edges),
        Err(CoordinationRefusal::DependencyDuplicate)
    );
    assert_eq!(
        check_dependency(&mission(2), &mission(3), true, &edges),
        Err(CoordinationRefusal::DependencyDuplicate)
    );
    // 4 → 2 closes 2 → 3 → 4 → 2.
    assert_eq!(
        check_dependency(&mission(4), &mission(2), true, &edges),
        Err(CoordinationRefusal::DependencyCycle)
    );
    let many: Vec<_> = (10..42).map(|n| (mission(1), mission(n))).collect();
    assert_eq!(
        check_dependency(&mission(1), &mission(50), true, &many)
            .unwrap_err()
            .code(),
        "coordination.field_invalid"
    );
}

#[test]
fn check_argument_vectors_are_bounded() {
    let argv = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    assert!(check_argv(&argv(&["cargo", "test"])).is_ok());
    for bad in [
        argv(&[]),
        argv(&[" "]),
        argv(&["a\0b"]),
        vec!["x".repeat(4097)],
        vec!["x".to_owned(); 65],
    ] {
        assert!(check_argv(&bad).is_err());
    }
}

// ------------------------------------------------------------------ scope ---

#[test]
fn scope_prefixes_are_validated_and_normalised() {
    let parsed = parse_scope(&["src/b", "docs/", "src/b"]).unwrap();
    let names: Vec<&str> = parsed.iter().map(ScopePath::as_str).collect();
    assert_eq!(names, ["docs", "src/b"]);
    for bad in [
        "",
        "/",
        "/etc",
        "a//b",
        "./a",
        "a/../b",
        "..",
        "a\\b",
        "a\nb",
        &"a".repeat(257),
    ] {
        assert!(ScopePath::parse(bad).is_err(), "{bad:?}");
    }
    assert!(parse_scope::<&str>(&[]).is_err());
    assert!(parse_scope(&vec!["a"; 33]).is_err());
}

#[test]
fn a_prefix_covers_by_whole_segments() {
    let prefix = ScopePath::parse("src/a").unwrap();
    assert!(prefix.covers("src/a"));
    assert!(prefix.covers("src/a/b.rs"));
    assert!(!prefix.covers("src/ab.rs"));
    assert!(!prefix.covers("src"));
}

#[test]
fn scopes_overlap_when_one_prefix_covers_the_other_or_one_is_undeclared() {
    let scope = |paths: &[&str]| Some(parse_scope(paths).unwrap());
    assert!(overlaps(&scope(&["src"]), &scope(&["src/a"])));
    assert!(overlaps(&scope(&["src/a"]), &scope(&["src"])));
    assert!(!overlaps(&scope(&["src/a"]), &scope(&["src/ab", "docs"])));
    assert!(overlaps(&None, &scope(&["docs"])));
    assert!(overlaps(&scope(&["docs"]), &None));
    assert!(overlaps(&None, &None));
}

#[test]
fn outside_lists_uncovered_changes_and_nothing_for_an_undeclared_scope() {
    let changed = vec![
        "src/a/x.rs".to_owned(),
        "src/ab.rs".to_owned(),
        "README.md".to_owned(),
    ];
    let scope = Some(parse_scope(&["src/a"]).unwrap());
    assert_eq!(outside(&scope, &changed), ["src/ab.rs", "README.md"]);
    assert!(outside(&None, &changed).is_empty());
}

// --------------------------------------------------------------- blockers ---

#[test]
fn run_blockers_follow_the_specified_order() {
    let blockers = run_blockers(
        &[
            (mission(2), State::Running),
            (mission(3), State::Cancelled),
            (mission(4), State::Accepted),
        ],
        &[request_id(1)],
        &[mission(5)],
    );
    let codes: Vec<&str> = blockers.iter().map(Blocker::code).collect();
    assert_eq!(
        codes,
        [
            "dependency.unsatisfiable",
            "dependency.pending",
            "request.pending",
            "scope.conflict"
        ]
    );
    assert!(run_blockers(&[(mission(4), State::Accepted)], &[], &[]).is_empty());
}

#[test]
fn accept_blockers_require_a_passing_check_at_the_submitted_commit() {
    let submitted = commit('a');
    let passed_elsewhere = CheckOutcome {
        criterion: 0,
        commit: commit('b'),
        passed: true,
    };
    let failed_here = CheckOutcome {
        criterion: 1,
        commit: submitted.clone(),
        passed: false,
    };
    let blockers = accept_blockers(
        &[request_id(1)],
        true,
        &[0, 1],
        &[passed_elsewhere, failed_here],
        Some(&submitted),
        Some(2),
    );
    assert_eq!(
        blockers,
        [
            Blocker::RequestPending(request_id(1)),
            Blocker::CheckRunning,
            Blocker::CriteriaUnverified(0),
            Blocker::CriteriaUnverified(1),
            Blocker::ScopeViolated(2),
        ]
    );
    let passing = CheckOutcome {
        criterion: 0,
        commit: submitted.clone(),
        passed: true,
    };
    assert!(accept_blockers(&[], false, &[0], &[passing], Some(&submitted), Some(0)).is_empty());
    assert!(accept_blockers(&[], false, &[], &[], None, None).is_empty());
}

#[test]
fn only_worktree_holding_states_can_conflict() {
    let holding: Vec<&str> = State::ALL
        .into_iter()
        .filter(|state| holds_worktree(*state))
        .map(State::as_str)
        .collect();
    assert_eq!(
        holding,
        [
            "provisioned",
            "running",
            "waiting-input",
            "exited",
            "result-submitted",
            "rejected"
        ]
    );
}
