#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! wsd end to end: coordination primitives (docs/work-supervision/coordination-v0.md).
//!
//! The fixture duplicates the one of `daemon.rs` on purpose: two test files,
//! below the rule of three for a shared support module.

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
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap()
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
    fs::create_dir_all(repo.join("docs")).unwrap();
    host_git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("README"), "base\n").unwrap();
    fs::write(repo.join("docs/guide.md"), "guide\n").unwrap();
    host_git(&repo, &["add", "README", "docs/guide.md"]);
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
    fn start(root: &Path, fault: Option<&str>) -> Self {
        let mut command = Process::new(WSD);
        command
            .arg("--root")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(point) = fault {
            command.env("WSD_FAULT", point);
        }
        let child = command.spawn().unwrap();
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

    fn died(mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                use std::os::unix::process::ExitStatusExt as _;
                assert_eq!(status.signal(), Some(9), "wsd died by SIGKILL");
                return;
            }
            assert!(Instant::now() < deadline, "wsd did not die at its fault");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Writes `file`, commits it, exits 0.
fn writes(file: &str) -> String {
    format!("print starting\nwrite-file {file} the result\ncommit add {file}\nexit 0\n")
}

fn call(client: &mut Client, request: Value) -> Result<Value, ClientError> {
    client.request(&request)
}

fn code(outcome: Result<Value, ClientError>) -> String {
    outcome.unwrap_err().code().to_owned()
}

fn new_mission(client: &mut Client, brief: &str, criteria: &[&str]) -> String {
    let data = call(
        client,
        json!({
            "op": "mission.new", "title": "A mission", "repository": "sample",
            "brief": brief, "criteria": criteria,
            "max_duration_seconds": 60, "max_output_bytes": 1_048_576
        }),
    )
    .unwrap();
    data["mission"].as_str().unwrap().to_owned()
}

fn show(client: &mut Client, mission: &str) -> Value {
    call(client, json!({ "op": "mission.show", "mission": mission })).unwrap()
}

fn wait_state(client: &mut Client, mission: &str, states: &[&str]) {
    call(
        client,
        json!({ "op": "wait", "mission": mission, "states": states, "timeout_ms": 20_000 }),
    )
    .unwrap();
}

fn evidence(root: &Path) -> String {
    let store = BlobStore::open(&Layout::new(root).evidence()).unwrap();
    store.put(b"evidence").unwrap().to_hex()
}

/// Ready, run to its exit and submit; returns the `result.submit` response.
fn run_and_submit(client: &mut Client, root: &Path, mission: &str) -> Value {
    if show(client, mission)["state"] == "draft" {
        call(client, json!({ "op": "mission.ready", "mission": mission })).unwrap();
    }
    call(client, json!({ "op": "run", "mission": mission })).unwrap();
    wait_state(client, mission, &["exited"]);
    call(
        client,
        json!({ "op": "result.submit", "mission": mission, "evidence_digest": evidence(root), "summary": "done" }),
    )
    .unwrap()
}

fn accept(client: &mut Client, mission: &str) -> Result<Value, ClientError> {
    call(
        client,
        json!({ "op": "decide", "mission": mission, "decision": "accept", "reason": "ok" }),
    )
}

/// Waits until no check of `mission` runs.
fn wait_checks(client: &mut Client, mission: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while show(client, mission)["checks_running"] == true {
        assert!(Instant::now() < deadline, "checks did not finish");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn assert_journal_and_projection(root: &Path) {
    let layout = Layout::new(root);
    let file = fs::File::open(layout.journal()).unwrap();
    let outcome = work_supervision_journal_verifier::verify(file).unwrap();
    assert!(
        matches!(
            outcome,
            work_supervision_journal_verifier::Outcome::Valid { .. }
        ),
        "{outcome:?}"
    );
    let live = Store::open_read_only(&layout.state())
        .unwrap()
        .dump()
        .unwrap();
    let rebuilt_path = root.join("rebuilt.sqlite");
    let _ = fs::remove_file(&rebuilt_path);
    let rebuilt = Store::rebuild(
        &layout.journal(),
        &layout.blob_store().unwrap(),
        &rebuilt_path,
    )
    .unwrap();
    assert_eq!(live, rebuilt.dump().unwrap());
}

#[test]
fn a_dependency_holds_the_run_until_it_is_accepted() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let first = new_mission(&mut client, &writes("a.txt"), &[]);
    let second = new_mission(&mut client, &writes("b.txt"), &[]);
    call(
        &mut client,
        json!({ "op": "mission.depend", "mission": second, "on": first }),
    )
    .unwrap();
    // A cycle is refused.
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "mission.depend", "mission": first, "on": second })
        )),
        "dependency.cycle"
    );
    call(
        &mut client,
        json!({ "op": "mission.ready", "mission": second }),
    )
    .unwrap();
    assert_eq!(
        code(call(&mut client, json!({ "op": "run", "mission": second }))),
        "dependency.pending"
    );
    assert_eq!(
        show(&mut client, &second)["blockers"],
        json!(["dependency.pending"])
    );
    run_and_submit(&mut client, &fixture.root, &first);
    accept(&mut client, &first).unwrap();
    assert_eq!(show(&mut client, &second)["blockers"], json!([]));
    call(&mut client, json!({ "op": "run", "mission": second })).unwrap();
    // Declarations are frozen with the brief.
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "mission.scope", "mission": second, "paths": ["src"] })
        )),
        "mission.not_draft"
    );
}

