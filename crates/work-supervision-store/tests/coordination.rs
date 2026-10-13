#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use serde_json::{Value, json};
use work_supervision_domain::coordination::{
    Actor, Harness, IdeaCommand, ReportedState, RequestCommand, RequestOption, Reversibility,
    SessionCommand, SessionOutcome,
};
use work_supervision_domain::{
    Budgets, CheckId, Command, CommitId, Digest32, ExecutorProfile, IdeaId, MissionId, RequestId,
    SessionId,
};
use work_supervision_journal::{Event, OpenMode, Timestamp};
use work_supervision_store::{CheckExit, Layout, Store, Supervisor, SupervisorError};

fn at(second: u32) -> Timestamp {
    Timestamp::parse(&format!(
        "2026-10-09T02:{:02}:{:02}.000Z",
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

    fn mission(&mut self, byte: u8, title: &str) -> MissionId {
        let id = MissionId::from_bytes([byte; 16]);
        let now = self.now();
        self.supervisor
            .execute(
                &id,
                &Command::Create {
                    title: title.to_owned(),
                    repository: "sample".to_owned(),
                    brief: format!("brief of {title}"),
                    criteria: vec!["tests pass".to_owned(), "docs updated".to_owned()],
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

fn session_id(byte: u8) -> SessionId {
    SessionId::from_bytes([byte; 16])
}

fn options() -> Vec<RequestOption> {
    vec![
        RequestOption {
            label: "Keep SQLite".to_owned(),
            consequence: "No migration; single user only.".to_owned(),
            reversibility: Reversibility::Reversible,
        },
        RequestOption {
            label: "Move to PostgreSQL".to_owned(),
            consequence: "Server to operate; multi-user ready.".to_owned(),
            reversibility: Reversibility::Costly,
        },
    ]
}

/// Drives every primitive once, through the supervisor only.
fn everything(world: &mut World) -> (MissionId, MissionId) {
    let first = world.mission(1, "Parser");
    let second = world.mission(2, "Cache");

    // A session declares itself, captures an idea and opens a request.
    let session = session_id(9);
    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::Register {
                harness: Harness::ClaudeCode,
                repository: Some("sample".to_owned()),
                mission: Some(first.clone()),
                label: "parser session".to_owned(),
                external: Some(Digest32::from_bytes([3; 32])),
            },
            &Actor::Owner,
            now,
        )
        .unwrap();
    let agent = Actor::Session(session.clone());
    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::Report {
                state: ReportedState::WaitingInput,
                note: Some("needs a storage decision".to_owned()),
            },
            &agent,
            now,
        )
        .unwrap();

    let idea = IdeaId::from_bytes([4; 16]);
    let now = world.now();
    world
        .supervisor
        .idea(
            &idea,
            &IdeaCommand::Capture {
                text: "memoise the tokenizer".to_owned(),
            },
            &agent,
            now,
        )
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .idea(
            &idea,
            &IdeaCommand::Qualify {
                repository: Some("sample".to_owned()),
                mission: Some(first.clone()),
                context: Some("seen while profiling".to_owned()),
            },
            &agent,
            now,
        )
        .unwrap();

    let request = RequestId::from_bytes([5; 16]);
    let now = world.now();
    world
        .supervisor
        .request(
            &request,
            &RequestCommand::Open {
                mission: Some((first.clone(), Some(work_supervision_domain::State::Draft))),
                question: "Which storage for the cache?".to_owned(),
                options: options(),
                recommended: Some(0),
            },
            &agent,
            now,
        )
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .request(
            &request,
            &RequestCommand::Answer {
                choice: 0,
                reason: "v0 is single user".to_owned(),
            },
            &Actor::Owner,
            now,
        )
        .unwrap();

    // Contract refinements on the second mission, still in draft.
    let now = world.now();
    world
        .supervisor
        .declare_dependency(&second, &first, &Actor::Owner, now)
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .declare_scope(&second, &["src/cache".to_owned()], &Actor::Owner, now)
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .declare_check(
            &second,
            0,
            &["cargo".to_owned(), "test".to_owned()],
            &Actor::Owner,
            now,
        )
        .unwrap();

    // A check runs on a submitted commit, and a scope check is recorded.
    let commit = CommitId::parse(&"a".repeat(40)).unwrap();
    let check = CheckId::from_bytes([6; 16]);
    let digest = world.supervisor.store().criterion_checks(&second).unwrap()[0]
        .argv_digest
        .clone();
    let now = world.now();
    world
        .supervisor
        .check_started(&second, &check, 0, &commit, &digest, now)
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .check_finished(
            &second,
            &check,
            &CheckExit {
                bytes: 12,
                digest: "b".repeat(64),
                exit_code: Some(0),
                signal: None,
                budget: None,
            },
            now,
        )
        .unwrap();
    let now = world.now();
    world
        .supervisor
        .record_scope_check(&second, &commit, 3, &["README.md"], now)
        .unwrap();

    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::End {
                outcome: SessionOutcome::Completed,
                summary: Some("parser refactored".to_owned()),
            },
            &agent,
            now,
        )
        .unwrap();
    (first, second)
}

#[test]
fn every_primitive_is_projected_as_the_domain_decided() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut world = World::open(&layout);
    let (first, second) = everything(&mut world);
    let store = world.supervisor.store();

    let sessions = store.sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        (
            sessions[0].harness.as_str(),
            sessions[0].state.as_str(),
            sessions[0].reported_state.as_deref(),
            sessions[0].outcome.as_deref()
        ),
        (
            "claude-code",
            "ended",
            Some("waiting-input"),
            Some("completed")
        )
    );
    assert_eq!(sessions[0].mission.as_deref(), Some(first.as_str()));

    let ideas = store.ideas().unwrap();
    assert_eq!(ideas.len(), 1);
    assert_eq!(ideas[0].state, "qualified");
    assert_eq!(ideas[0].text, "memoise the tokenizer");
    assert_eq!(ideas[0].captured_by, format!("session:{}", session_id(9)));

    let requests = store.requests(false).unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        (requests[0].state.as_str(), requests[0].choice),
        ("answered", Some(0))
    );
    assert_eq!(requests[0].options.len(), 2);
    assert_eq!(requests[0].options[1].reversibility, "costly");
    assert!(store.open_requests_of(&first).unwrap().is_empty());

    assert_eq!(
        store.dependencies_of(&second).unwrap(),
        [(first.clone(), work_supervision_domain::State::Draft)]
    );
    let scope = store.scope(&second).unwrap().unwrap();
    assert_eq!(scope[0].as_str(), "src/cache");
    assert!(store.scope(&first).unwrap().is_none());
    // The one-query form agrees with the per-mission one.
    let scopes = store.scopes().unwrap();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes.get(second.as_str()), Some(&scope));
    let checks = store.criterion_checks(&second).unwrap();
    assert_eq!((checks[0].criterion, checks[0].argv.len()), (0, 2));
    let outcomes = store.check_outcomes(&second).unwrap();
    assert!(outcomes[0].passed);
    let scope_check = store
        .scope_check(&second, &"a".repeat(40))
        .unwrap()
        .unwrap();
    assert_eq!(
        (scope_check.changed, scope_check.outside),
        (3, vec!["README.md".to_owned()])
    );

    // The timeline names every entry that mentions the mission.
    let kinds: Vec<String> = store
        .timeline(&second)
        .unwrap()
        .into_iter()
        .map(|row| row.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            "mission.created",
            "dependency.declared",
            "scope.declared",
            "check.declared",
            "check.started",
            "check.finished",
            "scope.checked"
        ]
    );
}

