//! Mission worktrees of Work Supervision v0.
//!
//! Owner decision Y29 (D-1, 2026-10-09): worktrees are managed by the **system
//! `git` in a separate process** — the executable already installed on the
//! machine, neither linked nor redistributed. Every call has a fixed argv
//! (no shell), an environment cleared to [`Git::environment`], and hooks
//! disabled (`core.hooksPath=/dev/null`).
//!
//! Each step is written in three parts — intent in the journal, effect in git,
//! confirmation in the journal:
//!
//! | Step | Intent | Effect | Confirmation |
//! | --- | --- | --- | --- |
//! | provision | `worktree.create.intent` | `git worktree add -b ws/<mission> worktrees/<mission> <base>` | `worktree.created` (HEAD observed) |
//! | release | `worktree.remove.intent` | archive (abandon) then `git worktree remove`, branch deleted on abandon | `worktree.archived`, `worktree.removed` |
//!
//! [`Worktrees::reconcile`] resolves every intent left without confirmation
//! by a crash: an intended worktree that exists at its base is confirmed,
//! anything else is removed and the intent aborted (`worktree.create.aborted`);
//! an intended removal is finished, archiving first when it was an abandon.
//! [`Worktrees::gc`] removes worktrees, directories and `ws/*` branches that
//! no live record explains.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};

use serde_json::{Map, Value};
use work_supervision_domain::{CommitId, MissionId, branch_of, worktree_of};
use work_supervision_journal::{Event, Timestamp};
use work_supervision_store::{BlobStore, Layout, Supervisor, SupervisorError, WorktreeRow};

/// `PATH` given to git: system locations only.
const GIT_PATH: &str = "/usr/bin:/bin:/usr/local/bin";

/// Where a crash can be injected, for the reconciliation tests and `ws doctor` drills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint {
    /// After `worktree.create.intent` is durable, before git runs.
    AfterCreateIntent,
    /// While git creates the worktree: a directory is left, git has not registered it.
    DuringCreate,
    /// After git created the worktree, before `worktree.created`.
    AfterCreateEffect,
    /// After `worktree.remove.intent` is durable, before anything is archived or removed.
    AfterRemoveIntent,
    /// After `worktree.archived`, before git removes the worktree.
    AfterArchive,
    /// After git removed the worktree, before `worktree.removed`.
    AfterRemoveEffect,
}

impl FaultPoint {
    /// Every injection point, in lifecycle order.
    pub const ALL: [Self; 6] = [
        Self::AfterCreateIntent,
        Self::DuringCreate,
        Self::AfterCreateEffect,
        Self::AfterRemoveIntent,
        Self::AfterArchive,
        Self::AfterRemoveEffect,
    ];
}

/// The crash plan of one operation: none, or one point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Faults(Option<FaultPoint>);

impl Faults {
    /// No crash.
    #[must_use]
    pub const fn none() -> Self {
        Self(None)
    }

    /// Crash at `point`.
    #[must_use]
    pub const fn at(point: FaultPoint) -> Self {
        Self(Some(point))
    }

    const fn check(&self, point: FaultPoint) -> Result<(), WorktreeError> {
        match self.0 {
            Some(planned) if planned as u8 == point as u8 => Err(WorktreeError::Fault(point)),
            _ => Ok(()),
        }
    }
}

/// Why a worktree operation was refused or failed. `Display` carries no path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeError {
    /// The base commit does not exist in the repository.
    BaseUnknown,
    /// The worktree has changes; only an abandon (which archives them) may remove it.
    Dirty,
    /// The mission has no worktree in the state the operation needs.
    NotProvisioned,
    /// The repository name of a record is not in the configuration.
    RepositoryUnknown,
    /// A crash was injected at this point.
    Fault(FaultPoint),
    /// git failed or answered something unexpected.
    Git,
    /// A file operation failed.
    Io,
    /// The journal, the projection or the blob store failed.
    Supervisor(SupervisorError),
}

