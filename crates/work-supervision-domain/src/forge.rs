//! Forge signals routed to a mission (`docs/work-supervision/coordination-v0.md`,
//! « Forge signals »).
//!
//! Work Supervision opens no network connection: the owner links a mission to
//! the pull requests that carry its result, then passes the forge's own answer
//! about their reviews and check runs (`gh api …`) to `ws forge ingest`. What
//! enters is a **reference** only — source, identifier, a state in the closed
//! vocabulary below, a commit, the repository and the pull request. No text
//! written on the forge (body, comment, title, check name, summary) is ever
//! read past the parser, so none can reach a journal, a blob or a brief
//! (I-32 in `libre-ai/project-governance`). The sender's login is compared
//! with the owner's list and dropped: the journal names the list entry that
//! admitted a signal, and nothing of the sender of an ignored one.
//!
//! Pure like the rest of the crate: the caller reads the projection and the
//! configuration, passes what a rule needs and records what it gets back.

use crate::coordination::{Actor, CoordinationRefusal, named_enum};
use crate::{CommitId, MissionId};

/// Most pull requests linked to one mission.
pub const MAX_FORGE_LINKS: usize = 32;
/// Most pull requests one signal may name (a check run lists every pull
/// request whose head it ran on).
pub const MAX_SIGNAL_PULLS: usize = 32;
/// Largest identifier or number accepted: the journal keeps integers exact up
/// to 2^53 - 1.
pub const MAX_FORGE_NUMBER: u64 = (1 << 53) - 1;

/// Longest login part of a sender, the forge's own limit.
const MAX_LOGIN_CHARS: usize = 39;
/// Longest repository name, the forge's own limit.
const MAX_REPOSITORY_NAME_CHARS: usize = 100;
/// Suffix the forge gives to the login of an application.
const BOT_SUFFIX: &str = "[bot]";

fn is_account_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_LOGIN_CHARS
        && !text.starts_with('-')
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// A forge repository, `owner/name`, kept in lowercase: the forge compares
/// both without case, so must the link.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ForgeRepository(String);

impl ForgeRepository {
    /// Parses `owner/name`: an account name, a slash, then 1 to 100 of
    /// `[A-Za-z0-9._-]`, neither `.` nor `..`.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`repository`).
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        let invalid = CoordinationRefusal::FieldInvalid {
            field: "repository",
        };
        let (owner, name) = text.split_once('/').ok_or(invalid)?;
        let name_ok = !name.is_empty()
            && name.len() <= MAX_REPOSITORY_NAME_CHARS
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if is_account_name(owner) && name_ok {
            Ok(Self(text.to_ascii_lowercase()))
        } else {
            Err(invalid)
        }
    }

    /// `owner/name`, in lowercase.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A pull request number, 1 to [`MAX_FORGE_NUMBER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PullNumber(u64);

impl PullNumber {
    /// Checks the range.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`pull`).
    pub const fn new(number: u64) -> Result<Self, CoordinationRefusal> {
        if number == 0 || number > MAX_FORGE_NUMBER {
            Err(CoordinationRefusal::FieldInvalid { field: "pull" })
        } else {
            Ok(Self(number))
        }
    }

    /// The number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The login of whoever sent a signal, in lowercase: an account name, or an
/// application's slug followed by `[bot]` as the forge spells it.
///
/// It is compared with the configured senders and never recorded, not even
/// as a digest: a login is a short public name, so any digest of it a reader
/// can recompute is the login itself. An admitted signal records the
/// [`SenderEntry`] that admitted it; an ignored one records no trace of its
/// sender.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForgeLogin(String);

impl ForgeLogin {
    /// Parses a login.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`sender`).
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        let account = text.strip_suffix(BOT_SUFFIX).unwrap_or(text);
        if is_account_name(account) {
            Ok(Self(text.to_ascii_lowercase()))
        } else {
            Err(CoordinationRefusal::FieldInvalid { field: "sender" })
        }
    }

    /// The login, in lowercase.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Longest sender entry name.
const MAX_ENTRY_CHARS: usize = 64;

/// The owner's own name for one entry of the sender list (`[forge.senders]`
/// in `config.toml`): what an admitted signal records instead of its sender.
/// A lowercase letter, then lowercase letters, digits and `-`, 64 at most.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SenderEntry(String);

impl SenderEntry {
    /// Parses an entry name.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`sender`).
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        let mut bytes = text.bytes();
        let valid = matches!(bytes.next(), Some(b'a'..=b'z'))
            && text.len() <= MAX_ENTRY_CHARS
            && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'));
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoordinationRefusal::FieldInvalid { field: "sender" })
        }
    }

    /// The entry name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One entry of the sender list: the owner's name for it and the login it
/// admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedSender {
    /// The owner's name for the entry, recorded with what it admits.
    pub entry: SenderEntry,
    /// The login it admits, never recorded.
    pub login: ForgeLogin,
}

named_enum!(
    /// Why a signal was ignored, in a closed vocabulary. It says nothing of
    /// the sender.
    IgnoreReason, "reason", {
        /// No sender is configured: every signal is ignored.
        NoSenderListed => "no_sender_listed",
        /// The sender is not one of the configured entries.
        SenderNotListed => "sender_not_listed",
    }
);

