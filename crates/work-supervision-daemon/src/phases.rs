//! Phase operations of the daemon (`docs/work-supervision/phases-v0.md`): the
//! workflow declaration, artifact submission, the owner's approval or return,
//! and the reads that show them.

use serde_json::{Value, json};
use work_supervision_domain::phases::{ArtifactDecision, MissionPhases, Phase};
use work_supervision_domain::{ArtifactId, Digest32, Mission};
use work_supervision_store::ArtifactRow;

use crate::Failure;
use crate::clock;
use crate::coordination::{actor, invalid, optional_text, strings};
use crate::core::{Core, Shared, field, lock, mission_field, random_bytes};

/// Every operation of this module.
const OPS: [&str; 6] = [
    "mission.workflow",
    "artifact.submit",
    "artifact.approve",
    "artifact.return",
    "artifact.list",
    "artifact.show",
];

/// Whether this module handles `op`.
pub(crate) fn handles(op: &str) -> bool {
    OPS.contains(&op)
}

fn artifact_field(request: &Value) -> Result<ArtifactId, Failure> {
    ArtifactId::parse(field(request, "artifact")?).map_err(|_| invalid())
}

/// An artifact row for a display; `content` only when it was read.
fn artifact_json(row: &ArtifactRow) -> Value {
    json!({
        "id": row.id,
        "mission": row.mission,
        "phase": row.phase,
        "state": row.state,
        "digest": row.digest,
        "bytes": row.bytes,
        "submitted_by": row.submitted_by,
        "submitted_at": row.submitted_at,
        "reason": row.reason,
        "decided_at": row.decided_at,
    })
}

/// Executes one phase request.
pub(crate) fn handle(shared: &Shared, op: &str, request: &Value) -> Result<Value, Failure> {
    let actor = actor(request)?;
    match op {
        "mission.workflow" => {
            let id = mission_field(request)?;
            let phases = strings(request, "phases")?;
            lock(shared)?
                .supervisor
                .declare_workflow(&id, &phases, &actor, clock::now()?)?;
            Ok(json!({}))
        }
        "artifact.submit" => {
            let id = mission_field(request)?;
            let phase = Phase::parse(field(request, "phase")?).map_err(|_| invalid())?;
            let artifact = ArtifactId::from_bytes(random_bytes()?);
            lock(shared)?.supervisor.submit_artifact(
                &id,
                &artifact,
                phase,
                field(request, "content")?,
                &actor,
                clock::now()?,
            )?;
            Ok(json!({ "artifact": artifact.as_str() }))
        }
        "artifact.approve" => {
            let artifact = artifact_field(request)?;
            let decision = ArtifactDecision::Approve {
                digest: Digest32::parse(field(request, "digest")?).map_err(|_| invalid())?,
                reason: optional_text(request, "reason")?,
            };
            lock(shared)?.supervisor.decide_artifact(
                &artifact,
                &decision,
                &actor,
                clock::now()?,
            )?;
            Ok(json!({}))
        }
        "artifact.return" => {
            let artifact = artifact_field(request)?;
            let decision = ArtifactDecision::Return {
                reason: field(request, "reason")?.to_owned(),
            };
            lock(shared)?.supervisor.decide_artifact(
                &artifact,
                &decision,
                &actor,
                clock::now()?,
            )?;
            Ok(json!({}))
        }
        "artifact.list" => {
            let id = mission_field(request)?;
            let core = lock(shared)?;
            core.mission(&id)?;
            let rows = core.supervisor.store().artifacts_of(&id)?;
            Ok(Value::Array(rows.iter().map(artifact_json).collect()))
        }
        "artifact.show" => {
            let artifact = artifact_field(request)?;
            let core = lock(shared)?;
            let row = core
                .supervisor
                .store()
                .artifact(artifact.as_str())?
                .ok_or(Failure::new("artifact.not_found"))?;
            let mut shown = artifact_json(&row);
            if let Value::Object(map) = &mut shown {
                map.insert("content".to_owned(), Value::String(row.content));
            }
            Ok(shown)
        }
        _ => Err(Failure::new("request.unknown_op")),
    }
}

/// Declared phases of `mission` without an approved artifact, in workflow order.
pub(crate) fn unapproved(core: &Core, mission: &Mission) -> Result<Vec<Phase>, Failure> {
    Ok(core
        .supervisor
        .store()
        .mission_phases(mission.id())?
        .unapproved())
}

/// Each declared phase with its current artifact (without content), for
/// `mission.show` and the report.
fn phases_json(phases: &MissionPhases, current: &[ArtifactRow]) -> Value {
    Value::Array(
        phases
            .workflow
            .iter()
            .flatten()
            .map(|phase| {
                let artifact = current.iter().find(|row| row.phase == phase.as_str());
                json!({
                    "phase": phase.as_str(),
                    "artifact": artifact.map(artifact_json),
                })
            })
            .collect(),
    )
}

/// What `mission.show` adds: the workflow and the current artifact of each phase.
pub(crate) fn mission_phases_json(core: &Core, mission: &Mission) -> Result<Value, Failure> {
    let store = core.supervisor.store();
    let phases = store.mission_phases(mission.id())?;
    let current = store.current_artifacts(mission.id())?;
    Ok(phases_json(&phases, &current))
}

/// What `ws report` adds: every declared phase, and the governing artifact
/// (the approved artifact of the latest declared phase) with its content.
pub(crate) fn report_phases(core: &Core, mission: &Mission) -> Result<(Value, Value), Failure> {
    let store = core.supervisor.store();
    let phases = store.mission_phases(mission.id())?;
    let current = store.current_artifacts(mission.id())?;
    let governing = phases.governing().and_then(|phase| {
        current
            .iter()
            .find(|row| row.phase == phase.as_str() && row.state == "approved")
    });
    let governing = governing.map_or(Value::Null, |row| {
        let mut shown = artifact_json(row);
        if let Value::Object(map) = &mut shown {
            map.insert("content".to_owned(), Value::String(row.content.clone()));
        }
        shown
    });
    Ok((phases_json(&phases, &current), governing))
}
