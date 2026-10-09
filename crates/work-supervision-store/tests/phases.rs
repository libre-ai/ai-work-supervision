#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use serde_json::{Value, json};
use work_supervision_domain::coordination::{Actor, Harness, SessionCommand};
use work_supervision_domain::phases::{ArtifactDecision, Phase};
use work_supervision_domain::{
    ArtifactId, Budgets, Command, Digest32, ExecutorProfile, MissionId, SessionId,
};
use work_supervision_journal::{Event, OpenMode, Timestamp};
use work_supervision_store::{Layout, Store, Supervisor, SupervisorError};

fn at(second: u32) -> Timestamp {
    Timestamp::parse(&format!(
        "2026-10-09T03:{:02}:{:02}.000Z",
        (second / 60) % 60,
        second % 60
    ))
    .unwrap()
}

struct World {
    supervisor: Supervisor,
    clock: u32,
}

impl World {
    fn open(layout: &Layout) -> Self {
        Self {
            supervisor: Supervisor::open(layout, OpenMode::Strict).unwrap(),
            clock: 0,
        }
    }

    fn now(&mut self) -> Timestamp {
        self.clock += 1;
        at(self.clock)
    }

    fn mission(&mut self) -> MissionId {
        let id = MissionId::from_bytes([1; 16]);
        let now = self.now();
        self.supervisor
            .execute(
                &id,
                &Command::Create {
                    title: "Cache".to_owned(),
                    repository: "sample".to_owned(),
                    brief: "memoise the parser".to_owned(),
                    criteria: vec!["tests pass".to_owned()],
                    budgets: Budgets::new(600, 1 << 20).unwrap(),
                    executor: ExecutorProfile::Fake,
                },
                0,
                now,
            )
            .unwrap();
        id
    }

    fn ready(&mut self, id: &MissionId) {
        let revision = self.supervisor.mission(id).unwrap().unwrap().revision();
        let now = self.now();
        self.supervisor
            .execute(id, &Command::Ready, revision, now)
            .unwrap();
    }

    fn workflow(&mut self, id: &MissionId, phases: &[&str]) -> Result<(), SupervisorError> {
        let names: Vec<String> = phases.iter().map(|name| (*name).to_owned()).collect();
        let now = self.now();
        self.supervisor
            .declare_workflow(id, &names, &Actor::Owner, now)
            .map(|_| ())
    }

    fn submit(
        &mut self,
        id: &MissionId,
        byte: u8,
        phase: Phase,
        content: &str,
    ) -> Result<ArtifactId, SupervisorError> {
        let artifact = ArtifactId::from_bytes([byte; 16]);
        let now = self.now();
        self.supervisor
            .submit_artifact(id, &artifact, phase, content, &Actor::Owner, now)
            .map(|_| artifact)
    }

    fn digest(&self, artifact: &ArtifactId) -> Digest32 {
        let row = self
            .supervisor
            .store()
            .artifact(artifact.as_str())
            .unwrap()
            .unwrap();
        Digest32::parse(&row.digest).unwrap()
    }

    fn approve(&mut self, artifact: &ArtifactId) -> Result<(), SupervisorError> {
        let digest = self.digest(artifact);
        let now = self.now();
        self.supervisor
            .decide_artifact(
                artifact,
                &ArtifactDecision::Approve {
                    digest,
                    reason: Some("facts checked".to_owned()),
                },
                &Actor::Owner,
                now,
            )
            .map(|_| ())
    }

    fn state(&self, artifact: &ArtifactId) -> String {
        self.supervisor
            .store()
            .artifact(artifact.as_str())
            .unwrap()
            .unwrap()
            .state
    }

    fn append(&mut self, kind: &str, data: Value) -> Result<(), SupervisorError> {
        let Value::Object(map) = data else {
            panic!("object expected")
        };
        let now = self.now();
        self.supervisor
            .append(now, Event::new(kind, map).unwrap())
            .map(|_| ())
    }
}

/// A mission with a three-phase workflow: research and design approved, an
/// outline submitted, an earlier design returned.
fn everything(world: &mut World) -> (MissionId, [ArtifactId; 4]) {
    let mission = world.mission();
    world
        .workflow(&mission, &["research", "design", "outline"])
        .unwrap();
    world.ready(&mission);
    let research = world
        .submit(
            &mission,
            10,
            Phase::Research,
            "parse.rs:42 rebuilds the AST",
        )
        .unwrap();
    world.approve(&research).unwrap();
    let first_design = world
        .submit(&mission, 11, Phase::Design, "keep a hash map")
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .decide_artifact(
            &first_design,
            &ArtifactDecision::Return {
                reason: "bound the memory".to_owned(),
            },
            &Actor::Owner,
            now,
        )
        .unwrap();
    let design = world
        .submit(&mission, 12, Phase::Design, "an LRU of 256 entries")
        .unwrap();
    world.approve(&design).unwrap();
    let outline = world
        .submit(&mission, 13, Phase::Outline, "slice 1: cache hit test")
        .unwrap();
    (mission, [research, first_design, design, outline])
}

