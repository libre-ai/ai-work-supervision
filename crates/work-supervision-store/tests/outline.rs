#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Outline split: plan parsing, write path and projection
//! (docs/work-supervision/coordination-v0.md, « Outline split »).

use serde_json::{Map, Value, json};
use work_supervision_domain::coordination::Actor;
use work_supervision_domain::phases::{ArtifactDecision, Phase};
use work_supervision_domain::{
    ArtifactId, Budgets, Command, Digest32, ExecutorProfile, MissionId, SessionId,
};
use work_supervision_journal::{Event, OpenMode, Timestamp};
use work_supervision_store::{Layout, Store, Supervisor, SupervisorError, parse_plan};

fn at(second: u32) -> Timestamp {
    Timestamp::parse(&format!(
        "2026-10-13T04:{:02}:{:02}.000Z",
        (second / 60) % 60,
        second % 60
    ))
    .unwrap()
}

struct World {
    supervisor: Supervisor,
    clock: u32,
}

const PLAN: &str = r#"{"missions": [
  {"key": "lexer", "title": "Lexer", "brief": "tokens", "demonstrates": "a file is tokenised"},
  {"key": "parser", "title": "Parser", "brief": "tree", "demonstrates": "a file is parsed",
   "criteria": ["errors carry a position"], "after": ["lexer"]}
]}"#;

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

    fn create(&mut self, id: &MissionId, title: &str) {
        let now = self.now();
        self.supervisor
            .execute(
                id,
                &Command::Create {
                    title: title.to_owned(),
                    repository: "sample".to_owned(),
                    brief: "work".to_owned(),
                    criteria: vec!["shown".to_owned()],
                    budgets: Budgets::new(600, 1 << 20).unwrap(),
                    executor: ExecutorProfile::Fake,
                },
                0,
                now,
            )
            .unwrap();
    }

    /// A mission with an approved outline; returns (mission, outline).
    fn outlined(&mut self) -> (MissionId, ArtifactId) {
        let id = MissionId::from_bytes([1; 16]);
        self.create(&id, "Design");
        let now = self.now();
        self.supervisor
            .declare_workflow(&id, &["outline".to_owned()], &Actor::Owner, now)
            .unwrap();
        let now = self.now();
        self.supervisor
            .execute(&id, &Command::Ready, 1, now)
            .unwrap();
        let artifact = ArtifactId::from_bytes([7; 16]);
        let now = self.now();
        self.supervisor
            .submit_artifact(
                &id,
                &artifact,
                Phase::Outline,
                "two slices",
                &Actor::Owner,
                now,
            )
            .unwrap();
        (id, artifact)
    }

    fn approve(&mut self, artifact: &ArtifactId) {
        let digest = Digest32::parse(
            &self
                .supervisor
                .store()
                .artifact(artifact.as_str())
                .unwrap()
                .unwrap()
                .digest,
        )
        .unwrap();
        let now = self.now();
        self.supervisor
            .decide_artifact(
                artifact,
                &ArtifactDecision::Approve {
                    digest,
                    reason: None,
                },
                &Actor::Owner,
                now,
            )
            .unwrap();
    }

    fn intent(
        &mut self,
        mission: &MissionId,
        artifact: &ArtifactId,
        plan: &str,
        children: &[MissionId],
    ) -> Result<(), SupervisorError> {
        let now = self.now();
        self.supervisor
            .outline_split_intent(mission, artifact, plan, children, &Actor::Owner, now)
    }

    fn append(&mut self, kind: &str, data: Value) -> Result<(), SupervisorError> {
        let Value::Object(map) = data else {
            panic!("object expected")
        };
        let map: Map<String, Value> = map;
        let now = self.now();
        self.supervisor
            .append(now, Event::new(kind, map).unwrap())
            .map(|_| ())
    }
}

fn children() -> [MissionId; 2] {
    [
        MissionId::from_bytes([2; 16]),
        MissionId::from_bytes([3; 16]),
    ]
}

#[test]
fn a_plan_parses_with_its_demonstration_as_first_criterion() {
    let plan = parse_plan(PLAN).unwrap();
    assert_eq!(plan.edges, [(1, 0)]);
    assert_eq!(
        plan.missions[1].all_criteria(),
        ["a file is parsed", "errors carry a position"]
    );
    for bad in [
        "not json",
        "[]",
        r#"{"missions": [], "extra": 1}"#,
        r#"{"missions": [{"key": "a", "title": "t", "brief": "b"}]}"#,
        r#"{"missions": [{"key": "a", "title": "t", "brief": "b", "demonstrates": "d", "scope": []}]}"#,
        r#"{"missions": [{"key": "a", "title": "t", "brief": "b", "demonstrates": "d", "after": "x"}]}"#,
        r#"{"missions": [{"key": "a", "title": "t", "brief": "b", "demonstrates": "d", "max_output_bytes": -1}]}"#,
    ] {
        assert_eq!(
            parse_plan(bad).unwrap_err().code(),
            "coordination.field_invalid",
            "{bad}"
        );
    }
    let cycle = r#"{"missions": [
      {"key": "a", "title": "t", "brief": "b", "demonstrates": "d", "after": ["b"]},
      {"key": "b", "title": "t", "brief": "b", "demonstrates": "d", "after": ["a"]}]}"#;
    assert_eq!(parse_plan(cycle).unwrap_err().code(), "dependency.cycle");
}