#[test]
fn an_abandoned_dependency_is_unsatisfiable() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let first = new_mission(&mut client, &writes("a.txt"), &[]);
    let second = new_mission(&mut client, &writes("b.txt"), &[]);
    call(
        &mut client,
        json!({ "op": "mission.depend", "mission": second, "on": first }),
    )
    .unwrap();
    call(
        &mut client,
        json!({ "op": "decide", "mission": first, "decision": "abandon", "reason": "dropped" }),
    )
    .unwrap();
    call(
        &mut client,
        json!({ "op": "mission.ready", "mission": second }),
    )
    .unwrap();
    assert_eq!(
        code(call(&mut client, json!({ "op": "run", "mission": second }))),
        "dependency.unsatisfiable"
    );
}

#[test]
fn overlapping_scopes_conflict_and_disjoint_ones_run_side_by_side() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let holder = new_mission(&mut client, &writes("src/a.txt"), &[]);
    let nested = new_mission(&mut client, &writes("src/x/b.txt"), &[]);
    let disjoint = new_mission(&mut client, &writes("docs/c.md"), &[]);
    let unscoped = new_mission(&mut client, &writes("d.txt"), &[]);
    for (mission, paths) in [
        (&holder, json!(["src"])),
        (&nested, json!(["src/x"])),
        (&disjoint, json!(["docs"])),
    ] {
        call(
            &mut client,
            json!({ "op": "mission.scope", "mission": mission, "paths": paths }),
        )
        .unwrap();
    }
    for mission in [&holder, &nested, &disjoint, &unscoped] {
        call(
            &mut client,
            json!({ "op": "mission.ready", "mission": mission }),
        )
        .unwrap();
    }
    call(&mut client, json!({ "op": "run", "mission": holder })).unwrap();
    assert_eq!(
        code(call(&mut client, json!({ "op": "run", "mission": nested }))),
        "scope.conflict"
    );
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "run", "mission": unscoped })
        )),
        "scope.conflict"
    );
    call(&mut client, json!({ "op": "run", "mission": disjoint })).unwrap();
    wait_state(&mut client, &holder, &["exited"]);
    wait_state(&mut client, &disjoint, &["exited"]);
}