/// Whether a signal's sender is admitted, and by which entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Not admitted, for this reason.
    Ignored(IgnoreReason),
    /// Admitted by this entry of the sender list.
    Admitted(SenderEntry),
}

/// Looks the sender of `signal` up in `senders`. An empty list admits nobody.
#[must_use]
pub fn admit(signal: &ForgeSignal, senders: &[AllowedSender]) -> Admission {
    if senders.is_empty() {
        return Admission::Ignored(IgnoreReason::NoSenderListed);
    }
    senders
        .iter()
        .find(|allowed| allowed.login == signal.sender)
        .map_or(
            Admission::Ignored(IgnoreReason::SenderNotListed),
            |allowed| Admission::Admitted(allowed.entry.clone()),
        )
}

named_enum!(
    /// What kind of forge object a signal is.
    SignalSource, "source", {
        /// A review of a pull request.
        Review => "review",
        /// A check run on a commit.
        Check => "check",
    }
);

named_enum!(
    /// The state of a signal, in a closed vocabulary: the review states and
    /// the check-run conclusions of the forge, plus `pending` for a review not
    /// submitted or a check run not completed. Anything else refuses the
    /// whole input.
    SignalState, "state", {
        /// Review: approved.
        Approved => "approved",
        /// Review: changes requested.
        ChangesRequested => "changes_requested",
        /// Review: commented.
        Commented => "commented",
        /// Review: dismissed.
        Dismissed => "dismissed",
        /// Review not submitted, or check run not completed.
        Pending => "pending",
        /// Check run: success.
        Success => "success",
        /// Check run: failure.
        Failure => "failure",
        /// Check run: neutral.
        Neutral => "neutral",
        /// Check run: cancelled.
        Cancelled => "cancelled",
        /// Check run: skipped.
        Skipped => "skipped",
        /// Check run: timed out.
        TimedOut => "timed_out",
        /// Check run: action required.
        ActionRequired => "action_required",
        /// Check run: stale.
        Stale => "stale",
    }
);

impl SignalState {
    /// Whether this state can be the state of a signal of `source`.
    #[must_use]
    pub const fn belongs_to(self, source: SignalSource) -> bool {
        match self {
            Self::Pending => true,
            Self::Approved | Self::ChangesRequested | Self::Commented | Self::Dismissed => {
                matches!(source, SignalSource::Review)
            }
            Self::Success
            | Self::Failure
            | Self::Neutral
            | Self::Cancelled
            | Self::Skipped
            | Self::TimedOut
            | Self::ActionRequired
            | Self::Stale => matches!(source, SignalSource::Check),
        }
    }
}

named_enum!(
    /// What an admitted signal that triggers does (configuration `[forge] mode`).
    ForgeMode, "mode", {
        /// Records what it would do, with the complete proposal; no effect.
        /// The default.
        Observe => "observe",
        /// Creates a draft follow-up mission.
        Active => "active",
    }
);

named_enum!(
    /// What became of one signal; each outcome is its own journal kind.
    SignalOutcome, "outcome", {
        /// The sender is not in the configured list: recorded, no effect.
        Ignored => "ignored",
        /// Admitted, but its state triggers nothing: recorded, no effect.
        Observed => "observed",
        /// Admitted and triggering, in `observe` mode: the proposal is
        /// recorded, no effect.
        WouldAccept => "would-accept",
        /// Admitted and triggering, in `active` mode: a draft follow-up
        /// mission is created.
        Accepted => "accepted",
    }
);

impl SignalOutcome {
    /// Journal kind, `forge.signal.<outcome>`.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Ignored => "forge.signal.ignored",
            Self::Observed => "forge.signal.observed",
            Self::WouldAccept => "forge.signal.would-accept",
            Self::Accepted => "forge.signal.accepted",
        }
    }
}

/// The states that call for follow-up work, by source. Everything else is
/// only observed. A constant on purpose: widening it is a reviewed change.
pub const TRIGGERS: [(SignalSource, SignalState); 3] = [
    (SignalSource::Review, SignalState::ChangesRequested),
    (SignalSource::Check, SignalState::Failure),
    (SignalSource::Check, SignalState::TimedOut),
];

/// Whether a signal of `source` in `state` calls for follow-up work.
#[must_use]
pub fn triggers(source: SignalSource, state: SignalState) -> bool {
    TRIGGERS.contains(&(source, state))
}

/// One review or check run, reduced to its reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeSignal {
    /// Review or check run.
    pub source: SignalSource,
    /// The forge's identifier of the review or check run.
    pub id: u64,
    /// Its state.
    pub state: SignalState,
    /// The commit it is about.
    pub commit: CommitId,
    /// The repository it belongs to.
    pub repository: ForgeRepository,
    /// The pull requests it concerns, in the forge's order.
    pub pulls: Vec<PullNumber>,
    /// Who sent it: compared with the sender list, never recorded.
    pub sender: ForgeLogin,
}

