#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Map, json};
use work_supervision_journal::{Event, Journal, OpenMode, Timestamp};

const BINARY: &str = env!("CARGO_BIN_EXE_ws-journal-verify");

fn run(arguments: &[&std::ffi::OsStr]) -> Output {
    Command::new(BINARY).args(arguments).output().unwrap()
}

fn write_notes(path: &Path, texts: &[&str]) -> String {
    let (mut journal, _) = Journal::open(path, OpenMode::Strict).unwrap();
    let mut last = String::new();
    for text in texts {
        let data = Map::from_iter([("text".to_owned(), json!(text))]);
        let head = journal
            .append(
                Timestamp::parse("2026-10-09T00:00:00.000Z").unwrap(),
                Event::new("mission.note", data).unwrap(),
            )
            .unwrap();
        last = head.digest().to_hex();
    }
    last
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn a_valid_journal_exits_zero_and_prints_what_it_verified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    let head = write_notes(&path, &["one", "two", "three"]);
    let output = run(&[path.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(&output.stdout),
        format!("valid: 3 entries verified; head seq 3 digest {head}\n")
    );
}

#[test]
fn an_empty_journal_exits_zero_and_says_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    fs::write(&path, b"").unwrap();
    let output = run(&[path.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stdout), "valid: 0 entries verified; no head\n");
}

#[test]
fn a_corrupted_journal_exits_one_without_echoing_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    write_notes(&path, &["one", "SECRET-CANARY", "three"]);
    let tampered = fs::read_to_string(&path)
        .unwrap()
        .replace("SECRET-CANARY", "SECRET-CANARZ");
    fs::write(&path, tampered).unwrap();
    let output = run(&[path.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    let shown = text(&output.stderr);
    assert_eq!(
        shown,
        "invalid: digest-mismatch at line 2; 1 entries verified before it\n"
    );
    assert!(!shown.contains("CANAR"));
    assert!(text(&output.stdout).is_empty());
}

#[test]
fn a_torn_tail_exits_three() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    write_notes(&path, &["one"]);
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"{\"at\"");
    fs::write(&path, bytes).unwrap();
    let output = run(&[path.as_os_str()]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        text(&output.stderr),
        "torn-tail: line 2 has no terminating newline; 1 entries verified before it\n"
    );
}

#[test]
fn an_unreadable_journal_exits_two_and_is_never_reported_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("absent.jsonl");
    let output = run(&[absent.as_os_str()]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        text(&output.stderr),
        "unreadable: the journal file could not be read\n"
    );
    let directory = run(&[dir.path().as_os_str()]);
    assert_eq!(directory.status.code(), Some(2));
}

#[test]
fn a_wrong_argument_count_exits_two_with_usage() {
    let none = run(&[]);
    assert_eq!(none.status.code(), Some(2));
    assert_eq!(
        text(&none.stderr),
        "usage: ws-journal-verify <journal-file> [--expected <expected-state-file>]\n"
    );
    let two = run(&["a".as_ref(), "b".as_ref()]);
    assert_eq!(two.status.code(), Some(2));
    let flag_without_file = run(&["a".as_ref(), "--expected".as_ref()]);
    assert_eq!(flag_without_file.status.code(), Some(2));
    let unknown_flag = run(&["a".as_ref(), "--other".as_ref(), "b".as_ref()]);
    assert_eq!(unknown_flag.status.code(), Some(2));
}

const MISSION: &str = "0123456789abcdef0123456789abcdef";
const COMMIT: &str = "cccccccccccccccccccccccccccccccccccccccc";
const EVIDENCE: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const CHECK: &str = "11111111111111111111111111111111";
/// `printf '["cargo","test"]' | shasum -a 256`.
const ARGV_DIGEST: &str = "fa2a5292a0673151d46d74454a4888cfbd446d7f9353ffe7f65b4d7a410b759d";

/// A mission submitted at `commit`, its one check declared, passed and the result accepted.
fn write_mission(path: &Path, commit: &str) -> String {
    let events = [
        (
            "check.declared",
            json!({ "mission": MISSION, "criterion": 0, "argv_digest": ARGV_DIGEST }),
        ),
        (
            "mission.result-submitted",
            json!({ "mission": MISSION, "commit": commit, "evidence_digest": EVIDENCE }),
        ),
        (
            "check.started",
            json!({ "mission": MISSION, "check": CHECK, "criterion": 0, "commit": COMMIT, "argv_digest": ARGV_DIGEST }),
        ),
        (
            "check.finished",
            json!({ "mission": MISSION, "check": CHECK, "exit_code": 0, "signal": null, "budget": null }),
        ),
        ("mission.accepted", json!({ "mission": MISSION })),
    ];
    let (mut journal, _) = Journal::open(path, OpenMode::Strict).unwrap();
    let mut last = String::new();
    for (kind, data) in events {
        let data = data.as_object().unwrap().clone();
        let head = journal
            .append(
                Timestamp::parse("2026-11-24T00:00:00.000Z").unwrap(),
                Event::new(kind, data).unwrap(),
            )
            .unwrap();
        last = head.digest().to_hex();
    }
    last
}

