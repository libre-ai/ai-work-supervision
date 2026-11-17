//! Flow measures of accepted missions (`docs/work-supervision/flow-v0.md`).
//!
//! Pure arithmetic over instants already resolved to seconds: this module
//! reads no clock and no store. The caller hands, for each accepted mission,
//! the entries of its projected timeline; [`measure`] returns the percentiles,
//! the counts and the missions it could not measure, each with its code. A
//! mission that cannot be measured is never counted and never dropped:
//! `retained + unusable.len() == examined`.

use crate::MissionId;

/// Kind of the entry that opens a mission's lead time.
pub const CREATED: &str = "mission.created";
/// Kind of a submitted result; the latest one opens the review time.
pub const RESULT_SUBMITTED: &str = "mission.result-submitted";
/// Kind of a rejection; one is enough to lose the first-pass count.
pub const REJECTED: &str = "mission.rejected";
/// Kind of the entry that closes both durations.
pub const ACCEPTED: &str = "mission.accepted";

/// Every timeline kind the measures read.
pub const EVENT_KINDS: [&str; 4] = [CREATED, RESULT_SUBMITTED, REJECTED, ACCEPTED];

/// Seconds in the week the throughput is expressed over.
pub const WEEK_SECONDS: u64 = 604_800;

/// The instant of one timeline entry the measures need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instant {
    /// The timeline carries no entry of that kind.
    Missing,
    /// The entry exists and its instant is not in the journal format.
    Unreadable,
    /// Seconds since the epoch.
    At(u64),
}

/// Why an accepted mission is left out of the measures. Closed list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unusable {
    /// A created, result-submitted or accepted entry is absent.
    TimelineMissing,
    /// One of the three instants is not in the journal format.
    InstantInvalid,
    /// The acceptance precedes the creation or the latest submission.
    DurationNegative,
}

impl Unusable {
    /// Stable code of the reason.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::TimelineMissing => "timeline.missing",
            Self::InstantInvalid => "instant.invalid",
            Self::DurationNegative => "duration.negative",
        }
    }
}

/// One accepted mission as the measures need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedMission {
    /// The mission.
    pub mission: MissionId,
    /// Instant of its creation.
    pub created: Instant,
    /// Instant of its latest submitted result.
    pub submitted: Instant,
    /// Instant of its acceptance.
    pub accepted: Instant,
    /// Rejections its timeline carries.
    pub rejections: u64,
    /// Whether the fake agent executed it.
    pub simulated: bool,
}

impl AcceptedMission {
    /// Reads a mission's timeline entries, in journal order: each is its kind
    /// and its instant in seconds, `None` when the instant is unreadable.
    /// Kinds outside [`EVENT_KINDS`] are ignored. When a kind appears more
    /// than once, the latest entry gives the instant.
    #[must_use]
    pub fn from_timeline(
        mission: MissionId,
        simulated: bool,
        events: &[(&str, Option<u64>)],
    ) -> Self {
        let mut read = Self {
            mission,
            created: Instant::Missing,
            submitted: Instant::Missing,
            accepted: Instant::Missing,
            rejections: 0,
            simulated,
        };
        for (kind, seconds) in events {
            let instant = seconds.map_or(Instant::Unreadable, Instant::At);
            match *kind {
                CREATED => read.created = instant,
                RESULT_SUBMITTED => read.submitted = instant,
                ACCEPTED => read.accepted = instant,
                REJECTED => read.rejections += 1,
                _ => {}
            }
        }
        read
    }

    /// Creation, latest submission and acceptance in seconds, or the first
    /// reason, in the order of [`Unusable`], that forbids the measure.
    fn instants(&self) -> Result<(u64, u64, u64), Unusable> {
        let all = [self.created, self.submitted, self.accepted];
        if all.contains(&Instant::Missing) {
            return Err(Unusable::TimelineMissing);
        }
        let (Instant::At(created), Instant::At(submitted), Instant::At(accepted)) =
            (self.created, self.submitted, self.accepted)
        else {
            return Err(Unusable::InstantInvalid);
        };
        if accepted < created || accepted < submitted {
            return Err(Unusable::DurationNegative);
        }
        Ok((created, submitted, accepted))
    }
}

