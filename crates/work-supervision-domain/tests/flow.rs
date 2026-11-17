#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Flow measures (docs/work-supervision/flow-v0.md). Expected values come
//! from the specification, worked by hand, never from the code's output.

use work_supervision_domain::MissionId;
use work_supervision_domain::flow::{
    ACCEPTED, AcceptedMission, CREATED, EVENT_KINDS, Instant, Percentiles, REJECTED,
    RESULT_SUBMITTED, Unusable, measure, nearest_rank,
};

const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;

fn mission(byte: u8) -> MissionId {
    MissionId::from_bytes([byte; 16])
}

/// An accepted mission created at `created`, submitted once at `submitted`
/// and accepted at `accepted`, without rejection, run by the fake agent.
fn accepted(byte: u8, created: u64, submitted: u64, accepted: u64) -> AcceptedMission {
    AcceptedMission {
        mission: mission(byte),
        created: Instant::At(created),
        submitted: Instant::At(submitted),
        accepted: Instant::At(accepted),
        rejections: 0,
        simulated: true,
    }
}

#[test]
fn nearest_rank_takes_the_value_at_the_ceiling_rank() {
    // 10, 20, 30, 40 and 100 hours: rank ceil(50 × 5 / 100) = 3, ceil(85 × 5 / 100) = 5.
    let leads = [36_000, 72_000, 108_000, 144_000, 360_000];
    assert_eq!(nearest_rank(&leads, 50), Some(108_000));
    assert_eq!(nearest_rank(&leads, 85), Some(360_000));
}

#[test]
fn nearest_rank_of_twenty_values_is_the_seventeenth_at_p85() {
    let values: Vec<u64> = (1..=20).collect();
    // ceil(85 × 20 / 100) = 17 exactly; ceil(50 × 20 / 100) = 10.
    assert_eq!(nearest_rank(&values, 85), Some(17));
    assert_eq!(nearest_rank(&values, 50), Some(10));
    // 81 × 20 / 100 = 16.2: the ceiling is 17, the floor would give 16.
    assert_eq!(nearest_rank(&values, 81), Some(17));
    assert_eq!(nearest_rank(&values, 100), Some(20));
}

#[test]
fn nearest_rank_has_no_value_for_an_empty_list() {
    assert_eq!(nearest_rank(&[], 50), None);
    assert_eq!(nearest_rank(&[], 85), None);
    assert_eq!(nearest_rank(&[7], 101), None);
    assert_eq!(nearest_rank(&[7, 9], 0), Some(7));
}

#[test]
fn percentiles_of_lead_and_review_come_from_sorted_durations() {
    // Leads of 100, 10, 40, 20 and 30 hours, handed out of order; their
    // reviews last 5, 1, 4, 2 and 3 hours.
    let missions = [
        accepted(1, 0, 95 * HOUR, 100 * HOUR),
        accepted(2, 0, 9 * HOUR, 10 * HOUR),
        accepted(3, 0, 36 * HOUR, 40 * HOUR),
        accepted(4, 0, 18 * HOUR, 20 * HOUR),
        accepted(5, 0, 27 * HOUR, 30 * HOUR),
    ];
    let measures = measure(&missions);
    assert_eq!(measures.examined, 5);
    assert_eq!(measures.retained, 5);
    assert_eq!(
        measures.lead_seconds,
        Percentiles {
            p50: Some(108_000),
            p85: Some(360_000)
        }
    );
    // Reviews: 1, 2, 3, 4 and 5 hours.
    assert_eq!(
        measures.review_seconds,
        Percentiles {
            p50: Some(3 * HOUR),
            p85: Some(5 * HOUR)
        }
    );
    assert_eq!(measures.accepted_first_pass, 5);
    assert_eq!(measures.simulated, 5);
    assert!(measures.unusable.is_empty());
}

#[test]
fn throughput_is_in_hundredths_of_a_mission_per_week_over_at_least_one_week() {
    // Three missions within two days: the window is raised to one week.
    let short = [
        accepted(1, 0, HOUR, 2 * HOUR),
        accepted(2, DAY, DAY + HOUR, DAY + 2 * HOUR),
        accepted(3, DAY, 2 * DAY - HOUR, 2 * DAY),
    ];
    assert_eq!(measure(&short).throughput_per_week_hundredths, Some(300));
    // Two missions over fourteen days, first creation to last acceptance.
    let long = [
        accepted(1, 0, HOUR, 2 * HOUR),
        accepted(2, 13 * DAY, 14 * DAY - HOUR, 14 * DAY),
    ];
    assert_eq!(measure(&long).throughput_per_week_hundredths, Some(100));
    // The window runs from the earliest creation to the latest acceptance,
    // whatever the order the missions are handed in.
    let reversed = [long[1].clone(), long[0].clone()];
    assert_eq!(measure(&reversed).throughput_per_week_hundredths, Some(100));
}

#[test]
fn a_mission_that_cannot_be_measured_is_listed_with_its_code_and_never_counted() {
    let missing = AcceptedMission {
        submitted: Instant::Missing,
        ..accepted(4, 0, 0, 10 * HOUR)
    };
    let invalid = AcceptedMission {
        created: Instant::Unreadable,
        ..accepted(3, 0, HOUR, 10 * HOUR)
    };
    let negative = accepted(2, 10 * HOUR, HOUR, 5 * HOUR);
    let good = accepted(1, 0, 8 * HOUR, 10 * HOUR);
    let measures = measure(&[missing, invalid, negative, good]);
    assert_eq!(measures.examined, 4);
    assert_eq!(measures.retained, 1);
    // By mission identifier, whatever the order handed in.
    assert_eq!(
        measures.unusable,
        vec![
            (mission(2), Unusable::DurationNegative),
            (mission(3), Unusable::InstantInvalid),
            (mission(4), Unusable::TimelineMissing),
        ]
    );
    assert_eq!(
        measures.retained + u64::try_from(measures.unusable.len()).unwrap(),
        measures.examined
    );
    // The percentiles stand on the retained mission alone.
    assert_eq!(measures.lead_seconds.p50, Some(10 * HOUR));
    assert_eq!(measures.lead_seconds.p85, Some(10 * HOUR));
    assert_eq!(measures.review_seconds.p50, Some(2 * HOUR));
    assert_eq!(measures.simulated, 1);
    assert_eq!(measures.accepted_first_pass, 1);
    assert_eq!(measures.throughput_per_week_hundredths, Some(100));
}

