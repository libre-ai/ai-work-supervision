#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! wsd end to end: flow measures (docs/work-supervision/flow-v0.md).
//!
//! The fixture is a reduced copy of the one of `coordination.rs`: sharing it
//! would move code out of test files this change does not own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use work_supervision_daemon::{Client, ClientError, init_root};
use work_supervision_store::{BlobStore, Layout, Store};

const WSD: &str = env!("CARGO_BIN_EXE_wsd");

fn fake_agent() -> PathBuf {
    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        let status = Process::new(env!("CARGO"))
            .args([
                "build",
                "--locked",
                "-p",
                "work-supervision-fake-agent",
                "--bin",
                "ws-fake-agent",
            ])
            .status()
            .unwrap();
        assert!(status.success());
    });
    let path = Path::new(WSD).with_file_name("ws-fake-agent");
    assert!(path.is_file());
    path
}

fn host_git(dir: &Path, args: &[&str]) {
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

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let root = base.join("root");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    host_git(&repo, &["init", "-q", "-b", "main"]);
    host_git(&repo, &["config", "user.name", "t"]);
    host_git(&repo, &["config", "user.email", "t@example.invalid"]);
    host_git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    init_root(&root).unwrap();
    let config = format!(
        "[repositories]\nsample = \"{}\"\n\n[executor]\nprofile = \"fake\"\nfake_agent = \"{}\"\n\n[runs]\nidle_after_ms = 300\ngrace_ms = 500\n",
        repo.display(),
        fake_agent().display()
    );
    fs::write(root.join("config.toml"), config).unwrap();
    Fixture { _dir: dir, root }
}

struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    fn start(root: &Path) -> Self {
        let child = Process::new(WSD)
            .arg("--root")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let socket = root.join("run").join("wsd.sock");
        let deadline = Instant::now() + Duration::from_secs(20);
        while Client::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "wsd did not listen");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { child, socket }
    }

    fn client(&self) -> Client {
        Client::connect(&self.socket).unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn call(client: &mut Client, request: Value) -> Result<Value, ClientError> {
    client.request(&request)
}

fn new_mission(client: &mut Client, file: &str) -> String {
    let brief =
        format!("print starting\nwrite-file {file} the result\ncommit add {file}\nexit 0\n");
    let data = call(
        client,
        json!({
            "op": "mission.new", "title": "A mission", "repository": "sample",
            "brief": brief, "criteria": [],
            "max_duration_seconds": 60, "max_output_bytes": 1_048_576
        }),
    )
    .unwrap();
    data["mission"].as_str().unwrap().to_owned()
}

/// Runs `mission` to its exit and submits its result.
fn run_and_submit(client: &mut Client, root: &Path, mission: &str) {
    call(client, json!({ "op": "run", "mission": mission })).unwrap();
    call(
        client,
        json!({ "op": "wait", "mission": mission, "states": ["exited"], "timeout_ms": 20_000 }),
    )
    .unwrap();
    let evidence = BlobStore::open(&Layout::new(root).evidence())
        .unwrap()
        .put(b"evidence")
        .unwrap()
        .to_hex();
    call(
        client,
        json!({ "op": "result.submit", "mission": mission, "evidence_digest": evidence, "summary": "done" }),
    )
    .unwrap();
}

fn decide(client: &mut Client, mission: &str, decision: &str) {
    call(
        client,
        json!({ "op": "decide", "mission": mission, "decision": decision, "reason": "the reason" }),
    )
    .unwrap();
}

/// Two accepted missions, the second one rejected once before; a third
/// mission is left a draft. Returns the two accepted identifiers.
fn two_accepted(client: &mut Client, root: &Path) -> (String, String) {
    let first = new_mission(client, "a.txt");
    call(client, json!({ "op": "mission.ready", "mission": first })).unwrap();
    run_and_submit(client, root, &first);
    decide(client, &first, "accept");

    let second = new_mission(client, "b.txt");
    call(client, json!({ "op": "mission.ready", "mission": second })).unwrap();
    run_and_submit(client, root, &second);
    decide(client, &second, "reject");
    // The brief is frozen: the second run replays it on the kept worktree.
    run_and_submit(client, root, &second);
    decide(client, &second, "accept");

    new_mission(client, "c.txt");
    (first, second)
}

fn measures(client: &mut Client) -> Value {
    call(client, json!({ "op": "flow.measures" })).unwrap()
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn two_accepted_missions_one_rejected_once_are_measured() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root);
    let mut client = daemon.client();
    two_accepted(&mut client, &fixture.root);

    let read = measures(&mut client);
    assert_eq!(read["schema"], "libre-ai.work-supervision.flow.v0");
    assert_eq!(read["examined"], 2);
    assert_eq!(read["retained"], 2);
    assert_eq!(read["accepted_first_pass"], 1);
    assert_eq!(read["simulated"], 2);
    assert_eq!(read["unusable"], json!([]));
    // The nine keys of the specification, and nothing else.
    assert_eq!(
        keys(&read),
        [
            "accepted_first_pass",
            "examined",
            "lead_seconds",
            "retained",
            "review_seconds",
            "schema",
            "simulated",
            "throughput_per_week_hundredths",
            "unusable",
        ]
    );
    for duration in ["lead_seconds", "review_seconds"] {
        assert_eq!(keys(&read[duration]), ["p50", "p85"]);
        assert!(read[duration]["p50"].is_u64(), "{duration}");
        assert!(read[duration]["p85"].is_u64(), "{duration}");
        assert!(read[duration]["p50"].as_u64() <= read[duration]["p85"].as_u64());
    }
    // Two missions inside a window far shorter than a week.
    assert_eq!(read["throughput_per_week_hundredths"], 200);
}