#[test]
fn the_projection_rebuilt_from_the_journal_equals_the_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let live = {
        let mut world = World::open(&layout);
        everything(&mut world);
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
fn the_journal_holds_no_coordination_text() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut world = World::open(&layout);
    everything(&mut world);
    let journal = fs::read_to_string(layout.journal()).unwrap();
    for text in [
        "memoise",
        "profiling",
        "Which storage",
        "Keep SQLite",
        "single user",
        "parser session",
        "needs a storage",
        "parser refactored",
        "src/cache",
        "cargo",
        "README.md",
    ] {
        assert!(!journal.contains(text), "journal leaks {text:?}");
    }
}

#[test]
fn refinements_are_refused_after_draft_and_for_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut world = World::open(&layout);
    let first = world.mission(1, "A");
    let second = world.mission(2, "B");
    let session = session_id(7);
    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::Register {
                harness: Harness::Codex,
                repository: None,
                mission: None,
                label: "x".to_owned(),
                external: None,
            },
            &Actor::Owner,
            now,
        )
        .unwrap();
    let now = world.now();
    let refused = world
        .supervisor
        .declare_dependency(&second, &first, &Actor::Session(session), now)
        .unwrap_err();
    assert_eq!(refused.code(), "actor.owner_only");

    world.ready(&second);
    let now = world.now();
    let refused = world
        .supervisor
        .declare_scope(&second, &["src".to_owned()], &Actor::Owner, now)
        .unwrap_err();
    assert_eq!(refused.code(), "mission.not_draft");

    // Refused declarations write nothing: one accepted dependency, then two
    // refusals, leave exactly one new entry.
    let now = world.now();
    world
        .supervisor
        .declare_dependency(&first, &second, &Actor::Owner, now)
        .unwrap();
    let third = world.mission(3, "C");
    let entries = world.supervisor.journal_entries();
    let now = world.now();
    world
        .supervisor
        .declare_dependency(&third, &first, &Actor::Owner, now)
        .unwrap();
    let now = world.now();
    let mut world_check =
        world
            .supervisor
            .declare_check(&third, 5, &["true".to_owned()], &Actor::Owner, now);
    assert_eq!(
        world_check.as_ref().unwrap_err().code(),
        "check.criterion_unknown"
    );
    let now = world.now();
    world_check =
        world
            .supervisor
            .declare_scope(&third, &["../escape".to_owned()], &Actor::Owner, now);
    assert_eq!(
        world_check.unwrap_err().code(),
        "coordination.field_invalid"
    );
    assert_eq!(world.supervisor.journal_entries(), entries + 1);
}