#[test]
fn the_codes_are_the_closed_list_of_the_specification() {
    assert_eq!(Unusable::TimelineMissing.code(), "timeline.missing");
    assert_eq!(Unusable::InstantInvalid.code(), "instant.invalid");
    assert_eq!(Unusable::DurationNegative.code(), "duration.negative");
    assert_eq!(
        EVENT_KINDS,
        [
            "mission.created",
            "mission.result-submitted",
            "mission.rejected",
            "mission.accepted"
        ]
    );
}

#[test]
fn one_code_per_mission_in_the_order_missing_invalid_negative() {
    // Missing wins over unreadable.
    let both = AcceptedMission {
        created: Instant::Unreadable,
        submitted: Instant::Missing,
        ..accepted(1, 0, 0, HOUR)
    };
    // Unreadable wins over an order that would be negative.
    let unreadable = AcceptedMission {
        submitted: Instant::Unreadable,
        ..accepted(2, 10 * HOUR, 0, HOUR)
    };
    // An acceptance before the latest submission is negative too.
    let review = accepted(3, 0, 6 * HOUR, 5 * HOUR);
    for (entry, code) in [
        (both, Unusable::TimelineMissing),
        (unreadable, Unusable::InstantInvalid),
        (review, Unusable::DurationNegative),
    ] {
        let id = entry.mission.clone();
        let measures = measure(&[entry]);
        assert_eq!(measures.unusable, vec![(id, code)]);
        assert_eq!(measures.retained, 0);
    }
}

#[test]
fn a_rejected_then_accepted_mission_is_retained_without_the_first_pass() {
    let once = AcceptedMission {
        rejections: 1,
        ..accepted(1, 0, 8 * HOUR, 10 * HOUR)
    };
    let clean = accepted(2, 0, 3 * HOUR, 4 * HOUR);
    let measures = measure(&[once, clean]);
    assert_eq!(measures.examined, 2);
    assert_eq!(measures.retained, 2);
    assert_eq!(measures.accepted_first_pass, 1);
    assert!(measures.unusable.is_empty());
}

#[test]
fn without_an_accepted_mission_every_measure_is_absent_not_zero() {
    let measures = measure(&[]);
    assert_eq!(measures.examined, 0);
    assert_eq!(measures.retained, 0);
    assert_eq!(measures.simulated, 0);
    assert_eq!(measures.accepted_first_pass, 0);
    let none = Percentiles {
        p50: None,
        p85: None,
    };
    assert_eq!(measures.lead_seconds, none);
    assert_eq!(measures.review_seconds, none);
    assert_eq!(measures.throughput_per_week_hundredths, None);
    assert!(measures.unusable.is_empty());
    // Only unusable missions: nothing retained, the measures stay absent.
    let only_unusable = measure(&[accepted(1, 10, 0, 5)]);
    assert_eq!(only_unusable.examined, 1);
    assert_eq!(only_unusable.retained, 0);
    assert_eq!(only_unusable.lead_seconds, none);
    assert_eq!(only_unusable.throughput_per_week_hundredths, None);
}

#[test]
fn a_timeline_gives_the_latest_submission_and_counts_the_rejections() {
    let read = AcceptedMission::from_timeline(
        mission(1),
        false,
        &[
            (CREATED, Some(100)),
            ("mission.readied", Some(150)),
            (RESULT_SUBMITTED, Some(200)),
            (REJECTED, Some(300)),
            (RESULT_SUBMITTED, Some(400)),
            (ACCEPTED, Some(450)),
        ],
    );
    assert_eq!(
        read,
        AcceptedMission {
            mission: mission(1),
            created: Instant::At(100),
            submitted: Instant::At(400),
            accepted: Instant::At(450),
            rejections: 1,
            simulated: false,
        }
    );
    let measures = measure(&[read]);
    assert_eq!(measures.lead_seconds.p50, Some(350));
    // From the latest submission, not the first.
    assert_eq!(measures.review_seconds.p50, Some(50));
    assert_eq!(measures.accepted_first_pass, 0);
    assert_eq!(measures.simulated, 0);
}

#[test]
fn a_timeline_without_an_entry_or_with_an_unreadable_instant_keeps_the_default() {
    let empty = AcceptedMission::from_timeline(mission(1), true, &[]);
    assert_eq!(empty.created, Instant::Missing);
    assert_eq!(empty.submitted, Instant::Missing);
    assert_eq!(empty.accepted, Instant::Missing);
    let unreadable = AcceptedMission::from_timeline(
        mission(2),
        true,
        &[
            (CREATED, Some(1)),
            (RESULT_SUBMITTED, Some(2)),
            (ACCEPTED, None),
        ],
    );
    assert_eq!(unreadable.accepted, Instant::Unreadable);
    assert_eq!(
        measure(&[empty, unreadable]).unusable,
        vec![
            (mission(1), Unusable::TimelineMissing),
            (mission(2), Unusable::InstantInvalid),
        ]
    );
}