/// Median and 85th percentile of one duration; `None` without a mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Percentiles {
    /// 50th percentile, in seconds.
    pub p50: Option<u64>,
    /// 85th percentile, in seconds.
    pub p85: Option<u64>,
}

/// The flow measures of a root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measures {
    /// Accepted missions looked at.
    pub examined: u64,
    /// Missions the measures stand on.
    pub retained: u64,
    /// Retained missions the fake agent executed.
    pub simulated: u64,
    /// Retained missions accepted without any rejection.
    pub accepted_first_pass: u64,
    /// Creation to acceptance.
    pub lead_seconds: Percentiles,
    /// Latest submission to acceptance.
    pub review_seconds: Percentiles,
    /// Hundredths of a mission per week; `None` without a retained mission.
    pub throughput_per_week_hundredths: Option<u64>,
    /// Accepted missions left out, by mission identifier, each with its reason.
    pub unusable: Vec<(MissionId, Unusable)>,
}

/// The value at the nearest rank of `percentile` in an ascending list: rank
/// `ceil(percentile × n / 100)`, counted from 1. `None` for an empty list and
/// for a percentile above 100; a percentile of 0 gives the first value.
#[must_use]
pub fn nearest_rank(sorted: &[u64], percentile: u32) -> Option<u64> {
    let count = u64::try_from(sorted.len()).ok()?;
    let rank = u64::from(percentile).checked_mul(count)?.div_ceil(100);
    let index = usize::try_from(rank.max(1) - 1).ok()?;
    sorted.get(index).copied()
}

fn percentiles(mut durations: Vec<u64>) -> Percentiles {
    durations.sort_unstable();
    Percentiles {
        p50: nearest_rank(&durations, 50),
        p85: nearest_rank(&durations, 85),
    }
}

/// Hundredths of a mission per week over the window from the first retained
/// creation to the last retained acceptance, never shorter than one week.
fn throughput(retained: u64, first_created: u64, last_accepted: u64) -> u64 {
    let window = last_accepted
        .saturating_sub(first_created)
        .max(WEEK_SECONDS);
    let scaled = u128::from(retained) * 100 * u128::from(WEEK_SECONDS) / u128::from(window);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Measures the flow of the accepted missions handed in.
#[must_use]
pub fn measure(missions: &[AcceptedMission]) -> Measures {
    let mut unusable = Vec::new();
    let mut leads = Vec::new();
    let mut reviews = Vec::new();
    let mut simulated = 0;
    let mut accepted_first_pass = 0;
    let mut window: Option<(u64, u64)> = None;
    for mission in missions {
        let (created, submitted, accepted) = match mission.instants() {
            Ok(instants) => instants,
            Err(reason) => {
                unusable.push((mission.mission.clone(), reason));
                continue;
            }
        };
        leads.push(accepted - created);
        reviews.push(accepted - submitted);
        simulated += u64::from(mission.simulated);
        accepted_first_pass += u64::from(mission.rejections == 0);
        window = Some(window.map_or((created, accepted), |(first, last)| {
            (first.min(created), last.max(accepted))
        }));
    }
    unusable.sort_by(|left, right| left.0.cmp(&right.0));
    let count = |length: usize| u64::try_from(length).unwrap_or(u64::MAX);
    let retained = count(leads.len());
    Measures {
        examined: count(missions.len()),
        retained,
        simulated,
        accepted_first_pass,
        lead_seconds: percentiles(leads),
        review_seconds: percentiles(reviews),
        throughput_per_week_hundredths: window
            .map(|(first, last)| throughput(retained, first, last)),
        unusable,
    }
}
