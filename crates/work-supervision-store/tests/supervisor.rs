#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use work_supervision_domain::{
    Budgets, Command, CommitId, Digest32, ExecutorProfile, MissionId, Refusal, RunId, State,
};
use work_supervision_journal::{OpenMode, Timestamp};
use work_supervision_store::{Layout, Store, Supervisor, SupervisorError};

const COMMIT: &str = "1111111111111111111111111111111111111111";
const EVIDENCE: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn at(second: u32) -> Timestamp {
    Timestamp::parse(&format!(
        "2026-10-09T01:{:02}:{:02}.000Z",
        second / 60,
        second % 60
    ))
    .unwrap()
}

fn create(title: &str) -> Command {
    Command::Create {
        title: title.to_owned(),
        repository: "sample".to_owned(),
        brief: format!("brief for {title}"),
        criteria: vec!["criterion one".to_owned(), "criterion two".to_owned()],
        budgets: Budgets::new(600, 1 << 20).unwrap(),
        executor: ExecutorProfile::Fake,
    }
}

fn mission_id(byte: u8) -> MissionId {
    MissionId::from_bytes([byte; 16])
}

fn run_id(byte: u8) -> RunId {
    RunId::from_bytes([byte; 16])
}

struct Driver {
    supervisor: Supervisor,
    clock: u32,
}

impl Driver {
    fn run(
        &mut self,
        id: &MissionId,
        command: Command,
    ) -> Result<work_supervision_domain::Mission, SupervisorError> {
        self.clock += 1;
        let revision = self
            .supervisor
            .mission(id)
            .unwrap()
            .map_or(0, |mission| mission.revision());
        let outcome = self
            .supervisor
            .execute(id, &command, revision, at(self.clock));
        if let Ok(mission) = &outcome {
            // The projection read back equals the domain's own fold.
            assert_eq!(self.supervisor.mission(id).unwrap().as_ref(), Some(mission));
        }
        outcome
    }
}

