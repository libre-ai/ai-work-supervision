#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use serde_json::{Value, json};
use work_supervision_journal::{Event, Journal, OpenMode, Timestamp};
use work_supervision_journal_verifier::{
    Code, Comparison, Expectation, ExpectationError, ExpectedOutcome, Gap, Outcome, verify,
    verify_expected,
};

const MISSION: &str = "0123456789abcdef0123456789abcdef";
const OTHER_MISSION: &str = "fedcba9876543210fedcba9876543210";
const COMMIT: &str = "cccccccccccccccccccccccccccccccccccccccc";
const OTHER_COMMIT: &str = "dddddddddddddddddddddddddddddddddddddddd";
const EVIDENCE: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const OTHER_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const CHECK: &str = "11111111111111111111111111111111";
/// `printf '["cargo","test"]' | shasum -a 256`: computed outside both implementations.
const ARGV_DIGEST: &str = "fa2a5292a0673151d46d74454a4888cfbd446d7f9353ffe7f65b4d7a410b759d";

type Events = Vec<(&'static str, Value)>;

fn declared(criterion: u64, argv_digest: &str) -> (&'static str, Value) {
    (
        "check.declared",
        json!({ "mission": MISSION, "criterion": criterion, "argv_digest": argv_digest }),
    )
}

fn submitted(mission: &str, commit: &str, evidence: &str) -> (&'static str, Value) {
    (
        "mission.result-submitted",
        json!({
            "mission": mission, "revision": 7, "state": "result-submitted",
            "commit": commit, "evidence_digest": evidence, "summary_digest": OTHER_DIGEST
        }),
    )
}

fn started(check: &str, commit: &str, argv_digest: &str) -> (&'static str, Value) {
    (
        "check.started",
        json!({
            "mission": MISSION, "check": check, "criterion": 0,
            "commit": commit, "argv_digest": argv_digest
        }),
    )
}

fn finished(check: &str, exit_code: Value, signal: Value, budget: Value) -> (&'static str, Value) {
    (
        "check.finished",
        json!({
            "mission": MISSION, "check": check, "bytes": 12, "digest": OTHER_DIGEST,
            "exit_code": exit_code, "signal": signal, "budget": budget
        }),
    )
}

fn passed(check: &str) -> (&'static str, Value) {
    finished(check, json!(0), Value::Null, Value::Null)
}

fn accepted() -> (&'static str, Value) {
    (
        "mission.accepted",
        json!({ "mission": MISSION, "revision": 8, "state": "accepted", "reason_digest": OTHER_DIGEST }),
    )
}

/// A journal that agrees with [`expectation`]: 5 entries.
fn coherent() -> Events {
    vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        started(CHECK, COMMIT, ARGV_DIGEST),
        passed(CHECK),
        accepted(),
    ]
}

/// Writes `events` with the real writer, so the chain is valid by construction.
fn journal(events: &Events) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    let (mut journal, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    for (kind, data) in events {
        let Value::Object(data) = data.clone() else {
            panic!("event data must be an object");
        };
        journal
            .append(
                Timestamp::parse("2026-11-24T00:00:00.000Z").unwrap(),
                Event::new(kind, data).unwrap(),
            )
            .unwrap();
    }
    drop(journal);
    fs::read(&path).unwrap()
}

fn expectation_json(checks: &Value) -> Value {
    json!({
        "schema": "libre-ai.work-supervision.expected-state.v0",
        "missions": [{
            "mission": MISSION, "commit": COMMIT, "evidence_digest": EVIDENCE, "checks": checks
        }]
    })
}

fn expectation() -> Expectation {
    parse(&expectation_json(
        &json!([{ "criterion": 0, "argv": ["cargo", "test"] }]),
    ))
    .unwrap()
}

fn parse(value: &Value) -> Result<Expectation, ExpectationError> {
    Expectation::parse(&serde_json::to_vec(value).unwrap())
}

fn assert_sums(comparison: &Comparison) {
    assert_eq!(
        comparison.missions_expected,
        comparison.missions_found + comparison.missions_missing
    );
    assert_eq!(
        comparison.journal_submitted,
        comparison.missions_found + comparison.journal_outside
    );
    assert_eq!(
        comparison.planned(),
        comparison.matched + comparison.diverged() + comparison.not_applicable
    );
}

