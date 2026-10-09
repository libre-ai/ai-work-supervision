#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! wsd end to end with the fake agent (tranche T6).

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use work_supervision_daemon::{Client, ClientError, init_root};
use work_supervision_store::{BlobStore, Layout, Store};

const WSD: &str = env!("CARGO_BIN_EXE_wsd");

/// The fake agent is a binary of another package: build it once, next to wsd.
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
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let root = base.join("root");
    let repo = base.join("repo");
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
    init_root(&root).unwrap();
    let config = format!(
        "# test configuration\n[repositories]\nsample = \"{}\"\n\n[executor]\nprofile = \"fake\"\nfake_agent = \"{}\"\n\n[runs]\nidle_after_ms = 300\ngrace_ms = 500\n",
        repo.display(),
        fake_agent().display()
    );
    fs::write(root.join("config.toml"), config).unwrap();
    Fixture {
        _dir: dir,
        root,
        repo,
    }
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

    fn kill(mut self) -> String {
        let _ = self.child.kill();
        let output = self.child.wait_with_output().unwrap();
        String::from_utf8(output.stderr).unwrap()
    }

    /// Waits for a daemon that killed itself at an injected fault.
    fn died(mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                use std::os::unix::process::ExitStatusExt as _;
                assert_eq!(status.signal(), Some(9), "wsd died by SIGKILL");
                break;
            }
            assert!(Instant::now() < deadline, "wsd did not die at its fault");
            std::thread::sleep(Duration::from_millis(20));
        }
        String::from_utf8(self.child.wait_with_output().unwrap().stderr).unwrap()
    }
}

const SCENARIO: &str =
    "print starting\nwrite-file result.txt the result\ncommit add the result\nprint done\nexit 0\n";

fn call(client: &mut Client, request: Value) -> Result<Value, ClientError> {
    client.request(&request)
}

fn new_mission(client: &mut Client, brief: &str) -> Result<String, ClientError> {
    let data = call(
        client,
        json!({
            "op": "mission.new", "title": "Write the result", "repository": "sample",
            "brief": brief, "criteria": ["result.txt is committed"],
            "max_duration_seconds": 60, "max_output_bytes": 1_048_576
        }),
    )?;
    Ok(data["mission"].as_str().unwrap().to_owned())
}

fn state(client: &mut Client, mission: &str) -> Result<String, ClientError> {
    let data = call(client, json!({ "op": "mission.show", "mission": mission }))?;
    Ok(data["state"].as_str().unwrap().to_owned())
}

fn wait_state(client: &mut Client, mission: &str, states: &[&str]) -> Result<String, ClientError> {
    let data = call(
        client,
        json!({ "op": "wait", "mission": mission, "states": states, "timeout_ms": 20_000 }),
    )?;
    Ok(data["state"].as_str().unwrap().to_owned())
}

fn evidence(root: &Path) -> String {
    let store = BlobStore::open(&Layout::new(root).evidence()).unwrap();
    store.put(b"cargo test: 12 passed").unwrap().to_hex()
}

/// Drives the mission one step from wherever it is, until it is accepted.
fn advance(client: &mut Client, root: &Path, mission: &str) -> Result<bool, ClientError> {
    match state(client, mission)?.as_str() {
        "draft" => {
            call(client, json!({ "op": "mission.ready", "mission": mission }))?;
        }
        "ready" | "provisioned" | "rejected" => {
            call(client, json!({ "op": "run", "mission": mission }))?;
        }
        "running" | "waiting-input" => {
            wait_state(client, mission, &["exited"])?;
        }
        "exited" => {
            call(
                client,
                json!({ "op": "result.submit", "mission": mission, "evidence_digest": evidence(root), "summary": "result written" }),
            )?;
        }
        "result-submitted" => {
            let accepted = call(
                client,
                json!({ "op": "decide", "mission": mission, "decision": "accept", "reason": "criteria met" }),
            );
            if let Err(error) = accepted {
                // An interrupted run may leave uncommitted work: send it back.
                if error.code() != "worktree.dirty" {
                    return Err(error);
                }
                call(
                    client,
                    json!({ "op": "decide", "mission": mission, "decision": "reject", "reason": "rerun" }),
                )?;
            }
        }
        "accepted" => return Ok(true),
        other => panic!("unexpected state {other}"),
    }
    Ok(false)
}

