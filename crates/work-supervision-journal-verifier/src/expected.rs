//! Expected-state check: the journal compared with artefacts held outside it.
//!
//! [`crate::verify`] re-reads what the writer wrote: it proves the file is
//! internally consistent. This module takes, besides the journal, an
//! **expectation** (`docs/work-supervision/journal-v0.md`, « Expected state »):
//! for each mission, the commit that was submitted, the digest of the evidence
//! and the argument vector of every declared check, all supplied by the caller
//! from sources the journal's writer does not control. From them it computes
//! what the journal must contain and names every gap.
//!
//! It does not say who wrote the file: there is no signature here.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::{Head, Outcome, canonical, has_exactly, hex, is_digest, verify};

/// Schema identifier of an expectation document.
pub const EXPECTATION_SCHEMA: &str = "libre-ai.work-supervision.expected-state.v0";

const MAX_ARGUMENTS: usize = 64;
const RESULT_SUBMITTED: &str = "mission.result-submitted";
const ACCEPTED: &str = "mission.accepted";
const CHECK_DECLARED: &str = "check.declared";
const CHECK_STARTED: &str = "check.started";
const CHECK_FINISHED: &str = "check.finished";

/// Why an expectation document is refused. An expectation that cannot be read
/// is an error, never an expectation of nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectationError {
    /// Not a JSON object.
    Malformed,
    /// The schema is not [`EXPECTATION_SCHEMA`].
    SchemaUnknown,
    /// A key is missing, unknown, or holds a value outside its domain.
    FieldInvalid,
    /// No mission is listed: there would be nothing to compare.
    Empty,
    /// A mission is listed twice.
    MissionDuplicate,
    /// A criterion carries two checks in one mission.
    CriterionDuplicate,
}

impl ExpectationError {
    /// Stable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Malformed => "expectation-malformed",
            Self::SchemaUnknown => "expectation-schema-unknown",
            Self::FieldInvalid => "expectation-field-invalid",
            Self::Empty => "expectation-empty",
            Self::MissionDuplicate => "expectation-mission-duplicate",
            Self::CriterionDuplicate => "expectation-criterion-duplicate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedCheck {
    criterion: u64,
    argv_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedMission {
    mission: String,
    commit: String,
    evidence_digest: String,
    checks: Vec<ExpectedCheck>,
}

/// What the caller holds independently of the journal, per mission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectation {
    missions: Vec<ExpectedMission>,
}

impl Expectation {
    /// Reads an expectation document, strictly: an unknown key refuses it.
    ///
    /// # Errors
    ///
    /// An [`ExpectationError`] naming the first defect.
    pub fn parse(bytes: &[u8]) -> Result<Self, ExpectationError> {
        let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(bytes) else {
            return Err(ExpectationError::Malformed);
        };
        if !has_exactly(&fields, &["missions", "schema"]) {
            return Err(ExpectationError::FieldInvalid);
        }
        if fields.get("schema").and_then(Value::as_str) != Some(EXPECTATION_SCHEMA) {
            return Err(ExpectationError::SchemaUnknown);
        }
        let Some(Value::Array(items)) = fields.get("missions") else {
            return Err(ExpectationError::FieldInvalid);
        };
        if items.is_empty() {
            return Err(ExpectationError::Empty);
        }
        let mut missions: Vec<ExpectedMission> = Vec::with_capacity(items.len());
        for item in items {
            let mission = parse_mission(item)?;
            if missions
                .iter()
                .any(|known| known.mission == mission.mission)
            {
                return Err(ExpectationError::MissionDuplicate);
            }
            missions.push(mission);
        }
        Ok(Self { missions })
    }

    /// Number of missions the expectation lists.
    #[must_use]
    pub const fn missions(&self) -> usize {
        self.missions.len()
    }
}

