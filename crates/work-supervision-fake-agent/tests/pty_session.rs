#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! PTY sessions driven by the fake agent (tranche T4).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};
use work_supervision_domain::{ExecutorProfile, Refusal};
use work_supervision_pty::{
    ALLOWED_ENVIRONMENT, Budget, Observation, RunBudgets, Session, SpawnSpec, executor_program,
};

const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_ws-fake-agent");

struct Run {
    dir: tempfile::TempDir,
}

impl Run {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn spec(&self, scenario: &str, budgets: RunBudgets) -> SpawnSpec {
        let scenario_path = self.path("scenario.txt");
        fs::write(&scenario_path, scenario).unwrap();
        let cwd = self.path("worktree");
        let home = self.path("home");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();
        SpawnSpec::new(
            executor_program(ExecutorProfile::Fake, Path::new(FAKE_AGENT)),
            vec![scenario_path.into_os_string()],
            cwd,
            home,
            self.path("pty.log"),
            budgets,
        )
    }
}

fn budgets(seconds: u64, bytes: u64) -> RunBudgets {
    RunBudgets::new(
        Duration::from_secs(seconds),
        bytes,
        Duration::from_millis(300),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn group_is_empty(pgid: u32) -> bool {
    // pgrep exits 1 when no process matches: a second instrument, outside this crate.
    let status = Process::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .status()
        .unwrap();
    status.code() == Some(1)
}

#[test]
fn ten_mebibytes_of_output_are_logged_without_loss_and_the_journalled_digest_is_the_log_digest() {
    let run = Run::new();
    let size = 10 * 1024 * 1024;
    let session =
        Session::spawn(run.spec(&format!("flood {size}\nexit 0\n"), budgets(60, 64 << 20)))
            .unwrap();
    let pgid = session.process_group();
    let mut checkpoints = Vec::new();
    let exit = session
        .wait_with(|observation| {
            if let Observation::Checkpoint { bytes, digest } = observation {
                checkpoints.push((bytes, digest));
            }
        })
        .unwrap();
    let log = fs::read(run.path("pty.log")).unwrap();
    assert_eq!(
        log.len() as u64,
        size,
        "every byte of the flood is in the log"
    );
    assert_eq!(exit.output_bytes(), size);
    assert_eq!(exit.output_digest(), hex(&Sha256::digest(&log)));
    assert_eq!(exit.exit_code(), Some(0));
    assert_eq!(exit.budget_exceeded(), None);
    let expected: Vec<u8> = (0..size).map(|index| b'a' + (index % 26) as u8).collect();
    assert!(log == expected, "the log holds the flood byte for byte");
    // Checkpoints are cumulative: each digest is the digest of the log prefix.
    assert!(checkpoints.len() >= 2, "{} checkpoints", checkpoints.len());
    for (bytes, digest) in &checkpoints {
        let prefix = &log[..usize::try_from(*bytes).unwrap()];
        assert_eq!(*digest, hex(&Sha256::digest(prefix)));
    }
    assert!(group_is_empty(pgid));
}

#[test]
fn input_reaches_the_agent_byte_for_byte_and_is_recorded_by_length_and_digest_only() {
    let run = Run::new();
    let spec = run
        .spec("print ready\nread-line\nexit 0\n", budgets(30, 1 << 20))
        .with_idle_after(Duration::from_millis(200));
    let session = Session::spawn(spec).unwrap();
    let text = "héllo wörld — 0123 ✓";
    let mut sent = None;
    let exit = session
        .wait_with_input(|observation, input| {
            if matches!(observation, Observation::Idle) && sent.is_none() {
                sent = Some(input.write(format!("{text}\n").as_bytes()).unwrap());
            }
        })
        .unwrap();
    let record = sent.expect("the agent went idle waiting for input");
    assert_eq!(record.bytes(), (text.len() + 1) as u64);
    assert_eq!(
        record.digest(),
        hex(&Sha256::digest(format!("{text}\n").as_bytes()))
    );
    let log = String::from_utf8(fs::read(run.path("pty.log")).unwrap()).unwrap();
    assert!(
        log.contains(&format!("received {}", hex(text.as_bytes()))),
        "the agent echoes the hex of what it read: {log:?}"
    );
    assert_eq!(exit.exit_code(), Some(0));
}

#[test]
fn a_duration_budget_terminates_the_group_with_sigterm() {
    let run = Run::new();
    let started = Instant::now();
    let session = Session::spawn(run.spec("print working\nblock\n", budgets(1, 1 << 20))).unwrap();
    let pgid = session.process_group();
    let exit = session.wait_with(|_| {}).unwrap();
    assert_eq!(exit.budget_exceeded(), Some(Budget::Duration));
    assert_eq!(exit.signal(), Some(15), "SIGTERM");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(group_is_empty(pgid));
}

#[test]
fn an_output_budget_terminates_the_group() {
    let run = Run::new();
    let session = Session::spawn(run.spec("flood 4194304\nblock\n", budgets(30, 65_536))).unwrap();
    let pgid = session.process_group();
    let exit = session.wait_with(|_| {}).unwrap();
    assert_eq!(exit.budget_exceeded(), Some(Budget::Output));
    assert!(exit.output_bytes() >= 65_536);
    assert!(exit.output_bytes() < 4_194_304, "output stopped early");
    assert!(group_is_empty(pgid));
}

#[test]
fn a_child_that_ignores_sigterm_is_killed_with_sigkill_after_the_grace_delay() {
    let run = Run::new();
    let scenario = "spawn-stubborn-child\nprint spawned\nblock\n";
    let session = Session::spawn(run.spec(scenario, budgets(1, 1 << 20))).unwrap();
    let pgid = session.process_group();
    let exit = session.wait_with(|_| {}).unwrap();
    assert_eq!(exit.budget_exceeded(), Some(Budget::Duration));
    assert!(exit.escalated_to_kill(), "SIGTERM was not enough");
    assert!(group_is_empty(pgid), "no process of the group survives");
}

#[test]
fn processes_left_behind_by_a_normal_exit_are_terminated() {
    let run = Run::new();
    let session =
        Session::spawn(run.spec("spawn-stubborn-child\nexit 0\n", budgets(30, 1 << 20))).unwrap();
    let pgid = session.process_group();
    let exit = session.wait_with(|_| {}).unwrap();
    assert_eq!(exit.exit_code(), Some(0));
    assert!(group_is_empty(pgid));
}

#[test]
fn the_environment_is_cleared_to_the_allow_list() {
    let run = Run::new();
    // The test process has PATH, HOME, USER, CARGO_* … in its environment; none but
    // the allow-list may reach the agent.
    let session =
        Session::spawn(run.spec("env\nprint-cwd\nexit 0\n", budgets(30, 1 << 20))).unwrap();
    session.wait_with(|_| {}).unwrap();
    let log = String::from_utf8(fs::read(run.path("pty.log")).unwrap()).unwrap();
    let names: Vec<&str> = log
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').strip_prefix("env "))
        .collect();
    let mut allowed: Vec<&str> = ALLOWED_ENVIRONMENT.to_vec();
    allowed.sort_unstable();
    assert_eq!(
        names, allowed,
        "only the allow-listed variables reach the agent"
    );
    for host_only in [
        "USER",
        "LOGNAME",
        "SSH_AUTH_SOCK",
        "CARGO",
        "RUSTUP_HOME",
        "TMPDIR",
    ] {
        assert!(!names.contains(&host_only));
    }
    let cwd = fs::canonicalize(run.path("worktree")).unwrap();
    assert!(log.contains(&format!("cwd {}", cwd.display())), "{log:?}");
}

#[test]
fn a_crash_is_reported_with_its_signal_and_everything_logged_before_it_is_kept() {
    let run = Run::new();
    let session =
        Session::spawn(run.spec("print before the crash\ncrash\n", budgets(30, 1 << 20))).unwrap();
    let exit = session.wait_with(|_| {}).unwrap();
    assert_eq!(exit.signal(), Some(6), "SIGABRT");
    let log = String::from_utf8(fs::read(run.path("pty.log")).unwrap()).unwrap();
    assert!(log.contains("before the crash"));
}

#[test]
fn spawning_refuses_a_missing_working_directory_instead_of_falling_back_to_home() {
    let run = Run::new();
    let mut spec = run.spec("exit 0\n", budgets(30, 1 << 20));
    spec = spec.with_cwd(run.path("absent"));
    assert_eq!(Session::spawn(spec).unwrap_err().code(), "pty.cwd_invalid");
}

#[test]
fn the_c0_guard_resolves_only_the_fake_agent() {
    assert_eq!(ExecutorProfile::ALL.len(), 1);
    assert_eq!(
        executor_program(ExecutorProfile::Fake, Path::new(FAKE_AGENT)),
        PathBuf::from(FAKE_AGENT)
    );
    for real in ["claude", "codex", "/opt/agents/claude"] {
        assert_eq!(
            ExecutorProfile::from_config_name(real),
            Err(Refusal::RealAgentForbiddenUntilC0)
        );
    }
}

#[test]
fn the_fake_agent_edits_files_and_commits_in_its_worktree() {
    let run = Run::new();
    let worktree = run.path("worktree");
    fs::create_dir_all(&worktree).unwrap();
    let git = |args: &[&str]| {
        let status = Process::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "base",
    ]);
    let session = Session::spawn(run.spec(
        "write-file notes/out.txt hello from the fake agent\ncommit add the output\nexit 0\n",
        budgets(30, 1 << 20),
    ))
    .unwrap();
    let exit = session.wait_with(|_| {}).unwrap();
    assert_eq!(
        exit.exit_code(),
        Some(0),
        "{}",
        fs::read_to_string(run.path("pty.log")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(worktree.join("notes/out.txt")).unwrap(),
        "hello from the fake agent\n"
    );
    let log = Process::new("git")
        .arg("-C")
        .arg(&worktree)
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(log.stdout).unwrap(),
        "add the output\nbase\n"
    );
}

#[test]
fn the_fake_agent_refuses_paths_outside_its_worktree_and_unknown_steps() {
    let run = Run::new();
    let session =
        Session::spawn(run.spec("write-file ../escape.txt x\n", budgets(30, 1 << 20))).unwrap();
    let exit = session.wait_with(|_| {}).unwrap();
    assert_eq!(exit.exit_code(), Some(2));
    assert!(!run.path("escape.txt").exists());
    let run = Run::new();
    let session = Session::spawn(run.spec("teleport\n", budgets(30, 1 << 20))).unwrap();
    assert_eq!(session.wait_with(|_| {}).unwrap().exit_code(), Some(2));
}
