#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The cockpit (tranche T8): read and decide, over HTTP on 127.0.0.1 only.

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use serde_json::json;
use work_supervision_cockpit::Cockpit;
use work_supervision_daemon::{Client, init_root};
use work_supervision_store::{BlobStore, Layout};

/// What `flood 40` prints: present in the terminal output only, never in a brief.
const MARKER: &str = "abcdefghijklmnopqrstuvwxyzabcdefghijklmn";

fn binaries() -> (PathBuf, PathBuf) {
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
    let directory = Path::new(env!("CARGO_BIN_EXE_ws-cockpit"))
        .parent()
        .unwrap()
        .to_owned();
    (directory.join("wsd"), directory.join("ws-fake-agent"))
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

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    daemon: Child,
    client: Client,
    mission: String,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// A root with one mission waiting for the owner's decision; its agent printed MARKER.
fn world() -> World {
    let (wsd, agent) = binaries();
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
            agent.display()
        ),
    )
    .unwrap();
    let daemon = Process::new(wsd)
        .arg("--root")
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let socket = root.join("run").join("wsd.sock");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut client = loop {
        if let Ok(client) = Client::connect(&socket) {
            break client;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    };
    let brief = "flood 40\nprint\nwrite-file out.txt done\ncommit add out\nexit 0\n".to_owned();
    let created = client
        .request(&json!({ "op": "mission.new", "title": "Cockpit <b>check</b>", "repository": "sample", "brief": brief, "criteria": ["out & committed"] }))
        .unwrap();
    let mission = created["mission"].as_str().unwrap().to_owned();
    client
        .request(&json!({ "op": "mission.ready", "mission": mission }))
        .unwrap();
    client
        .request(&json!({ "op": "run", "mission": mission }))
        .unwrap();
    client
        .request(&json!({ "op": "wait", "mission": mission, "states": ["exited"], "timeout_ms": 20_000 }))
        .unwrap();
    let evidence = BlobStore::open(&Layout::new(&root).evidence())
        .unwrap()
        .put(b"evidence")
        .unwrap();
    client
        .request(&json!({ "op": "note", "mission": mission, "text": "looked at the diff" }))
        .unwrap();
    client
        .request(&json!({ "op": "result.submit", "mission": mission, "evidence_digest": evidence.to_hex(), "summary": "wrote out.txt" }))
        .unwrap();
    World {
        _dir: dir,
        root,
        daemon,
        client,
        mission,
    }
}