#[test]
fn an_open_request_blocks_run_and_accept_until_the_owner_answers() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let mission = new_mission(&mut client, &writes("a.txt"), &[]);
    let session = call(
        &mut client,
        json!({ "op": "session.register", "harness": "claude-code", "repository": "sample",
                "mission": mission, "label": "parser work" }),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let agent = format!("session:{session}");
    let options = json!([
        { "label": "SQLite", "consequence": "No server.", "reversibility": "reversible" },
        { "label": "PostgreSQL", "consequence": "A server to run.", "reversibility": "costly" }
    ]);
    let opened = call(
        &mut client,
        json!({ "op": "request.open", "actor": agent, "mission": mission,
                "question": "Which storage?", "options": options, "recommended": 0 }),
    )
    .unwrap();
    let request = opened["request"].as_str().unwrap().to_owned();
    call(
        &mut client,
        json!({ "op": "mission.ready", "mission": mission }),
    )
    .unwrap();
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "run", "mission": mission })
        )),
        "request.pending"
    );
    let pending = call(&mut client, json!({ "op": "decisions.pending" })).unwrap();
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["options"][1]["reversibility"], "costly");
    // The session cannot answer its own question.
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "request.answer", "actor": agent, "request": request, "choice": 0, "reason": "self" })
        )),
        "actor.owner_only"
    );
    call(
        &mut client,
        json!({ "op": "request.answer", "request": request, "choice": 0, "reason": "v0 is single user" }),
    )
    .unwrap();
    run_and_submit(&mut client, &fixture.root, &mission);
    let second = call(
        &mut client,
        json!({ "op": "request.open", "actor": agent, "mission": mission,
                "question": "Ship now?", "options": options }),
    )
    .unwrap()["request"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(code(accept(&mut client, &mission)), "request.pending");
    call(
        &mut client,
        json!({ "op": "request.withdraw", "actor": agent, "request": second, "reason": "not needed" }),
    )
    .unwrap();
    accept(&mut client, &mission).unwrap();
    // A request cannot be opened on a terminal mission.
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "request.open", "mission": mission, "question": "Late?", "options": options })
        )),
        "request.mission_closed"
    );
}

#[test]
fn a_session_actor_cannot_take_owner_decisions() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let mission = new_mission(&mut client, &writes("a.txt"), &[]);
    let session = call(
        &mut client,
        json!({ "op": "session.register", "harness": "codex", "label": "codex run" }),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let agent = format!("session:{session}");
    for request in [
        json!({ "op": "decide", "actor": agent, "mission": mission, "decision": "abandon", "reason": "x" }),
        json!({ "op": "mission.ready", "actor": agent, "mission": mission }),
        json!({ "op": "run", "actor": agent, "mission": mission }),
        json!({ "op": "mission.new", "actor": agent, "repository": "sample", "title": "t", "brief": "print x\n" }),
        json!({ "op": "mission.scope", "actor": agent, "mission": mission, "paths": ["src"] }),
        json!({ "op": "check.run", "actor": agent, "mission": mission }),
        json!({ "op": "note", "actor": agent, "mission": mission, "text": "n" }),
    ] {
        assert_eq!(
            code(call(&mut client, request.clone())),
            "actor.owner_only",
            "{request}"
        );
    }
    // A malformed actor is refused, never read as the owner.
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "mission.ready", "actor": "Owner", "mission": mission })
        )),
        "request.field_invalid"
    );
    // A session reports for itself only.
    let other = call(
        &mut client,
        json!({ "op": "session.register", "harness": "pi", "label": "pi run" }),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "session.report", "actor": agent, "session": other, "state": "blocked" })
        )),
        "actor.not_its_own"
    );
    assert_eq!(show(&mut client, &mission)["state"], "draft");
}

#[test]
fn files_outside_the_scope_block_the_acceptance() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let mission = new_mission(&mut client, &writes("result.txt"), &[]);
    call(
        &mut client,
        json!({ "op": "mission.scope", "mission": mission, "paths": ["docs"] }),
    )
    .unwrap();
    let submitted = run_and_submit(&mut client, &fixture.root, &mission);
    assert_eq!(submitted["scope_outside"], 1);
    assert_eq!(code(accept(&mut client, &mission)), "scope.violated");
    let report = call(
        &mut client,
        json!({ "op": "mission.report", "mission": mission }),
    )
    .unwrap();
    assert_eq!(
        report["evidence"]["scope_check"]["outside"],
        json!(["result.txt"])
    );
    assert!(
        report["gaps"]
            .as_array()
            .unwrap()
            .contains(&json!("scope.violated"))
    );
    // The owner can still reject or abandon.
    call(
        &mut client,
        json!({ "op": "decide", "mission": mission, "decision": "abandon", "reason": "out of scope" }),
    )
    .unwrap();
}

