//! Plan parsing, write path, projection and reads of an outline split
//! (`docs/work-supervision/coordination-v0.md`, « Outline split »).
//!
//! A split is three entries at most: `outline.split.intent` fixes the plan
//! (a blob) and the identifiers of the missions to create, the missions and
//! their dependencies are recorded with their own events, and
//! `outline.split.confirmed` closes it — or `outline.split.aborted` when
//! recovery cannot finish it.

use rusqlite::{OptionalExtension as _, Transaction, params};
use serde_json::{Map, Value};
use work_supervision_domain::coordination::{Actor, CoordinationRefusal};
use work_supervision_domain::outline::{
    MAX_SLICES, OutlineView, Slice, SplitState, check_outline, check_plan,
};
use work_supervision_domain::phases::{ArtifactState, Phase};
use work_supervision_domain::{ArtifactId, MissionId};
use work_supervision_journal::{Event, Timestamp};

use crate::projection::{Fields, is_lower_hex};
use crate::{BlobStore, Store, StoreError, Supervisor, SupervisorError};

/// Keys a planned mission may carry; any other key refuses the plan.
const MISSION_KEYS: [&str; 8] = [
    "key",
    "title",
    "brief",
    "demonstrates",
    "criteria",
    "after",
    "max_duration_seconds",
    "max_output_bytes",
];

/// One mission of a plan, as the owner wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMission {
    /// Key naming the slice inside the plan.
    pub key: String,
    /// Title of the mission.
    pub title: String,
    /// Brief of the mission.
    pub brief: String,
    /// What can be demonstrated once it is done: the mission's first criterion.
    pub demonstrates: String,
    /// Further criteria.
    pub criteria: Vec<String>,
    /// Keys of the slices it waits for.
    pub after: Vec<String>,
    /// Duration budget, when given.
    pub max_duration_seconds: Option<u64>,
    /// Output budget, when given.
    pub max_output_bytes: Option<u64>,
}

impl PlannedMission {
    /// The mission's criteria: the demonstration first, then the others.
    #[must_use]
    pub fn all_criteria(&self) -> Vec<String> {
        std::iter::once(self.demonstrates.clone())
            .chain(self.criteria.iter().cloned())
            .collect()
    }
}

/// A validated plan and its dependency edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Missions, in plan order.
    pub missions: Vec<PlannedMission>,
    /// `(mission, waits for)` index pairs, in plan order.
    pub edges: Vec<(usize, usize)>,
}

fn invalid_plan() -> CoordinationRefusal {
    CoordinationRefusal::FieldInvalid { field: "plan" }
}

fn text_of(object: &Map<String, Value>, key: &str) -> Result<String, CoordinationRefusal> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(invalid_plan)
}

fn texts_of(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, CoordinationRefusal> {
    match object.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid_plan),
        Some(_) => Err(invalid_plan()),
    }
}

fn budget_of(object: &Map<String, Value>, key: &str) -> Result<Option<u64>, CoordinationRefusal> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(invalid_plan),
    }
}

