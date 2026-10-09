#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use work_supervision_journal::{
    Event, Journal, JournalError, OpenMode, SCHEMA, Timestamp, encode_line,
};

// Canonical bytes written by hand and hashed with `shasum -a 256`, a third
// instrument independent of both the writer and the verifier.
const VECTOR_WITHOUT_DIGEST: &str = "{\"at\":\"2026-10-09T00:00:00.000Z\",\"event\":{\"data\":{\"text\":\"\u{e9}\\u0001\\\"x\"},\"kind\":\"mission.note\"},\"prev\":null,\"schema\":\"libre-ai.work-supervision.journal.v0\",\"seq\":1}";
const VECTOR_DIGEST: &str = "a6f1c5db5d50f88b1156d93d26d094856f99a71084fe18ca10bbff485fbf5f8c";

fn ts(value: &str) -> Timestamp {
    Timestamp::parse(value).unwrap()
}

fn event(kind: &str, data: Value) -> Event {
    let Value::Object(map) = data else {
        panic!("test data must be an object");
    };
    Event::new(kind, map).unwrap()
}

fn path_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("journal.v0.jsonl")
}

fn lines(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap();
    assert!(
        text.ends_with('\n'),
        "every committed entry ends with a newline"
    );
    text.lines().map(str::to_owned).collect()
}

fn write_lines(path: &Path, lines: &[String]) {
    let mut text = lines.join("\n");
    text.push('\n');
    fs::write(path, text).unwrap();
}

fn append_notes(path: &Path, texts: &[&str]) {
    let (mut journal, recovery) = Journal::open(path, OpenMode::Strict).unwrap();
    assert!(recovery.is_none());
    for (index, text) in texts.iter().enumerate() {
        let at = format!("2026-10-09T00:00:0{index}.000Z");
        journal
            .append(ts(&at), event("mission.note", json!({ "text": text })))
            .unwrap();
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn encodes_the_hand_computed_vector() {
    let encoded = encode_line(
        1,
        None,
        &ts("2026-10-09T00:00:00.000Z"),
        &event("mission.note", json!({ "text": "\u{e9}\u{1}\"x" })),
    )
    .unwrap();
    assert_eq!(encoded.digest().to_hex(), VECTOR_DIGEST);
    let expected_line = format!(
        "{{\"at\":\"2026-10-09T00:00:00.000Z\",\"digest\":\"{VECTOR_DIGEST}\",\"event\":{{\"data\":{{\"text\":\"\u{e9}\\u0001\\\"x\"}},\"kind\":\"mission.note\"}},\"prev\":null,\"schema\":\"{SCHEMA}\",\"seq\":1}}"
    );
    assert_eq!(
        String::from_utf8(encoded.bytes().to_vec()).unwrap(),
        expected_line
    );
    assert_eq!(sha256_hex(VECTOR_WITHOUT_DIGEST.as_bytes()), VECTOR_DIGEST);
}

#[test]
fn genesis_entry_is_canonical_and_self_digested() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    let (mut journal, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    assert!(journal.head().is_none());
    let head = journal
        .append(
            ts("2026-10-09T00:00:00.000Z"),
            event("mission.note", json!({ "text": "\u{e9}\u{1}\"x" })),
        )
        .unwrap();
    assert_eq!(head.seq(), 1);
    assert_eq!(head.digest().to_hex(), VECTOR_DIGEST);
    let written = lines(&path);
    assert_eq!(written.len(), 1);
    let parsed: Value = serde_json::from_str(&written[0]).unwrap();
    assert_eq!(parsed["prev"], Value::Null);
    assert_eq!(parsed["seq"], json!(1));
    assert_eq!(parsed["schema"], json!(SCHEMA));
    assert_eq!(parsed["digest"], json!(VECTOR_DIGEST));
}

#[test]
fn each_entry_links_to_the_previous_digest_and_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one", "two", "three"]);
    let written = lines(&path);
    assert_eq!(written.len(), 3);
    let parsed: Vec<Value> = written
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (index, entry) in parsed.iter().enumerate() {
        assert_eq!(entry["seq"], json!(index + 1));
        if index > 0 {
            assert_eq!(entry["prev"], parsed[index - 1]["digest"]);
        }
    }
    let (mut journal, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    let head = journal.head().unwrap();
    assert_eq!(head.seq(), 3);
    assert_eq!(json!(head.digest().to_hex()), parsed[2]["digest"]);
    assert_eq!(journal.entries(), 3);
    let next = journal
        .append(
            ts("2026-10-09T00:00:09.000Z"),
            event("mission.note", json!({})),
        )
        .unwrap();
    assert_eq!(next.seq(), 4);
    assert_eq!(lines(&path).len(), 4);
}

#[test]
fn reopening_refuses_a_modified_byte() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one", "SECRET-CANARY", "three"]);
    let mut written = lines(&path);
    written[1] = written[1].replace("SECRET-CANARY", "SECRET-CANARZ");
    write_lines(&path, &written);
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::DigestMismatch { line: 2 });
    let shown = error.to_string();
    assert!(shown.contains("digest-mismatch"));
    assert!(shown.contains("line 2"));
    assert!(
        !shown.contains("CANAR"),
        "errors never echo journal content"
    );
}

