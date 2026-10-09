#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Tranche T10 — rehearsal of B′ with the fake agent.
//!
//! **SIMULATION.** B′ (the v0 exit criterion of `project.v1.yaml`) is ten
//! consecutive missions with a *real* agent behind the C0 confinement
//! qualification. This rehearsal runs the same campaign with the fake agent:
//! it proves the v0 machinery end to end and does **not** count toward B′.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Output, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::Value;

const WS: &str = env!("CARGO_BIN_EXE_ws");
const MISSIONS: usize = 10;

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
    if output.stdout.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

/// The scenario of mission `index`: three shapes, so the campaign exercises
/// plain runs, an input exchange, and a rejection followed by a second run.
fn scenario(index: usize) -> String {
    match index % 3 {
        0 => format!(
            "print mission {index}\nwrite-file result-{index}.txt done\ncommit result {index}\nexit 0\n"
        ),
        1 => format!(
            "print question {index}\nread-line\nwrite-file result-{index}.txt answered\ncommit result {index}\nexit 0\n"
        ),
        _ => format!(
            "print mission {index}\nwrite-file result-{index}.txt attempt\ncommit result {index}\nexit 0\n"
        ),
    }
}

fn files_outside(base: &Path, known: &[&Path]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(base).unwrap() {
        let path = entry.unwrap().path();
        if !known.iter().any(|known| path.starts_with(known)) {
            found.push(path);
        }
    }
    found
}

#[test]
fn b_prime_rehearsal_with_the_fake_agent_is_a_labelled_simulation() {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let (root, repo, private, inputs) = (
        base.join("root"),
        base.join("repo"),
        base.join("private"),
        base.join("inputs"),
    );
    for directory in [&repo, &private, &inputs] {
        fs::create_dir_all(directory).unwrap();
    }
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
    assert!(
        Process::new(WS)
            .args(["init", "--root"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        root.join("config.toml"),
        format!(
            "[repositories]\nsample = \"{}\"\n[executor]\nprofile = \"fake\"\nfake_agent = \"{}\"\n[runs]\nidle_after_ms = 300\ngrace_ms = 500\n[anchor]\npath = \"{}\"\n",
            repo.display(),
            sibling("ws-fake-agent").display(),
            private.join("anchors.v0").display()
        ),
    )
    .unwrap();
    let mut daemon: Child = Process::new(sibling("wsd"))
        .arg("--root")
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ws(&root, &["status"]).status.success() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut accepted = 0;
    let mut rejected_then_accepted = 0;
    let mut inputs_sent = 0;
    for index in 0..MISSIONS {
        let brief = inputs.join(format!("brief-{index}.txt"));
        fs::write(&brief, scenario(index)).unwrap();
        let created = ok(
            &root,
            &[
                "mission",
                "new",
                "--repository",
                "sample",
                "--title",
                &format!("Rehearsal mission {index}"),
                "--brief",
                brief.to_str().unwrap(),
                "--criterion",
                &format!("result-{index}.txt is committed"),
            ],
        );
        let id = created["mission"].as_str().unwrap().to_owned();
        let shown = ok(&root, &["mission", "show", &id]);
        assert_eq!(
            shown["simulation"], true,
            "a fake-agent mission is labelled a simulation"
        );
        ok(&root, &["mission", "ready", &id]);
        ok(&root, &["run", &id]);
        if index % 3 == 1 {
            assert_eq!(
                ok(&root, &["wait", &id, "waiting-input"])["state"],
                "waiting-input"
            );
            ok(&root, &["send", &id, "go on"]);
            inputs_sent += 1;
        }
        assert_eq!(ok(&root, &["wait", &id, "exited"])["state"], "exited");
        let evidence = inputs.join(format!("evidence-{index}.txt"));
        fs::write(&evidence, format!("checked result-{index}.txt\n")).unwrap();
        ok(
            &root,
            &[
                "result",
                "submit",
                &id,
                "--evidence",
                evidence.to_str().unwrap(),
                "--summary",
                "result committed",
            ],
        );
        if index % 3 == 2 {
            assert_eq!(
                ok(
                    &root,
                    &["decide", &id, "reject", "--reason", "run it again"]
                )["state"],
                "rejected"
            );
            // The brief is frozen: the second run replays it on the kept worktree.
            ok(&root, &["run", &id]);
            assert_eq!(ok(&root, &["wait", &id, "exited"])["state"], "exited");
            ok(
                &root,
                &[
                    "result",
                    "submit",
                    &id,
                    "--evidence",
                    evidence.to_str().unwrap(),
                    "--summary",
                    "second run",
                ],
            );
            rejected_then_accepted += 1;
        }
        assert_eq!(
            ok(
                &root,
                &["decide", &id, "accept", "--reason", "criteria met"]
            )["state"],
            "accepted"
        );
        accepted += 1;
        assert!(
            git(&repo, &["ls-tree", "--name-only", &format!("ws/{id}")])
                .contains(&format!("result-{index}.txt"))
        );
    }
    let gc = ok(&root, &["worktree", "gc"]);
    let doctor = ok(&root, &["doctor"]);
    daemon.kill().unwrap();
    daemon.wait().unwrap();

    let verified = ok(&root, &["journal", "verify"]);
    let rebuilt = ok(&root, &["rebuild", "--check"]);
    let registered = git(&repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count();
    let branches = git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/ws/",
        ],
    )
    .lines()
    .count();
    let left: Vec<_> = fs::read_dir(root.join("worktrees")).unwrap().collect();
    // No handoff file: nothing was written outside the root, the repository,
    // the private anchor and the test's own inputs.
    let stray = files_outside(&base, &[&root, &repo, &private, &inputs]);

    println!(
        "SIMULATION (fake agent, does not count toward B′): {accepted}/{MISSIONS} accepted \
         ({rejected_then_accepted} after a rejection, {inputs_sent} with input); journal {} entries, anchor {}; \
         projection rebuild equal: {}; worktrees registered besides main: {}; ws/* branches: {branches} (kept results); \
         gc removed {}+{}; doctor pending removals {}; stray files: {}",
        verified["entries"],
        verified["anchor"]["status"],
        rebuilt["equal"],
        registered - 1,
        gc["removed_worktrees"],
        gc["removed_branches"],
        doctor["worktree_removals_finished"],
        stray.len()
    );
    assert_eq!(accepted, MISSIONS);
    assert_eq!(verified["valid"], true);
    assert_eq!(verified["anchor"]["status"], "matched");
    assert_eq!(rebuilt["equal"], true);
    assert_eq!(registered, 1, "0 orphan worktree");
    assert!(left.is_empty());
    assert_eq!(
        branches, MISSIONS,
        "one kept branch per accepted mission, no orphan"
    );
    assert_eq!(gc["removed_worktrees"], 0);
    assert_eq!(gc["removed_branches"], 0);
    assert_eq!(doctor["worktree_removals_finished"], 0);
    assert!(stray.is_empty(), "{stray:?}");
    assert!(!base.join("notes").exists());
}