/// Parses and validates a plan:
/// `{"missions": [{"key", "title", "brief", "demonstrates", "criteria"?,
/// "after"?, "max_duration_seconds"?, "max_output_bytes"?}, …]}`.
///
/// # Errors
///
/// [`CoordinationRefusal::FieldInvalid`] (`plan`) for a text that is not this
/// shape (unknown keys included), then the refusals of
/// [`work_supervision_domain::outline::check_plan`].
pub fn parse_plan(text: &str) -> Result<Plan, CoordinationRefusal> {
    let value: Value = serde_json::from_str(text).map_err(|_| invalid_plan())?;
    let object = value.as_object().ok_or_else(invalid_plan)?;
    if object.len() != 1 {
        return Err(invalid_plan());
    }
    let items = object
        .get("missions")
        .and_then(Value::as_array)
        .ok_or_else(invalid_plan)?;
    if items.len() > MAX_SLICES {
        return Err(invalid_plan());
    }
    let missions = items
        .iter()
        .map(|item| {
            let object = item.as_object().ok_or_else(invalid_plan)?;
            if object
                .keys()
                .any(|key| !MISSION_KEYS.contains(&key.as_str()))
            {
                return Err(invalid_plan());
            }
            Ok(PlannedMission {
                key: text_of(object, "key")?,
                title: text_of(object, "title")?,
                brief: text_of(object, "brief")?,
                demonstrates: text_of(object, "demonstrates")?,
                criteria: texts_of(object, "criteria")?,
                after: texts_of(object, "after")?,
                max_duration_seconds: budget_of(object, "max_duration_seconds")?,
                max_output_bytes: budget_of(object, "max_output_bytes")?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let slices: Vec<Slice> = missions
        .iter()
        .map(|mission| Slice {
            key: mission.key.clone(),
            demonstrates: mission.demonstrates.clone(),
            after: mission.after.clone(),
        })
        .collect();
    let edges = check_plan(&slices)?;
    Ok(Plan { missions, edges })
}

/// A projected outline split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineSplitRow {
    /// The outline artifact.
    pub artifact: String,
    /// The mission whose outline it is.
    pub mission: String,
    /// `splitting`, `split` or `aborted`.
    pub state: String,
    /// The plan, as given.
    pub plan: String,
    /// The missions to create, in plan order.
    pub missions: Vec<String>,
    /// Instant of the intent.
    pub intent_at: String,
    /// Instant of the confirmation or the abort.
    pub decided_at: Option<String>,
}

fn event(kind: &str, fields: Vec<(&str, Value)>) -> Result<Event, SupervisorError> {
    let data: Map<String, Value> = fields
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    Ok(Event::new(kind, data)?)
}

fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

impl Supervisor {
    /// Records the intent to split the approved outline `artifact` of
    /// `mission` along `plan` into the missions `missions` (owner).
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Coordination`] (the refusals of
    /// [`work_supervision_domain::outline::check_outline`] and of [`parse_plan`],
    /// and `plan` when `missions` does not match the plan or names an existing
    /// mission); store and journal failures.
    pub fn outline_split_intent(
        &mut self,
        mission: &MissionId,
        artifact: &ArtifactId,
        plan: &str,
        missions: &[MissionId],
        actor: &Actor,
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        let view = self.store().outline_view(artifact)?;
        check_outline(mission, view.as_ref(), actor)?;
        let parsed = parse_plan(plan)?;
        if parsed.missions.len() != missions.len() {
            return Err(invalid_plan().into());
        }
        for id in missions {
            if self.store().mission(id)?.is_some() {
                return Err(invalid_plan().into());
            }
        }
        let digest = self.blobs().put_text(plan)?.to_hex();
        self.append(
            at,
            event(
                "outline.split.intent",
                vec![
                    ("mission", text(mission.as_str())),
                    ("artifact", text(artifact.as_str())),
                    ("plan_digest", Value::String(digest)),
                    (
                        "missions",
                        Value::Array(missions.iter().map(|id| text(id.as_str())).collect()),
                    ),
                ],
            )?,
        )?;
        Ok(())
    }

    /// Confirms (`true`) or aborts (`false`) the split of `artifact` under way.
    ///
    /// # Errors
    ///
    /// [`SupervisorError::Coordination`] (`outline.already_split` when no split
    /// of `artifact` is under way); store and journal failures.
    pub fn outline_split_close(
        &mut self,
        artifact: &ArtifactId,
        confirmed: bool,
        at: Timestamp,
    ) -> Result<(), SupervisorError> {
        let row = self
            .store()
            .outline_split(artifact.as_str())?
            .filter(|row| row.state == SplitState::Splitting.as_str())
            .ok_or(CoordinationRefusal::OutlineAlreadySplit)?;
        let kind = if confirmed {
            "outline.split.confirmed"
        } else {
            "outline.split.aborted"
        };
        self.append(
            at,
            event(
                kind,
                vec![
                    ("mission", text(&row.mission)),
                    ("artifact", text(artifact.as_str())),
                ],
            )?,
        )?;
        Ok(())
    }
}

/// The column a split lookup filters on. Each variant maps to a complete
/// literal query, so no SQL text is ever assembled at run time.
#[derive(Debug, Clone, Copy)]
enum SplitFilter {
    Artifact,
    Mission,
    State,
}

impl SplitFilter {
    const fn sql(self) -> &'static str {
        match self {
            Self::Artifact => {
                "SELECT artifact_id, mission_id, state, plan, intent_at, decided_at \
                 FROM outline_splits WHERE artifact_id = ?1 ORDER BY intent_seq"
            }
            Self::Mission => {
                "SELECT artifact_id, mission_id, state, plan, intent_at, decided_at \
                 FROM outline_splits WHERE mission_id = ?1 ORDER BY intent_seq"
            }
            Self::State => {
                "SELECT artifact_id, mission_id, state, plan, intent_at, decided_at \
                 FROM outline_splits WHERE state = ?1 ORDER BY intent_seq"
            }
        }
    }
}

impl Store {
    /// What the split rules need of `artifact`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn outline_view(&self, artifact: &ArtifactId) -> Result<Option<OutlineView>, StoreError> {
        let Some(row) = self.artifact(artifact.as_str())? else {
            return Ok(None);
        };
        let split = self
            .outline_split(artifact.as_str())?
            .map(|split| SplitState::parse(&split.state).map_err(|_| StoreError::Sqlite))
            .transpose()?;
        Ok(Some(OutlineView {
            mission: MissionId::parse(&row.mission).map_err(|_| StoreError::Sqlite)?,
            phase: Phase::parse(&row.phase).map_err(|_| StoreError::Sqlite)?,
            state: ArtifactState::parse(&row.state).map_err(|_| StoreError::Sqlite)?,
            split,
        }))
    }

    fn split_missions(&self, artifact: &str) -> Result<Vec<String>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT mission_id FROM outline_missions WHERE artifact_id = ?1 ORDER BY position",
        )?;
        let rows = statement
            .query_map([artifact], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn splits_where(
        &self,
        filter: SplitFilter,
        value: &str,
    ) -> Result<Vec<OutlineSplitRow>, StoreError> {
        let mut statement = self.connection().prepare(filter.sql())?;
        let rows = statement
            .query_map([value], |row| {
                Ok(OutlineSplitRow {
                    artifact: row.get(0)?,
                    mission: row.get(1)?,
                    state: row.get(2)?,
                    plan: row.get(3)?,
                    missions: Vec::new(),
                    intent_at: row.get(4)?,
                    decided_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|mut row| {
                row.missions = self.split_missions(&row.artifact)?;
                Ok(row)
            })
            .collect()
    }

    /// The split of `artifact`, if one was intended.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn outline_split(&self, artifact: &str) -> Result<Option<OutlineSplitRow>, StoreError> {
        Ok(self.splits_where(SplitFilter::Artifact, artifact)?.pop())
    }

    /// The splits of the outlines of `mission`, oldest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn outline_splits_of(
        &self,
        mission: &MissionId,
    ) -> Result<Vec<OutlineSplitRow>, StoreError> {
        self.splits_where(SplitFilter::Mission, mission.as_str())
    }

    /// Splits still under way (recovery).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn pending_outline_splits(&self) -> Result<Vec<OutlineSplitRow>, StoreError> {
        self.splits_where(SplitFilter::State, SplitState::Splitting.as_str())
    }

    /// The split that planned `mission`: outline artifact, its mission and
    /// the position of `mission` in the plan.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn outline_origin(
        &self,
        mission: &MissionId,
    ) -> Result<Option<(String, String, i64)>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT planned.artifact_id, split.mission_id, planned.position
                 FROM outline_missions AS planned
                 JOIN outline_splits AS split ON split.artifact_id = planned.artifact_id
                 WHERE planned.mission_id = ?1 AND split.state != 'aborted'",
                [mission.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?)
    }
}

/// `outline.split.*`.
pub(crate) fn project(
    transaction: &Transaction<'_>,
    kind: &str,
    fields: &Fields<'_>,
    blobs: &BlobStore,
    seq: i64,
    at: &str,
) -> Result<(), StoreError> {
    let mission = fields.mission()?;
    let artifact = fields.identifier("artifact")?;
    let current: Option<(String, String)> = transaction
        .query_row(
            "SELECT state, mission_id FROM outline_splits WHERE artifact_id = ?1",
            [artifact],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match kind {
        "outline.split.intent" => {
            let approved: i64 = transaction.query_row(
                "SELECT count(*) FROM artifacts WHERE id = ?1 AND mission_id = ?2
                   AND phase = 'outline' AND state = 'approved'",
                [artifact, mission],
                |row| row.get(0),
            )?;
            let retry = matches!(&current, Some((state, _)) if state == "aborted");
            if approved != 1 || (current.is_some() && !retry) {
                return Err(fields.invalid());
            }
            let (plan, digest) = fields.text("plan_digest", blobs)?;
            let parsed = parse_plan(&plan).map_err(|_| fields.invalid())?;
            let missions: Vec<&str> = fields
                .data
                .get("missions")
                .and_then(Value::as_array)
                .ok_or_else(|| fields.invalid())?
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| fields.invalid())?;
            let distinct: std::collections::BTreeSet<&str> = missions.iter().copied().collect();
            if missions.len() != parsed.missions.len()
                || distinct.len() != missions.len()
                || missions.iter().any(|id| !is_lower_hex(id, 32))
            {
                return Err(fields.invalid());
            }
            for id in &missions {
                let exists: i64 = transaction.query_row(
                    "SELECT count(*) FROM missions WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )?;
                if exists != 0 {
                    return Err(fields.invalid());
                }
            }
            if retry {
                transaction.execute(
                    "DELETE FROM outline_missions WHERE artifact_id = ?1",
                    [artifact],
                )?;
                transaction.execute(
                    "DELETE FROM outline_splits WHERE artifact_id = ?1",
                    [artifact],
                )?;
            }
            transaction.execute(
                "INSERT INTO outline_splits (artifact_id, mission_id, state, plan, plan_digest,
                   intent_seq, intent_at) VALUES (?1, ?2, 'splitting', ?3, ?4, ?5, ?6)",
                params![artifact, mission, plan, digest, seq, at],
            )?;
            for (position, id) in missions.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO outline_missions (artifact_id, position, mission_id)
                     VALUES (?1, ?2, ?3)",
                    params![
                        artifact,
                        i64::try_from(position).map_err(|_| fields.invalid())?,
                        id
                    ],
                )?;
            }
            Ok(())
        }
        "outline.split.confirmed" | "outline.split.aborted" => {
            if current.as_ref()
                != Some(&(
                    SplitState::Splitting.as_str().to_owned(),
                    mission.to_owned(),
                ))
            {
                return Err(fields.invalid());
            }
            let next = if kind == "outline.split.confirmed" {
                // Confirmed only once every planned mission exists.
                let missing: i64 = transaction.query_row(
                    "SELECT count(*) FROM outline_missions AS planned
                     WHERE planned.artifact_id = ?1
                       AND NOT EXISTS (SELECT 1 FROM missions WHERE id = planned.mission_id)",
                    [artifact],
                    |row| row.get(0),
                )?;
                if missing != 0 {
                    return Err(fields.invalid());
                }
                SplitState::Split
            } else {
                SplitState::Aborted
            };
            transaction.execute(
                "UPDATE outline_splits SET state = ?2, decided_seq = ?3, decided_at = ?4
                 WHERE artifact_id = ?1",
                params![artifact, next.as_str(), seq, at],
            )?;
            Ok(())
        }
        _ => Err(StoreError::UnknownKind { seq: fields.seq }),
    }
}
