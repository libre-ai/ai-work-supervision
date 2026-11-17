#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The read behind the flow measures (docs/work-supervision/flow-v0.md).
//!
//! The driver duplicates the one of `supervisor.rs` on purpose: two test
//! files, below the rule of three for a shared support module.

use work_supervision_domain::{
    Budgets, Command, CommitId, Digest32, ExecutorProfile, MissionId, RunId,
};
use work_supervision_journal::{OpenMode, Timestamp};
use work_supervision_store::{AcceptedTimeline, Layout, Store, Supervisor, TimelineRow};

const COMMIT: &str = "1111111111111111111111111111111111111111";
const EVIDENCE: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// The instant of the `second`-th command; every command appends one entry,
/// so `second` is also its journal sequence number.
fn instant(second: u32) -> String {
    format!("2026-10-09T01:{:02}:{:02}.000Z", second / 60, second % 60)
}

fn mission_id(byte: u8) -> MissionId {
    MissionId::from_bytes([byte; 16])
}

struct Driver {
    supervisor: Supervisor,
    clock: u32,
}

impl Driver {
    fn open(layout: &Layout) -> Self {
        Self {
            supervisor: Supervisor::open(layout, OpenMode::Strict).unwrap(),
            clock: 0,
        }
    }

    fn run(&mut self, id: &MissionId, command: Command) {
        self.clock += 1;
        let revision = self
            .supervisor
            .mission(id)
            .unwrap()
            .map_or(0, |mission| mission.revision());
        let at = Timestamp::parse(&instant(self.clock)).unwrap();
        self.supervisor.execute(id, &command, revision, at).unwrap();
    }

    fn create(&mut self, id: &MissionId) {
        self.run(
            id,
            Command::Create {
                title: "A mission".to_owned(),
                repository: "sample".to_owned(),
                brief: "the brief".to_owned(),
                criteria: vec!["criterion".to_owned()],
                budgets: Budgets::new(600, 1 << 20).unwrap(),
                executor: ExecutorProfile::Fake,
            },
        );
    }

    /// Start a run, exit it and submit its result: three entries.
    fn run_and_submit(&mut self, id: &MissionId, run: u8) {
        self.run(
            id,
            Command::StartRun {
                run: RunId::from_bytes([run; 16]),
            },
        );
        self.run(id, Command::ExitRun { interrupted: false });
        self.run(
            id,
            Command::SubmitResult {
                commit: Some(CommitId::parse(COMMIT).unwrap()),
                evidence: Some(Digest32::parse(EVIDENCE).unwrap()),
                summary: "the summary".to_owned(),
            },
        );
    }

    /// Create, ready, provision, run and submit: six entries.
    fn submit_first(&mut self, id: &MissionId, run: u8) {
        self.create(id);
        self.run(id, Command::Ready);
        self.run(
            id,
            Command::Provision {
                base_commit: CommitId::parse(COMMIT).unwrap(),
            },
        );
        self.run_and_submit(id, run);
    }

    fn decide(&mut self, id: &MissionId, command: fn(String) -> Command) {
        self.run(id, command("the reason".to_owned()));
    }
}

fn accept(reason: String) -> Command {
    Command::Accept { reason }
}

fn reject(reason: String) -> Command {
    Command::Reject { reason }
}

fn abandon(reason: String) -> Command {
    Command::Abandon { reason }
}

fn row(seq: u32, kind: &str) -> TimelineRow {
    TimelineRow {
        seq: i64::from(seq),
        kind: kind.to_owned(),
        at: instant(seq),
    }
}

/// Five missions: `03` rejected once then accepted (entries 1 to 11), `01`
/// accepted at the first pass (12 to 18), `02` left rejected (19 to 25), `04`
/// abandoned (26, 27), `05` left a draft (28).
fn world(driver: &mut Driver) {
    let twice = mission_id(3);
    driver.submit_first(&twice, 1);
    driver.decide(&twice, reject);
    driver.run_and_submit(&twice, 2);
    driver.decide(&twice, accept);

    let once = mission_id(1);
    driver.submit_first(&once, 3);
    driver.decide(&once, accept);

    let rejected = mission_id(2);
    driver.submit_first(&rejected, 4);
    driver.decide(&rejected, reject);

    let abandoned = mission_id(4);
    driver.create(&abandoned);
    driver.decide(&abandoned, abandon);

    driver.create(&mission_id(5));
}

/// What the specification says the read returns for [`world`].
fn expected() -> Vec<AcceptedTimeline> {
    vec![
        AcceptedTimeline {
            mission: mission_id(1).as_str().to_owned(),
            executor: "fake".to_owned(),
            events: vec![
                row(12, "mission.created"),
                row(17, "mission.result-submitted"),
                row(18, "mission.accepted"),
            ],
        },
        AcceptedTimeline {
            mission: mission_id(3).as_str().to_owned(),
            executor: "fake".to_owned(),
            events: vec![
                row(1, "mission.created"),
                row(6, "mission.result-submitted"),
                row(7, "mission.rejected"),
                row(10, "mission.result-submitted"),
                row(11, "mission.accepted"),
            ],
        },
    ]
}

#[test]
fn only_accepted_missions_and_their_four_kinds_are_read_by_identifier_then_sequence() {
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::new(root.path());
    let mut driver = Driver::open(&layout);
    world(&mut driver);
    assert_eq!(driver.clock, 28);
    let read = driver.supervisor.store().accepted_timelines().unwrap();
    assert_eq!(read, expected());
}

#[test]
fn the_live_projection_and_a_rebuilt_one_read_the_same_rows() {
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::new(root.path());
    let mut driver = Driver::open(&layout);
    world(&mut driver);
    let live = driver.supervisor.store().accepted_timelines().unwrap();
    let rebuilt = Store::rebuild(
        &layout.journal(),
        &layout.blob_store().unwrap(),
        &root.path().join("rebuilt.sqlite"),
    )
    .unwrap();
    assert_eq!(rebuilt.accepted_timelines().unwrap(), live);
    assert_eq!(live, expected());
}

#[test]
fn an_accepted_mission_without_timeline_rows_is_returned_with_no_event() {
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::new(root.path());
    let mut driver = Driver::open(&layout);
    world(&mut driver);
    // A root older than the timeline migration: the mission is accepted and
    // its earlier entries were never projected as timeline rows.
    let removed = driver
        .supervisor
        .store()
        .connection()
        .execute(
            "DELETE FROM mission_events WHERE mission_id = ?1",
            [mission_id(1).as_str()],
        )
        .unwrap();
    assert_eq!(removed, 7);
    let read = driver.supervisor.store().accepted_timelines().unwrap();
    let mut wanted = expected();
    wanted[0].events.clear();
    assert_eq!(read, wanted);
}

#[test]
fn a_root_without_an_accepted_mission_reads_nothing() {
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::new(root.path());
    let mut driver = Driver::open(&layout);
    assert_eq!(driver.supervisor.store().accepted_timelines().unwrap(), []);
    driver.submit_first(&mission_id(1), 1);
    assert_eq!(driver.supervisor.store().accepted_timelines().unwrap(), []);
}