fn write_expectation(path: &Path) {
    let document = json!({
        "schema": "libre-ai.work-supervision.expected-state.v0",
        "missions": [{
            "mission": MISSION, "commit": COMMIT, "evidence_digest": EVIDENCE,
            "checks": [{ "criterion": 0, "argv": ["cargo", "test"] }]
        }]
    });
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

#[test]
fn a_journal_consistent_with_its_expectation_exits_zero_and_prints_summing_counts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    let expected = dir.path().join("expected.json");
    let head = write_mission(&path, COMMIT);
    write_expectation(&expected);
    let output = run(&[
        path.as_os_str(),
        "--expected".as_ref(),
        expected.as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(&output.stdout),
        format!(
            "consistent: 5 entries verified; head seq 5 digest {head}\n\
             missions: 1 expected = 1 found + 0 missing; 1 submitted in the journal = 1 covered + 0 outside the expectation\n\
             comparisons: 7 planned = 7 matched + 0 diverged + 0 not applicable\n\
             planned: 1 results + 1 commits + 1 evidence digests + 1 check declarations + 1 check executions + 1 undeclared-check sweeps + 1 acceptances\n"
        )
    );
    assert!(text(&output.stderr).is_empty());
}

#[test]
fn a_journal_contradicting_its_expectation_exits_four_and_names_the_gap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    let expected = dir.path().join("expected.json");
    let journal_commit = "dddddddddddddddddddddddddddddddddddddddd";
    let head = write_mission(&path, journal_commit);
    write_expectation(&expected);
    // The chain alone is valid: only the comparison sees the gap.
    assert_eq!(run(&[path.as_os_str()]).status.code(), Some(0));
    let output = run(&[
        path.as_os_str(),
        "--expected".as_ref(),
        expected.as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(4));
    let shown = text(&output.stderr);
    assert_eq!(
        shown,
        format!(
            "diverged: 5 entries verified; head seq 5 digest {head}\n\
             missions: 1 expected = 1 found + 0 missing; 1 submitted in the journal = 1 covered + 0 outside the expectation\n\
             comparisons: 7 planned = 6 matched + 1 diverged + 0 not applicable\n\
             planned: 1 results + 1 commits + 1 evidence digests + 1 check declarations + 1 check executions + 1 undeclared-check sweeps + 1 acceptances\n\
             gap: commit-mismatch mission {MISSION} line 2\n"
        )
    );
    assert!(!shown.contains(journal_commit));
    assert!(text(&output.stdout).is_empty());
}

#[test]
fn an_unreadable_or_refused_expectation_exits_two_and_compares_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    write_mission(&path, COMMIT);
    let absent = dir.path().join("absent.json");
    let output = run(&[path.as_os_str(), "--expected".as_ref(), absent.as_os_str()]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        text(&output.stderr),
        "unreadable: the expected-state file could not be read\n"
    );
    assert!(text(&output.stdout).is_empty());
    let directory = run(&[
        path.as_os_str(),
        "--expected".as_ref(),
        dir.path().as_os_str(),
    ]);
    assert_eq!(directory.status.code(), Some(2));

    let empty = dir.path().join("empty.json");
    fs::write(
        &empty,
        br#"{"missions":[],"schema":"libre-ai.work-supervision.expected-state.v0"}"#,
    )
    .unwrap();
    let output = run(&[path.as_os_str(), "--expected".as_ref(), empty.as_os_str()]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        text(&output.stderr),
        "expectation-refused: expectation-empty\n"
    );
    let blank = dir.path().join("blank.json");
    fs::write(&blank, b"").unwrap();
    let output = run(&[path.as_os_str(), "--expected".as_ref(), blank.as_os_str()]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        text(&output.stderr),
        "expectation-refused: expectation-malformed\n"
    );
}

#[test]
fn an_unreadable_journal_with_an_expectation_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let expected = dir.path().join("expected.json");
    write_expectation(&expected);
    let absent = dir.path().join("absent.jsonl");
    let output = run(&[
        absent.as_os_str(),
        "--expected".as_ref(),
        expected.as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        text(&output.stderr),
        "unreadable: the journal file could not be read\n"
    );
}
