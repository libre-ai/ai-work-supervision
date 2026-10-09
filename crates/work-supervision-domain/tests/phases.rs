#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use work_supervision_domain::coordination::{Actor, Blocker, CoordinationRefusal};
use work_supervision_domain::phases::{
    ArtifactDecision, ArtifactState, ArtifactView, MAX_ARTIFACT_BYTES, MissionPhases, Phase,
    PhaseEvent, decide_artifact, decide_submission, decide_workflow, parse_workflow,
    with_phase_blockers,
};
use work_supervision_domain::{ArtifactId, Digest32, MissionId, RequestId, SessionId, State};

fn mission() -> MissionId {
    MissionId::from_bytes([1; 16])
}

fn artifact() -> ArtifactId {
    ArtifactId::from_bytes([2; 16])
}

fn names(phases: &[&str]) -> Vec<String> {
    phases.iter().map(|name| (*name).to_owned()).collect()
}

fn phases(state: State, workflow: &[Phase], current: &[(Phase, ArtifactState)]) -> MissionPhases {
    MissionPhases {
        state: Some(state),
        workflow: Some(workflow.to_vec()),
        current: current.to_vec(),
    }
}

fn session() -> Actor {
    Actor::Session(SessionId::from_bytes([3; 16]))
}

fn submit(current: &MissionPhases, phase: Phase) -> Result<PhaseEvent, CoordinationRefusal> {
    decide_submission(
        &mission(),
        &artifact(),
        current,
        phase,
        "# Findings\n",
        &Actor::Owner,
    )
}

fn view(state: ArtifactState) -> ArtifactView {
    ArtifactView {
        mission: mission(),
        mission_state: State::Ready,
        state,
        digest: Digest32::from_bytes([9; 32]),
    }
}

#[test]
fn phase_names_round_trip_in_canonical_order() {
    let mut previous = None;
    for phase in Phase::ALL {
        assert_eq!(Phase::parse(phase.as_str()), Ok(phase));
        assert!(previous < Some(phase), "{phase} out of order");
        previous = Some(phase);
    }
    assert_eq!(
        Phase::parse("plan"),
        Err(CoordinationRefusal::FieldInvalid { field: "phase" })
    );
}

#[test]
fn a_workflow_is_one_to_seven_phases_in_canonical_order_each_once() {
    assert_eq!(
        parse_workflow(&names(&["research", "design", "outline"])),
        Ok(vec![Phase::Research, Phase::Design, Phase::Outline])
    );
    let every: Vec<&str> = Phase::ALL.iter().map(|phase| phase.as_str()).collect();
    assert_eq!(parse_workflow(&names(&every)).unwrap().len(), 7);
    for invalid in [
        &[][..],
        &["design", "research"][..],
        &["research", "research"][..],
    ] {
        assert_eq!(
            parse_workflow(&names(invalid)),
            Err(CoordinationRefusal::FieldInvalid { field: "workflow" }),
            "{invalid:?}"
        );
    }
}

#[test]
fn the_workflow_is_the_owners_declared_once_in_draft() {
    let undeclared = MissionPhases {
        state: Some(State::Draft),
        workflow: None,
        current: Vec::new(),
    };
    let declared = decide_workflow(
        &mission(),
        &undeclared,
        &names(&["research"]),
        &Actor::Owner,
    );
    assert_eq!(
        declared,
        Ok(PhaseEvent::WorkflowDeclared {
            mission: mission(),
            phases: vec![Phase::Research],
        })
    );
    assert_eq!(
        decide_workflow(&mission(), &undeclared, &names(&["research"]), &session()),
        Err(CoordinationRefusal::OwnerOnly)
    );
    let again = phases(State::Draft, &[Phase::Research], &[]);
    assert_eq!(
        decide_workflow(&mission(), &again, &names(&["design"]), &Actor::Owner),
        Err(CoordinationRefusal::WorkflowAlreadyDeclared)
    );
    let ready = MissionPhases {
        state: Some(State::Ready),
        ..undeclared.clone()
    };
    assert_eq!(
        decide_workflow(&mission(), &ready, &names(&["research"]), &Actor::Owner),
        Err(CoordinationRefusal::MissionNotDraft)
    );
    let unknown = MissionPhases {
        state: None,
        ..undeclared
    };
    assert_eq!(
        decide_workflow(&mission(), &unknown, &names(&["research"]), &Actor::Owner),
        Err(CoordinationRefusal::MissionNotFound)
    );
}