#[test]
fn phases_and_artifacts_are_projected_as_the_domain_decided() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::open(&Layout::new(dir.path()));
    let (mission, [research, first_design, design, outline]) = everything(&mut world);
    let store = world.supervisor.store();

    assert_eq!(
        store.workflow(&mission).unwrap(),
        Some(vec![Phase::Research, Phase::Design, Phase::Outline])
    );
    for (artifact, state) in [
        (&research, "approved"),
        (&first_design, "returned"),
        (&design, "approved"),
        (&outline, "submitted"),
    ] {
        assert_eq!(world.state(artifact), state, "{artifact}");
    }
    let phases = store.mission_phases(&mission).unwrap();
    assert_eq!(phases.unapproved(), vec![Phase::Outline]);
    assert_eq!(phases.governing(), Some(Phase::Design));

    let listed = store.artifacts_of(&mission).unwrap();
    assert_eq!(listed.len(), 4);
    assert_eq!(listed[0].id, outline.as_str(), "newest first");
    assert!(listed.iter().all(|row| row.content.is_empty()));
    let shown = store.artifact(design.as_str()).unwrap().unwrap();
    assert_eq!(shown.content, "an LRU of 256 entries");
    assert_eq!(shown.bytes, 21);
    assert_eq!(shown.reason.as_deref(), Some("facts checked"));
    assert_eq!(
        store
            .artifact(first_design.as_str())
            .unwrap()
            .unwrap()
            .reason
            .as_deref(),
        Some("bound the memory")
    );
}

#[test]
fn a_resubmission_supersedes_its_phase_and_every_later_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::open(&Layout::new(dir.path()));
    let (mission, [research, first_design, design, outline]) = everything(&mut world);

    let revised = world
        .submit(&mission, 14, Phase::Research, "parse.rs:42 and lex.rs:7")
        .unwrap();
    assert_eq!(world.state(&research), "superseded");
    assert_eq!(world.state(&design), "superseded");
    assert_eq!(world.state(&outline), "superseded");
    assert_eq!(world.state(&first_design), "returned", "a return stays");
    assert_eq!(world.state(&revised), "submitted");
    assert_eq!(
        world
            .supervisor
            .store()
            .mission_phases(&mission)
            .unwrap()
            .unapproved(),
        vec![Phase::Research, Phase::Design, Phase::Outline]
    );
    // The superseded design cannot be approved any more.
    let digest = world.digest(&design);
    let now = world.now();
    let refusal = world
        .supervisor
        .decide_artifact(
            &design,
            &ArtifactDecision::Approve {
                digest,
                reason: None,
            },
            &Actor::Owner,
            now,
        )
        .unwrap_err();
    assert_eq!(refusal.code(), "artifact.not_pending");
    // And the design is not open before the new research is approved.
    assert_eq!(
        world
            .submit(&mission, 15, Phase::Design, "unchanged")
            .unwrap_err()
            .code(),
        "phase.previous_unapproved"
    );
}

#[test]
fn an_approval_names_the_content_it_approves() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::open(&Layout::new(dir.path()));
    let mission = world.mission();
    world.workflow(&mission, &["research"]).unwrap();
    world.ready(&mission);
    let artifact = world
        .submit(&mission, 10, Phase::Research, "what the owner read")
        .unwrap();
    let now = world.now();
    let refusal = world
        .supervisor
        .decide_artifact(
            &artifact,
            &ArtifactDecision::Approve {
                digest: Digest32::from_bytes([0; 32]),
                reason: None,
            },
            &Actor::Owner,
            now,
        )
        .unwrap_err();
    assert_eq!(refusal.code(), "artifact.digest_mismatch");
    assert_eq!(world.state(&artifact), "submitted");
    world.approve(&artifact).unwrap();
    assert_eq!(world.state(&artifact), "approved");
}