fn parse_mission(item: &Value) -> Result<ExpectedMission, ExpectationError> {
    let Value::Object(fields) = item else {
        return Err(ExpectationError::FieldInvalid);
    };
    if !has_exactly(fields, &["checks", "commit", "evidence_digest", "mission"]) {
        return Err(ExpectationError::FieldInvalid);
    }
    let mission = text(fields, "mission").filter(|id| is_lower_hex(id, &[32]));
    let commit = text(fields, "commit").filter(|commit| is_lower_hex(commit, &[40, 64]));
    let evidence = text(fields, "evidence_digest").filter(|digest| is_digest(digest));
    let (Some(mission), Some(commit), Some(evidence), Some(Value::Array(items))) =
        (mission, commit, evidence, fields.get("checks"))
    else {
        return Err(ExpectationError::FieldInvalid);
    };
    let mut checks: Vec<ExpectedCheck> = Vec::with_capacity(items.len());
    for item in items {
        let check = parse_check(item)?;
        if checks
            .iter()
            .any(|known| known.criterion == check.criterion)
        {
            return Err(ExpectationError::CriterionDuplicate);
        }
        checks.push(check);
    }
    Ok(ExpectedMission {
        mission: mission.to_owned(),
        commit: commit.to_owned(),
        evidence_digest: evidence.to_owned(),
        checks,
    })
}

fn parse_check(item: &Value) -> Result<ExpectedCheck, ExpectationError> {
    let Value::Object(fields) = item else {
        return Err(ExpectationError::FieldInvalid);
    };
    if !has_exactly(fields, &["argv", "criterion"]) {
        return Err(ExpectationError::FieldInvalid);
    }
    let (Some(criterion), Some(argv @ Value::Array(arguments))) = (
        fields.get("criterion").and_then(Value::as_u64),
        fields.get("argv"),
    ) else {
        return Err(ExpectationError::FieldInvalid);
    };
    if arguments.is_empty()
        || arguments.len() > MAX_ARGUMENTS
        || !arguments.iter().all(Value::is_string)
    {
        return Err(ExpectationError::FieldInvalid);
    }
    // The journal names an argument vector by the SHA-256 of its compact JSON
    // array: the digest the journal must hold is computed here from the vector
    // the caller declares, never read back from the journal.
    let encoded = canonical::encode(argv).ok_or(ExpectationError::FieldInvalid)?;
    Ok(ExpectedCheck {
        criterion,
        argv_digest: hex(&Sha256::digest(&encoded)),
    })
}

fn text<'a>(fields: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    fields.get(key).and_then(Value::as_str)
}

fn is_lower_hex(text: &str, lengths: &[usize]) -> bool {
    lengths.contains(&text.len())
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A way the journal contradicts the expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// No `mission.result-submitted` entry for the mission.
    ResultMissing,
    /// The last submitted result names another commit.
    CommitMismatch,
    /// The last submitted result names another evidence digest.
    EvidenceMismatch,
    /// An expected check has no `check.declared` entry.
    CheckUndeclared,
    /// The check declared for the criterion has another argument vector.
    CheckArgvMismatch,
    /// The journal declares a check on a criterion the expectation does not list.
    CheckUnexpected,
    /// An expected check has no finished execution at the expected commit.
    CheckNotExecuted,
    /// The last finished execution of an expected check at the expected commit did not pass.
    CheckNotPassed,
    /// The mission is accepted while a check had not passed before the
    /// acceptance, or while no check is expected at all.
    AcceptedWithoutPassedCheck,
}

impl Gap {
    /// Stable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ResultMissing => "result-missing",
            Self::CommitMismatch => "commit-mismatch",
            Self::EvidenceMismatch => "evidence-mismatch",
            Self::CheckUndeclared => "check-undeclared",
            Self::CheckArgvMismatch => "check-argv-mismatch",
            Self::CheckUnexpected => "check-unexpected",
            Self::CheckNotExecuted => "check-not-executed",
            Self::CheckNotPassed => "check-not-passed",
            Self::AcceptedWithoutPassedCheck => "accepted-without-passed-check",
        }
    }
}

