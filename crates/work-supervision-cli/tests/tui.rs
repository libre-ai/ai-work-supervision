#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The terminal interface of `ws attach` (tranche T7): rendering snapshot and a
//! complete keyboard journey against a real `wsd` and the fake agent.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use work_supervision_cli::tui::{Action, App, render};
use work_supervision_daemon::{Client, init_root};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn chars(text: &str) -> Vec<KeyEvent> {
    text.chars()
        .map(|character| key(KeyCode::Char(character)))
        .collect()
}

fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(app, frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn the_screen_shows_the_missions_the_terminal_and_the_decision_bar() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().to_owned(), None);
    app.set_missions(&json!([
        { "id": "0123456789abcdef0123456789abcdef", "title": "Add the parser", "state": "waiting-input" },
        { "id": "fedcba9876543210fedcba9876543210", "title": "Fix the flaky test", "state": "accepted" },
    ]));
    app.feed_terminal(b"\x1b[1mhello\x1b[0m world\r\nwhat now? ");
    let lines = screen(&app, 72, 10);
    let expected = [
        "┌Missions──────────────────────┐┌Terminal 01234567─────────────────────┐",
        "│> waiting-input  Add the parse││hello world                           │",
        "│  accepted       Fix the flaky││what now?                             │",
        "│                              ││                                      │",
        "│                              ││                                      │",
        "│                              ││                                      │",
        "│                              ││                                      │",
        "└──────────────────────────────┘└──────────────────────────────────────┘",
        " ↑↓ select · r ready · g run · ⏎ attach · u result · a accept · x reject",
        " d abandon · c cancel · q quit                                          ",
    ];
    assert_eq!(lines, expected, "\n{}", lines.join("\n"));
}

#[test]
fn keys_produce_requests_and_prompts_collect_reasons() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().to_owned(), None);
    app.set_missions(
        &json!([{ "id": "0123456789abcdef0123456789abcdef", "title": "T", "state": "ready" }]),
    );
    assert_eq!(
        app.on_key(key(KeyCode::Char('g'))),
        vec![Action::Request(
            json!({ "op": "run", "mission": "0123456789abcdef0123456789abcdef" })
        )]
    );
    assert!(
        app.on_key(key(KeyCode::Char('x'))).is_empty(),
        "a reject first asks for a reason"
    );
    let mut actions = Vec::new();
    for event in chars("not yet") {
        actions.extend(app.on_key(event));
    }
    actions.extend(app.on_key(key(KeyCode::Enter)));
    assert_eq!(
        actions,
        vec![Action::Request(json!({
            "op": "decide", "mission": "0123456789abcdef0123456789abcdef",
            "decision": "reject", "reason": "not yet"
        }))]
    );
    // Esc abandons a prompt without any request.
    app.on_key(key(KeyCode::Char('a')));
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    assert!(
        app.on_key(key(KeyCode::Enter)).is_empty(),
        "Enter attaches, it sends nothing"
    );
    assert_eq!(
        app.on_key(key(KeyCode::Char('q'))),
        Vec::<Action>::new(),
        "q is text while attached"
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), vec![Action::Quit]);
}

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
    let status = Process::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success());
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Executes the actions of a key through the real daemon, as `ws attach` does.
fn press(app: &mut App, client: &mut Client, event: KeyEvent) {
    for action in app.on_key(event) {
        if let Action::Request(request) = action {
            let response = client.request(&request);
            app.on_response(&request, response);
        }
    }
}

fn refresh(app: &mut App, client: &mut Client) {
    let list = client.request(&json!({ "op": "mission.list" })).unwrap();
    app.set_missions(&list);
    if let Some(id) = app.selected_id().map(str::to_owned) {
        let shown = client
            .request(&json!({ "op": "mission.show", "mission": id }))
            .unwrap();
        app.set_detail(&shown);
    }
    app.tail_log().unwrap();
}