fn assert_clean_end(fixture: &Fixture, mission: &str) {
    let layout = Layout::new(&fixture.root);
    // Independent verifier, outside the projection.
    let file = fs::File::open(layout.journal()).unwrap();
    let outcome = work_supervision_journal_verifier::verify(file).unwrap();
    assert!(
        matches!(
            outcome,
            work_supervision_journal_verifier::Outcome::Valid { .. }
        ),
        "{outcome:?}"
    );
    // The projection equals its reconstruction.
    let live = Store::open_read_only(&layout.state())
        .unwrap()
        .dump()
        .unwrap();
    let rebuilt_path = fixture.root.join("rebuilt.sqlite");
    let _ = fs::remove_file(&rebuilt_path);
    let rebuilt = Store::rebuild(
        &layout.journal(),
        &layout.blob_store().unwrap(),
        &rebuilt_path,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(live).unwrap(),
        String::from_utf8(rebuilt.dump().unwrap()).unwrap()
    );
    // No orphan worktree: only the main one is registered, worktrees/ is empty,
    // and the accepted branch holds the committed result.
    let listing = host_git(&fixture.repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        listing
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1,
        "{listing}"
    );
    let left: Vec<_> = fs::read_dir(layout.worktrees()).unwrap().collect();
    assert!(left.is_empty());
    let files = host_git(
        &fixture.repo,
        &["ls-tree", "--name-only", &format!("ws/{mission}")],
    );
    assert!(files.contains("result.txt"), "{files}");
}

#[test]
fn a_mission_runs_end_to_end_with_the_fake_agent() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let mission = new_mission(&mut client, SCENARIO).unwrap();
    assert_eq!(state(&mut client, &mission).unwrap(), "draft");
    let mut steps = 0;
    while !advance(&mut client, &fixture.root, &mission).unwrap() {
        steps += 1;
        assert!(steps < 20);
    }
    let shown = call(
        &mut client,
        json!({ "op": "mission.show", "mission": mission }),
    )
    .unwrap();
    assert_eq!(shown["worktree"]["state"], "removed");
    assert_eq!(shown["runs"][0]["state"], "exited");
    assert_eq!(shown["runs"][0]["exit_code"], 0);
    let stderr = daemon.kill();
    for secret in [
        "Write the result",
        "result.txt is committed",
        "the result",
        "result written",
        "criteria met",
    ] {
        assert!(
            !stderr.contains(secret),
            "wsd log leaks {secret:?}: {stderr}"
        );
    }
    let journal = fs::read_to_string(Layout::new(&fixture.root).journal()).unwrap();
    for secret in [
        "Write the result",
        "result.txt is committed",
        "result written",
        "criteria met",
        "print starting",
    ] {
        assert!(!journal.contains(secret), "journal leaks {secret:?}");
    }
    assert_clean_end(&fixture, &mission);
}

#[test]
fn input_moves_a_waiting_mission_back_to_running_and_is_journalled_by_length_and_digest() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let mission = new_mission(&mut client, "print what now?\nread-line\nexit 0\n").unwrap();
    call(
        &mut client,
        json!({ "op": "mission.ready", "mission": mission }),
    )
    .unwrap();
    call(&mut client, json!({ "op": "run", "mission": mission })).unwrap();
    assert_eq!(
        wait_state(&mut client, &mission, &["waiting-input"]).unwrap(),
        "waiting-input"
    );
    let sent = call(
        &mut client,
        json!({ "op": "send", "mission": mission, "text": "go ahead\n" }),
    )
    .unwrap();
    assert_eq!(sent["bytes"], 9);
    assert_eq!(
        wait_state(&mut client, &mission, &["exited"]).unwrap(),
        "exited"
    );
    let log_dir = Layout::new(&fixture.root).runs();
    let runs: Vec<_> = fs::read_dir(&log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let log = fs::read_to_string(runs[0].join("pty.log")).unwrap();
    assert!(log.contains(
        &format!("received {}", "go ahead".bytes().map(|b| format!("{b:02x}")).collect::<String>())
    ));
    let journal = fs::read_to_string(Layout::new(&fixture.root).journal()).unwrap();
    assert!(journal.contains("\"kind\":\"run.input\""));
    assert!(journal.contains("\"kind\":\"mission.input-awaited\""));
    assert!(journal.contains("\"kind\":\"mission.input-resumed\""));
    assert!(!journal.contains("go ahead"));
    daemon.kill();
}

