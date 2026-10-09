#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Anchoring the journal head outside the root (tranche T9).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Output, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use work_supervision_daemon::{Anchor, Client, check_anchor, init_root};
use work_supervision_journal::{Event, Journal, OpenMode, Timestamp};
use work_supervision_store::Layout;

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
    Path::new(WSD).with_file_name("ws-fake-agent")
}

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    anchor: PathBuf,
}

fn setup(anchor_inside_root: bool) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let root = base.join("root");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_root(&root).unwrap();
    let anchor = if anchor_inside_root {
        root.join("anchors.v0")
    } else {
        base.join("private").join("anchors.v0")
    };
    fs::create_dir_all(anchor.parent().unwrap()).unwrap();
    fs::write(
        root.join("config.toml"),
        format!(
            "[repositories]\nsample = \"{}\"\n[executor]\nprofile = \"fake\"\nfake_agent = \"{}\"\n[anchor]\npath = \"{}\"\n",
            repo.display(),
            fake_agent().display(),
            anchor.display()
        ),
    )
    .unwrap();
    Setup {
        _dir: dir,
        root,
        anchor,
    }
}

fn start(root: &Path) -> Child {
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
        assert!(Instant::now() < deadline, "wsd did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

fn refused_start(root: &Path) -> Output {
    let output = Process::new(WSD).arg("--root").arg(root).output().unwrap();
    assert!(!output.status.success());
    output
}

fn stop(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

fn create_missions(root: &Path, count: usize) {
    let mut client = Client::connect(&root.join("run").join("wsd.sock")).unwrap();
    for index in 0..count {
        client
            .request(&json!({ "op": "mission.new", "title": format!("M{index}"), "repository": "sample", "brief": "exit 0" }))
            .unwrap();
    }
}

/// Rewrites the whole journal as another, perfectly chained history of `entries` entries.
fn rewrite(root: &Path, entries: u64) {
    let path = Layout::new(root).journal();
    fs::remove_file(&path).unwrap();
    let (mut journal, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    for index in 0..entries {
        let mut data = Map::new();
        data.insert("forged".to_owned(), Value::from(index));
        journal
            .append(
                Timestamp::parse("2026-10-09T03:00:00.000Z").unwrap(),
                Event::new("journal.recovered", data).unwrap(),
            )
            .unwrap();
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_anchor_follows_the_head_and_a_restart_is_green() {
    let setup = setup(false);
    let daemon = start(&setup.root);
    create_missions(&setup.root, 3);
    stop(daemon);
    let anchor = Anchor::read(&setup.anchor).unwrap().unwrap();
    assert_eq!(
        anchor.seq(),
        3,
        "anchored at the head after the last request"
    );
    assert_eq!(
        check_anchor(&Layout::new(&setup.root).journal(), &anchor),
        Ok(())
    );
    let daemon = start(&setup.root);
    create_missions(&setup.root, 1);
    stop(daemon);
    assert_eq!(Anchor::read(&setup.anchor).unwrap().unwrap().seq(), 4);
    let text = fs::read_to_string(&setup.anchor).unwrap();
    assert!(text.starts_with("{\"digest\":\""), "{text}");
    assert!(!text.contains("M0"), "the anchor holds no content");
}

#[test]
fn a_complete_rewrite_of_the_journal_is_detected_by_the_anchor() {
    for forged_entries in [3_u64, 5, 1] {
        let setup = setup(false);
        let daemon = start(&setup.root);
        create_missions(&setup.root, 3);
        stop(daemon);
        rewrite(&setup.root, forged_entries);
        // The chain alone is green: only the anchor sees the rewrite.
        let file = fs::File::open(Layout::new(&setup.root).journal()).unwrap();
        assert!(matches!(
            work_supervision_journal_verifier::verify(file).unwrap(),
            work_supervision_journal_verifier::Outcome::Valid { .. }
        ));
        let output = refused_start(&setup.root);
        assert!(
            stderr(&output).contains("journal.anchor_mismatch"),
            "{forged_entries}: {}",
            stderr(&output)
        );
        let anchor = Anchor::read(&setup.anchor).unwrap().unwrap();
        assert_eq!(
            check_anchor(&Layout::new(&setup.root).journal(), &anchor)
                .unwrap_err()
                .code(),
            "journal.anchor_mismatch"
        );
    }
}

#[test]
fn a_missing_anchor_for_a_written_journal_is_refused_and_an_anchor_inside_the_root_is_refused() {
    let first = setup(false);
    let daemon = start(&first.root);
    create_missions(&first.root, 2);
    stop(daemon);
    fs::remove_file(&first.anchor).unwrap();
    let output = refused_start(&first.root);
    assert!(
        stderr(&output).contains("journal.anchor_missing"),
        "{}",
        stderr(&output)
    );

    let inside = setup(true);
    let output = refused_start(&inside.root);
    assert!(
        stderr(&output).contains("config.anchor_inside_root"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_anchor_is_read_strictly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anchors.v0");
    assert_eq!(
        Anchor::read(&path).unwrap(),
        None,
        "absent is None, not an error"
    );
    for broken in [
        "",
        "{}",
        "{\"digest\":\"zz\",\"schema\":\"libre-ai.work-supervision.anchor.v0\",\"seq\":1}\n",
        "not json\n",
    ] {
        fs::write(&path, broken).unwrap();
        assert_eq!(
            Anchor::read(&path).unwrap_err().code(),
            "journal.anchor_invalid",
            "{broken:?}"
        );
    }
}