/// The chain must be valid: the gap is found by the comparison, not by the chain.
fn compare(events: &Events, expectation: &Expectation) -> ExpectedOutcome {
    let bytes = journal(events);
    assert!(matches!(verify(&bytes[..]).unwrap(), Outcome::Valid { .. }));
    let outcome = verify_expected(&bytes, expectation).unwrap();
    match &outcome {
        ExpectedOutcome::Consistent(comparison) => {
            assert_sums(comparison);
            assert_eq!(comparison.diverged(), 0);
        }
        ExpectedOutcome::Diverged(comparison) => {
            assert_sums(comparison);
            assert!(comparison.diverged() > 0);
        }
        ExpectedOutcome::ChainRefused(_) => {}
    }
    outcome
}

/// The gaps found, each with its criterion and line.
fn gaps(events: &Events, expectation: &Expectation) -> Vec<(Gap, Option<u64>, Option<u64>)> {
    match compare(events, expectation) {
        ExpectedOutcome::Diverged(comparison) => comparison
            .divergences
            .iter()
            .map(|divergence| {
                assert_eq!(divergence.mission, MISSION);
                (divergence.gap, divergence.criterion, divergence.line)
            })
            .collect(),
        other => panic!("expected a divergence, got {other:?}"),
    }
}

fn consistent(events: &Events, expectation: &Expectation) -> Comparison {
    match compare(events, expectation) {
        ExpectedOutcome::Consistent(comparison) => comparison,
        other => panic!("expected a consistent journal, got {other:?}"),
    }
}

#[test]
fn a_journal_coherent_with_the_artefacts_passes_and_says_what_it_compared() {
    let comparison = consistent(&coherent(), &expectation());
    assert_eq!(comparison.entries, 5);
    assert_eq!(comparison.head.as_ref().map(|head| head.seq), Some(5));
    assert_eq!(
        (
            comparison.missions_expected,
            comparison.missions_found,
            comparison.missions_missing
        ),
        (1, 1, 0)
    );
    assert_eq!(
        (comparison.journal_submitted, comparison.journal_outside),
        (1, 0)
    );
    assert_eq!(
        (
            comparison.results,
            comparison.commits,
            comparison.evidence,
            comparison.declarations,
            comparison.executions,
            comparison.sweeps,
            comparison.acceptances
        ),
        (1, 1, 1, 1, 1, 1, 1)
    );
    assert_eq!(
        (
            comparison.planned(),
            comparison.matched,
            comparison.not_applicable
        ),
        (7, 7, 0)
    );
}

#[test]
fn a_submitted_mission_not_yet_accepted_passes_with_its_acceptance_not_applicable() {
    let mut events = coherent();
    events.pop();
    let comparison = consistent(&events, &expectation());
    assert_eq!((comparison.matched, comparison.not_applicable), (6, 1));
}

#[test]
fn an_absent_result_entry_is_named() {
    let events = vec![
        declared(0, ARGV_DIGEST),
        started(CHECK, COMMIT, ARGV_DIGEST),
        passed(CHECK),
    ];
    assert_eq!(
        gaps(&events, &expectation()),
        [(Gap::ResultMissing, None, None)]
    );
    let ExpectedOutcome::Diverged(comparison) = compare(&events, &expectation()) else {
        panic!("expected a divergence");
    };
    assert_eq!(
        (comparison.missions_found, comparison.missions_missing),
        (0, 1)
    );
    // Commit, evidence and acceptance could not be compared: counted, not hidden.
    assert_eq!((comparison.matched, comparison.not_applicable), (3, 3));
}

#[test]
fn a_result_at_another_commit_is_named() {
    let mut events = coherent();
    events[1] = submitted(MISSION, OTHER_COMMIT, EVIDENCE);
    assert_eq!(
        gaps(&events, &expectation()),
        [(Gap::CommitMismatch, None, Some(2))]
    );
}

#[test]
fn the_last_submission_is_the_one_compared() {
    let mut events = vec![submitted(MISSION, OTHER_COMMIT, OTHER_DIGEST)];
    events.extend(coherent());
    consistent(&events, &expectation());
    let mut resubmitted = coherent();
    resubmitted.pop();
    resubmitted.push(submitted(MISSION, OTHER_COMMIT, EVIDENCE));
    assert_eq!(
        gaps(&resubmitted, &expectation()),
        [(Gap::CommitMismatch, None, Some(5))]
    );
}