impl ForgeSignal {
    /// Builds a signal after checking its rules.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`]: `signal` (identifier out of
    /// range), `state` (a state of the other source), `pulls` (none, or more
    /// than [`MAX_SIGNAL_PULLS`]).
    pub fn new(
        source: SignalSource,
        id: u64,
        state: SignalState,
        commit: CommitId,
        repository: ForgeRepository,
        pulls: Vec<PullNumber>,
        sender: ForgeLogin,
    ) -> Result<Self, CoordinationRefusal> {
        if id == 0 || id > MAX_FORGE_NUMBER {
            return Err(CoordinationRefusal::FieldInvalid { field: "signal" });
        }
        if !state.belongs_to(source) {
            return Err(CoordinationRefusal::FieldInvalid { field: "state" });
        }
        if pulls.is_empty() || pulls.len() > MAX_SIGNAL_PULLS {
            return Err(CoordinationRefusal::FieldInvalid { field: "pulls" });
        }
        Ok(Self {
            source,
            id,
            state,
            commit,
            repository,
            pulls,
            sender,
        })
    }
}

/// Checks that `actor` may link the pull request `pull` of `repository` to a
/// mission (existing when `mission_exists`) already linked to `links`.
///
/// Linking writes no transition: a terminal mission can be linked (its result
/// is usually what the pull request carries), and stays terminal.
///
/// # Errors
///
/// [`CoordinationRefusal::OwnerOnly`], [`CoordinationRefusal::MissionNotFound`],
/// [`CoordinationRefusal::ForgeAlreadyLinked`] (a second link of the same pull
/// request is refused, not ignored) and [`CoordinationRefusal::FieldInvalid`]
/// (`links`, beyond [`MAX_FORGE_LINKS`]).
pub fn check_link(
    actor: &Actor,
    mission_exists: bool,
    links: &[(ForgeRepository, PullNumber)],
    repository: &ForgeRepository,
    pull: PullNumber,
) -> Result<(), CoordinationRefusal> {
    if *actor != Actor::Owner {
        return Err(CoordinationRefusal::OwnerOnly);
    }
    if !mission_exists {
        return Err(CoordinationRefusal::MissionNotFound);
    }
    if links
        .iter()
        .any(|(linked, number)| linked == repository && *number == pull)
    {
        return Err(CoordinationRefusal::ForgeAlreadyLinked);
    }
    if links.len() >= MAX_FORGE_LINKS {
        return Err(CoordinationRefusal::FieldInvalid { field: "links" });
    }
    Ok(())
}

/// The linked pull request `signal` concerns: the first of its pull requests
/// linked to the mission.
///
/// # Errors
///
/// [`CoordinationRefusal::ForgeNotLinked`] when none is.
pub fn route(
    signal: &ForgeSignal,
    links: &[(ForgeRepository, PullNumber)],
) -> Result<PullNumber, CoordinationRefusal> {
    signal
        .pulls
        .iter()
        .copied()
        .find(|pull| {
            links
                .iter()
                .any(|(repository, linked)| *repository == signal.repository && linked == pull)
        })
        .ok_or(CoordinationRefusal::ForgeNotLinked)
}

/// What becomes of `signal` given its [`admit`]ance: ignored unless admitted,
/// observed unless it triggers, then proposed (`observe`) or accepted
/// (`active`).
#[must_use]
pub fn outcome(signal: &ForgeSignal, admission: &Admission, mode: ForgeMode) -> SignalOutcome {
    if matches!(admission, Admission::Ignored(_)) {
        SignalOutcome::Ignored
    } else if !triggers(signal.source, signal.state) {
        SignalOutcome::Observed
    } else {
        match mode {
            ForgeMode::Observe => SignalOutcome::WouldAccept,
            ForgeMode::Active => SignalOutcome::Accepted,
        }
    }
}

/// The fixed part of a follow-up brief. Nothing of the signal but its
/// reference is appended to it.
pub const FOLLOW_UP_BRIEF: &str = "A forge signal on a pull request of the original mission calls for follow-up work.\n\
Only its reference was kept: no text written on the forge entered Work Supervision.\n\
Read the signal on the forge and rewrite this brief before readying the mission.\n";

/// Title and brief of a follow-up mission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUp {
    /// `Follow-up of owner/name#pull`.
    pub title: String,
    /// [`FOLLOW_UP_BRIEF`] then the reference, one field per line.
    pub brief: String,
}

/// The follow-up of `original` for `signal`, which concerns the linked pull
/// request `pull`: a fixed template filled with the reference only.
#[must_use]
pub fn follow_up(original: &MissionId, signal: &ForgeSignal, pull: PullNumber) -> FollowUp {
    let repository = signal.repository.as_str();
    let number = pull.get();
    FollowUp {
        title: format!("Follow-up of {repository}#{number}"),
        brief: format!(
            "{FOLLOW_UP_BRIEF}\nOriginal mission: {original}\nRepository: {repository}\n\
             Pull request: {number}\nSignal: {} {}\nState: {}\nCommit: {}\n",
            signal.source.as_str(),
            signal.id,
            signal.state.as_str(),
            signal.commit.as_str(),
        ),
    }
}
