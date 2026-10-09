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
        "usage: ws-journal-verify <journal-file>\n"
    );
    let two = run(&["a".as_ref(), "b".as_ref()]);
    assert_eq!(two.status.code(), Some(2));
}