#[test]
fn a_result_with_another_evidence_digest_is_named() {
    let mut events = coherent();
    events[1] = submitted(MISSION, COMMIT, OTHER_DIGEST);
    assert_eq!(
        gaps(&events, &expectation()),
        [(Gap::EvidenceMismatch, None, Some(2))]
    );
}

#[test]
fn an_expected_check_the_journal_never_declared_is_named() {
    let mut events = coherent();
    events.remove(0);
    assert_eq!(
        gaps(&events, &expectation()),
        [(Gap::CheckUndeclared, Some(0), None)]
    );
}

#[test]
fn a_check_declared_with_another_argument_vector_is_named() {
    let mut events = coherent();
    events[0] = declared(0, OTHER_DIGEST);
    assert_eq!(
        gaps(&events, &expectation()),
        [(Gap::CheckArgvMismatch, Some(0), Some(1))]
    );
}

#[test]
fn a_check_the_expectation_does_not_list_is_named() {
    let mut events = coherent();
    events.insert(1, declared(1, OTHER_DIGEST));
    assert_eq!(
        gaps(&events, &expectation()),
        [(Gap::CheckUnexpected, Some(1), Some(2))]
    );
}

#[test]
fn a_declared_check_without_execution_is_named() {
    let never_run = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
    ];
    let at_another_commit = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        started(CHECK, OTHER_COMMIT, ARGV_DIGEST),
        passed(CHECK),
    ];
    let of_another_vector = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        started(CHECK, COMMIT, OTHER_DIGEST),
        passed(CHECK),
    ];
    let interrupted = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        started(CHECK, COMMIT, ARGV_DIGEST),
        (
            "check.interrupted",
            json!({ "mission": MISSION, "check": CHECK }),
        ),
    ];
    let end_of_another_check = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        started(CHECK, COMMIT, ARGV_DIGEST),
        passed("22222222222222222222222222222222"),
    ];
    for events in [
        never_run,
        at_another_commit,
        of_another_vector,
        interrupted,
        end_of_another_check,
    ] {
        assert_eq!(
            gaps(&events, &expectation()),
            [(Gap::CheckNotExecuted, Some(0), None)]
        );
    }
}

#[test]
fn a_check_whose_last_execution_did_not_pass_is_named() {
    let failures = [
        finished(CHECK, json!(1), Value::Null, Value::Null),
        finished(CHECK, Value::Null, json!(9), Value::Null),
        finished(CHECK, json!(0), Value::Null, json!("duration")),
        finished(CHECK, Value::Null, Value::Null, Value::Null),
    ];
    for failure in failures {
        let events = vec![
            declared(0, ARGV_DIGEST),
            submitted(MISSION, COMMIT, EVIDENCE),
            started(CHECK, COMMIT, ARGV_DIGEST),
            failure,
        ];
        assert_eq!(
            gaps(&events, &expectation()),
            [(Gap::CheckNotPassed, Some(0), Some(4))]
        );
    }
}

#[test]
fn the_last_finished_execution_decides() {
    let second = "22222222222222222222222222222222";
    let red_then_green = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        started(CHECK, COMMIT, ARGV_DIGEST),
        finished(CHECK, json!(1), Value::Null, Value::Null),
        started(second, COMMIT, ARGV_DIGEST),
        passed(second),
    ];
    consistent(&red_then_green, &expectation());
    let green_then_red = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        started(CHECK, COMMIT, ARGV_DIGEST),
        passed(CHECK),
        started(second, COMMIT, ARGV_DIGEST),
        finished(second, json!(1), Value::Null, Value::Null),
    ];
    assert_eq!(
        gaps(&green_then_red, &expectation()),
        [(Gap::CheckNotPassed, Some(0), Some(6))]
    );
}

#[test]
fn an_acceptance_before_the_check_passed_is_named() {
    let events = vec![
        declared(0, ARGV_DIGEST),
        submitted(MISSION, COMMIT, EVIDENCE),
        accepted(),
        started(CHECK, COMMIT, ARGV_DIGEST),
        passed(CHECK),
    ];
    assert_eq!(
        gaps(&events, &expectation()),
        [(Gap::AcceptedWithoutPassedCheck, None, Some(3))]
    );
}

#[test]
fn an_acceptance_resting_on_no_check_at_all_is_named() {
    let none = parse(&expectation_json(&json!([]))).unwrap();
    let events = vec![submitted(MISSION, COMMIT, EVIDENCE), accepted()];
    assert_eq!(
        gaps(&events, &none),
        [(Gap::AcceptedWithoutPassedCheck, None, Some(2))]
    );
    // The same mission, submitted and not accepted, contradicts nothing.
    consistent(&vec![submitted(MISSION, COMMIT, EVIDENCE)], &none);
}