impl WorktreeError {
    /// Stable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BaseUnknown => "worktree.base_unknown",
            Self::Dirty => "worktree.dirty",
            Self::NotProvisioned => "worktree.not_provisioned",
            Self::RepositoryUnknown => "worktree.repository_unknown",
            Self::Fault(_) => "worktree.fault_injected",
            Self::Git => "worktree.git",
            Self::Io => "worktree.io",
            Self::Supervisor(error) => error.code(),
        }
    }
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for WorktreeError {}

impl From<SupervisorError> for WorktreeError {
    fn from(error: SupervisorError) -> Self {
        Self::Supervisor(error)
    }
}

impl From<work_supervision_store::StoreError> for WorktreeError {
    fn from(error: work_supervision_store::StoreError) -> Self {
        Self::Supervisor(SupervisorError::Store(error))
    }
}

/// Projected state of a mission worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeState {
    /// Intent recorded, not confirmed.
    Creating,
    /// Exists at its base on `ws/<mission>`.
    Created,
    /// The creation was rolled back by reconciliation.
    Aborted,
    /// Removal intended, not confirmed.
    Releasing,
    /// Removed.
    Removed,
}

impl WorktreeState {
    fn parse(text: &str) -> Result<Self, WorktreeError> {
        Ok(match text {
            "creating" => Self::Creating,
            "created" => Self::Created,
            "aborted" => Self::Aborted,
            "releasing" => Self::Releasing,
            "removed" => Self::Removed,
            _ => return Err(WorktreeError::Io),
        })
    }
}

/// How a worktree is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Clean worktree only; the branch `ws/<mission>` is kept (accepted result).
    Keep,
    /// Archive the full diff from the base (commits and uncommitted changes)
    /// as evidence, remove the worktree and delete the branch (abandon, cancel).
    Abandon,
}

/// The system git, run in a separate process with a fixed environment.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
}

impl Git {
    /// The `git` found in the system locations `/usr/bin`, `/bin` and `/usr/local/bin`.
    #[must_use]
    pub fn system() -> Self {
        Self {
            program: PathBuf::from("git"),
        }
    }

    /// Every variable git receives; nothing else of the host is passed.
    #[must_use]
    pub fn environment(&self) -> Vec<(String, String)> {
        vec![
            ("GIT_CONFIG_GLOBAL".to_owned(), "/dev/null".to_owned()),
            ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
            ("LC_ALL".to_owned(), "C".to_owned()),
            ("PATH".to_owned(), GIT_PATH.to_owned()),
        ]
    }

    fn output<S: AsRef<OsStr>>(
        &self,
        directory: &Path,
        arguments: &[S],
    ) -> Result<Output, WorktreeError> {
        let output = Process::new(&self.program)
            .arg("-C")
            .arg(directory)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
            ])
            .args(arguments)
            .env_clear()
            .envs(self.environment())
            .stdin(Stdio::null())
            .output()
            .map_err(|_| WorktreeError::Git)?;
        Ok(Output {
            success: output.status.success(),
            stdout: output.stdout,
        })
    }

    fn run<S: AsRef<OsStr>>(
        &self,
        directory: &Path,
        arguments: &[S],
    ) -> Result<Vec<u8>, WorktreeError> {
        let output = self.output(directory, arguments)?;
        if output.success {
            Ok(output.stdout)
        } else {
            Err(WorktreeError::Git)
        }
    }

    fn text<S: AsRef<OsStr>>(
        &self,
        directory: &Path,
        arguments: &[S],
    ) -> Result<String, WorktreeError> {
        let bytes = self.run(directory, arguments)?;
        Ok(String::from_utf8(bytes)
            .map_err(|_| WorktreeError::Git)?
            .trim()
            .to_owned())
    }

    /// The commit `reference` names, if it names one.
    fn commit_of(
        &self,
        repository: &Path,
        reference: &str,
    ) -> Result<Option<String>, WorktreeError> {
        let output = self.output(
            repository,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ],
        )?;
        if !output.success {
            return Ok(None);
        }
        let text = String::from_utf8(output.stdout).map_err(|_| WorktreeError::Git)?;
        Ok(Some(text.trim().to_owned()))
    }

    /// Canonical paths of the worktrees registered in `repository`.
    fn registered(&self, repository: &Path) -> Result<BTreeSet<PathBuf>, WorktreeError> {
        let listing = self.run(repository, &["worktree", "list", "--porcelain", "-z"])?;
        Ok(listing
            .split(|byte| *byte == 0)
            .filter_map(|field| field.strip_prefix(b"worktree "))
            .filter_map(|path| std::str::from_utf8(path).ok())
            .map(|path| fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)))
            .collect())
    }
}

