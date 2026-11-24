#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Forge signals routed to a mission (docs/work-supervision/coordination-v0.md,
//! « Forge signals »): the closed vocabulary, the link rule, the routing to a
//! linked pull request, the outcome table and the fixed follow-up template.

use work_supervision_domain::coordination::{Actor, CoordinationRefusal};
use work_supervision_domain::forge::{
    Admission, AllowedSender, ForgeLogin, ForgeMode, ForgeRepository, ForgeSignal, IgnoreReason,
    MAX_FORGE_LINKS, PullNumber, SenderEntry, SignalOutcome, SignalSource, SignalState, admit,
    check_link, follow_up, outcome, route, triggers,
};
use work_supervision_domain::{CommitId, MissionId, SessionId};

fn repository(text: &str) -> ForgeRepository {
    ForgeRepository::parse(text).unwrap()
}

fn pull(number: u64) -> PullNumber {
    PullNumber::new(number).unwrap()
}

fn login(text: &str) -> ForgeLogin {
    ForgeLogin::parse(text).unwrap()
}

fn signal(source: SignalSource, state: SignalState, pulls: &[u64]) -> ForgeSignal {
    ForgeSignal::new(
        source,
        42,
        state,
        CommitId::parse(&"a".repeat(40)).unwrap(),
        repository("libre-ai/sample"),
        pulls.iter().copied().map(pull).collect(),
        login("reviewer-1"),
    )
    .unwrap()
}

#[test]
fn repositories_and_logins_are_closed_forms_compared_without_case() {
    assert_eq!(
        repository("Libre-AI/Sample.rs").as_str(),
        "libre-ai/sample.rs"
    );
    for bad in [
        "",
        "sample",
        "/sample",
        "owner/",
        "owner/name/extra",
        "-owner/name",
        "owner/..",
        "owner/.",
        "own er/name",
        "owner/na\nme",
        &format!("{}/name", "o".repeat(40)),
        &format!("owner/{}", "n".repeat(101)),
    ] {
        assert_eq!(
            ForgeRepository::parse(bad),
            Err(CoordinationRefusal::FieldInvalid {
                field: "repository"
            }),
            "{bad:?}"
        );
    }
    assert_eq!(login("Reviewer-1").as_str(), "reviewer-1");
    assert_eq!(login("github-actions[bot]").as_str(), "github-actions[bot]");
    for bad in [
        "",
        "-lead",
        "two words",
        "a[bot",
        "[bot]",
        "name\u{7}",
        &"l".repeat(40),
    ] {
        assert_eq!(
            ForgeLogin::parse(bad),
            Err(CoordinationRefusal::FieldInvalid { field: "sender" }),
            "{bad:?}"
        );
    }
    assert!(PullNumber::new(0).is_err());
    assert!(PullNumber::new(1 << 53).is_err());
    assert_eq!(pull(7).get(), 7);
}

#[test]
fn a_state_belongs_to_its_source_and_the_vocabulary_is_closed() {
    for text in [
        "approved",
        "changes_requested",
        "commented",
        "dismissed",
        "pending",
        "success",
        "failure",
        "neutral",
        "cancelled",
        "skipped",
        "timed_out",
        "action_required",
        "stale",
    ] {
        assert_eq!(SignalState::parse(text).unwrap().as_str(), text);
    }
    assert!(SignalState::parse("APPROVED").is_err());
    assert!(SignalState::parse("merged").is_err());
    let commit = CommitId::parse(&"b".repeat(40)).unwrap();
    let make = |source, state| {
        ForgeSignal::new(
            source,
            1,
            state,
            commit.clone(),
            repository("o/r"),
            vec![pull(1)],
            login("x"),
        )
    };
    assert!(make(SignalSource::Review, SignalState::ChangesRequested).is_ok());
    assert!(make(SignalSource::Check, SignalState::Failure).is_ok());
    assert!(make(SignalSource::Review, SignalState::Pending).is_ok());
    assert!(make(SignalSource::Check, SignalState::Pending).is_ok());
    assert_eq!(
        make(SignalSource::Review, SignalState::Failure),
        Err(CoordinationRefusal::FieldInvalid { field: "state" })
    );
    assert_eq!(
        make(SignalSource::Check, SignalState::Approved),
        Err(CoordinationRefusal::FieldInvalid { field: "state" })
    );
    let no_pull = ForgeSignal::new(
        SignalSource::Check,
        1,
        SignalState::Failure,
        commit.clone(),
        repository("o/r"),
        Vec::new(),
        login("x"),
    );
    assert_eq!(
        no_pull,
        Err(CoordinationRefusal::FieldInvalid { field: "pulls" })
    );
    let zero = ForgeSignal::new(
        SignalSource::Check,
        0,
        SignalState::Failure,
        commit,
        repository("o/r"),
        vec![pull(1)],
        login("x"),
    );
    assert_eq!(
        zero,
        Err(CoordinationRefusal::FieldInvalid { field: "signal" })
    );
}