#[test]
fn reopening_refuses_a_removed_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one", "two", "three"]);
    let written = lines(&path);
    write_lines(&path, &[written[0].clone(), written[2].clone()]);
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::SequenceInvalid { line: 2 });
}

#[test]
fn reopening_refuses_reordered_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one", "two"]);
    let written = lines(&path);
    write_lines(&path, &[written[1].clone(), written[0].clone()]);
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::GenesisInvalid { line: 1 });
}

#[test]
fn reopening_refuses_a_spliced_previous_digest() {
    let dir = tempfile::tempdir().unwrap();
    let first = path_in(&dir);
    let second = dir.path().join("other.jsonl");
    append_notes(&first, &["one", "two"]);
    append_notes(&second, &["uno", "dos"]);
    let mine = lines(&first);
    let theirs = lines(&second);
    write_lines(&first, &[mine[0].clone(), theirs[1].clone()]);
    let error = Journal::open(&first, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::PreviousDigestMismatch { line: 2 });
}

#[test]
fn reopening_refuses_a_non_canonical_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one"]);
    let written = lines(&path);
    write_lines(&path, &[written[0].replacen("\"seq\":1", "\"seq\": 1", 1)]);
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::NonCanonical { line: 1 });
}

#[test]
fn reopening_refuses_a_duplicated_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one"]);
    let written = lines(&path);
    write_lines(
        &path,
        &[written[0].replacen("{\"at\"", "{\"at\":\"x\",\"at\"", 1)],
    );
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::NonCanonical { line: 1 });
}

#[test]
fn reopening_refuses_an_unknown_envelope_field_even_when_digested() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    let mut body: Map<String, Value> = Map::new();
    body.insert("at".into(), json!("2026-10-09T00:00:00.000Z"));
    body.insert(
        "event".into(),
        json!({ "data": {}, "kind": "mission.note" }),
    );
    body.insert("extra".into(), json!(true));
    body.insert("prev".into(), Value::Null);
    body.insert("schema".into(), json!(SCHEMA));
    body.insert("seq".into(), json!(1));
    let digest = sha256_hex(&serde_jcs::to_vec(&body).unwrap());
    body.insert("digest".into(), json!(digest));
    let line = String::from_utf8(serde_jcs::to_vec(&body).unwrap()).unwrap();
    write_lines(&path, &[line]);
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::EnvelopeInvalid { line: 1 });
}

#[test]
fn reopening_refuses_an_unknown_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one"]);
    let written = lines(&path);
    write_lines(
        &path,
        &[written[0].replace(SCHEMA, "libre-ai.work-supervision.journal.v9")],
    );
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::SchemaUnknown { line: 1 });
}

#[test]
fn events_refuse_numbers_outside_the_exact_integer_domain() {
    let float = Map::from_iter([("ratio".to_owned(), json!(1.5))]);
    assert_eq!(
        Event::new("mission.note", float).unwrap_err(),
        JournalError::NumberInvalid { line: 0 }
    );
    let huge = Map::from_iter([("count".to_owned(), json!(9_007_199_254_740_992_u64))]);
    assert_eq!(
        Event::new("mission.note", huge).unwrap_err(),
        JournalError::NumberInvalid { line: 0 }
    );
    let edge = Map::from_iter([
        ("max".to_owned(), json!(9_007_199_254_740_991_i64)),
        ("min".to_owned(), json!(-9_007_199_254_740_991_i64)),
    ]);
    assert!(Event::new("mission.note", edge).is_ok());
}

#[test]
fn events_refuse_excessive_nesting() {
    let mut nested = json!("leaf");
    for _ in 0..40 {
        nested = json!([nested]);
    }
    let data = Map::from_iter([("deep".to_owned(), nested)]);
    assert_eq!(
        Event::new("mission.note", data).unwrap_err(),
        JournalError::DepthExceeded { line: 0 }
    );
}

#[test]
fn event_kinds_follow_the_dotted_lowercase_grammar() {
    for valid in [
        "mission.note",
        "journal.recovered",
        "run.output-checkpoint",
        "x",
    ] {
        assert!(
            Event::new(valid, Map::new()).is_ok(),
            "{valid} should be valid"
        );
    }
    let too_long = "a".repeat(65);
    for invalid in [
        "",
        "Mission.note",
        "mission.",
        ".note",
        "mission..note",
        "1abc",
        "a b",
        too_long.as_str(),
    ] {
        assert_eq!(
            Event::new(invalid, Map::new()).unwrap_err(),
            JournalError::KindInvalid { line: 0 },
            "{invalid:?} should be refused"
        );
    }
}

