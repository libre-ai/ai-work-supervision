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
