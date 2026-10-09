#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Worktree lifecycle: intent → effect → confirmation, reconciliation, gc (tranche T5).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use work_supervision_domain::{Budgets, Command, CommitId, ExecutorProfile, MissionId};
use work_supervision_journal::{OpenMode, Timestamp};
use work_supervision_store::{Layout, Supervisor};
use work_supervision_worktree::{
    FaultPoint, Faults, Git, Release, WorktreeError, WorktreeState, Worktrees,
};

fn at() -> Timestamp {
    Timestamp::parse("2026-10-09T02:00:00.000Z").unwrap()
}

fn host_git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    layout: Layout,
    repo: PathBuf,
    base: CommitId,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let repo = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&repo).unwrap();
    host_git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("README"), "base\n").unwrap();
    host_git(&repo, &["add", "README"]);
    host_git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "base",
        ],
    );
    let base = CommitId::parse(host_git(&repo, &["rev-parse", "HEAD"]).trim()).unwrap();
    Fixture {
        layout: Layout::new(&root),
        repo,
        base,
        _dir: dir,
    }
}

fn open(fixture: &Fixture) -> Supervisor {
    Supervisor::open(&fixture.layout, OpenMode::Strict).unwrap()
}

fn ready_mission(supervisor: &mut Supervisor, byte: u8) -> MissionId {
    let id = MissionId::from_bytes([byte; 16]);
    let create = Command::Create {
        title: format!("Mission {byte}"),
        repository: "sample".to_owned(),
        brief: "print hi\n".to_owned(),
        criteria: vec![],
        budgets: Budgets::new(60, 1 << 20).unwrap(),
        executor: ExecutorProfile::Fake,
    };
    supervisor.execute(&id, &create, 0, at()).unwrap();
    supervisor.execute(&id, &Command::Ready, 1, at()).unwrap();
    id
}

fn worktrees(fixture: &Fixture) -> Worktrees {
    Worktrees::new(fixture.layout.clone(), Git::system())
}