struct Http {
    port: u16,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl Http {
    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|line| line.split_once(": "))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        Reply {
            status,
            headers,
            body: body.to_owned(),
        }
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Logs in with the token file and returns the session cookie.
    fn login(&self, root: &Path) -> String {
        let token = fs::read_to_string(root.join("run").join("cockpit.token")).unwrap();
        let reply = self.send(
            "POST",
            "/login",
            &[
                ("Host", &self.host()),
                ("Origin", &self.origin()),
                ("Sec-Fetch-Site", "same-origin"),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            &format!("token={}", token.trim()),
        );
        assert_eq!(reply.status, 303, "{}", reply.body);
        let cookie = reply.header("Set-Cookie").unwrap();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        cookie.split(';').next().unwrap().to_owned()
    }

    fn get(&self, path: &str, cookie: &str) -> Reply {
        self.send(
            "GET",
            path,
            &[("Host", &self.host()), ("Cookie", cookie)],
            "",
        )
    }
}

fn start(root: &Path) -> Http {
    let cockpit = Cockpit::bind(root, 0).unwrap();
    let address = cockpit.local_address();
    assert!(address.ip().is_loopback(), "bound to {address}");
    let port = address.port();
    std::thread::spawn(move || cockpit.serve());
    Http { port }
}

fn csrf(page: &str) -> String {
    let start = page.find("name=\"csrf\" value=\"").unwrap() + "name=\"csrf\" value=\"".len();
    page[start..start + 64].to_owned()
}

#[test]
fn a_decision_taken_at_the_cockpit_is_an_event_of_the_journal() {
    let mut world = world();
    let http = start(&world.root);
    let cookie = http.login(&world.root);
    let page = http.get(&format!("/missions/{}", world.mission), &cookie);
    assert_eq!(page.status, 200);
    assert!(
        page.body.contains("Cockpit &lt;b&gt;check&lt;/b&gt;"),
        "texts are escaped"
    );
    assert!(page.body.contains("out &amp; committed"));
    assert!(
        page.body.contains("looked at the diff"),
        "notes are visible"
    );
    assert!(
        page.body.contains("Simulation — fake agent"),
        "a fake-agent mission is labelled a simulation"
    );
    assert!(page.body.contains("wrote out.txt"));
    let journal = Layout::new(&world.root).journal();
    let before = fs::read_to_string(&journal)
        .unwrap()
        .matches("\"kind\":\"mission.accepted\"")
        .count();
    let reply = http.send(
        "POST",
        &format!("/missions/{}/decide", world.mission),
        &[
            ("Host", &http.host()),
            ("Origin", &http.origin()),
            ("Sec-Fetch-Site", "same-origin"),
            ("Cookie", &cookie),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        &format!(
            "csrf={}&decision=accept&reason=checked+at+the+cockpit",
            csrf(&page.body)
        ),
    );
    assert_eq!(reply.status, 303, "{}", reply.body);
    let after = fs::read_to_string(&journal)
        .unwrap()
        .matches("\"kind\":\"mission.accepted\"")
        .count();
    assert_eq!(after, before + 1);
    let shown = world
        .client
        .request(&json!({ "op": "mission.show", "mission": world.mission }))
        .unwrap();
    assert_eq!(shown["state"], "accepted");
    assert_eq!(shown["verdict"]["reason"], "checked at the cockpit");
}

#[test]
fn no_page_serves_raw_terminal_output_and_no_route_reaches_a_run() {
    let mut world = world();
    let http = start(&world.root);
    let cookie = http.login(&world.root);
    let shown = world
        .client
        .request(&json!({ "op": "mission.show", "mission": world.mission }))
        .unwrap();
    let run = shown["current_run"].as_str().unwrap().to_owned();
    let log = fs::read_to_string(world.root.join("runs").join(&run).join("pty.log")).unwrap();
    assert!(log.contains(MARKER), "the agent did print the marker");
    for path in ["/", &format!("/missions/{}", world.mission)] {
        let reply = http.get(path, &cookie);
        assert_eq!(reply.status, 200);
        assert!(
            !reply.body.contains(MARKER),
            "{path} serves terminal output"
        );
    }
    for path in [
        format!("/runs/{run}/pty.log"),
        format!("/missions/{}/terminal", world.mission),
        format!("/missions/{}/input", world.mission),
        "/../runs".to_owned(),
    ] {
        assert_eq!(http.get(&path, &cookie).status, 404, "{path}");
    }
    let token = csrf(
        &http
            .get(&format!("/missions/{}", world.mission), &cookie)
            .body,
    );
    let reply = http.send(
        "POST",
        &format!("/missions/{}/send", world.mission),
        &[
            ("Host", &http.host()),
            ("Origin", &http.origin()),
            ("Cookie", &cookie),
        ],
        &format!("csrf={token}&text=hello"),
    );
    assert_eq!(reply.status, 404, "no input route");
}

#[test]
fn origin_fetch_site_host_session_and_csrf_are_enforced() {
    let world = world();
    let http = start(&world.root);
    let cookie = http.login(&world.root);
    let page = http.get(&format!("/missions/{}", world.mission), &cookie);
    let token = csrf(&page.body);
    let path = format!("/missions/{}/decide", world.mission);
    let body = format!("csrf={token}&decision=reject&reason=x");
    let host = http.host();
    let origin = http.origin();
    type Case<'a> = (&'a str, Vec<(&'a str, &'a str)>, &'a str, u16);
    let cases: Vec<Case<'_>> = vec![
        (
            "no Origin",
            vec![("Host", &host), ("Cookie", &cookie)],
            &body,
            403,
        ),
        (
            "foreign Origin",
            vec![
                ("Host", &host),
                ("Origin", "http://evil.example"),
                ("Cookie", &cookie),
            ],
            &body,
            403,
        ),
        (
            "cross-site fetch",
            vec![
                ("Host", &host),
                ("Origin", &origin),
                ("Sec-Fetch-Site", "cross-site"),
                ("Cookie", &cookie),
            ],
            &body,
            403,
        ),
        (
            "foreign Host",
            vec![
                ("Host", "evil.example"),
                ("Origin", &origin),
                ("Cookie", &cookie),
            ],
            &body,
            421,
        ),
        (
            "no session",
            vec![("Host", &host), ("Origin", &origin)],
            &body,
            403,
        ),
        (
            "bad csrf",
            vec![("Host", &host), ("Origin", &origin), ("Cookie", &cookie)],
            "csrf=0000000000000000000000000000000000000000000000000000000000000000&decision=reject&reason=x",
            403,
        ),
    ];
    for (name, headers, body, status) in cases {
        let reply = http.send("POST", &path, &headers, body);
        assert_eq!(reply.status, status, "{name}: {}", reply.body);
    }
    // Nothing was decided by any refused request.
    let journal = fs::read_to_string(Layout::new(&world.root).journal()).unwrap();
    assert!(!journal.contains("\"kind\":\"mission.rejected\""));
    // A page without a session redirects to the login form; a wrong token is refused.
    let reply = http.send("GET", "/", &[("Host", &http.host())], "");
    assert_eq!(reply.status, 303);
    assert_eq!(reply.header("Location"), Some("/login"));
    let reply = http.send(
        "POST",
        "/login",
        &[
            ("Host", &http.host()),
            ("Origin", &http.origin()),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        "token=wrong",
    );
    assert_eq!(reply.status, 403);
    // Hardening headers on every page.
    let reply = http.get("/", &cookie);
    assert!(
        reply
            .header("Content-Security-Policy")
            .unwrap()
            .contains("default-src 'none'")
    );
    assert_eq!(reply.header("X-Content-Type-Options"), Some("nosniff"));
    assert_eq!(reply.header("Cache-Control"), Some("no-store"));
    let token_mode = {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(world.root.join("run").join("cockpit.token"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(token_mode, 0o600);
}

impl Http {
    fn post(&self, path: &str, cookie: &str, body: &str) -> Reply {
        self.send(
            "POST",
            path,
            &[
                ("Host", &self.host()),
                ("Origin", &self.origin()),
                ("Sec-Fetch-Site", "same-origin"),
                ("Cookie", cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            body,
        )
    }
}

#[test]
fn a_decision_request_is_compared_and_answered_at_the_cockpit() {
    let mut world = world();
    let session = world
        .client
        .request(
            &json!({ "op": "session.register", "harness": "claude-code", "label": "cache work",
                          "repository": "sample", "mission": world.mission }),
        )
        .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    world
        .client
        .request(&json!({ "op": "session.report", "actor": format!("session:{session}"),
                          "session": session, "state": "waiting-input", "note": "permission_prompt" }))
        .unwrap();
    let request = world
        .client
        .request(&json!({
            "op": "request.open", "actor": format!("session:{session}"), "mission": world.mission,
            "question": "Keep the <b>cache</b>?",
            "options": [
                { "label": "Keep", "consequence": "Nothing to migrate.", "reversibility": "reversible" },
                { "label": "Drop", "consequence": "<script>alert(1)</script> data lost", "reversibility": "irreversible" }
            ],
            "recommended": 0
        }))
        .unwrap()["request"]
        .as_str()
        .unwrap()
        .to_owned();
    let http = start(&world.root);
    let cookie = http.login(&world.root);
    // The CSRF token belongs to the cockpit session: any form carries it.
    let token = csrf(
        &http
            .get(&format!("/missions/{}", world.mission), &cookie)
            .body,
    );

    // The mission list says what the mission waits for.
    let list = http.get("/", &cookie);
    assert!(list.body.contains("request.pending"), "{}", list.body);
    assert!(list.body.contains("1 decision(s) waiting for you"));

    // The options are side by side, escaped, with their reversibility.
    let page = http.get("/decisions", &cookie);
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Keep the &lt;b&gt;cache&lt;/b&gt;?"));
    assert!(
        page.body
            .contains("&lt;script&gt;alert(1)&lt;/script&gt; data lost")
    );
    assert!(!page.body.contains("<script>"));
    assert!(
        page.body
            .contains("class=\"rev rev-irreversible\">irreversible")
    );
    assert!(
        page.body
            .contains("Keep</label> <strong>(recommended)</strong>")
    );
    assert!(page.body.contains(&format!("opened by session:{session}")));

    // Without the CSRF token nothing is answered; with it, the owner's answer is journalled.
    let path = format!("/requests/{request}/answer");
    let refused = http.post(&path, &cookie, "choice=1&reason=no");
    assert_eq!(refused.status, 403);
    let reply = http.post(
        &path,
        &cookie,
        &format!("csrf={}&choice=1&reason=space+is+short", csrf(&page.body)),
    );
    assert_eq!(reply.status, 303, "{}", reply.body);
    let answered = world
        .client
        .request(&json!({ "op": "request.list" }))
        .unwrap();
    assert_eq!(answered[0]["state"], "answered");
    assert_eq!(answered[0]["choice"], 1);
    assert_eq!(answered[0]["closed_by"], "owner");
    let page = http.get("/decisions", &cookie);
    assert!(page.body.contains("0 open request(s)"));
    assert!(page.body.contains("<td>Drop</td>"));

    // Ideas: captured by the session, promoted at the cockpit.
    let idea = world
        .client
        .request(
            &json!({ "op": "idea.capture", "actor": format!("session:{session}"),
                          "text": "index the <cache>" }),
        )
        .unwrap()["idea"]
        .as_str()
        .unwrap()
        .to_owned();
    let page = http.get("/ideas", &cookie);
    assert!(page.body.contains("index the &lt;cache&gt;"));
    let reply = http.post(
        &format!("/ideas/{idea}/promote"),
        &cookie,
        &format!(
            "csrf={}&title=Index+the+cache&repository=sample",
            csrf(&page.body)
        ),
    );
    assert_eq!(reply.status, 303, "{}", reply.body);
    let missions = world
        .client
        .request(&json!({ "op": "mission.list" }))
        .unwrap();
    assert_eq!(missions.as_array().unwrap().len(), 2);

    // Sessions: what they declared, labelled as such.
    let page = http.get("/sessions", &cookie);
    assert!(page.body.contains("claude-code"));
    assert!(page.body.contains("permission_prompt"));
    assert!(page.body.contains("never a verified result"));

    // A malformed identifier in a write route reaches nothing.
    let reply = http.post(
        "/requests/not-an-id/answer",
        &cookie,
        &format!("csrf={token}&choice=0&reason=x"),
    );
    assert_eq!(reply.status, 404);
}

#[test]
fn an_artifact_is_read_and_approved_at_the_cockpit_by_its_digest() {
    let mut world = world();
    let mission = world
        .client
        .request(
            &json!({ "op": "mission.new", "title": "Phased", "repository": "sample",
                          "brief": "exit 0\n", "criteria": [] }),
        )
        .unwrap()["mission"]
        .as_str()
        .unwrap()
        .to_owned();
    world
        .client
        .request(&json!({ "op": "mission.workflow", "mission": mission, "phases": ["research"] }))
        .unwrap();
    world
        .client
        .request(&json!({ "op": "mission.ready", "mission": mission }))
        .unwrap();
    let artifact = world
        .client
        .request(
            &json!({ "op": "artifact.submit", "mission": mission, "phase": "research",
                          "content": "README:1 <script>alert(1)</script>" }),
        )
        .unwrap()["artifact"]
        .as_str()
        .unwrap()
        .to_owned();
    let http = start(&world.root);
    let cookie = http.login(&world.root);
    let page = http.get(&format!("/missions/{mission}"), &cookie);
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<h2>Phases</h2>"), "{}", page.body);
    assert!(
        page.body
            .contains("README:1 &lt;script&gt;alert(1)&lt;/script&gt;")
    );
    assert!(!page.body.contains("<script>"));
    assert!(page.body.contains("phase.unapproved"));
    let digest = world
        .client
        .request(&json!({ "op": "artifact.show", "artifact": artifact }))
        .unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        page.body
            .contains(&format!("name=\"digest\" value=\"{digest}\"")),
        "the form carries the digest of the text shown"
    );
    let token = csrf(&page.body);
    let path = format!("/artifacts/{artifact}/approve");
    // A digest that is not the artifact's is refused by wsd.
    let reply = http.post(
        &path,
        &cookie,
        &format!("csrf={token}&mission={mission}&digest={}", "0".repeat(64)),
    );
    assert_eq!(reply.status, 409);
    assert!(
        reply.body.contains("artifact.digest_mismatch"),
        "{}",
        reply.body
    );
    // No mission to come back to, nothing is sent.
    let reply = http.post(
        &path,
        &cookie,
        &format!("csrf={token}&mission=x&digest={digest}"),
    );
    assert_eq!(reply.status, 400);
    let reply = http.post(
        &path,
        &cookie,
        &format!("csrf={token}&mission={mission}&digest={digest}&reason=checked+the+line"),
    );
    assert_eq!(reply.status, 303, "{}", reply.body);
    let shown = world
        .client
        .request(&json!({ "op": "artifact.show", "artifact": artifact }))
        .unwrap();
    assert_eq!(shown["state"], "approved");
    assert_eq!(shown["reason"], "checked the line");
    let page = http.get(&format!("/missions/{mission}"), &cookie);
    assert!(!page.body.contains("Approve this text"));
}