#[test]
fn a_split_records_intent_missions_dependencies_then_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let live = {
        let mut world = World::open(&layout);
        let (mission, artifact) = world.outlined();
        let [lexer, parser] = children();
        // A submitted, unapproved outline cannot be split.
        assert_eq!(
            world
                .intent(&mission, &artifact, PLAN, &children())
                .unwrap_err()
                .code(),
            "outline.unapproved"
        );
        world.approve(&artifact);
        // A session never splits; the identifiers must match the plan.
        let now = world.now();
        let session = Actor::Session(SessionId::from_bytes([5; 16]));
        assert_eq!(
            world
                .supervisor
                .outline_split_intent(&mission, &artifact, PLAN, &children(), &session, now)
                .unwrap_err()
                .code(),
            "actor.owner_only"
        );
        assert_eq!(
            world
                .intent(&mission, &artifact, PLAN, std::slice::from_ref(&lexer))
                .unwrap_err()
                .code(),
            "coordination.field_invalid"
        );
        world
            .intent(&mission, &artifact, PLAN, &children())
            .unwrap();
        let row = world
            .supervisor
            .store()
            .outline_split(artifact.as_str())
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "splitting");
        assert_eq!(row.missions, [lexer.as_str(), parser.as_str()]);
        assert_eq!(
            world
                .supervisor
                .store()
                .pending_outline_splits()
                .unwrap()
                .len(),
            1
        );
        world.create(&lexer, "Lexer");
        world.create(&parser, "Parser");
        let now = world.now();
        world
            .supervisor
            .declare_dependency(&parser, &lexer, &Actor::Owner, now)
            .unwrap();
        let now = world.now();
        world
            .supervisor
            .outline_split_close(&artifact, true, now)
            .unwrap();
        assert_eq!(
            world
                .supervisor
                .store()
                .outline_split(artifact.as_str())
                .unwrap()
                .unwrap()
                .state,
            "split"
        );
        assert_eq!(
            world.supervisor.store().outline_origin(&parser).unwrap(),
            Some((artifact.as_str().to_owned(), mission.as_str().to_owned(), 1))
        );
        // Once split, never twice.
        let others = [
            MissionId::from_bytes([8; 16]),
            MissionId::from_bytes([9; 16]),
        ];
        assert_eq!(
            world
                .intent(&mission, &artifact, PLAN, &others)
                .unwrap_err()
                .code(),
            "outline.already_split"
        );
        let timeline: Vec<String> = world
            .supervisor
            .store()
            .timeline(&mission)
            .unwrap()
            .into_iter()
            .map(|entry| entry.kind)
            .collect();
        assert!(timeline.contains(&"outline.split.intent".to_owned()));
        assert!(timeline.contains(&"outline.split.confirmed".to_owned()));
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
fn an_aborted_split_may_be_retried_with_new_missions() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut world = World::open(&layout);
    let (mission, artifact) = world.outlined();
    world.approve(&artifact);
    world
        .intent(&mission, &artifact, PLAN, &children())
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .outline_split_close(&artifact, false, now)
        .unwrap();
    let now = world.now();
    assert_eq!(
        world
            .supervisor
            .outline_split_close(&artifact, true, now)
            .unwrap_err()
            .code(),
        "outline.already_split"
    );
    let others = [
        MissionId::from_bytes([8; 16]),
        MissionId::from_bytes([9; 16]),
    ];
    world.intent(&mission, &artifact, PLAN, &others).unwrap();
    let row = world
        .supervisor
        .store()
        .outline_split(artifact.as_str())
        .unwrap()
        .unwrap();
    assert_eq!((row.state.as_str(), row.missions.len()), ("splitting", 2));
    assert_eq!(row.missions[0], others[0].as_str());
    // An intent naming an existing mission is refused by the projection too.
    let existing = MissionId::from_bytes([1; 16]);
    let forged = world.append(
        "outline.split.intent",
        json!({ "mission": mission.as_str(), "artifact": ArtifactId::from_bytes([6; 16]).as_str(),
                "plan_digest": "0".repeat(64), "missions": [existing.as_str()] }),
    );
    assert_eq!(forged.unwrap_err().code(), "projection.event_invalid");
}

#[test]
fn a_confirmation_before_every_mission_exists_is_refused_by_the_projection() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut world = World::open(&layout);
    let (mission, artifact) = world.outlined();
    world.approve(&artifact);
    world
        .intent(&mission, &artifact, PLAN, &children())
        .unwrap();
    world.create(&children()[0], "Lexer");
    let early = world.append(
        "outline.split.confirmed",
        json!({ "mission": mission.as_str(), "artifact": artifact.as_str() }),
    );
    assert_eq!(early.unwrap_err().code(), "projection.event_invalid");
}