#[test]
fn submission_refusals_follow_the_specified_order() {
    let workflow = [Phase::Research, Phase::Design];
    let unknown = MissionPhases {
        state: None,
        workflow: None,
        current: Vec::new(),
    };
    assert_eq!(
        submit(&unknown, Phase::Research),
        Err(CoordinationRefusal::MissionNotFound)
    );
    assert_eq!(
        submit(&phases(State::Draft, &workflow, &[]), Phase::Research),
        Err(CoordinationRefusal::ArtifactMissionDraft)
    );
    for terminal in [State::Accepted, State::Abandoned, State::Cancelled] {
        assert_eq!(
            submit(&phases(terminal, &workflow, &[]), Phase::Research),
            Err(CoordinationRefusal::ArtifactMissionClosed)
        );
    }
    let no_workflow = MissionPhases {
        state: Some(State::Ready),
        workflow: None,
        current: Vec::new(),
    };
    assert_eq!(
        submit(&no_workflow, Phase::Research),
        Err(CoordinationRefusal::WorkflowUndeclared)
    );
    let ready = phases(State::Ready, &workflow, &[]);
    assert_eq!(
        submit(&ready, Phase::Outline),
        Err(CoordinationRefusal::PhaseUndeclared)
    );
    assert_eq!(
        submit(&ready, Phase::Design),
        Err(CoordinationRefusal::PhasePreviousUnapproved)
    );
    // A submitted (not approved) research does not open the design.
    let submitted = phases(
        State::Ready,
        &workflow,
        &[(Phase::Research, ArtifactState::Submitted)],
    );
    assert_eq!(
        submit(&submitted, Phase::Design),
        Err(CoordinationRefusal::PhasePreviousUnapproved)
    );
    let approved = phases(
        State::Running,
        &workflow,
        &[(Phase::Research, ArtifactState::Approved)],
    );
    assert!(matches!(
        submit(&approved, Phase::Design),
        Ok(PhaseEvent::Submitted {
            phase: Phase::Design,
            ..
        })
    ));
}

#[test]
fn content_is_one_byte_to_one_mebibyte_of_text_and_a_session_may_submit() {
    let ready = phases(State::Ready, &[Phase::Research], &[]);
    for content in ["", "  \n", "a\0b"] {
        assert_eq!(
            decide_submission(
                &mission(),
                &artifact(),
                &ready,
                Phase::Research,
                content,
                &Actor::Owner
            ),
            Err(CoordinationRefusal::FieldInvalid { field: "content" }),
            "{content:?}"
        );
    }
    let largest = "a".repeat(MAX_ARTIFACT_BYTES);
    assert!(
        decide_submission(
            &mission(),
            &artifact(),
            &ready,
            Phase::Research,
            &largest,
            &session()
        )
        .is_ok()
    );
    let larger = "a".repeat(MAX_ARTIFACT_BYTES + 1);
    assert_eq!(
        decide_submission(
            &mission(),
            &artifact(),
            &ready,
            Phase::Research,
            &larger,
            &Actor::Owner
        ),
        Err(CoordinationRefusal::FieldInvalid { field: "content" })
    );
}

#[test]
fn approval_binds_the_digest_the_owner_read() {
    let digest = Digest32::from_bytes([9; 32]);
    let approve = ArtifactDecision::Approve {
        digest,
        reason: None,
    };
    assert_eq!(
        decide_artifact(
            &artifact(),
            Some(&view(ArtifactState::Submitted)),
            &approve,
            &Actor::Owner
        ),
        Ok(PhaseEvent::Approved {
            mission: mission(),
            artifact: artifact(),
            digest,
            reason: None,
        })
    );
    let other = ArtifactDecision::Approve {
        digest: Digest32::from_bytes([8; 32]),
        reason: None,
    };
    assert_eq!(
        decide_artifact(
            &artifact(),
            Some(&view(ArtifactState::Submitted)),
            &other,
            &Actor::Owner
        ),
        Err(CoordinationRefusal::ArtifactDigestMismatch)
    );
}