#[test]
fn only_the_owner_links_a_pull_request_once_to_an_existing_mission() {
    let sample = repository("libre-ai/sample");
    assert_eq!(
        check_link(&Actor::Owner, true, &[], &sample, pull(3)),
        Ok(())
    );
    let session = Actor::Session(SessionId::from_bytes([4; 16]));
    assert_eq!(
        check_link(&session, true, &[], &sample, pull(3)),
        Err(CoordinationRefusal::OwnerOnly)
    );
    assert_eq!(
        check_link(&Actor::Owner, false, &[], &sample, pull(3)),
        Err(CoordinationRefusal::MissionNotFound)
    );
    // Several pull requests per mission; the same one twice is refused.
    let links = vec![(sample.clone(), pull(3))];
    assert_eq!(
        check_link(&Actor::Owner, true, &links, &sample, pull(4)),
        Ok(())
    );
    assert_eq!(
        check_link(&Actor::Owner, true, &links, &sample, pull(3)),
        Err(CoordinationRefusal::ForgeAlreadyLinked)
    );
    let full: Vec<_> = (1..=MAX_FORGE_LINKS as u64)
        .map(|n| (sample.clone(), pull(n)))
        .collect();
    assert_eq!(
        check_link(&Actor::Owner, true, &full, &sample, pull(1000)),
        Err(CoordinationRefusal::FieldInvalid { field: "links" })
    );
}

#[test]
fn a_signal_routes_to_a_linked_pull_request_or_is_refused() {
    let links = vec![(repository("libre-ai/sample"), pull(12))];
    let review = signal(SignalSource::Review, SignalState::Approved, &[12]);
    assert_eq!(route(&review, &links), Ok(pull(12)));
    // A check run names every pull request whose head it ran on: the first
    // linked one is taken.
    let check = signal(SignalSource::Check, SignalState::Success, &[9, 12]);
    assert_eq!(route(&check, &links), Ok(pull(12)));
    let stranger = signal(SignalSource::Review, SignalState::Approved, &[13]);
    assert_eq!(
        route(&stranger, &links),
        Err(CoordinationRefusal::ForgeNotLinked)
    );
    let elsewhere = vec![(repository("libre-ai/other"), pull(12))];
    assert_eq!(
        route(&review, &elsewhere),
        Err(CoordinationRefusal::ForgeNotLinked)
    );
    assert_eq!(
        route(&review, &[]),
        Err(CoordinationRefusal::ForgeNotLinked)
    );
}

fn allowed(entry: &str, sender: &str) -> AllowedSender {
    AllowedSender {
        entry: SenderEntry::parse(entry).unwrap(),
        login: login(sender),
    }
}

#[test]
fn a_sender_is_admitted_by_an_entry_name_or_ignored_for_a_closed_reason() {
    let item = signal(SignalSource::Review, SignalState::Approved, &[1]);
    assert_eq!(
        admit(&item, &[]),
        Admission::Ignored(IgnoreReason::NoSenderListed)
    );
    assert_eq!(
        admit(&item, &[allowed("ci", "github-actions[bot]")]),
        Admission::Ignored(IgnoreReason::SenderNotListed)
    );
    // The admission names the owner's entry, never the login.
    assert_eq!(
        admit(
            &item,
            &[
                allowed("ci", "github-actions[bot]"),
                allowed("lead", "Reviewer-1")
            ]
        ),
        Admission::Admitted(SenderEntry::parse("lead").unwrap())
    );
    assert_eq!(
        IgnoreReason::ALL
            .iter()
            .map(|reason| reason.as_str())
            .collect::<Vec<_>>(),
        ["no_sender_listed", "sender_not_listed"]
    );
    for bad in [
        "",
        "Lead",
        "1st",
        "-x",
        "a_b",
        "a.b",
        "a b",
        &"e".repeat(65),
    ] {
        assert_eq!(
            SenderEntry::parse(bad),
            Err(CoordinationRefusal::FieldInvalid { field: "sender" }),
            "{bad:?}"
        );
    }
}