#[test]
fn timestamps_are_utc_with_milliseconds_and_a_real_calendar_date() {
    for valid in ["2026-10-09T00:00:00.000Z", "2024-02-29T23:59:59.999Z"] {
        assert!(Timestamp::parse(valid).is_ok(), "{valid} should be valid");
    }
    for invalid in [
        "2026-10-09T00:00:00Z",
        "2026-10-09T00:00:00.000+00:00",
        "2026-02-29T00:00:00.000Z",
        "2026-13-01T00:00:00.000Z",
        "2026-10-09T24:00:00.000Z",
        "2026-10-09 00:00:00.000Z",
        "",
    ] {
        assert_eq!(
            Timestamp::parse(invalid).unwrap_err(),
            JournalError::TimestampInvalid { line: 0 },
            "{invalid:?} should be refused"
        );
    }
}

fn append_torn_tail(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
}

#[test]
fn a_torn_tail_is_refused_in_strict_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one", "two"]);
    append_torn_tail(&path, b"{\"at\":\"20");
    let error = Journal::open(&path, OpenMode::Strict).unwrap_err();
    assert_eq!(error, JournalError::TornTail { line: 3 });
}

#[test]
fn recovery_quarantines_the_torn_tail_and_records_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one", "two"]);
    let torn = b"{\"at\":\"20";
    append_torn_tail(&path, torn);
    let (journal, recovery) = Journal::open(
        &path,
        OpenMode::RecoverTornTail {
            at: ts("2026-10-09T01:00:00.000Z"),
        },
    )
    .unwrap();
    let recovery = recovery.unwrap();
    assert_eq!(recovery.torn_bytes(), 9);
    assert_eq!(recovery.torn_digest().to_hex(), sha256_hex(torn));
    assert_eq!(fs::read(recovery.quarantine_path()).unwrap(), torn);
    assert_eq!(journal.head().unwrap().seq(), 3);
    drop(journal);

    let written = lines(&path);
    assert_eq!(written.len(), 3);
    let recorded: Value = serde_json::from_str(&written[2]).unwrap();
    assert_eq!(recorded["event"]["kind"], json!("journal.recovered"));
    assert_eq!(recorded["event"]["data"]["torn_bytes"], json!(9));
    assert_eq!(
        recorded["event"]["data"]["torn_digest"],
        json!(sha256_hex(torn))
    );
    assert!(Journal::open(&path, OpenMode::Strict).is_ok());
}

#[test]
fn recovery_without_a_torn_tail_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    append_notes(&path, &["one"]);
    let before = fs::read(&path).unwrap();
    let (journal, recovery) = Journal::open(
        &path,
        OpenMode::RecoverTornTail {
            at: ts("2026-10-09T01:00:00.000Z"),
        },
    )
    .unwrap();
    assert!(recovery.is_none());
    assert_eq!(journal.entries(), 1);
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn a_second_writer_is_refused_while_the_first_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    let first = Journal::open(&path, OpenMode::Strict).unwrap();
    assert_eq!(
        Journal::open(&path, OpenMode::Strict).unwrap_err(),
        JournalError::Locked
    );
    drop(first);
    assert!(Journal::open(&path, OpenMode::Strict).is_ok());
}

#[test]
fn an_oversized_entry_is_refused_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    let (mut journal, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    let big = "x".repeat(1 << 20);
    let error = journal
        .append(
            ts("2026-10-09T00:00:00.000Z"),
            event("mission.note", json!({ "text": big })),
        )
        .unwrap_err();
    assert_eq!(error, JournalError::LineTooLong { line: 1 });
    assert_eq!(fs::read(&path).unwrap().len(), 0);
    assert!(journal.head().is_none());
}

#[test]
fn an_empty_existing_file_opens_without_a_head() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir);
    fs::write(&path, b"").unwrap();
    let (journal, recovery) = Journal::open(&path, OpenMode::Strict).unwrap();
    assert!(recovery.is_none());
    assert!(journal.head().is_none());
    assert_eq!(journal.entries(), 0);
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_refused_instead_of_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere.jsonl");
    fs::write(&target, b"").unwrap();
    let path = path_in(&dir);
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert_eq!(
        Journal::open(&path, OpenMode::Strict).unwrap_err(),
        JournalError::NotRegularFile
    );
}

#[test]
fn a_missing_directory_is_an_io_error_not_an_empty_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent").join("journal.v0.jsonl");
    assert_eq!(
        Journal::open(&path, OpenMode::Strict).unwrap_err(),
        JournalError::Io
    );
}
