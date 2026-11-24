#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Forge links and signals through the supervisor, and their projection
//! (docs/work-supervision/coordination-v0.md, « Forge signals »).

use serde_json::{Value, json};
use work_supervision_domain::coordination::{Actor, CoordinationRefusal};
use work_supervision_domain::forge::{
    Admission, ForgeLogin, ForgeRepository, ForgeSignal, IgnoreReason, PullNumber, SenderEntry,
    SignalOutcome, SignalSource, SignalState,
};
use work_supervision_domain::{
    Budgets, Command, CommitId, ExecutorProfile, MissionId, SessionId, State,
};
use work_supervision_journal::{Event, OpenMode, Timestamp};
use work_supervision_store::{Layout, SignalRecord, Store, Supervisor, SupervisorError};

fn at(second: u32) -> Timestamp {
    Timestamp::parse(&format!(
        "2026-11-23T03:{:02}:{:02}.000Z",
        (second / 60) % 60,
        second % 60
    ))
    .unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    layout: Layout,
    supervisor: Supervisor,
    clock: u32,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        let supervisor = Supervisor::open(&layout, OpenMode::Strict).unwrap();
        Self {
            _dir: dir,
            layout,
            supervisor,
            clock: 0,
        }
    }

    fn now(&mut self) -> Timestamp {
        self.clock += 1;
        at(self.clock)
    }

    fn mission(&mut self, byte: u8) -> MissionId {
        let id = MissionId::from_bytes([byte; 16]);
        let now = self.now();
        self.supervisor
            .execute(
                &id,
                &Command::Create {
                    title: "Original".to_owned(),
                    repository: "sample".to_owned(),
                    brief: "the brief".to_owned(),
                    criteria: Vec::new(),
                    budgets: Budgets::new(600, 1 << 20).unwrap(),
                    executor: ExecutorProfile::Fake,
                },
                0,
                now,
            )
            .unwrap();
        id
    }

    fn abandon(&mut self, id: &MissionId) {
        let revision = self.supervisor.mission(id).unwrap().unwrap().revision();
        let now = self.now();
        self.supervisor
            .execute(
                id,
                &Command::Abandon {
                    reason: "done elsewhere".to_owned(),
                },
                revision,
                now,
            )
            .unwrap();
    }

    fn link(&mut self, id: &MissionId, actor: &Actor, pull: u64) -> Result<(), SupervisorError> {
        let now = self.now();
        self.supervisor
            .link_forge(id, &repository(), number(pull), actor, now)
    }

    fn record(
        &mut self,
        mission: &MissionId,
        signal: &ForgeSignal,
        outcome: SignalOutcome,
        proposal: Option<&str>,
        follow_up: Option<&MissionId>,
    ) -> Result<(), SupervisorError> {
        let admission = if outcome == SignalOutcome::Ignored {
            Admission::Ignored(IgnoreReason::SenderNotListed)
        } else {
            Admission::Admitted(SenderEntry::parse("lead").unwrap())
        };
        self.record_as(mission, signal, &admission, outcome, proposal, follow_up)
    }

    fn record_as(
        &mut self,
        mission: &MissionId,
        signal: &ForgeSignal,
        admission: &Admission,
        outcome: SignalOutcome,
        proposal: Option<&str>,
        follow_up: Option<&MissionId>,
    ) -> Result<(), SupervisorError> {
        let now = self.now();
        self.supervisor.record_forge_signal(
            &SignalRecord {
                mission,
                signal,
                pull: number(12),
                admission,
                outcome,
                proposal,
                follow_up,
            },
            now,
        )
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

fn repository() -> ForgeRepository {
    ForgeRepository::parse("libre-ai/sample").unwrap()
}

fn number(pull: u64) -> PullNumber {
    PullNumber::new(pull).unwrap()
}

fn signal(source: SignalSource, id: u64, state: SignalState, pulls: &[u64]) -> ForgeSignal {
    ForgeSignal::new(
        source,
        id,
        state,
        CommitId::parse(&"c".repeat(40)).unwrap(),
        repository(),
        pulls.iter().copied().map(number).collect(),
        ForgeLogin::parse("reviewer-1").unwrap(),
    )
    .unwrap()
}

fn coordination(error: SupervisorError) -> CoordinationRefusal {
    match error {
        SupervisorError::Coordination(refusal) => refusal,
        other => panic!("coordination refusal expected, got {other:?}"),
    }
}

#[test]
fn only_the_owner_links_and_a_terminal_mission_stays_as_it_is() {
    let mut world = World::new();
    let original = world.mission(1);
    world.abandon(&original);
    let before = world.supervisor.mission(&original).unwrap().unwrap();
    let session = Actor::Session(SessionId::from_bytes([9; 16]));
    assert_eq!(
        coordination(world.link(&original, &session, 12).unwrap_err()),
        // A session actor must first name a declared session.
        CoordinationRefusal::SessionNotFound
    );
    world.link(&original, &Actor::Owner, 12).unwrap();
    world.link(&original, &Actor::Owner, 13).unwrap();
    assert_eq!(
        coordination(world.link(&original, &Actor::Owner, 12).unwrap_err()),
        CoordinationRefusal::ForgeAlreadyLinked
    );
    assert_eq!(
        coordination(
            world
                .link(&MissionId::from_bytes([2; 16]), &Actor::Owner, 12)
                .unwrap_err()
        ),
        CoordinationRefusal::MissionNotFound
    );
    let after = world.supervisor.mission(&original).unwrap().unwrap();
    assert_eq!(after.state(), State::Abandoned);
    assert_eq!(after.revision(), before.revision());
    assert_eq!(
        world.supervisor.store().forge_links(&original).unwrap(),
        vec![(repository(), number(12)), (repository(), number(13))]
    );
}

#[test]
fn a_session_actor_declared_as_such_is_still_refused() {
    use work_supervision_domain::coordination::{Harness, SessionCommand};
    let mut world = World::new();
    let original = world.mission(1);
    let session = SessionId::from_bytes([9; 16]);
    let now = world.now();
    world
        .supervisor
        .session(
            &session,
            &SessionCommand::Register {
                harness: Harness::Other,
                repository: None,
                mission: None,
                label: "agent".to_owned(),
                external: None,
            },
            &Actor::Owner,
            now,
        )
        .unwrap();
    assert_eq!(
        coordination(
            world
                .link(&original, &Actor::Session(session), 12)
                .unwrap_err()
        ),
        CoordinationRefusal::OwnerOnly
    );
    assert!(
        world
            .supervisor
            .store()
            .forge_links(&original)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn signals_are_recorded_by_outcome_and_an_unlinked_one_writes_nothing() {
    let mut world = World::new();
    let original = world.mission(1);
    let follow_up = MissionId::from_bytes([3; 16]);
    let review = signal(
        SignalSource::Review,
        41,
        SignalState::ChangesRequested,
        &[12],
    );
    let entries = world.supervisor.journal_entries();
    assert_eq!(
        coordination(
            world
                .record(&original, &review, SignalOutcome::Ignored, None, None)
                .unwrap_err()
        ),
        CoordinationRefusal::ForgeNotLinked
    );
    assert_eq!(world.supervisor.journal_entries(), entries);
    world.link(&original, &Actor::Owner, 12).unwrap();

    world
        .record(&original, &review, SignalOutcome::Ignored, None, None)
        .unwrap();
    let approved = signal(SignalSource::Review, 40, SignalState::Approved, &[12]);
    world
        .record(&original, &approved, SignalOutcome::Observed, None, None)
        .unwrap();
    world
        .record(
            &original,
            &review,
            SignalOutcome::WouldAccept,
            Some("{\"title\":\"proposal\"}"),
            None,
        )
        .unwrap();
    let failed = signal(SignalSource::Check, 77, SignalState::Failure, &[11, 12]);
    world
        .record(
            &original,
            &failed,
            SignalOutcome::Accepted,
            Some("{\"title\":\"accepted proposal\"}"),
            Some(&follow_up),
        )
        .unwrap();

    let store = world.supervisor.store();
    let rows = store.forge_signals_of(&original).unwrap();
    // Outcome, source, identifier, state, follow-up.
    type Summary<'a> = (&'a str, &'a str, i64, Option<&'a str>, Option<&'a str>);
    let summary: Vec<Summary<'_>> = rows
        .iter()
        .map(|row| {
            (
                row.outcome.as_str(),
                row.source.as_str(),
                row.signal,
                row.state.as_deref(),
                row.follow_up.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("ignored", "review", 41, None, None),
            ("observed", "review", 40, Some("approved"), None),
            (
                "would-accept",
                "review",
                41,
                Some("changes_requested"),
                None
            ),
            (
                "accepted",
                "check",
                77,
                Some("failure"),
                Some(follow_up.as_str())
            ),
        ]
    );
    // An ignored signal keeps its commit and a closed reason, nothing of its
    // sender; an admitted one names the entry that admitted it.
    assert_eq!(rows[0].commit, "c".repeat(40));
    let senders: Vec<(Option<&str>, Option<&str>)> = rows
        .iter()
        .map(|row| (row.sender.as_deref(), row.reason.as_deref()))
        .collect();
    assert_eq!(
        senders,
        vec![
            (None, Some("sender_not_listed")),
            (Some("lead"), None),
            (Some("lead"), None),
            (Some("lead"), None),
        ]
    );
    assert_eq!(rows[3].pull, 12);
    assert_eq!(
        rows[2].proposal.as_deref(),
        Some("{\"title\":\"proposal\"}")
    );
    assert!(
        store
            .forge_signal_recorded(&original, &review, SignalOutcome::Ignored)
            .unwrap()
    );
    assert!(
        !store
            .forge_signal_recorded(&original, &approved, SignalOutcome::Ignored)
            .unwrap()
    );
    assert_eq!(
        store.forge_follow_ups().unwrap(),
        vec![(
            original.clone(),
            follow_up,
            "{\"title\":\"accepted proposal\"}".to_owned()
        )]
    );
}

#[test]
fn an_outcome_that_does_not_fit_its_signal_is_refused_before_writing() {
    let mut world = World::new();
    let original = world.mission(1);
    world.link(&original, &Actor::Owner, 12).unwrap();
    let approved = signal(SignalSource::Review, 40, SignalState::Approved, &[12]);
    let changes = signal(
        SignalSource::Review,
        41,
        SignalState::ChangesRequested,
        &[12],
    );
    let entries = world.supervisor.journal_entries();
    let invalid = |field| CoordinationRefusal::FieldInvalid { field };
    let cases: Vec<(&ForgeSignal, SignalOutcome, Option<&str>, Option<MissionId>)> = vec![
        // An approval never triggers, so it is never proposed or accepted.
        (&approved, SignalOutcome::WouldAccept, Some("{}"), None),
        // A requested change triggers, so it is never merely observed.
        (&changes, SignalOutcome::Observed, None, None),
        // A proposal goes with a triggering outcome only, and is required there.
        (&changes, SignalOutcome::WouldAccept, None, None),
        (&approved, SignalOutcome::Ignored, Some("{}"), None),
        // An accepted signal names a new mission, never an existing one.
        (
            &changes,
            SignalOutcome::Accepted,
            Some("{}"),
            Some(original.clone()),
        ),
        (&changes, SignalOutcome::Accepted, Some("{}"), None),
    ];
    for (item, outcome, proposal, follow_up) in cases {
        assert_eq!(
            coordination(
                world
                    .record(&original, item, outcome, proposal, follow_up.as_ref())
                    .unwrap_err()
            ),
            invalid("outcome"),
            "{outcome:?} {:?}",
            item.state
        );
    }
    // The outcome follows the admission: an admitted signal is never recorded
    // as ignored, an ignored one never as admitted.
    let admitted = Admission::Admitted(SenderEntry::parse("lead").unwrap());
    let ignored = Admission::Ignored(IgnoreReason::NoSenderListed);
    for (admission, outcome, item) in [
        (&admitted, SignalOutcome::Ignored, &approved),
        (&ignored, SignalOutcome::Observed, &approved),
    ] {
        assert_eq!(
            coordination(
                world
                    .record_as(&original, item, admission, outcome, None, None)
                    .unwrap_err()
            ),
            invalid("outcome"),
            "{admission:?} {outcome:?}"
        );
    }
    assert_eq!(world.supervisor.journal_entries(), entries);
}

/// A world with one mission, linked to `libre-ai/sample#12` when `linked`.
fn seeded(linked: bool) -> (World, MissionId) {
    let mut world = World::new();
    let original = world.mission(1);
    if linked {
        world.link(&original, &Actor::Owner, 12).unwrap();
    }
    (world, original)
}

#[test]
fn the_projection_refuses_forge_events_that_do_not_follow_from_its_state() {
    let mission = MissionId::from_bytes([1; 16]);
    let reference = |kind_state: Option<&str>| {
        let mut data = json!({
            "mission": mission.as_str(), "repository": "libre-ai/sample", "pull": 12,
            "source": "review", "signal": 1, "commit": "c".repeat(40),
        });
        if let Some(state) = kind_state {
            data["state"] = json!(state);
            data["sender"] = json!("lead");
        } else {
            data["reason"] = json!("sender_not_listed");
        }
        data
    };
    let without = |mut data: Value, key: &str| {
        data.as_object_mut().unwrap().remove(key);
        data
    };
    let with = |mut data: Value, key: &str, value: Value| {
        data[key] = value;
        data
    };
    let link = |repository: &str, pull: u64| json!({ "mission": mission.as_str(), "repository": repository, "pull": pull });
    let cases: Vec<(&str, bool, &str, Value)> = vec![
        (
            "a signal on a pull request that is not linked",
            false,
            "forge.signal.observed",
            reference(Some("approved")),
        ),
        (
            "a repository outside its lowercase form",
            false,
            "mission.forge-linked",
            link("Libre-AI/sample", 12),
        ),
        (
            "a pull request number of zero",
            false,
            "mission.forge-linked",
            link("libre-ai/sample", 0),
        ),
        (
            "the same pull request linked twice",
            true,
            "mission.forge-linked",
            link("libre-ai/sample", 12),
        ),
        (
            "a state of the other source",
            true,
            "forge.signal.observed",
            reference(Some("failure")),
        ),
        (
            "a triggering state merely observed",
            true,
            "forge.signal.observed",
            reference(Some("changes_requested")),
        ),
        (
            "an ignored signal that keeps more than its reference",
            true,
            "forge.signal.ignored",
            reference(Some("approved")),
        ),
        (
            "a proposal without its blob digest",
            true,
            "forge.signal.would-accept",
            reference(Some("changes_requested")),
        ),
        (
            "an accepted signal whose follow-up is the mission itself",
            true,
            "forge.signal.accepted",
            with(
                reference(Some("changes_requested")),
                "follow_up",
                json!(mission.as_str()),
            ),
        ),
        (
            "an ignored signal carrying a key outside its catalogue",
            true,
            "forge.signal.ignored",
            with(reference(None), "login", json!("reviewer-1")),
        ),
        (
            "an admitted signal carrying a key outside its catalogue",
            true,
            "forge.signal.observed",
            with(reference(Some("approved")), "body", json!("text")),
        ),
        (
            "a link carrying a key outside its catalogue",
            false,
            "mission.forge-linked",
            with(link("libre-ai/sample", 12), "title", json!("text")),
        ),
        (
            "an ignored signal that names a sender",
            true,
            "forge.signal.ignored",
            with(reference(None), "sender", json!("lead")),
        ),
        (
            "an ignored signal without its reason",
            true,
            "forge.signal.ignored",
            without(reference(None), "reason"),
        ),
        (
            "a reason outside the closed vocabulary",
            true,
            "forge.signal.ignored",
            with(reference(None), "reason", json!("spam")),
        ),
        (
            "an ignored signal without its commit",
            true,
            "forge.signal.ignored",
            without(reference(None), "commit"),
        ),
        (
            "an admitted signal that carries a reason",
            true,
            "forge.signal.observed",
            with(
                reference(Some("approved")),
                "reason",
                json!("sender_not_listed"),
            ),
        ),
        (
            "an admitted signal without its entry",
            true,
            "forge.signal.observed",
            without(reference(Some("approved")), "sender"),
        ),
        (
            "a sender that is a login, not an entry name",
            true,
            "forge.signal.observed",
            with(
                reference(Some("approved")),
                "sender",
                json!("github-actions[bot]"),
            ),
        ),
    ];
    for (case, linked, kind, data) in cases {
        let (mut world, _) = seeded(linked);
        let error = world.append(kind, data).unwrap_err();
        assert_eq!(error.code(), "projection.event_invalid", "{case}");
    }
    // The same events, well formed, are projected.
    let (mut world, _) = seeded(true);
    world
        .append("forge.signal.observed", reference(Some("approved")))
        .unwrap();
    world
        .append("forge.signal.ignored", reference(None))
        .unwrap();
    let (mut world, _) = seeded(false);
    let unknown = world.append(
        "forge.signal.merged",
        json!({ "mission": mission.as_str() }),
    );
    assert_eq!(unknown.unwrap_err().code(), "projection.unknown_kind");
}

#[test]
fn the_rebuilt_projection_equals_the_live_one() {
    let mut world = World::new();
    let original = world.mission(1);
    world.link(&original, &Actor::Owner, 12).unwrap();
    let review = signal(
        SignalSource::Review,
        41,
        SignalState::ChangesRequested,
        &[12],
    );
    world
        .record(&original, &review, SignalOutcome::Ignored, None, None)
        .unwrap();
    world
        .record(
            &original,
            &review,
            SignalOutcome::WouldAccept,
            Some("{\"title\":\"proposal\"}"),
            None,
        )
        .unwrap();
    let live = world.supervisor.store().dump().unwrap();
    let target = world.layout.root().join("rebuilt.sqlite");
    let rebuilt = Store::rebuild(
        &world.layout.journal(),
        &world.layout.blob_store().unwrap(),
        &target,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(rebuilt.dump().unwrap()).unwrap(),
        String::from_utf8(live).unwrap()
    );
}