/// One gap, located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    /// What differs.
    pub gap: Gap,
    /// Mission identifier, as the expectation gives it.
    pub mission: String,
    /// Criterion index, for a gap about a check.
    pub criterion: Option<u64>,
    /// Journal line (1-based) holding the contradicting entry, when there is one.
    pub line: Option<u64>,
}

/// What was compared. Three sums hold, and the tests pin them:
/// `missions_expected = missions_found + missions_missing`,
/// `journal_submitted = missions_found + journal_outside`, and
/// `planned() = matched + diverged() + not_applicable`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Comparison {
    /// Journal entries verified by the chain check.
    pub entries: u64,
    /// Head of the verified journal.
    pub head: Option<Head>,
    /// Missions listed by the expectation.
    pub missions_expected: u64,
    /// Of which the journal holds a submitted result.
    pub missions_found: u64,
    /// Of which the journal holds none.
    pub missions_missing: u64,
    /// Missions with a submitted result in the journal.
    pub journal_submitted: u64,
    /// Of which the expectation does not list: not compared, counted.
    pub journal_outside: u64,
    /// Comparisons planned: a submitted result exists (one per mission).
    pub results: u64,
    /// Comparisons planned: submitted commit (one per mission).
    pub commits: u64,
    /// Comparisons planned: evidence digest (one per mission).
    pub evidence: u64,
    /// Comparisons planned: check declaration (one per expected check).
    pub declarations: u64,
    /// Comparisons planned: passed execution at the commit (one per expected check).
    pub executions: u64,
    /// Comparisons planned: no check declared outside the expectation (one per mission).
    pub sweeps: u64,
    /// Comparisons planned: acceptance rests on passed checks (one per mission).
    pub acceptances: u64,
    /// Comparisons that held.
    pub matched: u64,
    /// Comparisons that could not be made: commit and evidence without a
    /// submitted result, acceptance of a mission the journal does not accept.
    pub not_applicable: u64,
    /// Comparisons that failed, one divergence each.
    pub divergences: Vec<Divergence>,
}

impl Comparison {
    /// Every comparison planned from the expectation.
    #[must_use]
    pub const fn planned(&self) -> u64 {
        self.results
            + self.commits
            + self.evidence
            + self.declarations
            + self.executions
            + self.sweeps
            + self.acceptances
    }

    /// Comparisons that failed.
    #[must_use]
    pub fn diverged(&self) -> u64 {
        u64::try_from(self.divergences.len()).unwrap_or(u64::MAX)
    }

    fn settle(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Matched => self.matched += 1,
            Verdict::NotApplicable => self.not_applicable += 1,
            Verdict::Diverged(divergence) => self.divergences.push(divergence),
        }
    }
}

enum Verdict {
    Matched,
    NotApplicable,
    Diverged(Divergence),
}

/// Result of an expected-state check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedOutcome {
    /// The chain is valid and every comparison that applies held.
    Consistent(Comparison),
    /// The chain is valid and the journal contradicts the expectation.
    Diverged(Comparison),
    /// The chain itself is refused (or torn): nothing was compared.
    ChainRefused(Outcome),
}

/// Verifies the chain of `journal`, then compares its content with `expectation`.
///
/// # Errors
///
/// Any read error of the chain verification.
pub fn verify_expected(journal: &[u8], expectation: &Expectation) -> io::Result<ExpectedOutcome> {
    let (entries, head) = match verify(journal)? {
        Outcome::Valid { entries, head } => (entries, head),
        refused => return Ok(ExpectedOutcome::ChainRefused(refused)),
    };
    let wanted: BTreeSet<&str> = expectation
        .missions
        .iter()
        .map(|mission| mission.mission.as_str())
        .collect();
    let seen = read(journal, &wanted)?;
    let mut comparison = Comparison {
        entries,
        head,
        ..Comparison::default()
    };
    for expected in &expectation.missions {
        compare_mission(
            expected,
            seen.missions.get(&expected.mission),
            &mut comparison,
        );
    }
    comparison.missions_expected = comparison.results;
    comparison.missions_missing = comparison.missions_expected - comparison.missions_found;
    comparison.journal_submitted = seen.submitted;
    comparison.journal_outside = seen.submitted - comparison.missions_found;
    if comparison.divergences.is_empty() {
        Ok(ExpectedOutcome::Consistent(comparison))
    } else {
        Ok(ExpectedOutcome::Diverged(comparison))
    }
}

