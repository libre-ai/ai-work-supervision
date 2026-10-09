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

use serde_json::{Value, json};
use work_supervision_journal::{Event, Journal, OpenMode, Timestamp};
use work_supervision_store::{BlobStore, Store, StoreError};

const MISSION_A: &str = "0123456789abcdef0123456789abcdef";
const MISSION_B: &str = "fedcba9876543210fedcba9876543210";
const COMMIT: &str = "1111111111111111111111111111111111111111";

struct Root {
    _dir: tempfile::TempDir,
    journal: PathBuf,
    blobs: BlobStore,
    state: PathBuf,
    base: PathBuf,
}

fn root() -> Root {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_owned();
    let blobs = BlobStore::open(&base.join("blobs")).unwrap();
    Root {
        journal: base.join("journal.v0.jsonl"),
        state: base.join("state.sqlite"),
        blobs,
        base,
        _dir: dir,
    }
}

/// Appends events to the journal, storing their texts as blobs first, and
/// optionally applies each entry to a live store right after it is durable.
struct Writer<'a> {
    root: &'a Root,
    journal: Journal,
    clock: u32,
}

impl<'a> Writer<'a> {
    fn open(root: &'a Root) -> Self {
        let (journal, _) = Journal::open(&root.journal, OpenMode::Strict).unwrap();
        Self {
            root,
            journal,
            clock: 0,
        }
    }

    fn text(&self, text: &str) -> String {
        self.root.blobs.put_text(text).unwrap().to_hex()
    }

    fn append(&mut self, kind: &str, data: Value, live: Option<&mut Store>) {
        self.clock += 1;
        let at = Timestamp::parse(&format!(
            "2026-10-09T00:{:02}:{:02}.000Z",
            self.clock / 60,
            self.clock % 60
        ))
        .unwrap();
        let Value::Object(map) = data else {
            panic!("event data must be an object");
        };
        self.journal
            .append(at, Event::new(kind, map).unwrap())
            .unwrap();
        if let Some(store) = live {
            store.catch_up(&self.journal, &self.root.blobs).unwrap();
        }
    }

    fn create(&mut self, mission: &str, title: &str, live: Option<&mut Store>) {
        let data = json!({
            "mission": mission,
            "revision": 1,
            "state": "draft",
            "title_digest": self.text(title),
            "repository": "sample",
            "brief_digest": self.text(&format!("brief of {title}")),
            "criteria_digests": [self.text("tests pass"), self.text("no new warning")],
            "max_duration_seconds": 600,
            "max_output_bytes": 10_485_760,
            "executor": "fake",
        });
        self.append("mission.created", data, live);
    }

    fn step(
        &mut self,
        kind: &str,
        mission: &str,
        revision: u64,
        state: &str,
        extra: Value,
        live: Option<&mut Store>,
    ) {
        let mut data = json!({ "mission": mission, "revision": revision, "state": state });
        if let (Value::Object(target), Value::Object(fields)) = (&mut data, extra) {
            target.extend(fields);
        }
        self.append(kind, data, live);
    }

    fn note(&mut self, mission: &str, text: &str, live: Option<&mut Store>) {
        let data = json!({ "mission": mission, "note_digest": self.text(text) });
        self.append("mission.noted", data, live);
    }
}

fn provisioned(mission: &str) -> Value {
    json!({ "base_commit": COMMIT, "branch": format!("ws/{mission}"), "worktree": format!("worktrees/{mission}") })
}

fn result(writer: &Writer<'_>) -> Value {
    json!({
        "commit": COMMIT,
        "evidence_digest": writer.text("evidence bytes"),
        "summary_digest": writer.text("did the thing"),
    })
}

fn reason(writer: &Writer<'_>, text: &str) -> Value {
    json!({ "reason_digest": writer.text(text) })
}

fn run(id: &str) -> Value {
    json!({ "run": id })
}

fn rebuilt_dump(root: &Root) -> Vec<u8> {
    let target = root.base.join("rebuilt.sqlite");
    let _ = fs::remove_file(&target);
    let store = Store::rebuild(&root.journal, &root.blobs, &target).unwrap();
    store.dump().unwrap()
}

