//! Outline split (`docs/work-supervision/coordination-v0.md`, « Outline
//! split »): an approved outline becomes draft missions linked by their
//! dependencies, in one intent the recovery finishes after a crash.

use serde_json::{Value, json};
use work_supervision_domain::coordination::Actor;
use work_supervision_domain::outline::check_outline;
use work_supervision_domain::{ArtifactId, Budgets, Command, Mission, MissionId};
use work_supervision_store::{Plan, parse_plan};

use crate::coordination::{actor, invalid};
use crate::core::{
    Core, DEFAULT_DURATION_SECONDS, DEFAULT_OUTPUT_BYTES, Recovered, Shared, field, lock,
    mission_field, random_bytes,
};
use crate::{Failure, clock, fault};

/// The creation command of each planned mission, validated by the domain:
/// a plan with one invalid mission is refused before anything is written.
fn creations(core: &Core, parent: &Mission, plan: &Plan) -> Result<Vec<Command>, Failure> {
    core.config.repository(parent.repository())?;
    plan.missions
        .iter()
        .map(|planned| {
            let create = Command::Create {
                title: planned.title.clone(),
                repository: parent.repository().to_owned(),
                brief: planned.brief.clone(),
                criteria: planned.all_criteria(),
                budgets: Budgets::new(
                    planned
                        .max_duration_seconds
                        .unwrap_or(DEFAULT_DURATION_SECONDS),
                    planned.max_output_bytes.unwrap_or(DEFAULT_OUTPUT_BYTES),
                )?,
                executor: core.config.executor(),
            };
            let probe = MissionId::from_bytes([0; 16]);
            work_supervision_domain::decide(&probe, None, &create, 0)?;
            Ok(create)
        })
        .collect()
}

/// Creates the planned missions that do not exist yet, declares the
/// dependencies not declared yet, then confirms the split. Idempotent, so the
/// same steps finish a split a crash interrupted.
fn complete(
    core: &mut Core,
    parent: &Mission,
    artifact: &ArtifactId,
    plan: &Plan,
    missions: &[MissionId],
) -> Result<(), Failure> {
    let creations = creations(core, parent, plan)?;
    if creations.len() != missions.len() {
        return Err(invalid());
    }
    for (id, create) in missions.iter().zip(&creations) {
        if core.supervisor.mission(id)?.is_none() {
            core.execute(id, create)?;
        }
    }
    fault::hit("outline-missions-created");
    for (from, on) in &plan.edges {
        let (Some(from), Some(on)) = (missions.get(*from), missions.get(*on)) else {
            return Err(invalid());
        };
        let declared = core
            .supervisor
            .store()
            .dependency_edges()?
            .iter()
            .any(|(mission, target)| mission == from && target == on);
        if !declared {
            core.supervisor
                .declare_dependency(from, on, &Actor::Owner, clock::now()?)?;
        }
    }
    core.supervisor
        .outline_split_close(artifact, true, clock::now()?)?;
    Ok(())
}

/// `outline.split`: `mission`, `artifact` (its approved outline), `plan` (text).
pub(crate) fn split(shared: &Shared, request: &Value) -> Result<Value, Failure> {
    let mission = mission_field(request)?;
    let artifact = ArtifactId::parse(field(request, "artifact")?).map_err(|_| invalid())?;
    let text = field(request, "plan")?.to_owned();
    let actor = actor(request)?;
    let mut core = lock(shared)?;
    let parent = core.mission(&mission)?;
    // The outline first, then the plan: a plan is only read against an
    // outline the owner approved.
    check_outline(
        &mission,
        core.supervisor.store().outline_view(&artifact)?.as_ref(),
        &actor,
    )
    .map_err(work_supervision_store::SupervisorError::from)?;
    let plan = parse_plan(&text).map_err(work_supervision_store::SupervisorError::from)?;
    creations(&core, &parent, &plan)?;
    let missions = plan
        .missions
        .iter()
        .map(|_| random_bytes().map(MissionId::from_bytes))
        .collect::<Result<Vec<_>, _>>()?;
    core.supervisor.outline_split_intent(
        &mission,
        &artifact,
        &text,
        &missions,
        &actor,
        clock::now()?,
    )?;
    fault::hit("outline-split-intended");
    complete(&mut core, &parent, &artifact, &plan, &missions)?;
    Ok(json!({
        "missions": plan.missions.iter().zip(&missions).map(|(planned, id)| json!({
            "key": planned.key, "mission": id.as_str(),
        })).collect::<Vec<_>>(),
    }))
}

/// Finishes every split a crash left under way; one that cannot be finished
/// (its repository left the configuration, a journal failure) is aborted,
/// leaving the missions already created as drafts the owner sees.
pub(crate) fn recover(core: &mut Core, recovered: &mut Recovered) -> Result<(), Failure> {
    for row in core.supervisor.store().pending_outline_splits()? {
        let artifact =
            ArtifactId::parse(&row.artifact).map_err(|_| Failure::new("projection.sqlite"))?;
        let finished = (|| -> Result<(), Failure> {
            let parent = core.mission(&MissionId::parse(&row.mission).map_err(|_| invalid())?)?;
            let plan = parse_plan(&row.plan).map_err(|_| invalid())?;
            let missions = row
                .missions
                .iter()
                .map(|id| MissionId::parse(id).map_err(|_| invalid()))
                .collect::<Result<Vec<_>, _>>()?;
            complete(core, &parent, &artifact, &plan, &missions)
        })();
        if finished.is_ok() {
            recovered.outline_splits_confirmed += 1;
        } else {
            core.supervisor
                .outline_split_close(&artifact, false, clock::now()?)?;
            recovered.outline_splits_aborted += 1;
        }
    }
    Ok(())
}

/// What `mission.show` adds about outline splits: the split that planned the
/// mission, and the splits of its own outlines.
pub(crate) fn mission_json(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    let store = core.supervisor.store();
    let origin = store
        .outline_origin(mission.id())?
        .map(|(artifact, parent, position)| {
            json!({ "artifact": artifact, "mission": parent, "position": position })
        });
    let splits: Vec<Value> = store
        .outline_splits_of(mission.id())?
        .into_iter()
        .map(|row| {
            json!({
                "artifact": row.artifact,
                "state": row.state,
                "missions": row.missions,
                "intended_at": row.intent_at,
                "decided_at": row.decided_at,
            })
        })
        .collect();
    Ok(json!({ "planned_by": origin, "splits": splits }))
}
