#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `ws` against a real `wsd` and the fake agent.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Output, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::Value;

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

fn git(dir: &Path, args: &[&str]) -> String {
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
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
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
    repo: PathBuf,
    base: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let root = base.join("root");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    let init = Process::new(WS)
        .args(["init", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
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
        repo,
        base,
    }
}

fn start(root: &Path) -> Child {
    let child = Process::new(sibling("wsd"))
        .arg("--root")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ws(root, &["status"]).status.success() {
        assert!(Instant::now() < deadline, "wsd did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

#[test]
fn a_mission_goes_from_creation_to_acceptance_through_the_command_line() {
    let setup = setup();
    let mut daemon = start(&setup.root);
    let brief = setup.base.join("brief.txt");
    fs::write(&brief, "write-file out.txt done\ncommit add out\nexit 0\n").unwrap();
    let created = ok(
        &setup.root,
        &[
            "mission",
            "new",
            "--repository",
            "sample",
            "--title",
            "Write out",
            "--brief",
            brief.to_str().unwrap(),
            "--criterion",
            "out.txt committed",
        ],
    );
    let id = created["mission"].as_str().unwrap().to_owned();
    assert_eq!(
        ok(&setup.root, &["mission", "ready", &id])["state"],
        "ready"
    );
    ok(&setup.root, &["run", &id]);
    assert_eq!(ok(&setup.root, &["wait", &id, "exited"])["state"], "exited");
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
            "out.txt written",
        ],
    );
    assert_eq!(
        ok(&setup.root, &["decide", &id, "accept", "--reason", "ok"])["state"],
        "accepted"
    );
    let list = ok(&setup.root, &["mission", "list"]);
    assert_eq!(list[0]["state"], "accepted");
    let shown = ok(&setup.root, &["mission", "show", &id]);
    assert_eq!(shown["criteria"][0], "out.txt committed");
    assert_eq!(shown["worktree"]["state"], "removed");
    assert!(
        git(
            &setup.repo,
            &["ls-tree", "--name-only", &format!("ws/{id}")]
        )
        .contains("out.txt")
    );

    // rebuild refuses while the daemon runs, then rebuilds an equal base.
    let refused = ws(&setup.root, &["rebuild", "--check"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("daemon.running"));
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let checked = ok(&setup.root, &["rebuild", "--check"]);
    assert_eq!(checked["equal"], true);
    let rebuilt = ok(&setup.root, &["rebuild"]);
    assert_eq!(rebuilt["equal"], true);
    assert!(setup.root.join("state.sqlite").exists());

    let verified = ok(&setup.root, &["journal", "verify"]);
    assert!(verified["entries"].as_u64().unwrap() >= 10);
    let head = ok(&setup.root, &["journal", "head"]);
    assert_eq!(head["seq"], verified["entries"]);
    assert_eq!(head["digest"].as_str().unwrap().len(), 64);
}

#[test]
fn the_journal_check_is_red_on_an_altered_byte_and_unreadable_on_a_missing_root() {
    let setup = setup();
    let mut daemon = start(&setup.root);
    let brief = setup.base.join("brief.txt");
    fs::write(&brief, "exit 0\n").unwrap();
    ok(
        &setup.root,
        &[
            "mission",
            "new",
            "--repository",
            "sample",
            "--title",
            "T",
            "--brief",
            brief.to_str().unwrap(),
        ],
    );
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let journal = setup.root.join("journal").join("journal.v0.jsonl");
    let text = fs::read_to_string(&journal).unwrap();
    fs::write(&journal, text.replacen("\"draft\"", "\"ready\"", 1)).unwrap();
    let output = ws(&setup.root, &["journal", "verify"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("line 1"));
    let output = ws(&setup.base.join("absent"), &["journal", "verify"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn usage_errors_and_a_missing_root_are_refused() {
    let output = Process::new(WS)
        .args(["mission", "list"])
        .env_remove("WS_ROOT")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let output = Process::new(WS)
        .args(["--root", "/tmp/x", "teleport"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let setup = setup();
    let output = ws(&setup.root, &["status"]);
    assert!(!output.status.success(), "no daemon: refused");
    assert!(String::from_utf8_lossy(&output.stderr).contains("transport.connect"));
}

#[test]
fn journal_verify_checks_the_anchor_kept_outside_the_root() {
    let setup = setup();
    let anchor = setup.base.join("private-anchors.v0");
    let config = fs::read_to_string(setup.root.join("config.toml")).unwrap();
    fs::write(
        setup.root.join("config.toml"),
        format!("{config}[anchor]\npath = \"{}\"\n", anchor.display()),
    )
    .unwrap();
    let mut daemon = start(&setup.root);
    let brief = setup.base.join("brief.txt");
    fs::write(&brief, "exit 0\n").unwrap();
    for title in ["one", "two"] {
        ok(
            &setup.root,
            &[
                "mission",
                "new",
                "--repository",
                "sample",
                "--title",
                title,
                "--brief",
                brief.to_str().unwrap(),
            ],
        );
    }
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let verified = ok(&setup.root, &["journal", "verify"]);
    assert_eq!(verified["anchor"]["status"], "matched");
    assert_eq!(verified["anchor"]["seq"], 2);
    // A complete, consistent rewrite: the chain is valid, the anchor is not.
    let journal = setup.root.join("journal").join("journal.v0.jsonl");
    let lines: Vec<String> = fs::read_to_string(&journal)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    fs::write(&journal, format!("{}\n", lines[0])).unwrap();
    let output = ws(&setup.root, &["journal", "verify"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("journal.anchor_mismatch"));
}

/// `ws` with `input` on standard input.
fn ws_input(root: &Path, args: &[&str], input: &str) -> Output {
    use std::io::Write as _;
    let mut child = Process::new(WS)
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn refusal(root: &Path, args: &[&str]) -> String {
    let output = ws(root, args);
    assert_eq!(output.status.code(), Some(1), "ws {args:?}");
    String::from_utf8(output.stderr).unwrap().trim().to_owned()
}

#[test]
fn ideas_decisions_sessions_and_contracts_through_the_command_line() {
    let setup = setup();
    let mut daemon = start(&setup.root);
    let root = &setup.root;
    let session = ok(
        root,
        &[
            "session",
            "register",
            "--harness",
            "codex",
            "--label",
            "parser work",
            "--repository",
            "sample",
        ],
    )["session"]
        .as_str()
        .unwrap()
        .to_owned();
    // A session captures an idea without stopping; the owner promotes it.
    let idea = ok(
        root,
        &[
            "idea",
            "add",
            "memoise the tokenizer",
            "--session",
            &session,
        ],
    )["idea"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        root,
        &[
            "idea",
            "qualify",
            &idea,
            "--repository",
            "sample",
            "--session",
            &session,
        ],
    );
    let promoted = ok(
        root,
        &[
            "idea",
            "promote",
            &idea,
            "--title",
            "Memoise",
            "--criterion",
            "tests pass",
        ],
    );
    let mission = promoted["mission"].as_str().unwrap().to_owned();
    let ideas = ok(root, &["idea", "list"]);
    assert_eq!(ideas[0]["state"], "promoted");
    assert_eq!(ideas[0]["promoted_mission"], mission.as_str());

    // Contract refinements.
    let other = ok(root, &["idea", "add", "second idea"])["idea"]
        .as_str()
        .unwrap()
        .to_owned();
    let dependency = ok(
        root,
        &[
            "idea",
            "promote",
            &other,
            "--title",
            "Base",
            "--repository",
            "sample",
        ],
    )["mission"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(root, &["mission", "depend", &mission, "--on", &dependency]);
    ok(root, &["mission", "scope", &mission, "src/cache", "docs"]);
    ok(
        root,
        &[
            "mission", "check", &mission, "0", "--", "/bin/sh", "-c", "exit 0",
        ],
    );
    let shown = ok(root, &["mission", "show", &mission]);
    assert_eq!(shown["scope"], serde_json::json!(["docs", "src/cache"]));
    assert_eq!(shown["checks"][0]["argv"][0], "/bin/sh");
    assert_eq!(shown["dependencies"][0]["mission"], dependency.as_str());

    // A structured decision request, answered by the owner only.
    let request = ok(
        root,
        &[
            "request",
            "open",
            "--mission",
            &mission,
            "--question",
            "Which storage?",
            "--option",
            "reversible:SQLite:No server to run.",
            "--option",
            "costly:PostgreSQL:A server, multi-user ready.",
            "--recommended",
            "0",
            "--session",
            &session,
        ],
    )["request"]
        .as_str()
        .unwrap()
        .to_owned();
    let open = ok(root, &["request", "list", "--open"]);
    assert_eq!(open.as_array().unwrap().len(), 1);
    assert_eq!(open[0]["opened_by"], format!("session:{session}"));
    assert_eq!(
        open[0]["options"][1]["consequence"],
        "A server, multi-user ready."
    );
    ok(root, &["mission", "ready", &mission]);
    assert_eq!(refusal(root, &["run", &mission]), "ws: dependency.pending");
    ok(
        root,
        &[
            "request",
            "answer",
            &request,
            "0",
            "--reason",
            "single user",
        ],
    );
    assert_eq!(
        ok(root, &["request", "list", "--open"]),
        serde_json::json!([])
    );

    // The session reports and ends; the report renders.
    ok(
        root,
        &[
            "session",
            "report",
            &session,
            "blocked",
            "--note",
            "waits for the base",
        ],
    );
    ok(
        root,
        &[
            "session",
            "end",
            &session,
            "completed",
            "--summary",
            "idea filed",
        ],
    );
    let sessions = ok(root, &["session", "list"]);
    assert_eq!(sessions[0]["state"], "ended");
    assert_eq!(sessions[0]["reported_state"], "blocked");
    let report = ok(root, &["report", &mission]);
    assert_eq!(report["decisions"][0]["choice"], 0);
    let markdown = ws(root, &["report", &mission, "--markdown"]);
    assert!(markdown.status.success());
    let markdown = String::from_utf8(markdown.stdout).unwrap();
    assert!(markdown.contains("# Memoise"), "{markdown}");
    assert!(
        markdown
            .contains("| 0 | **SQLite (recommended)** — chosen | No server to run. | reversible |"),
        "{markdown}"
    );
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}

#[test]
fn ws_hook_follows_a_claude_code_session_and_writes_nothing_on_standard_output() {
    let setup = setup();
    let mut daemon = start(&setup.root);
    let root = &setup.root;
    let cwd = setup.repo.join("src");
    let payload = |event: &str, extra: &str| {
        format!(
            "{{\"hook_event_name\": \"{event}\", \"session_id\": \"cc-session-77\", \"cwd\": \"{}\"{extra}}}",
            cwd.display()
        )
    };
    let hook = |input: String| {
        let output = ws_input(root, &["hook", "claude-code"], &input);
        assert!(output.status.success());
        assert!(output.stdout.is_empty(), "stdout must stay empty");
        String::from_utf8(output.stderr).unwrap()
    };
    assert_eq!(hook(payload("SessionStart", "")), "");
    let sessions = ok(root, &["session", "list"]);
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["harness"], "claude-code");
    assert_eq!(sessions[0]["repository"], "sample");
    assert_eq!(sessions[0]["reported_state"], "working");
    hook(payload(
        "Notification",
        ", \"notification_type\": \"permission_prompt\", \"message\": \"needs the secret plan\"",
    ));
    let sessions = ok(root, &["session", "list"]);
    assert_eq!(
        sessions.as_array().unwrap().len(),
        1,
        "one session per harness id"
    );
    assert_eq!(sessions[0]["reported_state"], "waiting-input");
    assert_eq!(sessions[0]["note"], "permission_prompt");
    hook(payload("SessionEnd", ""));
    assert_eq!(ok(root, &["session", "list"])[0]["state"], "ended");

    // Codex's notify passes its payload as the last argument.
    let notify =
        r#"{"type": "agent-turn-complete", "thread-id": "codex-thread-9", "cwd": "/elsewhere"}"#;
    let output = ws(root, &["hook", "codex", notify]);
    assert!(output.status.success() && output.stdout.is_empty());
    let sessions = ok(root, &["session", "list"]);
    let codex = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["harness"] == "codex")
        .unwrap();
    assert_eq!(codex["repository"], serde_json::Value::Null);
    assert_eq!(codex["reported_state"], "waiting-input");

    daemon.kill().unwrap();
    daemon.wait().unwrap();
    // Neither the harness identifier nor the agent's message reaches the journal.
    let journal = fs::read_to_string(root.join("journal").join("journal.v0.jsonl")).unwrap();
    for secret in ["cc-session-77", "codex-thread-9", "secret plan", "parser"] {
        assert!(!journal.contains(secret), "journal leaks {secret:?}");
    }
    // Without a daemon, the hook still exits 0 and says why on standard error only.
    let output = ws_input(root, &["hook", "claude-code"], &payload("Stop", ""));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("ws: hook "));
}

/// Exit code 2 blocks a prompt or a stop in Claude Code: whatever is wrong
/// with the root, `ws hook` exits 0 and writes nothing on standard output.
#[test]
fn ws_hook_exits_zero_without_a_usable_root() {
    use std::io::Write as _;
    let payload = r#"{"hook_event_name": "Stop", "session_id": "s", "cwd": "/"}"#;
    let cases: [(&[&str], bool); 4] = [
        (&["--root", "", "hook", "claude-code"], false),
        (&["hook", "claude-code"], true),
        (&["--root", "relative/root", "hook", "claude-code"], false),
        (
            &["--root", "/nonexistent/ws-root", "hook", "claude-code"],
            false,
        ),
    ];
    for (args, clear_root) in cases {
        let mut command = Process::new(WS);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if clear_root {
            command.env_remove("WS_ROOT");
        }
        let mut child = command.spawn().unwrap();
        // Without a usable root `ws` may exit before reading its input: a
        // broken pipe is then the expected outcome of the write, not a failure.
        let written = child.stdin.take().unwrap().write_all(payload.as_bytes());
        if let Err(error) = written {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "ws {args:?}");
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0), "ws {args:?}");
        assert!(output.stdout.is_empty(), "ws {args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).starts_with("ws: hook "),
            "ws {args:?}"
        );
    }
}