fn full_life(driver: &mut Driver, id: &MissionId, title: &str) {
    driver.run(id, create(title)).unwrap();
    driver
        .run(
            id,
            Command::Note {
                text: "first note".to_owned(),
            },
        )
        .unwrap();
    driver.run(id, Command::Ready).unwrap();
    driver
        .run(
            id,
            Command::Provision {
                base_commit: CommitId::parse(COMMIT).unwrap(),
            },
        )
        .unwrap();
    driver
        .run(id, Command::StartRun { run: run_id(1) })
        .unwrap();
    driver.run(id, Command::AwaitInput).unwrap();
    driver.run(id, Command::ResumeInput).unwrap();
    driver
        .run(id, Command::ExitRun { interrupted: false })
        .unwrap();
    driver.run(id, submit()).unwrap();
    driver
        .run(
            id,
            Command::Reject {
                reason: "try again".to_owned(),
            },
        )
        .unwrap();
    driver
        .run(id, Command::StartRun { run: run_id(2) })
        .unwrap();
    driver
        .run(id, Command::ExitRun { interrupted: true })
        .unwrap();
    driver.run(id, submit()).unwrap();
    let accepted = driver
        .run(
            id,
            Command::Accept {
                reason: "fine".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(accepted.state(), State::Accepted);
    assert_eq!(accepted.revision(), 13);
}

fn submit() -> Command {
    Command::SubmitResult {
        commit: Some(CommitId::parse(COMMIT).unwrap()),
        evidence: Some(Digest32::parse(EVIDENCE).unwrap()),
        summary: "the summary".to_owned(),
    }
}

fn open(layout: &Layout) -> Driver {
    Driver {
        supervisor: Supervisor::open(layout, OpenMode::Strict).unwrap(),
        clock: 0,
    }
}

#[test]
fn every_transition_goes_through_the_journal_and_the_projection_agrees_with_the_domain() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut driver = open(&layout);
    full_life(&mut driver, &mission_id(1), "First");
    let id = mission_id(2);
    driver.run(&id, create("Second")).unwrap();
    driver
        .run(
            &id,
            Command::Abandon {
                reason: "not needed".to_owned(),
            },
        )
        .unwrap();
    let id = mission_id(3);
    driver.run(&id, create("Third")).unwrap();
    driver.run(&id, Command::Ready).unwrap();
    driver
        .run(
            &id,
            Command::Provision {
                base_commit: CommitId::parse(COMMIT).unwrap(),
            },
        )
        .unwrap();
    driver
        .run(
            &id,
            Command::Cancel {
                reason: "stop".to_owned(),
            },
        )
        .unwrap();
    let missions = driver.supervisor.missions().unwrap();
    let states: Vec<State> = missions
        .iter()
        .map(work_supervision_domain::Mission::state)
        .collect();
    assert_eq!(
        states,
        vec![State::Accepted, State::Abandoned, State::Cancelled]
    );
    assert_eq!(driver.supervisor.journal_entries(), 20);
}

#[test]
fn refused_commands_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut driver = open(&layout);
    let id = mission_id(1);
    driver.run(&id, create("One")).unwrap();
    let before = fs::read(layout.journal()).unwrap();
    assert_eq!(
        driver
            .run(
                &id,
                Command::Accept {
                    reason: "x".to_owned()
                }
            )
            .unwrap_err(),
        SupervisorError::Refused(Refusal::TransitionForbidden {
            state: State::Draft,
            command: work_supervision_domain::CommandKind::Accept
        })
    );
    assert_eq!(
        driver
            .supervisor
            .execute(&id, &Command::Ready, 7, at(99))
            .unwrap_err(),
        SupervisorError::Refused(Refusal::RevisionStale {
            expected: 7,
            actual: 1
        })
    );
    assert_eq!(
        driver.run(&id, create("One again")).unwrap_err(),
        SupervisorError::Refused(Refusal::AlreadyExists)
    );
    assert_eq!(
        driver.run(&mission_id(9), Command::Ready).unwrap_err(),
        SupervisorError::Refused(Refusal::NotFound)
    );
    assert_eq!(
        fs::read(layout.journal()).unwrap(),
        before,
        "no entry for a refusal"
    );
    let error = SupervisorError::Refused(Refusal::NotFound);
    assert_eq!(error.code(), "mission.not_found");
}

#[test]
fn a_lost_projection_is_rebuilt_on_open_equal_to_the_lost_one() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let before = {
        let mut driver = open(&layout);
        full_life(&mut driver, &mission_id(1), "First");
        driver.supervisor.store().dump().unwrap()
    };
    fs::remove_file(layout.state()).unwrap();
    let _ = fs::remove_file(layout.state().with_extension("sqlite-wal"));
    let _ = fs::remove_file(layout.state().with_extension("sqlite-shm"));
    let driver = open(&layout);
    assert_eq!(driver.supervisor.store().dump().unwrap(), before);
    let rebuilt = Store::rebuild(
        &layout.journal(),
        &layout.blob_store().unwrap(),
        &dir.path().join("r.sqlite"),
    )
    .unwrap();
    assert_eq!(rebuilt.dump().unwrap(), before);
}

#[test]
fn the_journal_holds_no_text_of_any_mission() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut driver = open(&layout);
    full_life(&mut driver, &mission_id(1), "Secret title");
    let journal = fs::read_to_string(layout.journal()).unwrap();
    for text in [
        "Secret title",
        "brief for",
        "criterion one",
        "first note",
        "the summary",
        "try again",
        "fine\"",
    ] {
        assert!(!journal.contains(text), "journal leaks {text:?}");
    }
}

#[test]
fn the_layout_places_everything_under_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    for path in [
        layout.journal(),
        layout.state(),
        layout.blobs(),
        layout.evidence(),
        layout.worktrees(),
        layout.runs(),
        layout.config(),
    ] {
        assert!(path.starts_with(dir.path()), "{path:?}");
    }
    assert!(layout.journal().ends_with("journal/journal.v0.jsonl"));
}