/// Scenario 1: one mission from creation to acceptance, applied entry by entry.
fn scenario_happy_path(root: &Root) -> Vec<u8> {
    let mut live = Store::open(&root.state).unwrap();
    let mut w = Writer::open(root);
    w.create(MISSION_A, "Add the parser", Some(&mut live));
    w.step(
        "mission.readied",
        MISSION_A,
        2,
        "ready",
        json!({}),
        Some(&mut live),
    );
    w.step(
        "mission.provisioned",
        MISSION_A,
        3,
        "provisioned",
        provisioned(MISSION_A),
        Some(&mut live),
    );
    w.step(
        "mission.run-started",
        MISSION_A,
        4,
        "running",
        run("r1"),
        Some(&mut live),
    );
    w.note(MISSION_A, "halfway there", Some(&mut live));
    w.step(
        "mission.input-awaited",
        MISSION_A,
        5,
        "waiting-input",
        run("r1"),
        Some(&mut live),
    );
    w.step(
        "mission.input-resumed",
        MISSION_A,
        6,
        "running",
        run("r1"),
        Some(&mut live),
    );
    w.step(
        "mission.run-exited",
        MISSION_A,
        7,
        "exited",
        json!({ "run": "r1", "interrupted": false }),
        Some(&mut live),
    );
    let submitted = result(&w);
    w.step(
        "mission.result-submitted",
        MISSION_A,
        8,
        "result-submitted",
        submitted,
        Some(&mut live),
    );
    let accepted = reason(&w, "matches the criteria");
    w.step(
        "mission.accepted",
        MISSION_A,
        9,
        "accepted",
        accepted,
        Some(&mut live),
    );
    live.dump().unwrap()
}

/// Scenario 2: two interleaved missions; the projection is closed, lags behind
/// the journal, and catches up on reopen; one mission is rejected, resumed and
/// accepted, the other cancelled.
fn scenario_interleaved_with_lag(root: &Root) -> Vec<u8> {
    let mut w = Writer::open(root);
    {
        let mut live = Store::open(&root.state).unwrap();
        w.create(MISSION_A, "Fix the flaky test", Some(&mut live));
        w.create(MISSION_B, "Rename the module", Some(&mut live));
        w.step(
            "mission.readied",
            MISSION_A,
            2,
            "ready",
            json!({}),
            Some(&mut live),
        );
    }
    // The projection is closed while the journal advances (crash between the
    // journal append and the projection update).
    w.step("mission.readied", MISSION_B, 2, "ready", json!({}), None);
    w.step(
        "mission.provisioned",
        MISSION_A,
        3,
        "provisioned",
        provisioned(MISSION_A),
        None,
    );
    w.step(
        "mission.provisioned",
        MISSION_B,
        3,
        "provisioned",
        provisioned(MISSION_B),
        None,
    );
    let mut live = Store::open(&root.state).unwrap();
    assert_eq!(live.catch_up(&w.journal, &root.blobs).unwrap(), 3);
    w.step(
        "mission.run-started",
        MISSION_A,
        4,
        "running",
        run("r1"),
        Some(&mut live),
    );
    w.step(
        "mission.run-started",
        MISSION_B,
        4,
        "running",
        run("r2"),
        Some(&mut live),
    );
    w.step(
        "mission.run-exited",
        MISSION_A,
        5,
        "exited",
        json!({ "run": "r1", "interrupted": true }),
        Some(&mut live),
    );
    let cancelled = reason(&w, "superseded");
    w.step(
        "mission.cancelled",
        MISSION_B,
        5,
        "cancelled",
        cancelled,
        Some(&mut live),
    );
    let submitted = result(&w);
    w.step(
        "mission.result-submitted",
        MISSION_A,
        6,
        "result-submitted",
        submitted,
        Some(&mut live),
    );
    let rejected = reason(&w, "the test is still flaky");
    w.step(
        "mission.rejected",
        MISSION_A,
        7,
        "rejected",
        rejected,
        Some(&mut live),
    );
    w.step(
        "mission.run-started",
        MISSION_A,
        8,
        "running",
        run("r3"),
        Some(&mut live),
    );
    w.step(
        "mission.run-exited",
        MISSION_A,
        9,
        "exited",
        json!({ "run": "r3", "interrupted": false }),
        Some(&mut live),
    );
    let submitted = result(&w);
    w.step(
        "mission.result-submitted",
        MISSION_A,
        10,
        "result-submitted",
        submitted,
        Some(&mut live),
    );
    let accepted = reason(&w, "stable over 100 runs");
    w.step(
        "mission.accepted",
        MISSION_A,
        11,
        "accepted",
        accepted,
        Some(&mut live),
    );
    live.dump().unwrap()
}