#[test]
fn two_reads_of_the_same_root_answer_the_same_bytes() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root);
    let mut client = daemon.client();
    two_accepted(&mut client, &fixture.root);
    let first = serde_json::to_vec(&measures(&mut client)).unwrap();
    let second = serde_json::to_vec(&measures(&mut client)).unwrap();
    assert_eq!(first, second);
    // The read appended nothing to the journal.
    let before = call(&mut client, json!({ "op": "status" })).unwrap()["journal_entries"].clone();
    measures(&mut client);
    let after = call(&mut client, json!({ "op": "status" })).unwrap()["journal_entries"].clone();
    assert_eq!(before, after);
}

#[test]
fn a_session_actor_is_refused_the_measures() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root);
    let mut client = daemon.client();
    let session = call(
        &mut client,
        json!({ "op": "session.register", "harness": "claude-code", "label": "a session" }),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = call(
        &mut client,
        json!({ "op": "flow.measures", "actor": format!("session:{session}") }),
    );
    assert_eq!(refused.unwrap_err().code(), "actor.owner_only");
    // The owner, named or implied, is served.
    assert_eq!(measures(&mut client)["examined"], 0);
    let named = call(
        &mut client,
        json!({ "op": "flow.measures", "actor": "owner" }),
    );
    assert_eq!(named.unwrap()["examined"], 0);
}

#[test]
fn a_root_without_an_accepted_mission_answers_null_measures() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root);
    let mut client = daemon.client();
    new_mission(&mut client, "a.txt");
    assert_eq!(
        measures(&mut client),
        json!({
            "schema": "libre-ai.work-supervision.flow.v0",
            "examined": 0,
            "retained": 0,
            "simulated": 0,
            "accepted_first_pass": 0,
            "lead_seconds": { "p50": null, "p85": null },
            "review_seconds": { "p50": null, "p85": null },
            "throughput_per_week_hundredths": null,
            "unusable": [],
        })
    );
}

#[test]
fn an_accepted_mission_the_timeline_does_not_cover_is_listed_not_dropped() {
    let fixture = fixture();
    let (first, second) = {
        let daemon = Daemon::start(&fixture.root);
        let mut client = daemon.client();
        two_accepted(&mut client, &fixture.root)
    };
    // The daemon is stopped. A root older than the timeline migration holds
    // accepted missions whose earlier entries have no timeline row: the same
    // state is made here by removing the submissions of one mission.
    let layout = Layout::new(&fixture.root);
    let removed = Store::open(&layout.state())
        .unwrap()
        .connection()
        .execute(
            "DELETE FROM mission_events WHERE mission_id = ?1 AND kind = 'mission.result-submitted'",
            [second.as_str()],
        )
        .unwrap();
    assert_eq!(removed, 2);

    let daemon = Daemon::start(&fixture.root);
    let mut client = daemon.client();
    let read = measures(&mut client);
    assert_eq!(read["examined"], 2);
    assert_eq!(read["retained"], 1);
    assert_eq!(
        read["unusable"],
        json!([{ "mission": second, "code": "timeline.missing" }])
    );
    // The measures stand on the first mission alone.
    assert_eq!(read["accepted_first_pass"], 1);
    assert_eq!(read["simulated"], 1);
    assert_eq!(read["throughput_per_week_hundredths"], 100);
    assert_ne!(first, second);
}
