//! From an approved outline to missions linked by their dependencies
//! (`docs/work-supervision/coordination-v0.md`, « Outline split »).
//!
//! The `outline` phase (`phases-v0.md`) cuts the work into vertical slices.
//! Once the owner approved it, the owner gives a plan — one entry per slice,
//! with the slices it waits for — and the factory creates one draft mission
//! per slice and declares their dependencies. Pure rules only: the caller
//! parses the plan, reads the projection and records the events.

use std::collections::{BTreeMap, BTreeSet};

use crate::coordination::{Actor, CoordinationRefusal};
use crate::phases::{ArtifactState, Phase};
use crate::{MAX_LINE_TEXT_CHARS, MissionId};

/// Most slices of one plan.
pub const MAX_SLICES: usize = 32;
/// Longest slice key, in bytes.
pub const MAX_KEY_BYTES: usize = 32;

/// What the plan rules need of one slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    /// Key naming the slice inside the plan.
    pub key: String,
    /// What can be demonstrated once the slice is done.
    pub demonstrates: String,
    /// Keys of the slices it waits for.
    pub after: Vec<String>,
}

/// State of a split of one outline artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitState {
    /// The intent is journalled; the missions are being created.
    Splitting,
    /// Every mission and dependency exists.
    Split,
    /// Recovery could not finish the split.
    Aborted,
}

impl SplitState {
    /// Name used in rows and displays.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Splitting => "splitting",
            Self::Split => "split",
            Self::Aborted => "aborted",
        }
    }

    /// The state named `text`.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`split`).
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        match text {
            "splitting" => Ok(Self::Splitting),
            "split" => Ok(Self::Split),
            "aborted" => Ok(Self::Aborted),
            _ => Err(CoordinationRefusal::FieldInvalid { field: "split" }),
        }
    }
}

/// What the split rules need of the artifact named by the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineView {
    /// Mission the artifact belongs to.
    pub mission: MissionId,
    /// Its phase.
    pub phase: Phase,
    /// Its state.
    pub state: ArtifactState,
    /// State of an earlier split of this artifact, if any.
    pub split: Option<SplitState>,
}

fn key_valid(key: &str) -> bool {
    let bytes = key.as_bytes();
    (1..=MAX_KEY_BYTES).contains(&bytes.len())
        && bytes
            .first()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Checks that `mission` may be split along its outline artifact.
///
/// # Errors
///
/// [`CoordinationRefusal::OwnerOnly`], [`CoordinationRefusal::ArtifactNotFound`],
/// [`CoordinationRefusal::OutlineUnapproved`] (another mission's artifact, another
/// phase, not approved) and [`CoordinationRefusal::OutlineAlreadySplit`] (a split
/// under way or done; an aborted one may be retried).
pub fn check_outline(
    mission: &MissionId,
    artifact: Option<&OutlineView>,
    actor: &Actor,
) -> Result<(), CoordinationRefusal> {
    if *actor != Actor::Owner {
        return Err(CoordinationRefusal::OwnerOnly);
    }
    let artifact = artifact.ok_or(CoordinationRefusal::ArtifactNotFound)?;
    if artifact.mission != *mission
        || artifact.phase != Phase::Outline
        || artifact.state != ArtifactState::Approved
    {
        return Err(CoordinationRefusal::OutlineUnapproved);
    }
    if matches!(
        artifact.split,
        Some(SplitState::Splitting | SplitState::Split)
    ) {
        return Err(CoordinationRefusal::OutlineAlreadySplit);
    }
    Ok(())
}

/// Validates a plan and returns its dependency edges as `(slice, waits for)`
/// index pairs, in plan order.
///
/// # Errors
///
/// [`CoordinationRefusal::FieldInvalid`] (`plan`: 0 or more than
/// [`MAX_SLICES`] slices; `demonstrates`: blank or longer than a criterion),
/// [`CoordinationRefusal::OutlineKeyInvalid`],
/// [`CoordinationRefusal::OutlineKeyDuplicate`],
/// [`CoordinationRefusal::OutlineDependencyUnknown`],
/// [`CoordinationRefusal::DependencyDuplicate`] (a slice waiting for itself, or
/// twice for the same one) and [`CoordinationRefusal::DependencyCycle`].
pub fn check_plan(slices: &[Slice]) -> Result<Vec<(usize, usize)>, CoordinationRefusal> {
    if slices.is_empty() || slices.len() > MAX_SLICES {
        return Err(CoordinationRefusal::FieldInvalid { field: "plan" });
    }
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    for (position, slice) in slices.iter().enumerate() {
        if !key_valid(&slice.key) {
            return Err(CoordinationRefusal::OutlineKeyInvalid);
        }
        if index.insert(slice.key.as_str(), position).is_some() {
            return Err(CoordinationRefusal::OutlineKeyDuplicate);
        }
        let demonstrates = slice.demonstrates.trim();
        if demonstrates.is_empty() || slice.demonstrates.chars().count() > MAX_LINE_TEXT_CHARS {
            return Err(CoordinationRefusal::FieldInvalid {
                field: "demonstrates",
            });
        }
    }
    let mut edges = Vec::new();
    for (position, slice) in slices.iter().enumerate() {
        let mut seen = BTreeSet::new();
        for key in &slice.after {
            let on = *index
                .get(key.as_str())
                .ok_or(CoordinationRefusal::OutlineDependencyUnknown)?;
            if on == position || !seen.insert(on) {
                return Err(CoordinationRefusal::DependencyDuplicate);
            }
            edges.push((position, on));
        }
    }
    if has_cycle(slices.len(), &edges) {
        return Err(CoordinationRefusal::DependencyCycle);
    }
    Ok(edges)
}

/// Kahn's algorithm: a cycle leaves nodes whose in-degree never reaches 0.
fn has_cycle(nodes: usize, edges: &[(usize, usize)]) -> bool {
    let mut waiting = vec![0_usize; nodes];
    let mut unblocks: Vec<Vec<usize>> = vec![Vec::new(); nodes];
    for (from, on) in edges {
        if let Some(count) = waiting.get_mut(*from) {
            *count += 1;
        }
        if let Some(list) = unblocks.get_mut(*on) {
            list.push(*from);
        }
    }
    let mut ready: Vec<usize> = (0..nodes)
        .filter(|node| waiting.get(*node) == Some(&0))
        .collect();
    let mut done = 0;
    while let Some(node) = ready.pop() {
        done += 1;
        for next in unblocks.get(node).into_iter().flatten() {
            if let Some(count) = waiting.get_mut(*next) {
                *count -= 1;
                if *count == 0 {
                    ready.push(*next);
                }
            }
        }
    }
    done != nodes
}