/// Scenario 3: a torn tail quarantined at reopen (`journal.recovered`), then
/// a mission abandoned from draft, with notes on both sides of the recovery.
fn scenario_recovered_torn_tail(root: &Root) -> Vec<u8> {
    let mut live = Store::open(&root.state).unwrap();
    {
        let mut w = Writer::open(root);
        w.create(MISSION_A, "Draft only", Some(&mut live));
        w.note(MISSION_A, "first note", Some(&mut live));
    }
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&root.journal)
        .unwrap();
    file.write_all(b"{\"at\":\"2026").unwrap();
    drop(file);
    let at = Timestamp::parse("2026-10-09T01:00:00.000Z").unwrap();
    let (journal, recovery) =
        Journal::open(&root.journal, OpenMode::RecoverTornTail { at }).unwrap();
    assert!(recovery.is_some());
    live.catch_up(&journal, &root.blobs).unwrap();
    drop(journal);
    let mut w = Writer::open(root);
    w.note(MISSION_A, "second note", Some(&mut live));
    let abandoned = reason(&w, "not needed");
    w.step(
        "mission.abandoned",
        MISSION_A,
        2,
        "abandoned",
        abandoned,
        Some(&mut live),
    );
    live.dump().unwrap()
}

#[test]
fn migrations_create_exactly_the_described_schema() {
    let root = root();
    let store = Store::open(&root.state).unwrap();
    let described = store.describe_schema().unwrap();
    let expected: Value =
        serde_json::from_str(include_str!("../migrations/schema.v0.json")).unwrap();
    assert_eq!(described, expected);
}

#[test]
fn reopening_a_store_does_not_reapply_migrations() {
    let root = root();
    drop(Store::open(&root.state).unwrap());
    let store = Store::open(&root.state).unwrap();
    assert_eq!(store.position().unwrap(), None);
}

#[test]
fn rebuild_is_byte_equal_to_the_live_projection_on_three_scenarios() {
    for scenario in [
        scenario_happy_path as fn(&Root) -> Vec<u8>,
        scenario_interleaved_with_lag,
        scenario_recovered_torn_tail,
    ] {
        let root = root();
        let live = scenario(&root);
        let rebuilt = rebuilt_dump(&root);
        assert!(!live.is_empty());
        assert_eq!(
            String::from_utf8(live).unwrap(),
            String::from_utf8(rebuilt).unwrap()
        );
    }
}

#[test]
fn the_projection_carries_texts_from_blobs_and_states_from_the_journal() {
    let root = root();
    scenario_interleaved_with_lag(&root);
    let store = Store::open(&root.state).unwrap();
    let dump = String::from_utf8(store.dump().unwrap()).unwrap();
    assert!(dump.contains("Fix the flaky test"));
    assert!(dump.contains("\"accepted\""));
    assert!(dump.contains("\"cancelled\""));
    assert!(dump.contains("stable over 100 runs"));
    let position = store.position().unwrap().unwrap();
    assert_eq!(position.seq(), 16);
}

#[test]
fn the_journal_never_carries_the_texts() {
    let root = root();
    scenario_happy_path(&root);
    let journal = fs::read_to_string(&root.journal).unwrap();
    for text in [
        "Add the parser",
        "brief of",
        "tests pass",
        "halfway there",
        "did the thing",
        "matches the criteria",
    ] {
        assert!(!journal.contains(text), "journal leaks {text:?}");
    }
}

#[test]
fn a_projection_ahead_of_the_journal_is_refused() {
    let root = root();
    scenario_happy_path(&root);
    let text = fs::read_to_string(&root.journal).unwrap();
    let shorter: String = text
        .lines()
        .take(3)
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&root.journal, shorter).unwrap();
    let mut store = Store::open(&root.state).unwrap();
    let error = store.catch_up_from(&root.journal, &root.blobs).unwrap_err();
    assert_eq!(
        error,
        StoreError::ProjectionAhead {
            projection: 10,
            journal: 3
        }
    );
    assert_eq!(error.code(), "projection.ahead");
}