#[test]
fn recovery_is_green_after_a_kill_9_at_every_injection_point() {
    let points = work_supervision_daemon::FAULT_POINTS;
    assert!(points.len() >= 12, "{} points", points.len());
    for point in points {
        let fixture = fixture();
        let daemon = Daemon::start(&fixture.root, Some(point));
        let mut client = daemon.client();
        let mut mission = None;
        // Drive until the daemon dies at its fault.
        let died = loop {
            let step = match &mission {
                None => new_mission(&mut client, SCENARIO).map(|id| {
                    mission = Some(id);
                    false
                }),
                Some(id) => advance(&mut client, &fixture.root, id),
            };
            match step {
                Ok(true) => break false,
                Ok(false) => {}
                Err(_) => break true,
            }
        };
        assert!(died, "{point}: the fault was never reached");
        daemon.died();
        // Restart without fault: recovery, then finish the mission.
        let daemon = Daemon::start(&fixture.root, None);
        let mut client = daemon.client();
        let mission = match mission {
            Some(id) => id,
            None => {
                let listed = call(&mut client, json!({ "op": "mission.list" })).unwrap();
                listed[0]["id"].as_str().unwrap().to_owned()
            }
        };
        let mut steps = 0;
        while !advance(&mut client, &fixture.root, &mission)
            .unwrap_or_else(|error| panic!("{point}: {error}"))
        {
            steps += 1;
            assert!(
                steps < 30,
                "{point}: stuck at {}",
                state(&mut client, &mission).unwrap()
            );
        }
        daemon.kill();
        assert_clean_end(&fixture, &mission);
    }
}

#[test]
fn the_socket_is_private_to_its_owner() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let socket = fs::metadata(&daemon.socket).unwrap().permissions().mode() & 0o777;
    let directory = fs::metadata(fixture.root.join("run"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket, 0o600);
    assert_eq!(directory, 0o700);
    assert_eq!(
        fs::metadata(&fixture.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    daemon.kill();
}

#[test]
fn a_peer_of_another_uid_is_refused() {
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    let peer = work_supervision_daemon::peer_uid(&ours).unwrap();
    assert_eq!(peer, rustix::process::geteuid().as_raw());
    assert!(work_supervision_daemon::authorize_peer(peer, peer).is_ok());
    let error = work_supervision_daemon::authorize_peer(peer.wrapping_add(1), peer).unwrap_err();
    assert_eq!(error.code(), "socket.peer_refused");
    drop(theirs);
}

#[test]
fn a_real_agent_profile_keeps_the_daemon_from_starting() {
    let fixture = fixture();
    let config = fs::read_to_string(fixture.root.join("config.toml")).unwrap();
    fs::write(
        fixture.root.join("config.toml"),
        config.replace("profile = \"fake\"", "profile = \"claude\""),
    )
    .unwrap();
    let output = Process::new(WSD)
        .arg("--root")
        .arg(&fixture.root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("agent.real_forbidden_until_c0"));
    assert!(!fixture.root.join("run").join("wsd.sock").exists());
}

#[test]
fn a_root_inside_a_git_repository_or_relative_is_refused() {
    let fixture = fixture();
    assert_eq!(
        init_root(&fixture.repo.join("instance"))
            .unwrap_err()
            .code(),
        "root.inside_repository"
    );
    assert_eq!(
        init_root(Path::new("relative/root")).unwrap_err().code(),
        "root.not_absolute"
    );
    assert_eq!(init_root(&fixture.root).unwrap_err().code(), "root.exists");
}

#[test]
fn malformed_requests_and_unknown_missions_are_refused_with_codes() {
    let fixture = fixture();
    let daemon = Daemon::start(&fixture.root, None);
    let mut client = daemon.client();
    let error = call(&mut client, json!({ "op": "teleport" })).unwrap_err();
    assert_eq!(error.code(), "request.unknown_op");
    let error = call(
        &mut client,
        json!({ "op": "mission.show", "mission": "0".repeat(32) }),
    )
    .unwrap_err();
    assert_eq!(error.code(), "mission.not_found");
    let error = call(
        &mut client,
        json!({ "op": "mission.new", "title": "x", "repository": "elsewhere", "brief": "exit 0" }),
    )
    .unwrap_err();
    assert_eq!(error.code(), "config.repository_unknown");
    daemon.kill();
}
