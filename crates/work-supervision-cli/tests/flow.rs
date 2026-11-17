#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `ws flow` against a real `wsd` and the fake agent
//! (docs/work-supervision/flow-v0.md).
//!
//! The fixture is a reduced copy of the one of `cli.rs`: sharing it would
//! move code out of test files this change does not own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Output, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::Value;
use work_supervision_store::{Layout, Store};

const WS: &str = env!("CARGO_BIN_EXE_ws");

fn sibling(name: &str) -> PathBuf {
    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        let status = Process::new(env!("CARGO"))
            .args([
                "build",
                "--locked",
                "-p",
                "work-supervision-daemon",
                "--bin",
                "wsd",
                "-p",
                "work-supervision-fake-agent",
                "--bin",
                "ws-fake-agent",
            ])
            .status()
            .unwrap();
        assert!(status.success());
    });
    Path::new(WS).with_file_name(name)
}

fn git(dir: &Path, args: &[&str]) {
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
    assert!(output.status.success(), "git {args:?}");
}

fn ws(root: &Path, args: &[&str]) -> Output {
    Process::new(WS)
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> Value {
    let output = ws(root, args);
    assert!(
        output.status.success(),
        "ws {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    base: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let root = base.join("root");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "user.email", "t@example.invalid"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let init = Process::new(WS)
        .args(["init", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(init.status.success());
    fs::write(
        root.join("config.toml"),
        format!(
            "[repositories]\nsample = \"{}\"\n[executor]\nprofile = \"fake\"\nfake_agent = \"{}\"\n[runs]\nidle_after_ms = 300\ngrace_ms = 500\n",
            repo.display(),
            sibling("ws-fake-agent").display()
        ),
    )
    .unwrap();
    Setup {
        _dir: dir,
        root,
        base,
    }
}

struct Daemon(Child);

impl Daemon {
    fn start(root: &Path) -> Self {
        let child = Process::new(sibling("wsd"))
            .arg("--root")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Self(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ws(root, &["status"]).status.success() {
            assert!(Instant::now() < deadline, "wsd did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }

    fn stop(mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Creates a mission, runs it to its exit, submits and accepts its result.
fn accepted_mission(setup: &Setup, name: &str) -> String {
    let brief = setup.base.join(format!("brief-{name}.txt"));
    fs::write(
        &brief,
        format!("write-file {name}.txt done\ncommit add {name}\nexit 0\n"),
    )
    .unwrap();
    let created = ok(
        &setup.root,
        &[
            "mission",
            "new",
            "--repository",
            "sample",
            "--title",
            "Write a file",
            "--brief",
            brief.to_str().unwrap(),
        ],
    );
    let id = created["mission"].as_str().unwrap().to_owned();
    ok(&setup.root, &["mission", "ready", &id]);
    ok(&setup.root, &["run", &id]);
    ok(&setup.root, &["wait", &id, "exited"]);
    let evidence = setup.base.join("evidence.txt");
    fs::write(&evidence, "1 test passed\n").unwrap();
    ok(
        &setup.root,
        &[
            "result",
            "submit",
            &id,
            "--evidence",
            evidence.to_str().unwrap(),
            "--summary",
            "written",
        ],
    );
    ok(&setup.root, &["decide", &id, "accept", "--reason", "ok"]);
    id
}

#[test]
fn ws_flow_prints_the_measures_and_exits_0_when_every_mission_is_measured() {
    let setup = setup();
    let daemon = Daemon::start(&setup.root);
    accepted_mission(&setup, "one");

    let output = ws(&setup.root, &["flow"]);
    assert_eq!(output.status.code(), Some(0));
    let measures: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(measures["schema"], "libre-ai.work-supervision.flow.v0");
    assert_eq!(measures["examined"], 1);
    assert_eq!(measures["retained"], 1);
    assert_eq!(measures["accepted_first_pass"], 1);
    assert_eq!(measures["simulated"], 1);
    assert_eq!(measures["unusable"], serde_json::json!([]));

    // One word too many is a usage error, and prints no measure.
    let usage = ws(&setup.root, &["flow", "extra"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(usage.stdout.is_empty());

    // A projection rebuilt from the journal answers the same bytes.
    daemon.stop();
    assert_eq!(ok(&setup.root, &["rebuild"])["replaced"], true);
    let _daemon = Daemon::start(&setup.root);
    let rebuilt = ws(&setup.root, &["flow"]);
    assert_eq!(rebuilt.status.code(), Some(0));
    assert_eq!(rebuilt.stdout, output.stdout);
}

#[test]
fn ws_flow_without_a_daemon_is_a_refusal() {
    let setup = setup();
    let output = ws(&setup.root, &["flow"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("ws: "));
}

#[test]
fn ws_flow_prints_the_measures_and_exits_3_when_a_mission_cannot_be_measured() {
    let setup = setup();
    let daemon = Daemon::start(&setup.root);
    let kept = accepted_mission(&setup, "one");
    let uncovered = accepted_mission(&setup, "two");
    daemon.stop();
    // A root older than the timeline migration holds accepted missions whose
    // earlier entries have no timeline row: the same state is made here.
    let removed = Store::open(&Layout::new(&setup.root).state())
        .unwrap()
        .connection()
        .execute(
            "DELETE FROM mission_events WHERE mission_id = ?1 AND kind = 'mission.created'",
            [uncovered.as_str()],
        )
        .unwrap();
    assert_eq!(removed, 1);

    let _daemon = Daemon::start(&setup.root);
    let output = ws(&setup.root, &["flow"]);
    assert_eq!(output.status.code(), Some(3));
    let measures: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(measures["examined"], 2);
    assert_eq!(measures["retained"], 1);
    assert_eq!(
        measures["unusable"],
        serde_json::json!([{ "mission": uncovered, "code": "timeline.missing" }])
    );
    assert_ne!(kept, uncovered);

    // `ws rebuild` fills the timeline from the journal: the measure is whole again.
    drop(_daemon);
    ok(&setup.root, &["rebuild"]);
    let _daemon = Daemon::start(&setup.root);
    let repaired = ws(&setup.root, &["flow"]);
    assert_eq!(repaired.status.code(), Some(0));
    let measures: Value = serde_json::from_slice(&repaired.stdout).unwrap();
    assert_eq!(measures["retained"], 2);
    assert_eq!(measures["unusable"], serde_json::json!([]));
}
