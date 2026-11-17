//! The read behind the flow measures (`docs/work-supervision/flow-v0.md`):
//! the timeline entries of every accepted mission, from the tables the
//! projection already keeps. Nothing is written and no table is added.

use rusqlite::params_from_iter;
use work_supervision_domain::flow::EVENT_KINDS;

use crate::records::TimelineRow;
use crate::{Store, StoreError};

/// An accepted mission and the entries of its timeline the measures read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedTimeline {
    /// Mission identifier.
    pub mission: String,
    /// Executor profile of the mission.
    pub executor: String,
    /// Its entries of the kinds the measures read, in journal order. Empty
    /// when the projection holds no timeline row for the mission.
    pub events: Vec<TimelineRow>,
}

impl Store {
    /// Every mission in state `accepted`, by identifier, each with its
    /// timeline entries of the kinds in [`EVENT_KINDS`], by sequence number.
    ///
    /// A mission without any such entry is still returned, with no event: an
    /// accepted mission the timeline does not cover is the caller's to report,
    /// not this read's to hide.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`].
    pub fn accepted_timelines(&self) -> Result<Vec<AcceptedTimeline>, StoreError> {
        // One query for every mission: the join is outer so that a mission
        // with no timeline row yields one row whose event columns are null.
        let mut statement = self.connection().prepare(
            "SELECT m.id, m.executor, e.seq, e.kind, e.at
             FROM missions m
             LEFT JOIN mission_events e
               ON e.mission_id = m.id AND e.kind IN (?1, ?2, ?3, ?4)
             WHERE m.state = 'accepted'
             ORDER BY m.id, e.seq",
        )?;
        let mut rows = statement.query(params_from_iter(EVENT_KINDS))?;
        let mut timelines: Vec<AcceptedTimeline> = Vec::new();
        while let Some(row) = rows.next()? {
            let mission: String = row.get(0)?;
            if timelines.last().is_none_or(|last| last.mission != mission) {
                timelines.push(AcceptedTimeline {
                    mission,
                    executor: row.get(1)?,
                    events: Vec::new(),
                });
            }
            let event: Option<(i64, String, String)> = match row.get::<_, Option<i64>>(2)? {
                Some(seq) => Some((seq, row.get(3)?, row.get(4)?)),
                None => None,
            };
            if let (Some((seq, kind, at)), Some(timeline)) = (event, timelines.last_mut()) {
                timeline.events.push(TimelineRow { seq, kind, at });
            }
        }
        Ok(timelines)
    }
}