#[test]
fn a_projection_diverging_from_the_journal_is_refused() {
    let first = root();
    scenario_happy_path(&first);
    let second = root();
    scenario_interleaved_with_lag(&second);
    let mut store = Store::open(&first.state).unwrap();
    let error = store
        .catch_up_from(&second.journal, &second.blobs)
        .unwrap_err();
    assert_eq!(error, StoreError::ProjectionDiverged { seq: 10 });
}

#[test]
fn an_entry_that_does_not_follow_the_position_is_refused() {
    let root = root();
    {
        let mut w = Writer::open(&root);
        w.create(MISSION_A, "One", None);
        w.create(MISSION_B, "Two", None);
    }
    let mut entries = Vec::new();
    work_supervision_journal::replay(&root.journal, |entry| {
        entries.push(entry);
        Ok::<(), work_supervision_journal::JournalError>(())
    })
    .unwrap();
    let mut store = Store::open(&root.state).unwrap();
    let error = store.apply(&entries[1], &root.blobs).unwrap_err();
    assert_eq!(
        error,
        StoreError::OutOfOrder {
            seq: 2,
            expected: 1
        }
    );
    store.apply(&entries[0], &root.blobs).unwrap();
    assert_eq!(
        store.apply(&entries[0], &root.blobs).unwrap_err(),
        StoreError::OutOfOrder {
            seq: 1,
            expected: 2
        }
    );
}

#[test]
fn an_unknown_kind_and_a_stale_revision_are_refused_with_their_sequence() {
    let root = root();
    {
        let mut w = Writer::open(&root);
        w.create(MISSION_A, "One", None);
        w.step("mission.readied", MISSION_A, 5, "ready", json!({}), None);
    }
    let mut store = Store::open(&root.state).unwrap();
    assert_eq!(
        store.catch_up_from(&root.journal, &root.blobs).unwrap_err(),
        StoreError::EventInvalid { seq: 2 }
    );
    assert_eq!(
        store.position().unwrap().unwrap().seq(),
        1,
        "the refused entry is not applied"
    );

    let other = self::root();
    {
        let mut w = Writer::open(&other);
        w.append("mission.teleported", json!({ "mission": MISSION_A }), None);
    }
    let mut store = Store::open(&other.state).unwrap();
    let error = store
        .catch_up_from(&other.journal, &other.blobs)
        .unwrap_err();
    assert_eq!(error, StoreError::UnknownKind { seq: 1 });
}

#[test]
fn a_missing_blob_refuses_the_rebuild_without_naming_any_content() {
    let root = root();
    scenario_happy_path(&root);
    let digest = root.blobs.put_text("Add the parser").unwrap();
    fs::remove_file(root.blobs.path_of(&digest)).unwrap();
    let error =
        Store::rebuild(&root.journal, &root.blobs, &root.base.join("r.sqlite")).unwrap_err();
    assert_eq!(error, StoreError::BlobMissing { seq: 1 });
    let shown = error.to_string();
    assert!(!shown.contains("parser"));
    assert!(!shown.contains(&digest.to_hex()));
    assert!(
        !root.base.join("r.sqlite").exists(),
        "a failed rebuild leaves no database"
    );
}

#[test]
fn rebuild_refuses_an_existing_target() {
    let root = root();
    scenario_happy_path(&root);
    assert_eq!(
        Store::rebuild(&root.journal, &root.blobs, &root.state).unwrap_err(),
        StoreError::TargetExists
    );
}

#[test]
fn blobs_are_content_addressed_idempotent_and_checked_on_read() {
    let root = root();
    let digest = root.blobs.put(b"some bytes").unwrap();
    assert_eq!(root.blobs.put(b"some bytes").unwrap(), digest);
    assert_eq!(root.blobs.get(&digest).unwrap(), b"some bytes");
    fs::write(root.blobs.path_of(&digest), b"other bytes").unwrap();
    assert_eq!(
        root.blobs.get(&digest).unwrap_err(),
        StoreError::BlobCorrupt
    );
    let invalid = root.blobs.put(&[0xff, 0xfe]).unwrap();
    assert_eq!(
        root.blobs.get_text(&invalid).unwrap_err(),
        StoreError::BlobCorrupt
    );
}

#[test]
fn a_missing_journal_is_unreadable_not_empty() {
    let root = root();
    let mut store = Store::open(&root.state).unwrap();
    let error = store
        .catch_up_from(&root.base.join("absent.jsonl"), &root.blobs)
        .unwrap_err();
    assert_eq!(error.code(), "journal.io");
}

fn _assert_path(_: &Path) {}
