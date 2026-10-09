//! Write path, projection and reads of the phases of a mission and their
//! artifacts (`docs/work-supervision/phases-v0.md`).
//!
//! Same order as the other primitives: the projection gives the current value,
//! the pure domain decides, the texts become blobs, the event is appended to
//! the journal, and only the durable entry reaches the projection.

use rusqlite::{OptionalExtension as _, Transaction, params};
use serde_json::{Map, Value};
use work_supervision_domain::coordination::Actor;
use work_supervision_domain::phases::{
    ArtifactDecision, ArtifactState, ArtifactView, MissionPhases, Phase, PhaseEvent,
    decide_artifact, decide_submission, decide_workflow,
};
use work_supervision_domain::{ArtifactId, Digest32, MissionId};
use work_supervision_journal::{Event, Timestamp};

use crate::projection::Fields;
use crate::{BlobStore, Store, StoreError, Supervisor, SupervisorError};

/// A projected artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRow {
    /// Identifier.
    pub id: String,
    /// Mission.
    pub mission: String,
    /// Phase.
    pub phase: String,
    /// `submitted`, `approved`, `returned` or `superseded`.
    pub state: String,
    /// Content; empty in listings.
    pub content: String,
    /// SHA-256 of the content.
    pub digest: String,
    /// Size of the content, in bytes.
    pub bytes: i64,
    /// Author (`owner`, `session:<id>`).
    pub submitted_by: String,
    /// Submission instant.
    pub submitted_at: String,
    /// Reason of the approval or the return.
    pub reason: Option<String>,
    /// Instant of the approval or the return.
    pub decided_at: Option<String>,
}

fn encode(event: &PhaseEvent, blobs: &BlobStore) -> Result<Event, SupervisorError> {
    let mut data = Map::new();
    let text = |value: &str| Value::String(value.to_owned());
    match event {
        PhaseEvent::WorkflowDeclared { mission, phases } => {
            data.insert("mission".to_owned(), text(mission.as_str()));
            data.insert(
                "phases".to_owned(),
                Value::Array(phases.iter().map(|phase| text(phase.as_str())).collect()),
            );
        }
        PhaseEvent::Submitted {
            mission,
            artifact,
            phase,
            content,
            actor,
        } => {
            data.insert("mission".to_owned(), text(mission.as_str()));
            data.insert("artifact".to_owned(), text(artifact.as_str()));
            data.insert("phase".to_owned(), text(phase.as_str()));
            data.insert(
                "content_digest".to_owned(),
                Value::String(blobs.put_text(content)?.to_hex()),
            );
            data.insert(
                "bytes".to_owned(),
                Value::from(u64::try_from(content.len()).unwrap_or(u64::MAX)),
            );
            data.insert("actor".to_owned(), Value::String(actor.to_text()));
        }
        PhaseEvent::Approved {
            mission,
            artifact,
            digest,
            reason,
        } => {
            data.insert("mission".to_owned(), text(mission.as_str()));
            data.insert("artifact".to_owned(), text(artifact.as_str()));
            data.insert("content_digest".to_owned(), Value::String(digest.to_hex()));
            data.insert(
                "reason_digest".to_owned(),
                match reason {
                    Some(reason) => Value::String(blobs.put_text(reason)?.to_hex()),
                    None => Value::Null,
                },
            );
        }
        PhaseEvent::Returned {
            mission,
            artifact,
            reason,
        } => {
            data.insert("mission".to_owned(), text(mission.as_str()));
            data.insert("artifact".to_owned(), text(artifact.as_str()));
            data.insert(
                "reason_digest".to_owned(),
                Value::String(blobs.put_text(reason)?.to_hex()),
            );
        }
    }
    Ok(Event::new(event.kind(), data)?)
}