#[test]
fn a_mission_outside_the_expectation_is_counted_and_not_compared() {
    let mut events = coherent();
    events.push(submitted(OTHER_MISSION, OTHER_COMMIT, OTHER_DIGEST));
    let comparison = consistent(&events, &expectation());
    assert_eq!(
        (comparison.journal_submitted, comparison.journal_outside),
        (2, 1)
    );
    assert_eq!(comparison.planned(), 7);
}

#[test]
fn a_refused_chain_is_reported_before_any_comparison() {
    let bytes = journal(&coherent());
    let tampered = String::from_utf8(bytes.clone())
        .unwrap()
        .replace(COMMIT, OTHER_COMMIT)
        .into_bytes();
    assert!(matches!(
        verify_expected(&tampered, &expectation()).unwrap(),
        ExpectedOutcome::ChainRefused(Outcome::Invalid {
            line: 2,
            code: Code::DigestMismatch,
            verified: 1
        })
    ));
    let mut torn = bytes;
    torn.pop();
    assert!(matches!(
        verify_expected(&torn, &expectation()).unwrap(),
        ExpectedOutcome::ChainRefused(Outcome::TornTail {
            line: 5,
            verified: 4
        })
    ));
}

#[test]
fn an_empty_journal_diverges_instead_of_passing_on_nothing() {
    let ExpectedOutcome::Diverged(comparison) = verify_expected(b"", &expectation()).unwrap()
    else {
        panic!("expected a divergence");
    };
    assert_sums(&comparison);
    let found: Vec<Gap> = comparison.divergences.iter().map(|d| d.gap).collect();
    assert_eq!(
        found,
        [
            Gap::ResultMissing,
            Gap::CheckUndeclared,
            Gap::CheckNotExecuted
        ]
    );
}

#[test]
fn an_expectation_that_cannot_be_read_is_an_error_never_an_expectation_of_nothing() {
    let check = json!([{ "criterion": 0, "argv": ["cargo", "test"] }]);
    let with = |key: &str, value: Value| {
        let mut document = expectation_json(&check);
        document["missions"][0][key] = value;
        document
    };
    assert_eq!(Expectation::parse(b""), Err(ExpectationError::Malformed));
    assert_eq!(Expectation::parse(b"[]"), Err(ExpectationError::Malformed));
    assert_eq!(
        parse(&json!({ "schema": "other", "missions": [] })),
        Err(ExpectationError::SchemaUnknown)
    );
    assert_eq!(
        parse(&json!({ "schema": "libre-ai.work-supervision.expected-state.v0", "missions": [] })),
        Err(ExpectationError::Empty)
    );
    assert_eq!(
        parse(&json!({ "schema": "libre-ai.work-supervision.expected-state.v0" })),
        Err(ExpectationError::FieldInvalid)
    );
    let mut unknown_key = expectation_json(&check);
    unknown_key["extra"] = json!(1);
    let mut without_checks = expectation_json(&check);
    without_checks["missions"][0]
        .as_object_mut()
        .unwrap()
        .remove("checks");
    for document in [
        unknown_key,
        without_checks,
        with("mission", json!("ABC")),
        with("commit", json!("abc")),
        with("evidence_digest", json!(COMMIT)),
        with("checks", json!([{ "criterion": 0, "argv": [] }])),
        with("checks", json!([{ "criterion": 0, "argv": ["a", 1] }])),
        with("checks", json!([{ "criterion": -1, "argv": ["a"] }])),
        with(
            "checks",
            json!([{ "criterion": 0, "argv": ["a"], "more": 1 }]),
        ),
    ] {
        assert_eq!(parse(&document), Err(ExpectationError::FieldInvalid));
    }
    assert_eq!(
        parse(&with(
            "checks",
            json!([{ "criterion": 0, "argv": ["a"] }, { "criterion": 0, "argv": ["b"] }])
        )),
        Err(ExpectationError::CriterionDuplicate)
    );
    let mut twice = expectation_json(&check);
    let again = twice["missions"][0].clone();
    twice["missions"].as_array_mut().unwrap().push(again);
    assert_eq!(parse(&twice), Err(ExpectationError::MissionDuplicate));
    assert_eq!(expectation().missions(), 1);
}