#[test]
fn only_the_owner_decides_and_only_on_a_submitted_artifact_of_a_live_mission() {
    let approve = ArtifactDecision::Approve {
        digest: Digest32::from_bytes([9; 32]),
        reason: Some("facts checked".to_owned()),
    };
    let give_back = ArtifactDecision::Return {
        reason: "cite the lines".to_owned(),
    };
    assert_eq!(
        decide_artifact(
            &artifact(),
            Some(&view(ArtifactState::Submitted)),
            &approve,
            &session()
        ),
        Err(CoordinationRefusal::OwnerOnly)
    );
    assert_eq!(
        decide_artifact(&artifact(), None, &give_back, &Actor::Owner),
        Err(CoordinationRefusal::ArtifactNotFound)
    );
    for state in [
        ArtifactState::Approved,
        ArtifactState::Returned,
        ArtifactState::Superseded,
    ] {
        assert_eq!(
            decide_artifact(&artifact(), Some(&view(state)), &give_back, &Actor::Owner),
            Err(CoordinationRefusal::ArtifactNotPending),
            "{state:?}"
        );
    }
    let closed = ArtifactView {
        mission_state: State::Accepted,
        ..view(ArtifactState::Submitted)
    };
    assert_eq!(
        decide_artifact(&artifact(), Some(&closed), &give_back, &Actor::Owner),
        Err(CoordinationRefusal::ArtifactMissionClosed)
    );
    assert!(matches!(
        decide_artifact(
            &artifact(),
            Some(&view(ArtifactState::Submitted)),
            &give_back,
            &Actor::Owner
        ),
        Ok(PhaseEvent::Returned { .. })
    ));
    let blank = ArtifactDecision::Return {
        reason: " ".to_owned(),
    };
    assert_eq!(
        decide_artifact(
            &artifact(),
            Some(&view(ArtifactState::Submitted)),
            &blank,
            &Actor::Owner
        ),
        Err(CoordinationRefusal::FieldInvalid { field: "reason" })
    );
}

#[test]
fn unapproved_and_governing_phases_follow_the_workflow() {
    let workflow = [Phase::Research, Phase::Design, Phase::Outline];
    let current = phases(
        State::Ready,
        &workflow,
        &[
            (Phase::Research, ArtifactState::Approved),
            (Phase::Design, ArtifactState::Approved),
            (Phase::Outline, ArtifactState::Submitted),
        ],
    );
    assert_eq!(current.unapproved(), vec![Phase::Outline]);
    assert_eq!(current.governing(), Some(Phase::Design));
    let none = phases(State::Ready, &workflow, &[]);
    assert_eq!(none.unapproved(), workflow.to_vec());
    assert_eq!(none.governing(), None);
    let undeclared = MissionPhases {
        state: Some(State::Ready),
        workflow: None,
        current: Vec::new(),
    };
    assert!(undeclared.unapproved().is_empty());
}

#[test]
fn phase_blockers_come_after_dependencies_and_requests_and_before_the_rest() {
    let request = RequestId::from_bytes([4; 16]);
    let other = MissionId::from_bytes([5; 16]);
    let blockers = with_phase_blockers(
        vec![
            Blocker::DependencyPending(other.clone()),
            Blocker::RequestPending(request.clone()),
            Blocker::ScopeConflict(other.clone()),
        ],
        &[Phase::Research, Phase::Outline],
    );
    assert_eq!(
        blockers,
        vec![
            Blocker::DependencyPending(other.clone()),
            Blocker::RequestPending(request.clone()),
            Blocker::PhaseUnapproved(Phase::Research),
            Blocker::PhaseUnapproved(Phase::Outline),
            Blocker::ScopeConflict(other),
        ]
    );
    assert_eq!(blockers[2].code(), "phase.unapproved");
    let accept = with_phase_blockers(
        vec![Blocker::RequestPending(request), Blocker::CheckRunning],
        &[Phase::Design],
    );
    assert_eq!(accept[1], Blocker::PhaseUnapproved(Phase::Design));
    assert_eq!(accept[2], Blocker::CheckRunning);
    assert_eq!(
        with_phase_blockers(Vec::new(), &[Phase::Design]),
        vec![Blocker::PhaseUnapproved(Phase::Design)]
    );
}