impl Supervisor {
    /// Declares the workflow of `mission` (owner, mission in `draft`, once).
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Coordination`]; store and journal failures.
    pub fn declare_workflow(
        &mut self,
        mission: &MissionId,
        phases: &[String],
        actor: &Actor,
        at: Timestamp,
    ) -> Result<PhaseEvent, SupervisorError> {
        let current = self.store().mission_phases(mission)?;
        let event = decide_workflow(mission, &current, phases, actor)?;
        self.append(at, encode(&event, self.blobs())?)?;
        Ok(event)
    }

    /// Submits the artifact `artifact` of `phase` for `mission`.
    ///
    /// # Errors
    ///
    /// As [`Supervisor::declare_workflow`]; a session actor must be active.
    pub fn submit_artifact(
        &mut self,
        mission: &MissionId,
        artifact: &ArtifactId,
        phase: Phase,
        content: &str,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<PhaseEvent, SupervisorError> {
        self.require_actor(actor)?;
        let current = self.store().mission_phases(mission)?;
        let event = decide_submission(mission, artifact, &current, phase, content, actor)?;
        self.append(at, encode(&event, self.blobs())?)?;
        Ok(event)
    }

    /// Approves or returns the artifact `artifact` (owner).
    ///
    /// # Errors
    ///
    /// As [`Supervisor::declare_workflow`].
    pub fn decide_artifact(
        &mut self,
        artifact: &ArtifactId,
        decision: &ArtifactDecision,
        actor: &Actor,
        at: Timestamp,
    ) -> Result<PhaseEvent, SupervisorError> {
        let current = self.store().artifact_view(artifact)?;
        let event = decide_artifact(artifact, current.as_ref(), decision, actor)?;
        self.append(at, encode(&event, self.blobs())?)?;
        Ok(event)
    }
}

const ARTIFACT_COLUMNS: &str = "id, mission_id, phase, state, content, content_digest, bytes,
    submitted_by, submitted_at, reason, decided_at";

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRow> {
    Ok(ArtifactRow {
        id: row.get(0)?,
        mission: row.get(1)?,
        phase: row.get(2)?,
        state: row.get(3)?,
        content: row.get(4)?,
        digest: row.get(5)?,
        bytes: row.get(6)?,
        submitted_by: row.get(7)?,
        submitted_at: row.get(8)?,
        reason: row.get(9)?,
        decided_at: row.get(10)?,
    })
}

impl Store {
    /// The declared workflow of `mission`, in canonical order (`None`: none declared).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn workflow(&self, mission: &MissionId) -> Result<Option<Vec<Phase>>, StoreError> {
        let mut statement = self
            .connection()
            .prepare("SELECT phase FROM mission_phases WHERE mission_id = ?1 ORDER BY position")?;
        let names = statement
            .query_map([mission.as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if names.is_empty() {
            return Ok(None);
        }
        names
            .iter()
            .map(|name| Phase::parse(name).map_err(|_| StoreError::Sqlite))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// What the phase rules need of `mission`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn mission_phases(&self, mission: &MissionId) -> Result<MissionPhases, StoreError> {
        let state = self.mission(mission)?.map(|mission| mission.state());
        let current = self
            .current_artifacts(mission)?
            .iter()
            .map(|row| {
                Ok((
                    Phase::parse(&row.phase).map_err(|_| StoreError::Sqlite)?,
                    ArtifactState::parse(&row.state).map_err(|_| StoreError::Sqlite)?,
                ))
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(MissionPhases {
            state,
            workflow: self.workflow(mission)?,
            current,
        })
    }

    /// The artifact `id` as its rules need it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn artifact_view(&self, id: &ArtifactId) -> Result<Option<ArtifactView>, StoreError> {
        let Some(row) = self.artifact(id.as_str())? else {
            return Ok(None);
        };
        let mission = MissionId::parse(&row.mission).map_err(|_| StoreError::Sqlite)?;
        let mission_state = self.mission(&mission)?.ok_or(StoreError::Sqlite)?.state();
        Ok(Some(ArtifactView {
            mission,
            mission_state,
            state: ArtifactState::parse(&row.state).map_err(|_| StoreError::Sqlite)?,
            digest: Digest32::parse(&row.digest).map_err(|_| StoreError::Sqlite)?,
        }))
    }

    /// One artifact, with its content.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn artifact(&self, id: &str) -> Result<Option<ArtifactRow>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE id = ?1"),
                [id],
                read_row,
            )
            .optional()?)
    }

    /// Every artifact of `mission`, newest first, without content.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn artifacts_of(&self, mission: &MissionId) -> Result<Vec<ArtifactRow>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE mission_id = ?1
             ORDER BY submitted_seq DESC"
        ))?;
        let rows = statement
            .query_map([mission.as_str()], read_row)?
            .map(|row| {
                row.map(|mut row| {
                    row.content.clear();
                    row
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The current artifact (`submitted` or `approved`) of each phase of `mission`, with content.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn current_artifacts(&self, mission: &MissionId) -> Result<Vec<ArtifactRow>, StoreError> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {ARTIFACT_COLUMNS} FROM artifacts
             WHERE mission_id = ?1 AND state IN ('submitted', 'approved')
             ORDER BY submitted_seq"
        ))?;
        let rows = statement
            .query_map([mission.as_str()], read_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// `workflow.*` and `artifact.*`.
pub(crate) fn project(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let mission = fields.mission()?;
    let state: String = transaction
        .query_row(
            "SELECT state FROM missions WHERE id = ?1",
            [mission],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| fields.invalid())?;
    let terminal = matches!(state.as_str(), "accepted" | "abandoned" | "cancelled");
    let workflow: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT phase FROM mission_phases WHERE mission_id = ?1 ORDER BY position")?;
        statement
            .query_map([mission], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    match kind {
        "workflow.declared" => {
            let names: Vec<String> = fields
                .data
                .get("phases")
                .and_then(Value::as_array)
                .ok_or_else(|| fields.invalid())?
                .iter()
                .map(|name| name.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| fields.invalid())?;
            let phases = work_supervision_domain::phases::parse_workflow(&names)
                .map_err(|_| fields.invalid())?;
            if state != "draft" || !workflow.is_empty() {
                return Err(fields.invalid());
            }
            for (position, phase) in phases.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO mission_phases (mission_id, position, phase) VALUES (?1, ?2, ?3)",
                    params![
                        mission,
                        i64::try_from(position).map_err(|_| fields.invalid())?,
                        phase.as_str()
                    ],
                )?;
            }
            Ok(())
        }
        "artifact.submitted" => {
            let artifact = fields.identifier("artifact")?;
            let phase = Phase::parse(fields.string("phase")?).map_err(|_| fields.invalid())?;
            let author = fields.string("actor")?;
            Actor::parse(author).map_err(|_| fields.invalid())?;
            let (content, digest) = fields.text("content_digest", blobs)?;
            let bytes = fields.integer("bytes")?;
            let exists: i64 = transaction.query_row(
                "SELECT count(*) FROM artifacts WHERE id = ?1",
                [artifact],
                |row| row.get(0),
            )?;
            if state == "draft"
                || terminal
                || exists != 0
                || !workflow.iter().any(|name| name == phase.as_str())
                || i64::try_from(content.len()).ok() != Some(bytes)
            {
                return Err(fields.invalid());
            }
            // The submission supersedes the current artifact of its phase and of
            // every later phase: their approval stood on what this one replaces.
            let later: Vec<&str> = workflow
                .iter()
                .filter_map(|name| Phase::parse(name).ok())
                .filter(|other| *other >= phase)
                .map(Phase::as_str)
                .collect();
            for name in later {
                transaction.execute(
                    "UPDATE artifacts SET state = 'superseded'
                     WHERE mission_id = ?1 AND phase = ?2 AND state IN ('submitted', 'approved')",
                    params![mission, name],
                )?;
            }
            transaction.execute(
                "INSERT INTO artifacts (id, mission_id, phase, state, content, content_digest,
                   bytes, submitted_by, submitted_seq, submitted_at)
                 VALUES (?1, ?2, ?3, 'submitted', ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    artifact,
                    mission,
                    phase.as_str(),
                    content,
                    digest,
                    bytes,
                    author,
                    seq,
                    at
                ],
            )?;
            Ok(())
        }
        "artifact.approved" | "artifact.returned" => {
            let artifact = fields.identifier("artifact")?;
            let stored: Option<String> = transaction
                .query_row(
                    "SELECT content_digest FROM artifacts
                     WHERE id = ?1 AND mission_id = ?2 AND state = 'submitted'",
                    [artifact, mission],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(stored) = stored else {
                return Err(fields.invalid());
            };
            let (next, reason) = if kind == "artifact.approved" {
                // The approval binds the content it names, and no other.
                if fields.digest("content_digest")?.to_hex() != stored {
                    return Err(fields.invalid());
                }
                ("approved", fields.optional_text("reason_digest", blobs)?)
            } else {
                ("returned", Some(fields.text("reason_digest", blobs)?))
            };
            if terminal {
                return Err(fields.invalid());
            }
            let (reason, reason_digest) = reason.unzip();
            transaction.execute(
                "UPDATE artifacts SET state = ?2, reason = ?3, reason_digest = ?4,
                   decided_seq = ?5, decided_at = ?6 WHERE id = ?1",
                params![artifact, next, reason, reason_digest, seq, at],
            )?;
            Ok(())
        }
        _ => Err(StoreError::UnknownKind { seq: fields.seq }),
    }
}