#[test]
fn the_outcome_is_closed_by_the_admission_then_the_trigger_then_the_mode() {
    let listed = vec![allowed("lead", "reviewer-1")];
    let changes = signal(SignalSource::Review, SignalState::ChangesRequested, &[1]);
    let approved = signal(SignalSource::Review, SignalState::Approved, &[1]);
    let failed = signal(SignalSource::Check, SignalState::Failure, &[1]);
    let of = |item: &ForgeSignal, senders: &[AllowedSender], mode| {
        outcome(item, &admit(item, senders), mode)
    };
    // An empty list ignores everything, whatever the mode.
    for mode in [ForgeMode::Observe, ForgeMode::Active] {
        for item in [&changes, &approved, &failed] {
            assert_eq!(of(item, &[], mode), SignalOutcome::Ignored);
            assert_eq!(
                of(item, &[allowed("other", "someone-else")], mode),
                SignalOutcome::Ignored
            );
        }
        assert_eq!(of(&approved, &listed, mode), SignalOutcome::Observed);
    }
    assert_eq!(
        of(&changes, &listed, ForgeMode::Observe),
        SignalOutcome::WouldAccept
    );
    assert_eq!(
        of(&failed, &listed, ForgeMode::Active),
        SignalOutcome::Accepted
    );
    assert_eq!(ForgeMode::parse("observe").unwrap(), ForgeMode::Observe);
    assert!(ForgeMode::parse("auto").is_err());
    assert_eq!(SignalOutcome::Ignored.kind(), "forge.signal.ignored");
    assert_eq!(
        SignalOutcome::WouldAccept.kind(),
        "forge.signal.would-accept"
    );
}

#[test]
fn only_a_requested_change_and_a_failed_or_timed_out_check_trigger() {
    let triggering: Vec<(SignalSource, SignalState)> = SignalState::ALL
        .iter()
        .flat_map(|state| {
            [SignalSource::Review, SignalSource::Check]
                .into_iter()
                .map(move |source| (source, *state))
        })
        .filter(|(source, state)| triggers(*source, *state))
        .collect();
    assert_eq!(
        triggering,
        vec![
            (SignalSource::Review, SignalState::ChangesRequested),
            (SignalSource::Check, SignalState::Failure),
            (SignalSource::Check, SignalState::TimedOut),
        ]
    );
}

#[test]
fn the_follow_up_is_a_fixed_template_and_the_reference() {
    let original = MissionId::from_bytes([5; 16]);
    let item = signal(SignalSource::Review, SignalState::ChangesRequested, &[12]);
    let proposal = follow_up(&original, &item, pull(12));
    assert_eq!(proposal.title, "Follow-up of libre-ai/sample#12");
    assert!(
        proposal
            .brief
            .starts_with(work_supervision_domain::forge::FOLLOW_UP_BRIEF)
    );
    let reference = proposal
        .brief
        .strip_prefix(work_supervision_domain::forge::FOLLOW_UP_BRIEF)
        .unwrap();
    assert_eq!(
        reference,
        format!(
            "\nOriginal mission: {original}\nRepository: libre-ai/sample\nPull request: 12\n\
             Signal: review 42\nState: changes_requested\nCommit: {}\n",
            "a".repeat(40)
        )
    );
    // The template is valid for a mission creation.
    let create = work_supervision_domain::Command::Create {
        title: proposal.title.clone(),
        repository: "sample".to_owned(),
        brief: proposal.brief.clone(),
        criteria: Vec::new(),
        budgets: work_supervision_domain::Budgets::new(600, 1 << 20).unwrap(),
        executor: work_supervision_domain::ExecutorProfile::Fake,
    };
    assert!(work_supervision_domain::decide(&original, None, &create, 0).is_ok());
}
