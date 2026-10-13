#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Outline split rules (docs/work-supervision/coordination-v0.md, « Outline split »).

use work_supervision_domain::MissionId;
use work_supervision_domain::SessionId;
use work_supervision_domain::coordination::{Actor, CoordinationRefusal};
use work_supervision_domain::outline::{
    MAX_SLICES, OutlineView, Slice, SplitState, check_outline, check_plan,
};
use work_supervision_domain::phases::{ArtifactState, Phase};

fn slice(key: &str, after: &[&str]) -> Slice {
    Slice {
        key: key.to_owned(),
        demonstrates: format!("{key} can be shown"),
        after: after.iter().map(|key| (*key).to_owned()).collect(),
    }
}

fn mission(byte: u8) -> MissionId {
    MissionId::from_bytes([byte; 16])
}

fn approved_outline(of: &MissionId) -> OutlineView {
    OutlineView {
        mission: of.clone(),
        phase: Phase::Outline,
        state: ArtifactState::Approved,
        split: None,
    }
}

#[test]
fn a_plan_yields_its_edges_in_plan_order() {
    let edges = check_plan(&[
        slice("lexer", &[]),
        slice("parser", &["lexer"]),
        slice("cli", &["parser", "lexer"]),
    ])
    .unwrap();
    assert_eq!(edges, [(1, 0), (2, 1), (2, 0)]);
}

#[test]
fn a_cycle_is_refused_however_long() {
    let refusal = check_plan(&[
        slice("a", &["c"]),
        slice("b", &["a"]),
        slice("c", &["b"]),
        slice("d", &[]),
    ])
    .unwrap_err();
    assert_eq!(refusal, CoordinationRefusal::DependencyCycle);
}

#[test]
fn keys_and_references_are_checked() {
    let code = |slices: &[Slice]| check_plan(slices).unwrap_err().code();
    assert_eq!(code(&[slice("a", &["z"])]), "outline.dependency_unknown");
    assert_eq!(code(&[slice("a", &["a"])]), "dependency.duplicate");
    assert_eq!(
        code(&[slice("a", &[]), slice("b", &["a", "a"])]),
        "dependency.duplicate"
    );
    assert_eq!(
        code(&[slice("a", &[]), slice("a", &[])]),
        "outline.key_duplicate"
    );
    for bad in ["", "-a", "A", "a_b", "a/b", &"k".repeat(33)] {
        assert_eq!(code(&[slice(bad, &[])]), "outline.key_invalid", "{bad:?}");
    }
    assert!(check_plan(&[slice(&"k".repeat(32), &[]), slice("0-x", &[])]).is_ok());
}

#[test]
fn every_slice_says_what_it_demonstrates() {
    let mut blank = slice("a", &[]);
    blank.demonstrates = " \n".to_owned();
    assert_eq!(
        check_plan(&[blank]).unwrap_err(),
        CoordinationRefusal::FieldInvalid {
            field: "demonstrates"
        }
    );
}

#[test]
fn a_plan_holds_one_to_thirty_two_slices() {
    assert_eq!(
        check_plan(&[]).unwrap_err(),
        CoordinationRefusal::FieldInvalid { field: "plan" }
    );
    let many: Vec<Slice> = (0..=MAX_SLICES)
        .map(|n| slice(&format!("s{n}"), &[]))
        .collect();
    assert_eq!(
        check_plan(&many).unwrap_err(),
        CoordinationRefusal::FieldInvalid { field: "plan" }
    );
    assert!(check_plan(&many[..MAX_SLICES]).is_ok());
}

#[test]
fn only_the_owner_splits_only_an_approved_outline_of_the_mission_once() {
    let parent = mission(1);
    let session = Actor::Session(SessionId::from_bytes([2; 16]));
    let view = approved_outline(&parent);
    assert!(check_outline(&parent, Some(&view), &Actor::Owner).is_ok());
    assert_eq!(
        check_outline(&parent, Some(&view), &session).unwrap_err(),
        CoordinationRefusal::OwnerOnly
    );
    assert_eq!(
        check_outline(&parent, None, &Actor::Owner).unwrap_err(),
        CoordinationRefusal::ArtifactNotFound
    );
    let unapproved = |change: fn(&mut OutlineView)| {
        let mut other = approved_outline(&parent);
        change(&mut other);
        check_outline(&parent, Some(&other), &Actor::Owner)
            .unwrap_err()
            .code()
    };
    assert_eq!(
        unapproved(|view| view.state = ArtifactState::Submitted),
        "outline.unapproved"
    );
    assert_eq!(
        unapproved(|view| view.state = ArtifactState::Superseded),
        "outline.unapproved"
    );
    assert_eq!(
        unapproved(|view| view.phase = Phase::Design),
        "outline.unapproved"
    );
    assert_eq!(
        unapproved(|view| view.mission = MissionId::from_bytes([9; 16])),
        "outline.unapproved"
    );
    assert_eq!(
        unapproved(|view| view.split = Some(SplitState::Splitting)),
        "outline.already_split"
    );
    assert_eq!(
        unapproved(|view| view.split = Some(SplitState::Split)),
        "outline.already_split"
    );
    let mut aborted = approved_outline(&parent);
    aborted.split = Some(SplitState::Aborted);
    assert!(check_outline(&parent, Some(&aborted), &Actor::Owner).is_ok());
}

#[test]
fn split_states_round_trip() {
    for state in [
        SplitState::Splitting,
        SplitState::Split,
        SplitState::Aborted,
    ] {
        assert_eq!(SplitState::parse(state.as_str()).unwrap(), state);
    }
    assert!(SplitState::parse("done").is_err());
}