#[test]
fn checks_verify_criteria_at_the_submitted_commit() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let mission = new_mission(
        &mut client,
        &writes("result.txt"),
        &["result.txt exists", "the second check passes"],
    );
    call(
        &mut client,
        json!({ "op": "mission.check", "mission": mission, "criterion": 0,
                "argv": ["/bin/sh", "-c", "test -f result.txt"] }),
    )
    .unwrap();
    call(
        &mut client,
        json!({ "op": "mission.check", "mission": mission, "criterion": 1,
                "argv": ["/bin/sh", "-c", "exit 3"] }),
    )
    .unwrap();
    assert_eq!(
        code(call(
            &mut client,
            json!({ "op": "mission.check", "mission": mission, "criterion": 2, "argv": ["true"] })
        )),
        "check.criterion_unknown"
    );
    run_and_submit(&mut client, &fixture.root, &mission);
    assert_eq!(code(accept(&mut client, &mission)), "criteria.unverified");
    let started = call(
        &mut client,
        json!({ "op": "check.run", "mission": mission }),
    )
    .unwrap();
    assert_eq!(started["checks"], 2);
    wait_checks(&mut client, &mission);
    let shown = show(&mut client, &mission);
    let runs = shown["check_runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(
        (runs[0]["passed"].clone(), runs[0]["exit_code"].clone()),
        (json!(true), json!(0))
    );
    assert_eq!(
        (runs[1]["passed"].clone(), runs[1]["exit_code"].clone()),
        (json!(false), json!(3))
    );
    // One criterion still fails: no acceptance.
    assert_eq!(code(accept(&mut client, &mission)), "criteria.unverified");
    // Its worktree covers the whole repository until it is released, so a
    // second unscoped mission would conflict with it.
    call(
        &mut client,
        json!({ "op": "decide", "mission": mission, "decision": "abandon", "reason": "check fails" }),
    )
    .unwrap();

    let passing = new_mission(&mut client, &writes("ok.txt"), &["ok.txt exists"]);
    call(
        &mut client,
        json!({ "op": "mission.check", "mission": passing, "criterion": 0,
                "argv": ["/bin/sh", "-c", "test -f ok.txt"] }),
    )
    .unwrap();
    run_and_submit(&mut client, &fixture.root, &passing);
    call(
        &mut client,
        json!({ "op": "check.run", "mission": passing }),
    )
    .unwrap();
    wait_checks(&mut client, &passing);
    accept(&mut client, &passing).unwrap();
    let report = call(
        &mut client,
        json!({ "op": "mission.report", "mission": passing }),
    )
    .unwrap();
    assert_eq!(report["schema"], "libre-ai.work-supervision.report.v0");
    assert_eq!(
        report["intent"]["criteria"][0]["last_execution"]["passed"],
        true
    );
    assert_eq!(
        report["intent"]["criteria"][0]["last_execution"]["at_submitted_commit"],
        true
    );
    let gaps = report["gaps"].as_array().unwrap();
    assert!(gaps.contains(&json!("simulation")));
    assert!(!gaps.contains(&json!("criteria.unverified")));
    let kinds: Vec<&str> = report["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    for kind in [
        "mission.created",
        "check.declared",
        "mission.result-submitted",
        "scope.checked",
        "check.started",
        "check.finished",
        "mission.accepted",
    ] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    let position = |kind: &str| kinds.iter().position(|seen| *seen == kind).unwrap();
    assert!(position("check.finished") < position("mission.accepted"));
    assert_journal_and_projection(&fixture.root);
}