fn registered_worktrees(repo: &Path) -> usize {
    host_git(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count()
}

fn mission_branches(repo: &Path) -> Vec<String> {
    host_git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/ws/",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

fn resolver(fixture: &Fixture) -> impl Fn(&str) -> Option<PathBuf> + '_ {
    move |name: &str| (name == "sample").then(|| fixture.repo.clone())
}

#[test]
fn provisioning_records_intent_then_confirmation_with_the_observed_head() {
    let fixture = fixture();
    let mut supervisor = open(&fixture);
    let id = ready_mission(&mut supervisor, 1);
    let before = supervisor.journal_entries();
    let head = worktrees(&fixture)
        .provision(
            &mut supervisor,
            &fixture.repo,
            &id,
            "sample",
            &fixture.base,
            &mut Faults::none(),
            at(),
        )
        .unwrap();
    assert_eq!(head, fixture.base);
    assert_eq!(
        supervisor.journal_entries(),
        before + 2,
        "intent + confirmation"
    );
    let journal = fs::read_to_string(fixture.layout.journal()).unwrap();
    let intent = journal.find("\"kind\":\"worktree.create.intent\"").unwrap();
    let created = journal.find("\"kind\":\"worktree.created\"").unwrap();
    assert!(intent < created);
    let path = fixture.layout.worktrees().join(id.as_str());
    assert_eq!(
        host_git(&path, &["rev-parse", "HEAD"]).trim(),
        fixture.base.as_str()
    );
    assert_eq!(
        host_git(&path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        format!("ws/{id}")
    );
    assert_eq!(
        worktrees(&fixture).state(&supervisor, &id).unwrap(),
        Some(WorktreeState::Created)
    );
}

#[test]
fn an_unknown_base_is_refused_before_any_intent_is_written() {
    let fixture = fixture();
    let mut supervisor = open(&fixture);
    let id = ready_mission(&mut supervisor, 1);
    let before = supervisor.journal_entries();
    let unknown = CommitId::parse(&"9".repeat(40)).unwrap();
    let error = worktrees(&fixture)
        .provision(
            &mut supervisor,
            &fixture.repo,
            &id,
            "sample",
            &unknown,
            &mut Faults::none(),
            at(),
        )
        .unwrap_err();
    assert_eq!(error, WorktreeError::BaseUnknown);
    assert_eq!(supervisor.journal_entries(), before);
}

#[test]
fn a_dirty_worktree_is_refused_unless_abandoned_and_abandon_archives_the_diff_first() {
    let fixture = fixture();
    let mut supervisor = open(&fixture);
    let id = ready_mission(&mut supervisor, 1);
    let manager = worktrees(&fixture);
    manager
        .provision(
            &mut supervisor,
            &fixture.repo,
            &id,
            "sample",
            &fixture.base,
            &mut Faults::none(),
            at(),
        )
        .unwrap();
    let path = fixture.layout.worktrees().join(id.as_str());
    fs::write(path.join("README"), "changed\n").unwrap();
    fs::write(path.join("new-file.txt"), "untracked work\n").unwrap();
    let before = supervisor.journal_entries();
    assert_eq!(
        manager
            .release(
                &mut supervisor,
                &fixture.repo,
                &id,
                Release::Keep,
                &mut Faults::none(),
                at()
            )
            .unwrap_err(),
        WorktreeError::Dirty
    );
    assert_eq!(
        supervisor.journal_entries(),
        before,
        "a refused release writes nothing"
    );
    assert!(path.exists());

    let archive = manager
        .release(
            &mut supervisor,
            &fixture.repo,
            &id,
            Release::Abandon,
            &mut Faults::none(),
            at(),
        )
        .unwrap()
        .expect("abandon archives");
    let diff =
        String::from_utf8(fs::read(fixture.layout.evidence().join(&archive)).unwrap()).unwrap();
    assert!(diff.contains("+changed"));
    assert!(diff.contains("+untracked work"));
    assert!(!path.exists());
    assert_eq!(registered_worktrees(&fixture.repo), 1);
    assert!(
        mission_branches(&fixture.repo).is_empty(),
        "abandon deletes ws/<mission>"
    );
    let journal = fs::read_to_string(fixture.layout.journal()).unwrap();
    assert!(
        journal.contains(&archive),
        "the archive digest is journalled"
    );
    assert!(!journal.contains("untracked work"), "never its content");
    assert_eq!(
        manager.state(&supervisor, &id).unwrap(),
        Some(WorktreeState::Removed)
    );
}

#[test]
fn an_accepted_mission_keeps_its_branch_with_the_committed_result() {
    let fixture = fixture();
    let mut supervisor = open(&fixture);
    let id = ready_mission(&mut supervisor, 1);
    let manager = worktrees(&fixture);
    manager
        .provision(
            &mut supervisor,
            &fixture.repo,
            &id,
            "sample",
            &fixture.base,
            &mut Faults::none(),
            at(),
        )
        .unwrap();
    let path = fixture.layout.worktrees().join(id.as_str());
    fs::write(path.join("result.txt"), "done\n").unwrap();
    host_git(&path, &["add", "result.txt"]);
    host_git(
        &path,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "result",
        ],
    );
    let result = host_git(&path, &["rev-parse", "HEAD"]);
    assert_eq!(
        manager
            .release(
                &mut supervisor,
                &fixture.repo,
                &id,
                Release::Keep,
                &mut Faults::none(),
                at()
            )
            .unwrap(),
        None
    );
    assert!(!path.exists());
    assert_eq!(mission_branches(&fixture.repo), vec![format!("ws/{id}")]);
    assert_eq!(
        host_git(&fixture.repo, &["rev-parse", &format!("ws/{id}")]),
        result
    );
}

#[test]
fn a_hundred_cycles_with_injected_crashes_leave_no_orphan_worktree_nor_branch() {
    let fixture = fixture();
    let points = FaultPoint::ALL;
    let mut crashes = 0;
    for cycle in 0..100_u32 {
        let byte = u8::try_from(cycle % 250).unwrap() + 1;
        let point = points[usize::try_from(cycle).unwrap() % points.len()];
        let id = {
            let mut supervisor = open(&fixture);
            let id = MissionId::from_bytes([
                byte,
                u8::try_from(cycle / 250).unwrap(),
                7,
                7,
                7,
                7,
                7,
                7,
                7,
                7,
                7,
                7,
                7,
                7,
                7,
                7,
            ]);
            let create = Command::Create {
                title: format!("cycle {cycle}"),
                repository: "sample".to_owned(),
                brief: "print hi\n".to_owned(),
                criteria: vec![],
                budgets: Budgets::new(60, 1 << 20).unwrap(),
                executor: ExecutorProfile::Fake,
            };
            supervisor.execute(&id, &create, 0, at()).unwrap();
            let mut faults = Faults::at(point);
            match worktrees(&fixture).provision(
                &mut supervisor,
                &fixture.repo,
                &id,
                "sample",
                &fixture.base,
                &mut faults,
                at(),
            ) {
                Ok(_) => {}
                Err(WorktreeError::Fault(hit)) => {
                    assert_eq!(hit, point);
                    crashes += 1;
                }
                Err(other) => panic!("cycle {cycle}: {other:?}"),
            }
            id
            // The supervisor is dropped here: the process "dies" with its lock.
        };
        {
            let mut supervisor = open(&fixture);
            let report = worktrees(&fixture)
                .reconcile(&mut supervisor, &resolver(&fixture), at())
                .unwrap();
            assert_eq!(report.pending(), 0, "cycle {cycle}: {report:?}");
            let state = worktrees(&fixture).state(&supervisor, &id).unwrap();
            if state == Some(WorktreeState::Created) {
                let mut faults = Faults::at(point);
                match worktrees(&fixture).release(
                    &mut supervisor,
                    &fixture.repo,
                    &id,
                    Release::Abandon,
                    &mut faults,
                    at(),
                ) {
                    Ok(_) => {}
                    Err(WorktreeError::Fault(_)) => crashes += 1,
                    Err(other) => panic!("cycle {cycle}: {other:?}"),
                }
            }
        }
        let mut supervisor = open(&fixture);
        worktrees(&fixture)
            .reconcile(&mut supervisor, &resolver(&fixture), at())
            .unwrap();
        let state = worktrees(&fixture).state(&supervisor, &id).unwrap();
        assert!(
            matches!(state, Some(WorktreeState::Removed | WorktreeState::Aborted)),
            "cycle {cycle} at {point:?}: {state:?}"
        );
        assert_eq!(
            registered_worktrees(&fixture.repo),
            1,
            "cycle {cycle} at {point:?}: orphan worktree"
        );
        assert!(
            mission_branches(&fixture.repo).is_empty(),
            "cycle {cycle} at {point:?}: orphan branch"
        );
        let residue: Vec<_> = fs::read_dir(fixture.layout.worktrees())
            .map(|entries| entries.map(|entry| entry.unwrap().file_name()).collect())
            .unwrap_or_default();
        assert!(
            residue.is_empty(),
            "cycle {cycle} at {point:?}: {residue:?}"
        );
    }
    assert!(crashes >= 60, "{crashes} crashes injected");
}

#[test]
fn gc_removes_unrecorded_worktrees_and_branches_and_reports_them() {
    let fixture = fixture();
    let supervisor = open(&fixture);
    let stray = fixture
        .layout
        .worktrees()
        .join("ffffffffffffffffffffffffffffffff");
    fs::create_dir_all(fixture.layout.worktrees()).unwrap();
    host_git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "ws/ffffffffffffffffffffffffffffffff",
            stray.to_str().unwrap(),
            fixture.base.as_str(),
        ],
    );
    fs::create_dir_all(fixture.layout.worktrees().join("not-a-mission")).unwrap();
    let report = worktrees(&fixture)
        .gc(&supervisor, &[("sample".to_owned(), fixture.repo.clone())])
        .unwrap();
    assert_eq!(report.removed_worktrees(), 2);
    assert_eq!(report.removed_branches(), 1);
    assert_eq!(registered_worktrees(&fixture.repo), 1);
    assert!(mission_branches(&fixture.repo).is_empty());
}

#[test]
fn git_runs_without_the_host_environment_shell_or_hooks() {
    let fixture = fixture();
    // A hook that would leave a mark if git ran it.
    let hooks = fixture.repo.join(".git").join("hooks");
    fs::write(
        hooks.join("post-checkout"),
        "#!/bin/sh\ntouch \"$GIT_DIR/../hook-ran\"\n",
    )
    .unwrap();
    Process::new("chmod")
        .arg("+x")
        .arg(hooks.join("post-checkout"))
        .status()
        .unwrap();
    let mut supervisor = open(&fixture);
    let id = ready_mission(&mut supervisor, 1);
    worktrees(&fixture)
        .provision(
            &mut supervisor,
            &fixture.repo,
            &id,
            "sample",
            &fixture.base,
            &mut Faults::none(),
            at(),
        )
        .unwrap();
    assert!(
        !fixture.repo.join("hook-ran").exists(),
        "hooks are disabled"
    );
    assert!(
        !fixture
            .layout
            .worktrees()
            .join(id.as_str())
            .join("hook-ran")
            .exists()
    );
    let environment = Git::system().environment();
    let names: Vec<&str> = environment.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec!["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "LC_ALL", "PATH"]
    );
}