struct Output {
    success: bool,
    stdout: Vec<u8>,
}

/// Counts of what reconciliation or gc did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    confirmed: u64,
    aborted: u64,
    finished_removals: u64,
    pending: u64,
    removed_worktrees: u64,
    removed_branches: u64,
}

impl Report {
    /// Creation intents confirmed against git.
    #[must_use]
    pub const fn confirmed(&self) -> u64 {
        self.confirmed
    }

    /// Creation intents rolled back.
    #[must_use]
    pub const fn aborted(&self) -> u64 {
        self.aborted
    }

    /// Removal intents finished.
    #[must_use]
    pub const fn finished_removals(&self) -> u64 {
        self.finished_removals
    }

    /// Intents still unresolved afterwards (0 after a successful reconciliation).
    #[must_use]
    pub const fn pending(&self) -> u64 {
        self.pending
    }

    /// Worktrees or directories removed by gc.
    #[must_use]
    pub const fn removed_worktrees(&self) -> u64 {
        self.removed_worktrees
    }

    /// `ws/*` branches deleted by gc.
    #[must_use]
    pub const fn removed_branches(&self) -> u64 {
        self.removed_branches
    }
}

/// Worktree manager of one root.
#[derive(Debug, Clone)]
pub struct Worktrees {
    layout: Layout,
    git: Git,
}

fn event(kind: &str, fields: Vec<(&str, Value)>) -> Result<Event, WorktreeError> {
    let data: Map<String, Value> = fields
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    Event::new(kind, data)
        .map_err(|error| WorktreeError::Supervisor(SupervisorError::Journal(error)))
}

fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

impl Worktrees {
    /// Manager of the worktrees of `layout`, driving `git`.
    #[must_use]
    pub const fn new(layout: Layout, git: Git) -> Self {
        Self { layout, git }
    }

    /// Absolute path of the worktree of `mission`.
    #[must_use]
    pub fn path_of(&self, mission: &str) -> PathBuf {
        self.layout.worktrees().join(mission)
    }

    /// The commit `HEAD` of `repository` points at (the base of a new worktree).
    ///
    /// # Errors
    ///
    /// [`WorktreeError::Git`].
    pub fn head_of(&self, repository: &Path) -> Result<CommitId, WorktreeError> {
        let head = self
            .git
            .commit_of(repository, "HEAD")?
            .ok_or(WorktreeError::Git)?;
        CommitId::parse(&head).map_err(|_| WorktreeError::Git)
    }

    /// Whether the worktree of `mission` has neither changes nor untracked files.
    ///
    /// # Errors
    ///
    /// [`WorktreeError::Git`].
    pub fn is_clean(&self, mission: &MissionId) -> Result<bool, WorktreeError> {
        let status = self.git.run(
            &self.path_of(mission.as_str()),
            &["status", "--porcelain", "--untracked-files=all"],
        )?;
        Ok(status.is_empty())
    }