#[test]
fn an_idea_is_promoted_once_even_across_a_crash() {
    let fixture = fixture();
    let capture = |client: &mut Client| -> String {
        let idea = call(
            client,
            json!({ "op": "idea.capture", "text": "memoise the tokenizer" }),
        )
        .unwrap()["idea"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            client,
            json!({ "op": "idea.qualify", "idea": idea, "repository": "sample" }),
        )
        .unwrap();
        idea
    };
    let promote = |idea: &str| json!({ "op": "idea.promote", "idea": idea, "title": "Memoise" });
    let missions = |client: &mut Client| {
        call(client, json!({ "op": "mission.list" }))
            .unwrap()
            .as_array()
            .unwrap()
            .len()
    };
    let idea_state = |client: &mut Client, idea: &str| {
        call(client, json!({ "op": "idea.list" }))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == idea)
            .unwrap()["state"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // Crash after the intent, before the mission: the promotion is aborted.
    let first = {
        let daemon = Daemon::start(&fixture.root, Some("idea-promotion-intended"));
        let mut client = daemon.client();
        let idea = capture(&mut client);
        let _ = call(&mut client, promote(&idea));
        daemon.died();
        idea
    };
    {
        let daemon = Daemon::start(&fixture.root, None);
        let mut client = daemon.client();
        assert_eq!(idea_state(&mut client, &first), "qualified");
        assert_eq!(missions(&mut client), 0);
    }
    // Crash after the mission, before the confirmation: the promotion is confirmed.
    let second = {
        let daemon = Daemon::start(&fixture.root, Some("idea-mission-created"));
        let mut client = daemon.client();
        let _ = call(&mut client, promote(&first));
        daemon.died();
        first.clone()
    };
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    assert_eq!(idea_state(&mut client, &second), "promoted");
    assert_eq!(missions(&mut client), 1);
    assert_eq!(
        code(call(&mut client, promote(&second))),
        "idea.transition_forbidden"
    );
    // A promotion without a repository is refused before any intent.
    let loose = call(
        &mut client,
        json!({ "op": "idea.capture", "text": "later" }),
    )
    .unwrap()["idea"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        code(call(&mut client, promote(&loose))),
        "idea.repository_required"
    );
    assert_eq!(idea_state(&mut client, &loose), "captured");
    drop(daemon);
    assert_journal_and_projection(&fixture.root);
}

#[test]
fn a_check_left_running_by_a_crash_is_interrupted_and_never_passes() {
    let fixture = fixture();
    let mission = {
        let daemon = Daemon::start(&fixture.root, Some("check-started"));
        let mut client = daemon.client();
        let mission = new_mission(&mut client, &writes("r.txt"), &["r.txt exists"]);
        call(
            &mut client,
            json!({ "op": "mission.check", "mission": mission, "criterion": 0,
                    "argv": ["/bin/sh", "-c", "test -f r.txt"] }),
        )
        .unwrap();
        run_and_submit(&mut client, &fixture.root, &mission);
        call(
            &mut client,
            json!({ "op": "check.run", "mission": mission }),
        )
        .unwrap();
        daemon.died();
        mission
    };
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let shown = show(&mut client, &mission);
    assert_eq!(shown["check_runs"][0]["state"], "interrupted");
    assert_eq!(code(accept(&mut client, &mission)), "criteria.unverified");
    call(
        &mut client,
        json!({ "op": "check.run", "mission": mission }),
    )
    .unwrap();
    wait_checks(&mut client, &mission);
    accept(&mut client, &mission).unwrap();
    drop(daemon);
    assert_journal_and_projection(&fixture.root);
}

#[test]
fn a_hooked_session_is_found_by_its_harness_identifier_digest_only() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let digest = "c".repeat(64);
    let session = call(
        &mut client,
        json!({ "op": "session.register", "harness": "claude-code", "label": "hooked",
                "external_digest": digest }),
    )
    .unwrap()["session"]
        .clone();
    let found = call(
        &mut client,
        json!({ "op": "session.find", "harness": "claude-code", "external_digest": digest }),
    )
    .unwrap();
    assert_eq!(found["session"], session);
    let other_harness = call(
        &mut client,
        json!({ "op": "session.find", "harness": "codex", "external_digest": digest }),
    )
    .unwrap();
    assert_eq!(other_harness["session"], Value::Null);
    let listed = call(&mut client, json!({ "op": "session.list" })).unwrap();
    assert_eq!(listed[0]["silent"], false);
    assert_eq!(listed[0]["state"], "active");
}

/// Git on the host `PATH`, as an absolute path a check can be given.
fn host_git_program() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .unwrap()
}

