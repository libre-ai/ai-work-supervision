//! Flow measures of the accepted missions (`docs/work-supervision/flow-v0.md`):
//! one read-only operation over the projected timeline. It appends nothing to
//! the journal and does not read the clock, so two reads of the same root
//! answer the same bytes.

use serde_json::{Value, json};
use work_supervision_domain::flow::{AcceptedMission, Measures, Percentiles, measure};
use work_supervision_domain::{ExecutorProfile, MissionId};
use work_supervision_store::AcceptedTimeline;

use crate::Failure;
use crate::clock;
use crate::core::{Shared, lock};

/// Schema name of the response.
const SCHEMA: &str = "libre-ai.work-supervision.flow.v0";

/// Every operation of this module.
const OPS: [&str; 1] = ["flow.measures"];

/// Whether this module handles `op`.
pub(crate) fn handles(op: &str) -> bool {
    OPS.contains(&op)
}

/// Executes one flow request. The operation takes no parameter.
pub(crate) fn handle(shared: &Shared, op: &str, _request: &Value) -> Result<Value, Failure> {
    match op {
        "flow.measures" => {
            let timelines = lock(shared)?.supervisor.store().accepted_timelines()?;
            let missions = timelines
                .iter()
                .map(accepted)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(measures_json(&measure(&missions)))
        }
        _ => Err(Failure::new("request.unknown_op")),
    }
}

/// One projected timeline as the domain measures it. An instant the journal
/// format does not explain stays in the entry as unreadable: the domain
/// reports the mission, it is not dropped here.
fn accepted(timeline: &AcceptedTimeline) -> Result<AcceptedMission, Failure> {
    let corrupt = |_| Failure::new("projection.sqlite");
    let mission = MissionId::parse(&timeline.mission).map_err(corrupt)?;
    // Same rule as `core::is_simulation`: every run of the fake agent is a
    // simulation. The match is exhaustive so that a new profile must decide.
    let simulated = match ExecutorProfile::from_config_name(&timeline.executor).map_err(corrupt)? {
        ExecutorProfile::Fake => true,
    };
    let events: Vec<(&str, Option<u64>)> = timeline
        .events
        .iter()
        .map(|row| (row.kind.as_str(), clock::epoch_seconds(&row.at)))
        .collect();
    Ok(AcceptedMission::from_timeline(mission, simulated, &events))
}

fn percentiles_json(percentiles: Percentiles) -> Value {
    json!({ "p50": percentiles.p50, "p85": percentiles.p85 })
}

fn measures_json(measures: &Measures) -> Value {
    json!({
        "schema": SCHEMA,
        "examined": measures.examined,
        "retained": measures.retained,
        "simulated": measures.simulated,
        "accepted_first_pass": measures.accepted_first_pass,
        "lead_seconds": percentiles_json(measures.lead_seconds),
        "review_seconds": percentiles_json(measures.review_seconds),
        "throughput_per_week_hundredths": measures.throughput_per_week_hundredths,
        "unusable": measures
            .unusable
            .iter()
            .map(|(mission, reason)| json!({ "mission": mission.as_str(), "code": reason.code() }))
            .collect::<Vec<_>>(),
    })
}
