#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::io::Write as _;

use serde_json::{Value, json};
use work_supervision_journal::{Entry, Event, Journal, JournalError, OpenMode, Timestamp, replay};

fn ts(value: &str) -> Timestamp {
    Timestamp::parse(value).unwrap()
}

fn event(kind: &str, data: Value) -> Event {
    let Value::Object(map) = data else {
        panic!("test data must be an object");
    };
    Event::new(kind, map).unwrap()
}

fn write_three(path: &std::path::Path) -> Vec<work_supervision_journal::Head> {
    let (mut journal, _) = Journal::open(path, OpenMode::Strict).unwrap();
    (1..=3)
        .map(|index| {
            journal
                .append(
                    ts(&format!("2026-10-09T00:00:0{index}.000Z")),
                    event("mission.noted", json!({ "n": index })),
                )
                .unwrap()
        })
        .collect()
}

#[test]
fn replay_returns_every_entry_in_order_with_its_digest_instant_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    let heads = write_three(&path);
    let mut seen: Vec<Entry> = Vec::new();
    let head = replay(&path, |entry| {
        seen.push(entry);
        Ok::<(), JournalError>(())
    })
    .unwrap();
    assert_eq!(head, Some(heads[2]));
    assert_eq!(seen.len(), 3);
    for (index, entry) in seen.iter().enumerate() {
        assert_eq!(entry.seq(), heads[index].seq());
        assert_eq!(entry.digest(), heads[index].digest());
        assert_eq!(
            entry.at().as_str(),
            format!("2026-10-09T00:00:0{}.000Z", index + 1)
        );
        assert_eq!(entry.event().kind(), "mission.noted");
        assert_eq!(entry.event().data()["n"], json!(index + 1));
        assert_eq!(entry.head(), heads[index]);
    }
}

#[test]
fn replay_of_a_missing_file_is_an_io_refusal_never_an_empty_journal() {
    let dir = tempfile::tempdir().unwrap();
    let result = replay(&dir.path().join("absent.jsonl"), |_| {
        Ok::<(), JournalError>(())
    });
    assert_eq!(result, Err(JournalError::Io));
}

#[test]
fn replay_refuses_a_torn_tail_and_a_corrupted_entry_with_their_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    write_three(&path);
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"{\"at\":").unwrap();
    drop(file);
    let mut count = 0;
    let result = replay(&path, |_| {
        count += 1;
        Ok::<(), JournalError>(())
    });
    assert_eq!(result, Err(JournalError::TornTail { line: 4 }));
    assert_eq!(count, 3, "entries before the torn tail are still visited");

    let text = fs::read_to_string(&path).unwrap();
    let corrupted = text.replacen("\"n\":2", "\"n\":9", 1);
    fs::write(&path, corrupted).unwrap();
    let result = replay(&path, |_| Ok::<(), JournalError>(()));
    assert_eq!(result, Err(JournalError::DigestMismatch { line: 2 }));
}

#[test]
fn replay_stops_at_the_first_visitor_error_and_returns_it() {
    #[derive(Debug, PartialEq)]
    enum Stop {
        Journal(JournalError),
        Visitor(u64),
    }
    impl From<JournalError> for Stop {
        fn from(error: JournalError) -> Self {
            Self::Journal(error)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    write_three(&path);
    let result = replay(&path, |entry| {
        if entry.seq() == 2 {
            Err(Stop::Visitor(2))
        } else {
            Ok(())
        }
    });
    assert_eq!(result, Err(Stop::Visitor(2)));
}

#[test]
fn an_open_writer_replays_entries_after_a_sequence_number_while_holding_its_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    write_three(&path);
    let (journal, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    let mut seqs = Vec::new();
    journal
        .replay_after(1, |entry| {
            seqs.push(entry.seq());
            Ok::<(), JournalError>(())
        })
        .unwrap();
    assert_eq!(seqs, vec![2, 3]);
    let mut none = 0;
    journal
        .replay_after(3, |_| {
            none += 1;
            Ok::<(), JournalError>(())
        })
        .unwrap();
    assert_eq!(none, 0);
}

#[test]
fn append_entry_returns_the_entry_replay_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    let (mut journal, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    let appended = journal
        .append_entry(
            ts("2026-10-09T00:00:01.000Z"),
            event("mission.noted", json!({ "n": 1 })),
        )
        .unwrap();
    let mut read = Vec::new();
    replay(&path, |entry| {
        read.push(entry);
        Ok::<(), JournalError>(())
    })
    .unwrap();
    assert_eq!(read, vec![appended]);
}