#[test]
fn a_session_submits_but_never_decides_and_the_draft_is_closed_to_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::open(&Layout::new(dir.path()));
    let mission = world.mission();
    world.workflow(&mission, &["research"]).unwrap();
    assert_eq!(
        world
            .submit(&mission, 10, Phase::Research, "too early")
            .unwrap_err()
            .code(),
        "artifact.mission_draft"
    );
    assert_eq!(
        world.workflow(&mission, &["design"]).unwrap_err().code(),
        "workflow.already_declared"
    );
    world.ready(&mission);
    assert_eq!(
        world.workflow(&mission, &["research"]).unwrap_err().code(),
        "mission.not_draft"
    );

    let session = SessionId::from_bytes([3; 16]);
    let agent = Actor::Session(session.clone());
    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::Register {
                harness: Harness::Pi,
                repository: None,
                mission: Some(mission.clone()),
                label: "research session".to_owned(),
                external: None,
            },
            &agent,
            now,
        )
        .unwrap();
    let artifact = ArtifactId::from_bytes([10; 16]);
    let now = world.now();
    world
        .supervisor
        .submit_artifact(
            &mission,
            &artifact,
            Phase::Research,
            "found by the agent",
            &agent,
            now,
        )
        .unwrap();
    let row = world
        .supervisor
        .store()
        .artifact(artifact.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(row.submitted_by, format!("session:{session}"));
    let digest = world.digest(&artifact);
    let now = world.now();
    assert_eq!(
        world
            .supervisor
            .decide_artifact(
                &artifact,
                &ArtifactDecision::Approve {
                    digest,
                    reason: None
                },
                &agent,
                now
            )
            .unwrap_err()
            .code(),
        "actor.owner_only"
    );
    // An unregistered session is refused before anything is decided.
    let stranger = Actor::Session(SessionId::from_bytes([4; 16]));
    let now = world.now();
    assert_eq!(
        world
            .supervisor
            .submit_artifact(
                &mission,
                &ArtifactId::from_bytes([11; 16]),
                Phase::Research,
                "x",
                &stranger,
                now
            )
            .unwrap_err()
            .code(),
        "session.not_found"
    );
}

#[test]
fn the_projection_rebuilt_from_the_journal_equals_the_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let live = {
        let mut world = World::open(&layout);
        let (mission, _) = everything(&mut world);
        world
            .submit(&mission, 14, Phase::Research, "revised")
            .unwrap();
        world.supervisor.store().dump().unwrap()
    };
    let rebuilt = Store::rebuild(
        &layout.journal(),
        &layout.blob_store().unwrap(),
        &dir.path().join("rebuilt.sqlite"),
    )
    .unwrap();
    assert_eq!(rebuilt.dump().unwrap(), live);
}

#[test]
fn the_journal_holds_no_artifact_text() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut world = World::open(&layout);
    everything(&mut world);
    let journal = fs::read_to_string(layout.journal()).unwrap();
    for text in [
        "parse.rs",
        "hash map",
        "LRU",
        "slice 1",
        "bound the memory",
        "facts checked",
    ] {
        assert!(!journal.contains(text), "journal leaks {text:?}");
    }
}

/// The projection is a second line: each case appends, behind the write
/// path's back, an entry the projected state does not allow.
#[test]
fn inconsistent_phase_events_are_refused_by_the_projection() {
    let mission = MissionId::from_bytes([1; 16]).as_str().to_owned();
    let artifact = "a".repeat(32);
    let seeded = |dir: &std::path::Path| {
        let mut world = World::open(&Layout::new(dir));
        let id = world.mission();
        world.workflow(&id, &["research", "design"]).unwrap();
        world.ready(&id);
        world.submit(&id, 0xaa, Phase::Research, "seed").unwrap();
        let digest = world.digest(&ArtifactId::from_bytes([0xaa; 16])).to_hex();
        let text = world.supervisor.blobs().put_text("four").unwrap().to_hex();
        (world, digest, text)
    };
    let (other, digest, text) = {
        let dir = tempfile::tempdir().unwrap();
        let (_, digest, text) = seeded(dir.path());
        ("b".repeat(32), digest, text)
    };
    let cases: Vec<(&str, Value)> = vec![
        // A second workflow, and one after draft.
        (
            "workflow.declared",
            json!({ "mission": mission, "phases": ["outline"] }),
        ),
        // A phase outside the workflow.
        (
            "artifact.submitted",
            json!({ "mission": mission, "artifact": other, "phase": "outline",
                    "content_digest": text, "bytes": 4, "actor": "owner" }),
        ),
        // A size that is not the blob's.
        (
            "artifact.submitted",
            json!({ "mission": mission, "artifact": other, "phase": "research",
                    "content_digest": text, "bytes": 5, "actor": "owner" }),
        ),
        // An identifier taken already.
        (
            "artifact.submitted",
            json!({ "mission": mission, "artifact": artifact, "phase": "research",
                    "content_digest": text, "bytes": 4, "actor": "owner" }),
        ),
        // An approval of another content.
        (
            "artifact.approved",
            json!({ "mission": mission, "artifact": artifact,
                    "content_digest": text, "reason_digest": null }),
        ),
        // A decision on an artifact that does not exist.
        (
            "artifact.returned",
            json!({ "mission": mission, "artifact": other, "reason_digest": text }),
        ),
    ];
    assert_ne!(digest, text);
    for (kind, data) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (mut world, _, _) = seeded(dir.path());
        let error = world.append(kind, data.clone()).unwrap_err();
        assert_eq!(error.code(), "projection.event_invalid", "{kind} {data}");
    }
    // The same approval with the stored digest is accepted.
    let dir = tempfile::tempdir().unwrap();
    let (mut world, digest, _) = seeded(dir.path());
    world
        .append(
            "artifact.approved",
            json!({ "mission": mission, "artifact": artifact,
                    "content_digest": digest, "reason_digest": null }),
        )
        .unwrap();
}