#[test]
fn an_ended_session_cannot_act_and_a_session_cannot_answer() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let mut world = World::open(&layout);
    let session = session_id(1);
    let agent = Actor::Session(session.clone());
    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::Register {
                harness: Harness::Pi,
                repository: None,
                mission: None,
                label: "pi".to_owned(),
                external: None,
            },
            &Actor::Owner,
            now,
        )
        .unwrap();
    let request = RequestId::from_bytes([2; 16]);
    let now = world.now();
    world
        .supervisor
        .request(
            &request,
            &RequestCommand::Open {
                mission: None,
                question: "Which?".to_owned(),
                options: options(),
                recommended: None,
            },
            &agent,
            now,
        )
        .unwrap();
    let answer = RequestCommand::Answer {
        choice: 1,
        reason: "self-approval".to_owned(),
    };
    let now = world.now();
    assert_eq!(
        world
            .supervisor
            .request(&request, &answer, &agent, now)
            .unwrap_err()
            .code(),
        "actor.owner_only"
    );
    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::End {
                outcome: SessionOutcome::Failed,
                summary: None,
            },
            &agent,
            now,
        )
        .unwrap();
    let now = world.now();
    assert_eq!(
        world
            .supervisor
            .idea(
                &IdeaId::from_bytes([3; 16]),
                &IdeaCommand::Capture {
                    text: "late".to_owned()
                },
                &agent,
                now
            )
            .unwrap_err()
            .code(),
        "session.ended"
    );
}

/// A root with one mission and one captured idea; the digest of the idea's text.
fn seeded(dir: &std::path::Path) -> (World, String) {
    let layout = Layout::new(dir);
    let mut world = World::open(&layout);
    world.mission(1, "A");
    let digest = world
        .supervisor
        .blobs()
        .put_text("an idea")
        .unwrap()
        .to_hex();
    world
        .append(
            "idea.captured",
            json!({ "idea": "44444444444444444444444444444444", "text_digest": digest, "actor": "owner" }),
        )
        .unwrap();
    (world, digest)
}

/// The projection is a second line: the write path validates before it
/// appends, and an entry the projection refuses stays in the journal, so each
/// case runs on a fresh root.
#[test]
fn inconsistent_coordination_events_are_refused_by_the_projection() {
    let mission = MissionId::from_bytes([1; 16]);
    let idea = "44444444444444444444444444444444";
    let digest = {
        let dir = tempfile::tempdir().unwrap();
        seeded(dir.path()).1
    };
    let cases: Vec<(&str, Value)> = vec![
        // Promoted without an intent.
        (
            "idea.promoted",
            json!({ "idea": idea, "mission": mission.as_str() }),
        ),
        // An actor that is not one.
        (
            "idea.captured",
            json!({ "idea": "55555555555555555555555555555555", "text_digest": digest, "actor": "root" }),
        ),
        // A request with a single option.
        (
            "request.opened",
            json!({ "request": idea, "mission": null, "question_digest": digest,
                    "options": [{ "label_digest": digest, "consequence_digest": digest, "reversibility": "reversible" }],
                    "recommended": null, "actor": "owner" }),
        ),
        // Answering a request that does not exist.
        (
            "request.answered",
            json!({ "request": idea, "choice": 0, "reason_digest": digest }),
        ),
        // A check started without a declaration.
        (
            "check.started",
            json!({ "mission": mission.as_str(), "check": idea, "criterion": 0,
                    "commit": "a".repeat(40), "argv_digest": digest }),
        ),
        // A harness outside the list.
        (
            "session.registered",
            json!({ "session": idea, "harness": "gpt", "repository": null, "mission": null,
                    "label_digest": digest, "external_digest": null }),
        ),
        // A dependency on itself.
        (
            "dependency.declared",
            json!({ "mission": mission.as_str(), "on": mission.as_str() }),
        ),
    ];
    for (kind, data) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (mut world, _) = seeded(dir.path());
        let error = world.append(kind, data).unwrap_err();
        assert_eq!(error.code(), "projection.event_invalid", "{kind}");
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut world, _) = seeded(dir.path());
    let unknown = world.append("idea.forgotten", json!({ "idea": idea }));
    assert_eq!(unknown.unwrap_err().code(), "projection.unknown_kind");
}