fn until_state(app: &mut App, client: &mut Client, state: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        refresh(app, client);
        if app.selected_state() == Some(state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never reached {state}: {:?}",
            app.selected_state()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_mission_is_driven_from_ready_to_accepted_with_the_keyboard_only() {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let (root, repo) = (base.join("root"), base.join("repo"));
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
    init_root(&root).unwrap();
    fs::write(
        root.join("config.toml"),
        format!(
            "[repositories]\nsample = \"{}\"\n[executor]\nprofile = \"fake\"\nfake_agent = \"{}\"\n[runs]\nidle_after_ms = 300\ngrace_ms = 500\n",
            repo.display(),
            sibling("ws-fake-agent").display()
        ),
    )
    .unwrap();
    let _daemon = Daemon(
        Process::new(sibling("wsd"))
            .arg("--root")
            .arg(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let socket = root.join("run").join("wsd.sock");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut client = loop {
        if let Ok(client) = Client::connect(&socket) {
            break client;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    };
    let created: Value = client
        .request(&json!({
            "op": "mission.new", "title": "Answer the agent", "repository": "sample",
            "brief": "print ready for input\nread-line\nwrite-file answer.txt answered\ncommit record the answer\nexit 0\n"
        }))
        .unwrap();
    let id = created["mission"].as_str().unwrap().to_owned();
    let mut app = App::new(root.clone(), Some(id.clone()));
    refresh(&mut app, &mut client);
    assert_eq!(app.selected_id(), Some(id.as_str()));

    press(&mut app, &mut client, key(KeyCode::Char('r')));
    until_state(&mut app, &mut client, "ready");
    press(&mut app, &mut client, key(KeyCode::Char('g')));
    until_state(&mut app, &mut client, "waiting-input");
    assert!(
        app.terminal_text().contains("ready for input"),
        "{}",
        app.terminal_text()
    );

    press(&mut app, &mut client, key(KeyCode::Enter));
    for event in chars("yes, go") {
        press(&mut app, &mut client, event);
    }
    press(&mut app, &mut client, key(KeyCode::Enter));
    press(&mut app, &mut client, key(KeyCode::Esc));
    until_state(&mut app, &mut client, "exited");
    let echoed: String = "yes, go"
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(
        app.terminal_text().contains(&echoed),
        "{}",
        app.terminal_text()
    );

    let evidence = base.join("evidence.txt");
    fs::write(&evidence, "answer recorded\n").unwrap();
    press(&mut app, &mut client, key(KeyCode::Char('u')));
    for event in chars(evidence.to_str().unwrap()) {
        press(&mut app, &mut client, event);
    }
    press(&mut app, &mut client, key(KeyCode::Enter));
    for event in chars("answered the question") {
        press(&mut app, &mut client, event);
    }
    press(&mut app, &mut client, key(KeyCode::Enter));
    until_state(&mut app, &mut client, "result-submitted");

    press(&mut app, &mut client, key(KeyCode::Char('a')));
    for event in chars("criteria met") {
        press(&mut app, &mut client, event);
    }
    press(&mut app, &mut client, key(KeyCode::Enter));
    until_state(&mut app, &mut client, "accepted");
    assert_eq!(app.status(), "", "no refusal on the way");
}

#[test]
fn a_refusal_is_shown_by_its_code() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().to_owned(), None);
    app.set_missions(
        &json!([{ "id": "0123456789abcdef0123456789abcdef", "title": "T", "state": "draft" }]),
    );
    let request = json!({ "op": "run", "mission": "0123456789abcdef0123456789abcdef" });
    let refusal: Result<Value, work_supervision_daemon::ClientError> =
        Err(Client::connect(&root.path().join("absent.sock")).unwrap_err());
    app.on_response(&request, refusal);
    assert_eq!(app.status(), "transport.connect");
    let lines = screen(&app, 72, 10);
    assert!(lines[9].contains("transport.connect"), "{lines:?}");
}