struct Submitted {
    line: u64,
    commit: Option<String>,
    evidence: Option<String>,
}

struct Declared {
    line: u64,
    criterion: Option<u64>,
    argv_digest: Option<String>,
}

struct Started {
    criterion: Option<u64>,
    commit: Option<String>,
    argv_digest: Option<String>,
}

struct Finished {
    line: u64,
    start: Started,
    passed: bool,
}

#[derive(Default)]
struct Seen {
    submitted: Option<Submitted>,
    accepted: Option<u64>,
    declared: Vec<Declared>,
    finished: Vec<Finished>,
}

struct Journal {
    missions: BTreeMap<String, Seen>,
    submitted: u64,
}

/// One pass over a chain-verified journal: what it says about the wanted
/// missions, and how many missions it holds a submitted result for.
fn read(journal: &[u8], wanted: &BTreeSet<&str>) -> io::Result<Journal> {
    let mut missions: BTreeMap<String, Seen> = BTreeMap::new();
    let mut submitted: BTreeSet<String> = BTreeSet::new();
    let mut started: BTreeMap<(String, String), Started> = BTreeMap::new();
    let mut line: u64 = 0;
    for bytes in journal.split(|byte| *byte == b'\n') {
        if bytes.is_empty() {
            continue;
        }
        line += 1;
        let entry: Value = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        let event = entry.get("event");
        let kind = event
            .and_then(|event| event.get("kind"))
            .and_then(Value::as_str);
        let data = event
            .and_then(|event| event.get("data"))
            .and_then(Value::as_object);
        let (Some(kind), Some(data)) = (kind, data) else {
            continue;
        };
        let Some(mission) = text(data, "mission") else {
            continue;
        };
        if kind == RESULT_SUBMITTED {
            submitted.insert(mission.to_owned());
        }
        if !wanted.contains(mission) {
            continue;
        }
        let owned = |key: &str| text(data, key).map(str::to_owned);
        let seen = missions.entry(mission.to_owned()).or_default();
        match kind {
            RESULT_SUBMITTED => {
                seen.submitted = Some(Submitted {
                    line,
                    commit: owned("commit"),
                    evidence: owned("evidence_digest"),
                });
            }
            ACCEPTED => seen.accepted = Some(line),
            CHECK_DECLARED => seen.declared.push(Declared {
                line,
                criterion: data.get("criterion").and_then(Value::as_u64),
                argv_digest: owned("argv_digest"),
            }),
            CHECK_STARTED => {
                if let Some(check) = owned("check") {
                    started.insert(
                        (mission.to_owned(), check),
                        Started {
                            criterion: data.get("criterion").and_then(Value::as_u64),
                            commit: owned("commit"),
                            argv_digest: owned("argv_digest"),
                        },
                    );
                }
            }
            CHECK_FINISHED => {
                let start =
                    owned("check").and_then(|check| started.remove(&(mission.to_owned(), check)));
                if let Some(start) = start {
                    seen.finished.push(Finished {
                        line,
                        start,
                        passed: passed(data),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(Journal {
        missions,
        submitted: u64::try_from(submitted.len()).map_err(io::Error::other)?,
    })
}

/// Exit 0, no signal, no overrun budget (`coordination-v0.md`, « Checks »).
fn passed(data: &Map<String, Value>) -> bool {
    data.get("exit_code").and_then(Value::as_i64) == Some(0)
        && data.get("signal") == Some(&Value::Null)
        && data.get("budget") == Some(&Value::Null)
}

/// The last finished execution of `check` at the expected commit, among the
/// entries before line `before` when given.
fn last_finished<'a>(
    seen: &'a Seen,
    expected: &ExpectedMission,
    check: &ExpectedCheck,
    before: Option<u64>,
) -> Option<&'a Finished> {
    seen.finished.iter().rev().find(|finished| {
        finished.start.criterion == Some(check.criterion)
            && finished.start.commit.as_deref() == Some(expected.commit.as_str())
            && finished.start.argv_digest.as_deref() == Some(check.argv_digest.as_str())
            && before.is_none_or(|limit| finished.line < limit)
    })
}

fn compare_mission(expected: &ExpectedMission, seen: Option<&Seen>, comparison: &mut Comparison) {
    let empty = Seen::default();
    let seen = seen.unwrap_or(&empty);
    let gap = |gap: Gap, criterion: Option<u64>, line: Option<u64>| {
        Verdict::Diverged(Divergence {
            gap,
            mission: expected.mission.clone(),
            criterion,
            line,
        })
    };
    let equal = |found: Option<&String>, wanted: &str, line: u64, kind: Gap| {
        if found.map(String::as_str) == Some(wanted) {
            Verdict::Matched
        } else {
            gap(kind, None, Some(line))
        }
    };

    comparison.results += 1;
    comparison.commits += 1;
    comparison.evidence += 1;
    match &seen.submitted {
        Some(submitted) => {
            comparison.missions_found += 1;
            comparison.settle(Verdict::Matched);
            comparison.settle(equal(
                submitted.commit.as_ref(),
                &expected.commit,
                submitted.line,
                Gap::CommitMismatch,
            ));
            comparison.settle(equal(
                submitted.evidence.as_ref(),
                &expected.evidence_digest,
                submitted.line,
                Gap::EvidenceMismatch,
            ));
        }
        None => {
            comparison.settle(gap(Gap::ResultMissing, None, None));
            comparison.settle(Verdict::NotApplicable);
            comparison.settle(Verdict::NotApplicable);
        }
    }

    for check in &expected.checks {
        comparison.declarations += 1;
        let declared = seen
            .declared
            .iter()
            .rev()
            .find(|declared| declared.criterion == Some(check.criterion));
        comparison.settle(match declared {
            None => gap(Gap::CheckUndeclared, Some(check.criterion), None),
            Some(declared) if declared.argv_digest.as_deref() != Some(&check.argv_digest) => gap(
                Gap::CheckArgvMismatch,
                Some(check.criterion),
                Some(declared.line),
            ),
            Some(_) => Verdict::Matched,
        });
        comparison.executions += 1;
        comparison.settle(match last_finished(seen, expected, check, None) {
            None => gap(Gap::CheckNotExecuted, Some(check.criterion), None),
            Some(finished) if !finished.passed => gap(
                Gap::CheckNotPassed,
                Some(check.criterion),
                Some(finished.line),
            ),
            Some(_) => Verdict::Matched,
        });
    }

    comparison.sweeps += 1;
    let unexpected = seen.declared.iter().find(|declared| {
        !expected
            .checks
            .iter()
            .any(|check| declared.criterion == Some(check.criterion))
    });
    comparison.settle(match unexpected {
        Some(declared) => gap(
            Gap::CheckUnexpected,
            declared.criterion,
            Some(declared.line),
        ),
        None => Verdict::Matched,
    });

    comparison.acceptances += 1;
    comparison.settle(match seen.accepted {
        None => Verdict::NotApplicable,
        Some(line) => {
            let rests_on_checks = !expected.checks.is_empty()
                && expected.checks.iter().all(|check| {
                    last_finished(seen, expected, check, Some(line))
                        .is_some_and(|finished| finished.passed)
                });
            if rests_on_checks {
                Verdict::Matched
            } else {
                gap(Gap::AcceptedWithoutPassedCheck, None, Some(line))
            }
        }
    });
}