/// Runs git with raw byte arguments in `dir` (paths that are not UTF-8).
fn raw_git(dir: &Path, args: &[&[u8]]) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let output = Process::new("git")
        .arg("-C")
        .arg(dir)
        .args(args.iter().map(|arg| std::ffi::OsStr::from_bytes(arg)))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn a_path_that_is_not_utf8_never_blocks_submission_restart_or_acceptance() {
    let fixture = fixture();
    let mission = {
        let daemon = Daemon::start(&fixture.root, None);
        let mut client = daemon.client();
        let mission = new_mission(&mut client, &writes("a.txt"), &[]);
        call(
            &mut client,
            json!({ "op": "mission.ready", "mission": mission }),
        )
        .unwrap();
        call(&mut client, json!({ "op": "run", "mission": mission })).unwrap();
        wait_state(&mut client, &mission, &["exited"]);
        // The agent's commit adds a file whose name is not UTF-8 (plumbing,
        // since the file system may refuse the name), kept out of the
        // working tree so the worktree stays clean.
        let worktree = fixture.root.join("worktrees").join(&mission);
        let blob = raw_git(&worktree, &[b"hash-object", b"-w", b"/dev/null"]);
        let entry = format!("100644,{blob},").into_bytes();
        let mut entry = entry;
        entry.extend_from_slice(b"bad\xffname");
        raw_git(
            &worktree,
            &[b"update-index", b"--add", b"--cacheinfo", &entry],
        );
        raw_git(
            &worktree,
            &[
                b"-c",
                b"user.name=a",
                b"-c",
                b"user.email=a@example.invalid",
                b"commit",
                b"-q",
                b"-m",
                b"odd name",
            ],
        );
        raw_git(
            &worktree,
            &[b"update-index", b"--skip-worktree", b"bad\xffname"],
        );
        let submitted = call(
            &mut client,
            json!({ "op": "result.submit", "mission": mission, "evidence_digest": evidence(&fixture.root), "summary": "done" }),
        )
        .unwrap();
        // Two changed paths, none outside an undeclared scope, no failure.
        assert_eq!(submitted["scope_outside"], 0);
        assert!(submitted.get("scope_check_failed").is_none(), "{submitted}");
        mission
    };
    // The daemon restarts, and the mission can be accepted.
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let report = call(
        &mut client,
        json!({ "op": "mission.report", "mission": mission }),
    )
    .unwrap();
    assert_eq!(report["evidence"]["scope_check"]["changed"], 2);
    accept(&mut client, &mission).unwrap();
}

#[test]
fn a_commit_made_after_submission_blocks_acceptance_and_stops_the_checks() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let mission = new_mission(
        &mut client,
        &writes("r.txt"),
        &["a check that commits", "a check that would pass"],
    );
    let git = host_git_program();
    call(
        &mut client,
        json!({ "op": "mission.check", "mission": mission, "criterion": 0,
                "argv": [git, "-c", "user.name=c", "-c", "user.email=c@example.invalid",
                         "commit", "-q", "--allow-empty", "-m", "injected after submission"] }),
    )
    .unwrap();
    call(
        &mut client,
        json!({ "op": "mission.check", "mission": mission, "criterion": 1,
                "argv": ["/bin/sh", "-c", "exit 0"] }),
    )
    .unwrap();
    run_and_submit(&mut client, &fixture.root, &mission);
    call(
        &mut client,
        json!({ "op": "check.run", "mission": mission }),
    )
    .unwrap();
    wait_checks(&mut client, &mission);
    // The first check moved HEAD: the second never ran on a moved tree.
    let runs = show(&mut client, &mission)["check_runs"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(runs, 1);
    assert_eq!(code(accept(&mut client, &mission)), "worktree.head_moved");
    let report = call(
        &mut client,
        json!({ "op": "mission.report", "mission": mission }),
    )
    .unwrap();
    assert_eq!(report["intent"]["criteria"][1]["verified"], false);
    assert!(
        report["gaps"]
            .as_array()
            .unwrap()
            .contains(&json!("criteria.unverified"))
    );
}