    /// Creates the worktree of `mission` on `ws/<mission>` at `base` in `repository`.
    ///
    /// Returns the HEAD observed in the new worktree (the base).
    ///
    /// # Errors
    ///
    /// [`WorktreeError::BaseUnknown`] before any intent; git, journal and
    /// injected failures after it — reconciliation resolves those.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a distinct input of the step"
    )]
    pub fn provision(
        &self,
        supervisor: &mut Supervisor,
        repository: &Path,
        mission: &MissionId,
        repository_name: &str,
        base: &CommitId,
        faults: &mut Faults,
        at: Timestamp,
    ) -> Result<CommitId, WorktreeError> {
        if self.git.commit_of(repository, base.as_str())?.as_deref() != Some(base.as_str()) {
            return Err(WorktreeError::BaseUnknown);
        }
        fs::create_dir_all(self.layout.worktrees()).map_err(|_| WorktreeError::Io)?;
        let path = self.path_of(mission.as_str());
        supervisor.append(
            at.clone(),
            event(
                "worktree.create.intent",
                vec![
                    ("mission", text(mission.as_str())),
                    ("repository", text(repository_name)),
                    ("path", Value::String(worktree_of(mission))),
                    ("branch", Value::String(branch_of(mission))),
                    ("base_commit", text(base.as_str())),
                ],
            )?,
        )?;
        faults.check(FaultPoint::AfterCreateIntent)?;
        if faults.check(FaultPoint::DuringCreate).is_err() {
            // What an interrupted `git worktree add` can leave: a directory git never registered.
            fs::create_dir_all(&path).map_err(|_| WorktreeError::Io)?;
            fs::write(path.join(".partial-checkout"), b"").map_err(|_| WorktreeError::Io)?;
            return Err(WorktreeError::Fault(FaultPoint::DuringCreate));
        }
        self.git.run(
            repository,
            &[
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--quiet"),
                OsStr::new("-b"),
                OsStr::new(&branch_of(mission)),
                path.as_os_str(),
                OsStr::new(base.as_str()),
            ],
        )?;
        faults.check(FaultPoint::AfterCreateEffect)?;
        let head = CommitId::parse(&self.git.text(&path, &["rev-parse", "HEAD"])?)
            .map_err(|_| WorktreeError::Git)?;
        if head != *base {
            return Err(WorktreeError::Git);
        }
        supervisor.append(
            at,
            event(
                "worktree.created",
                vec![
                    ("mission", text(mission.as_str())),
                    ("head", text(head.as_str())),
                ],
            )?,
        )?;
        Ok(head)
    }

    /// Releases the worktree of `mission`; returns the archive digest on an abandon.
    ///
    /// # Errors
    ///
    /// [`WorktreeError::NotProvisioned`], [`WorktreeError::Dirty`] (a
    /// [`Release::Keep`] of a worktree with changes; nothing is written), and
    /// git, journal and injected failures.
    pub fn release(
        &self,
        supervisor: &mut Supervisor,
        repository: &Path,
        mission: &MissionId,
        release: Release,
        faults: &mut Faults,
        at: Timestamp,
    ) -> Result<Option<String>, WorktreeError> {
        let row = supervisor
            .store()
            .worktree(mission.as_str())?
            .ok_or(WorktreeError::NotProvisioned)?;
        if row.state != "created" {
            return Err(WorktreeError::NotProvisioned);
        }
        let path = self.path_of(mission.as_str());
        let status = self
            .git
            .run(&path, &["status", "--porcelain", "--untracked-files=all"])?;
        if release == Release::Keep && !status.is_empty() {
            return Err(WorktreeError::Dirty);
        }
        let delete_branch = release == Release::Abandon;
        supervisor.append(
            at.clone(),
            event(
                "worktree.remove.intent",
                vec![
                    ("mission", text(mission.as_str())),
                    ("delete_branch", Value::Bool(delete_branch)),
                ],
            )?,
        )?;
        faults.check(FaultPoint::AfterRemoveIntent)?;
        let archive = if delete_branch {
            let digest = self.archive(supervisor, &path, &row, at.clone())?;
            faults.check(FaultPoint::AfterArchive)?;
            Some(digest)
        } else {
            None
        };
        self.remove(repository, &path, &row.branch, delete_branch)?;
        faults.check(FaultPoint::AfterRemoveEffect)?;
        supervisor.append(
            at,
            event(
                "worktree.removed",
                vec![("mission", text(mission.as_str()))],
            )?,
        )?;
        Ok(archive)
    }

    /// Stores the full diff from the base (commits and working changes) as evidence.
    fn archive(
        &self,
        supervisor: &mut Supervisor,
        path: &Path,
        row: &WorktreeRow,
        at: Timestamp,
    ) -> Result<String, WorktreeError> {
        self.git.run(path, &["add", "-A"])?;
        let diff = self.git.run(
            path,
            &["diff", "--cached", "--binary", row.base_commit.as_str()],
        )?;
        let evidence = BlobStore::open(&self.layout.evidence())?;
        let digest = evidence.put(&diff)?.to_hex();
        supervisor.append(
            at,
            event(
                "worktree.archived",
                vec![
                    ("mission", text(&row.mission)),
                    ("archive_digest", text(&digest)),
                    (
                        "archive_bytes",
                        Value::from(u64::try_from(diff.len()).map_err(|_| WorktreeError::Io)?),
                    ),
                ],
            )?,
        )?;
        Ok(digest)
    }

    /// Removes the worktree at `path` and optionally its branch; idempotent.
    fn remove(
        &self,
        repository: &Path,
        path: &Path,
        branch: &str,
        delete_branch: bool,
    ) -> Result<(), WorktreeError> {
        let canonical = fs::canonicalize(path).ok();
        let registered = self.git.registered(repository)?;
        if canonical
            .as_ref()
            .is_some_and(|path| registered.contains(path))
        {
            self.git.run(
                repository,
                &[
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    OsStr::new("--force"),
                    OsStr::new("--force"),
                    path.as_os_str(),
                ],
            )?;
        }
        if path.exists() {
            fs::remove_dir_all(path).map_err(|_| WorktreeError::Io)?;
        }
        self.git.run(repository, &["worktree", "prune"])?;
        if delete_branch
            && self
                .git
                .commit_of(repository, &format!("refs/heads/{branch}"))?
                .is_some()
        {
            self.git.run(repository, &["branch", "-D", "--", branch])?;
        }
        Ok(())
    }

    /// Projected state of the worktree of `mission`.
    ///
    /// # Errors
    ///
    /// Projection failures.
    pub fn state(
        &self,
        supervisor: &Supervisor,
        mission: &MissionId,
    ) -> Result<Option<WorktreeState>, WorktreeError> {
        supervisor
            .store()
            .worktree(mission.as_str())?
            .map(|row| WorktreeState::parse(&row.state))
            .transpose()
    }

    /// Resolves every intent left without confirmation.
    ///
    /// `repository` maps a repository name of the configuration to its path.
    ///
    /// # Errors
    ///
    /// [`WorktreeError::RepositoryUnknown`], git, journal and projection failures.
    pub fn reconcile(
        &self,
        supervisor: &mut Supervisor,
        repository: &dyn Fn(&str) -> Option<PathBuf>,
        at: Timestamp,
    ) -> Result<Report, WorktreeError> {
        let mut report = Report::default();
        for row in supervisor.store().worktrees()? {
            if row.state != "creating" && row.state != "releasing" {
                continue;
            }
            let repo = repository(&row.repository).ok_or(WorktreeError::RepositoryUnknown)?;
            let path = self.path_of(&row.mission);
            if row.state == "creating" {
                let registered = self.git.registered(&repo)?;
                let present = fs::canonicalize(&path).is_ok_and(|path| registered.contains(&path));
                let head = if present {
                    self.git.commit_of(&path, "HEAD")?
                } else {
                    None
                };
                let branch = self
                    .git
                    .commit_of(&repo, &format!("refs/heads/{}", row.branch))?;
                if present && head.as_deref() == Some(row.base_commit.as_str()) && branch.is_some()
                {
                    supervisor.append(
                        at.clone(),
                        event(
                            "worktree.created",
                            vec![
                                ("mission", text(&row.mission)),
                                ("head", text(&row.base_commit)),
                            ],
                        )?,
                    )?;
                    report.confirmed += 1;
                } else {
                    // Delete the branch only if it is still the one this intent created.
                    let ours = branch.as_deref() == Some(row.base_commit.as_str());
                    self.remove(&repo, &path, &row.branch, ours)?;
                    supervisor.append(
                        at.clone(),
                        event(
                            "worktree.create.aborted",
                            vec![("mission", text(&row.mission))],
                        )?,
                    )?;
                    report.aborted += 1;
                }
            } else {
                let delete_branch = row.delete_branch.unwrap_or(false);
                if delete_branch && row.archive_digest.is_none() && path.exists() {
                    self.archive(supervisor, &path, &row, at.clone())?;
                }
                self.remove(&repo, &path, &row.branch, delete_branch)?;
                supervisor.append(
                    at.clone(),
                    event("worktree.removed", vec![("mission", text(&row.mission))])?,
                )?;
                report.finished_removals += 1;
            }
        }
        report.pending = u64::try_from(
            supervisor
                .store()
                .worktrees()?
                .iter()
                .filter(|row| row.state == "creating" || row.state == "releasing")
                .count(),
        )
        .map_err(|_| WorktreeError::Io)?;
        Ok(report)
    }

    /// Removes what no live record explains, in each `(name, path)` repository:
    /// registered worktrees and directories under `worktrees/` without a live
    /// record, and `ws/*` branches without a mission or whose release deleted them.
    ///
    /// # Errors
    ///
    /// git, file and projection failures.
    pub fn gc(
        &self,
        supervisor: &Supervisor,
        repositories: &[(String, PathBuf)],
    ) -> Result<Report, WorktreeError> {
        let mut report = Report::default();
        let rows = supervisor.store().worktrees()?;
        let live = |mission: &str| {
            rows.iter().any(|row| {
                row.mission == mission
                    && matches!(row.state.as_str(), "creating" | "created" | "releasing")
            })
        };
        let root = fs::canonicalize(self.layout.worktrees()).ok();
        let mut seen = BTreeSet::new();
        for (_, repository) in repositories {
            for registered in self.git.registered(repository)? {
                let under_root = root
                    .as_ref()
                    .is_some_and(|root| registered.parent() == Some(root.as_path()));
                if !under_root {
                    continue;
                }
                let name = registered
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default()
                    .to_owned();
                seen.insert(name.clone());
                if !live(&name) {
                    self.git.run(
                        repository,
                        &[
                            OsStr::new("worktree"),
                            OsStr::new("remove"),
                            OsStr::new("--force"),
                            OsStr::new("--force"),
                            registered.as_os_str(),
                        ],
                    )?;
                    report.removed_worktrees += 1;
                }
            }
            self.git.run(repository, &["worktree", "prune"])?;
            let branches = self.git.text(
                repository,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)",
                    "refs/heads/ws/",
                ],
            )?;
            for branch in branches.lines().filter(|line| !line.is_empty()) {
                let mission = branch.strip_prefix("ws/").unwrap_or_default();
                let keep = rows.iter().any(|row| {
                    row.mission == mission
                        && (live(mission)
                            || (row.state == "removed" && row.delete_branch == Some(false)))
                });
                if !keep {
                    self.git.run(repository, &["branch", "-D", "--", branch])?;
                    report.removed_branches += 1;
                }
            }
        }
        if let Ok(entries) = fs::read_dir(self.layout.worktrees()) {
            for entry in entries {
                let entry = entry.map_err(|_| WorktreeError::Io)?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if live(&name) || !entry.path().exists() {
                    continue;
                }
                fs::remove_dir_all(entry.path()).map_err(|_| WorktreeError::Io)?;
                if !seen.contains(&name) {
                    report.removed_worktrees += 1;
                }
            }
        }
        Ok(report)
    }
}
