//! `ws-cockpit` — the web cockpit of Work Supervision v0 (owner decision Y30:
//! a Rust server rendering HTML, one stack).
//!
//! The cockpit **reads and decides**, nothing else:
//!
//! - it listens on `127.0.0.1` only and opens the projection read-only;
//! - pages show missions, results, evidence digests, run counters and digests,
//!   verdicts and notes, the contract (scope, dependencies, checks), open
//!   decision requests with their options side by side, deferred ideas and
//!   declared agent sessions — never terminal output, and no route reaches a
//!   PTY, a run log or an input;
//! - every write — a decision, a note, an answer to or a withdrawal of a
//!   decision request, a promotion or a dismissal of an idea — is sent to
//!   `wsd` over its socket, which journals it exactly as `ws` does;
//! - blockers depend on what only `wsd` holds, so they are asked of it and
//!   shown unknown when it cannot be reached, never guessed.
//!
//! Every request must carry `Host: 127.0.0.1:<port>` or `localhost:<port>`
//! (DNS rebinding, `421`). Every page but the login form needs the session
//! cookie (`HttpOnly`, `SameSite=Strict`) obtained by posting the token the
//! cockpit writes to `run/cockpit.token` (mode 0600) at start. Every POST must
//! carry an `Origin` of the cockpit itself, a `Sec-Fetch-Site` of
//! `same-origin` when present, and the session's CSRF token (`403`).
//! Responses carry a `default-src 'none'` content security policy, no script.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::net::SocketAddr;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::json;
use work_supervision_daemon::Client;
use work_supervision_domain::MissionId;
use work_supervision_store::{Layout, Store};

/// Largest accepted request body.
const MAX_BODY_BYTES: u64 = 64 * 1024;

/// A rendered response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Page {
    fn new(status: u16, body: String) -> Self {
        Self {
            status,
            headers: vec![(
                "Content-Type".to_owned(),
                "text/html; charset=utf-8".to_owned(),
            )],
            body,
        }
    }

    fn refused(status: u16, code: &str) -> Self {
        Self::new(
            status,
            layout("Refused", &format!("<p>refused: {}</p>", escape(code))),
        )
    }

    fn redirect(location: &str) -> Self {
        let mut page = Self::new(303, String::new());
        page.headers
            .push(("Location".to_owned(), location.to_owned()));
        page
    }

    /// HTTP status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Body.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}

struct Session {
    csrf: String,
}

/// A bound cockpit, ready to serve.
pub struct Cockpit {
    server: tiny_http::Server,
    address: SocketAddr,
    root: PathBuf,
    token: String,
    sessions: Mutex<HashMap<String, Session>>,
}

impl std::fmt::Debug for Cockpit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Cockpit")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

fn random_hex() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| "cockpit.random".to_owned())?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        }))
}

/// Comparison whose duration does not depend on where the inputs differ.
fn same(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

impl Cockpit {
    /// Binds `127.0.0.1:<port>` (`0` for an ephemeral port) and writes a new
    /// login token to `run/cockpit.token`.
    ///
    /// # Errors
    ///
    /// `cockpit.bind`, `cockpit.token_io`, `cockpit.random`.
    pub fn bind(root: &Path, port: u16) -> Result<Self, String> {
        let server =
            tiny_http::Server::http(("127.0.0.1", port)).map_err(|_| "cockpit.bind".to_owned())?;
        let address = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| "cockpit.bind".to_owned())?;
        let token = random_hex()?;
        let path = root.join("run").join("cockpit.token");
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cockpit.token_io".to_owned()),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| "cockpit.token_io".to_owned())?;
        file.write_all(format!("{token}\n").as_bytes())
            .map_err(|_| "cockpit.token_io".to_owned())?;
        Ok(Self {
            server,
            address,
            root: root.to_owned(),
            token,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    /// The bound address (always a loopback address).
    #[must_use]
    pub const fn local_address(&self) -> SocketAddr {
        self.address
    }

    /// Serves requests until the process ends.
    pub fn serve(self) {
        for mut request in self.server.incoming_requests() {
            let method = request.method().as_str().to_owned();
            let path = request.url().to_owned();
            let headers: Vec<(String, String)> = request
                .headers()
                .iter()
                .map(|header| {
                    (
                        header.field.as_str().as_str().to_owned(),
                        header.value.as_str().to_owned(),
                    )
                })
                .collect();
            let mut body = Vec::new();
            let page = if request
                .as_reader()
                .take(MAX_BODY_BYTES + 1)
                .read_to_end(&mut body)
                .is_err()
                || u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_BODY_BYTES
            {
                Page::refused(413, "request.too_large")
            } else {
                self.handle(&method, &path, &headers, &body)
            };
            let mut response = tiny_http::Response::from_string(page.body)
                .with_status_code(tiny_http::StatusCode(page.status));
            let fixed = SECURITY_HEADERS.iter().map(|(name, value)| (*name, *value));
            for (name, value) in page
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .chain(fixed)
            {
                if let Ok(header) = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                {
                    response.add_header(header);
                }
            }
            let _ = request.respond(response);
        }
    }

    /// Answers one request (the whole policy lives here, free of any socket).
    #[must_use]
    pub fn handle(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Page {
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        };
        let port = self.address.port();
        let hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
        if !header("Host").is_some_and(|host| hosts.iter().any(|allowed| allowed == host)) {
            return Page::refused(421, "request.host_refused");
        }
        if method == "POST" {
            let origins = hosts
                .iter()
                .map(|host| format!("http://{host}"))
                .collect::<Vec<_>>();
            if !header("Origin")
                .is_some_and(|origin| origins.iter().any(|allowed| allowed == origin))
            {
                return Page::refused(403, "request.origin_refused");
            }
            if header("Sec-Fetch-Site").is_some_and(|site| site != "same-origin") {
                return Page::refused(403, "request.cross_site");
            }
        }
        let form = parse_form(body);
        if path == "/login" {
            return match method {
                "GET" => Page::new(200, login_page()),
                "POST" => self.login(form.get("token").map(String::as_str).unwrap_or_default()),
                _ => Page::refused(405, "request.method"),
            };
        }
        let session = header("Cookie").and_then(|cookie| {
            cookie
                .split(';')
                .filter_map(|part| part.trim().strip_prefix("ws_session="))
                .next()
                .map(str::to_owned)
        });
        let Ok(sessions) = self.sessions.lock() else {
            return Page::refused(500, "cockpit.poisoned");
        };
        let Some(csrf) = session
            .as_ref()
            .and_then(|id| sessions.iter().find(|(known, _)| same(known, id)))
            .map(|(_, session)| session.csrf.clone())
        else {
            return if method == "GET" {
                Page::redirect("/login")
            } else {
                Page::refused(403, "session.required")
            };
        };
        drop(sessions);
        if method == "POST" && !form.get("csrf").is_some_and(|value| same(value, &csrf)) {
            return Page::refused(403, "session.csrf_refused");
        }
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (method, segments.as_slice()) {
            ("GET", [""]) => self.list(),
            ("GET", ["missions", id]) => self.detail(id, &csrf),
            ("POST", ["missions", id, "decide"]) => self.forward(
                id,
                json!({
                    "op": "decide", "mission": id,
                    "decision": form.get("decision").cloned().unwrap_or_default(),
                    "reason": form.get("reason").cloned().unwrap_or_default(),
                }),
            ),
            ("POST", ["missions", id, "note"]) => self.forward(
                id,
                json!({ "op": "note", "mission": id, "text": form.get("text").cloned().unwrap_or_default() }),
            ),
            ("GET", ["decisions"]) => self.decisions(&csrf),
            ("GET", ["ideas"]) => self.ideas(&csrf),
            ("GET", ["sessions"]) => self.sessions(),
            ("POST", ["requests", id, "answer"]) => {
                let Some(choice) = form.get("choice").and_then(|value| value.parse::<u64>().ok())
                else {
                    return Page::refused(400, "request.field_invalid");
                };
                self.forward_to(
                    id,
                    json!({
                        "op": "request.answer", "request": id, "choice": choice,
                        "reason": form.get("reason").cloned().unwrap_or_default(),
                    }),
                    "/decisions",
                )
            }
            ("POST", ["requests", id, "withdraw"]) => self.forward_to(
                id,
                json!({
                    "op": "request.withdraw", "request": id,
                    "reason": form.get("reason").cloned().unwrap_or_default(),
                }),
                "/decisions",
            ),
            ("POST", ["ideas", id, "dismiss"]) => self.forward_to(
                id,
                json!({
                    "op": "idea.dismiss", "idea": id,
                    "reason": form.get("reason").cloned().unwrap_or_default(),
                }),
                "/ideas",
            ),
            ("POST", ["ideas", id, "promote"]) => self.forward_to(
                id,
                json!({
                    "op": "idea.promote", "idea": id,
                    "title": form.get("title").cloned().unwrap_or_default(),
                    "repository": form.get("repository").filter(|name| !name.is_empty()),
                }),
                "/ideas",
            ),
            _ => Page::refused(404, "request.not_found"),
        }
    }

    /// Asks `wsd` a read-only question; `None` when it cannot be reached.
    fn ask(&self, request: &serde_json::Value) -> Option<serde_json::Value> {
        let socket = self.root.join("run").join("wsd.sock");
        Client::connect(&socket)
            .and_then(|mut client| client.request(request))
            .ok()
    }

    /// Sends a write to `wsd` for the object `id` (32 hexadecimal characters),
    /// then shows `back`.
    fn forward_to(&self, id: &str, request: serde_json::Value, back: &str) -> Page {
        if !is_identifier(id) {
            return Page::refused(404, "request.not_found");
        }
        let socket = self.root.join("run").join("wsd.sock");
        let outcome = Client::connect(&socket).and_then(|mut client| client.request(&request));
        match outcome {
            Ok(_) => Page::redirect(back),
            Err(error) => Page::refused(409, error.code()),
        }
    }

    /// Open decision requests, their options side by side, and the answer form.
    fn decisions(&self, csrf: &str) -> Page {
        let store = match self.store() {
            Ok(store) => store,
            Err(page) => return page,
        };
        let requests = match store.requests(false) {
            Ok(requests) => requests,
            Err(error) => return Page::refused(500, error.code()),
        };
        let open: Vec<_> = requests.iter().filter(|row| row.state == "open").collect();
        let mut html = format!(
            "{nav}<h1>Decisions</h1><p>{count} open request(s). Each option shows what choosing it entails and whether it can be undone.</p>",
            nav = nav(),
            count = open.len()
        );
        for request in &open {
            let mission = request.mission.as_deref().map_or_else(String::new, |id| {
                format!(
                    " · mission <a href=\"/missions/{id}\">{short}</a>",
                    short = id.get(..8).unwrap_or_default()
                )
            });
            let _ = write!(
                html,
                "<section class=\"request\"><h2>{question}</h2><p class=\"muted\">opened by {opener} at <time>{at}</time>{mission}</p><form method=\"post\" action=\"/requests/{id}/answer\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><table><thead><tr><th>Choose</th><th>Option</th><th>Consequence</th><th>Reversibility</th></tr></thead><tbody>",
                question = escape(&request.question),
                opener = escape(&request.opened_by),
                at = escape(&request.created_at),
                id = request.id,
            );
            for (position, option) in request.options.iter().enumerate() {
                let recommended = request
                    .recommended
                    .and_then(|index| usize::try_from(index).ok())
                    == Some(position);
                let _ = write!(
                    html,
                    "<tr><td><input type=\"radio\" name=\"choice\" value=\"{position}\" id=\"c{id}{position}\" required></td><td><label for=\"c{id}{position}\">{label}</label>{badge}</td><td>{consequence}</td><td><span class=\"rev rev-{reversibility}\">{reversibility}</span></td></tr>",
                    id = request.id,
                    label = escape(&option.label),
                    badge = if recommended {
                        " <strong>(recommended)</strong>"
                    } else {
                        ""
                    },
                    consequence = escape(&option.consequence),
                    reversibility = escape(&option.reversibility),
                );
            }
            let _ = write!(
                html,
                "</tbody></table><label>Reason <textarea name=\"reason\" required></textarea></label> <button>Answer</button></form><form method=\"post\" action=\"/requests/{id}/withdraw\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><label>Withdraw, reason <textarea name=\"reason\" required></textarea></label> <button>Withdraw</button></form></section>",
                id = request.id,
            );
        }
        let closed: Vec<_> = requests
            .iter()
            .filter(|row| row.state != "open")
            .take(20)
            .collect();
        if !closed.is_empty() {
            html.push_str("<h2>Recently closed</h2><table><thead><tr><th>Question</th><th>State</th><th>Choice</th><th>Reason</th></tr></thead><tbody>");
            for request in closed {
                let choice = request
                    .choice
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| request.options.get(index))
                    .map(|option| option.label.as_str())
                    .unwrap_or_default();
                let _ = write!(
                    html,
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape(&request.question),
                    escape(&request.state),
                    escape(choice),
                    escape(request.reason.as_deref().unwrap_or_default()),
                );
            }
            html.push_str("</tbody></table>");
        }
        Page::new(200, layout("Decisions", &html))
    }

    /// Deferred ideas, with promotion and dismissal for the open ones.
    fn ideas(&self, csrf: &str) -> Page {
        let store = match self.store() {
            Ok(store) => store,
            Err(page) => return page,
        };
        let ideas = match store.ideas() {
            Ok(ideas) => ideas,
            Err(error) => return Page::refused(500, error.code()),
        };
        let mut html = format!(
            "{nav}<h1>Ideas</h1><p>{count} idea(s), newest first.</p><table><thead><tr><th>Captured</th><th>By</th><th>State</th><th>Idea</th><th>Repository</th><th>Act</th></tr></thead><tbody>",
            nav = nav(),
            count = ideas.len()
        );
        for idea in &ideas {
            let act = match idea.state.as_str() {
                "captured" | "qualified" => format!(
                    "<form method=\"post\" action=\"/ideas/{id}/promote\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><input name=\"title\" placeholder=\"Mission title\" required> <input name=\"repository\" placeholder=\"repository\" value=\"{repository}\"> <button>Promote</button></form><form method=\"post\" action=\"/ideas/{id}/dismiss\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><input name=\"reason\" placeholder=\"reason\" required> <button>Dismiss</button></form>",
                    id = idea.id,
                    repository = escape(idea.repository.as_deref().unwrap_or_default()),
                ),
                "promoted" => idea
                    .promoted_mission
                    .as_deref()
                    .map_or_else(String::new, |id| {
                        format!("<a href=\"/missions/{id}\">mission</a>")
                    }),
                _ => escape(idea.dismiss_reason.as_deref().unwrap_or_default()),
            };
            let _ = write!(
                html,
                "<tr><td><time>{}</time></td><td>{}</td><td>{}</td><td>{}{}</td><td>{}</td><td>{act}</td></tr>",
                escape(&idea.created_at),
                escape(&idea.captured_by),
                escape(&idea.state),
                escape(&idea.text),
                idea.context
                    .as_deref()
                    .map(|context| format!("<br><small>{}</small>", escape(context)))
                    .unwrap_or_default(),
                escape(idea.repository.as_deref().unwrap_or_default()),
            );
        }
        html.push_str("</tbody></table>");
        Page::new(200, layout("Ideas", &html))
    }

    /// Declared agent sessions; their states are what they reported, not verified.
    fn sessions(&self) -> Page {
        let Some(serde_json::Value::Array(sessions)) = self.ask(&json!({ "op": "session.list" }))
        else {
            return Page::new(
                200,
                layout(
                    "Sessions",
                    &format!(
                        "{}<h1>Sessions</h1><p>wsd unreachable: session silence cannot be computed.</p>",
                        nav()
                    ),
                ),
            );
        };
        let mut html = format!(
            "{nav}<h1>Agent sessions</h1><p>What each session declared through the bridge. A reported state is never a verified result; a session silent for more than 15 minutes is marked silent, never presumed ended.</p><table><thead><tr><th>Harness</th><th>State</th><th>Reported</th><th>Note</th><th>Repository</th><th>Mission</th><th>Label</th><th>Last report</th></tr></thead><tbody>",
            nav = nav()
        );
        let text = |value: &serde_json::Value| escape(value.as_str().unwrap_or_default());
        for session in &sessions {
            let state = if session["silent"] == true {
                "<strong>silent</strong>".to_owned()
            } else {
                text(&session["state"])
            };
            let mission = session["mission"]
                .as_str()
                .filter(|id| is_identifier(id))
                .map_or_else(String::new, |id| {
                    format!(
                        "<a href=\"/missions/{id}\">{}</a>",
                        id.get(..8).unwrap_or_default()
                    )
                });
            let _ = write!(
                html,
                "<tr><td>{}</td><td>{state}</td><td>{}</td><td>{}</td><td>{}</td><td>{mission}</td><td>{}</td><td><time>{}</time></td></tr>",
                text(&session["harness"]),
                text(&session["reported_state"]),
                text(&session["note"]),
                text(&session["repository"]),
                text(&session["label"]),
                text(&session["updated_at"]),
            );
        }
        html.push_str("</tbody></table>");
        Page::new(200, layout("Sessions", &html))
    }

    fn login(&self, token: &str) -> Page {
        if !same(token.trim(), &self.token) {
            return Page::refused(403, "session.token_refused");
        }
        let (Ok(id), Ok(csrf)) = (random_hex(), random_hex()) else {
            return Page::refused(500, "cockpit.random");
        };
        let Ok(mut sessions) = self.sessions.lock() else {
            return Page::refused(500, "cockpit.poisoned");
        };
        sessions.insert(id.clone(), Session { csrf });
        let mut page = Page::redirect("/");
        page.headers.push((
            "Set-Cookie".to_owned(),
            format!("ws_session={id}; Path=/; HttpOnly; SameSite=Strict"),
        ));
        page
    }

    fn store(&self) -> Result<Store, Page> {
        Store::open_read_only(&Layout::new(&self.root).state())
            .map_err(|error| Page::refused(503, error.code()))
    }

    fn list(&self) -> Page {
        let store = match self.store() {
            Ok(store) => store,
            Err(page) => return page,
        };
        let missions = match store.missions() {
            Ok(missions) => missions,
            Err(error) => return Page::refused(500, error.code()),
        };
        // Blockers depend on what only wsd holds (running checks, worktrees of
        // other missions): asked, never guessed; shown unknown when unreachable.
        let blockers: Option<HashMap<String, String>> = self
            .ask(&json!({ "op": "mission.list" }))
            .and_then(|list| list.as_array().cloned())
            .map(|list| {
                list.iter()
                    .filter_map(|row| {
                        let id = row["id"].as_str()?.to_owned();
                        let codes: Vec<&str> = row["blockers"]
                            .as_array()?
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect();
                        Some((id, codes.join(", ")))
                    })
                    .collect()
            });
        let open_requests = store
            .requests(true)
            .map(|requests| requests.len())
            .unwrap_or_default();
        let mut rows = String::new();
        for mission in &missions {
            let worktree = store
                .worktree(mission.id().as_str())
                .ok()
                .flatten()
                .map(|row| row.state)
                .unwrap_or_default();
            let blocked = blockers.as_ref().map_or_else(
                || "unknown (wsd unreachable)".to_owned(),
                |map| escape(map.get(mission.id().as_str()).map_or("", String::as_str)),
            );
            let _ = write!(
                rows,
                "<tr><td><a href=\"/missions/{id}\">{short}</a></td><td>{title}</td><td class=\"state\">{state}</td><td>{blocked}</td><td>{worktree}</td><td>{revision}</td></tr>",
                id = mission.id().as_str(),
                short = mission.id().as_str().get(..8).unwrap_or_default(),
                title = escape(mission.title()),
                state = mission.state().as_str(),
                worktree = escape(&worktree),
                revision = mission.revision(),
            );
        }
        Page::new(
            200,
            layout(
                "Missions",
                &format!(
                    "{nav}<h1>Missions</h1><p>{count} mission(s). <a href=\"/decisions\">{open_requests} decision(s) waiting for you</a>.</p><table><thead><tr><th>Id</th><th>Title</th><th>State</th><th>Blocked by</th><th>Worktree</th><th>Revision</th></tr></thead><tbody>{rows}</tbody></table>",
                    nav = nav(),
                    count = missions.len()
                ),
            ),
        )
    }

    fn detail(&self, id: &str, csrf: &str) -> Page {
        let Ok(mission_id) = work_supervision_domain_id(id) else {
            return Page::refused(404, "request.not_found");
        };
        let store = match self.store() {
            Ok(store) => store,
            Err(page) => return page,
        };
        let Ok(Some(mission)) = store.mission(&mission_id) else {
            return Page::refused(404, "mission.not_found");
        };
        let mut html = format!(
            "<p><a href=\"/\">All missions</a></p><h1>{title}</h1>{badge}<dl><dt>State</dt><dd class=\"state\">{state}</dd><dt>Revision</dt><dd>{revision}</dd><dt>Repository</dt><dd>{repository}</dd><dt>Budgets</dt><dd>{duration} s, {output} bytes</dd><dt>Base commit</dt><dd><code>{base}</code></dd></dl><h2>Brief</h2><pre>{brief}</pre><h2>Acceptance criteria</h2><ul>",
            title = escape(mission.title()),
            state = mission.state().as_str(),
            revision = mission.revision(),
            repository = escape(mission.repository()),
            duration = mission.budgets().max_duration_seconds(),
            output = mission.budgets().max_output_bytes(),
            base = mission
                .base_commit()
                .map(|commit| commit.as_str().to_owned())
                .unwrap_or_default(),
            brief = escape(mission.brief()),
            badge = simulation_badge(&mission),
        );
        let checks = store.criterion_checks(&mission_id).unwrap_or_default();
        let executions = store.check_runs(&mission_id).unwrap_or_default();
        for (position, criterion) in mission.criteria().iter().enumerate() {
            let check = checks.iter().find(|check| check.criterion == position);
            let last = executions
                .iter()
                .rev()
                .find(|row| usize::try_from(row.criterion).ok() == Some(position));
            let verification = match (check, last) {
                (None, _) => " <small>(no check)</small>".to_owned(),
                (Some(check), None) => format!(
                    " <small>check <code>{}</code>, never run</small>",
                    escape(&check.argv.join(" "))
                ),
                (Some(check), Some(row)) => format!(
                    " <small>check <code>{}</code>: {} at <code>{}</code></small>",
                    escape(&check.argv.join(" ")),
                    if row.passed() { "passed" } else { "not passed" },
                    escape(row.commit.get(..12).unwrap_or_default()),
                ),
            };
            let _ = write!(html, "<li>{}{verification}</li>", escape(criterion));
        }
        html.push_str("</ul><h2>Contract</h2><dl><dt>Scope</dt><dd>");
        match store.scope(&mission_id) {
            Ok(Some(paths)) => {
                let names: Vec<String> = paths
                    .iter()
                    .map(|path| format!("<code>{}</code>", escape(path.as_str())))
                    .collect();
                html.push_str(&names.join(", "));
            }
            _ => html.push_str("undeclared (whole repository)"),
        }
        html.push_str("</dd><dt>Depends on</dt><dd>");
        for (on, state) in store.dependencies_of(&mission_id).unwrap_or_default() {
            let _ = write!(
                html,
                "<a href=\"/missions/{on}\">{short}</a> ({state}) ",
                short = on.as_str().get(..8).unwrap_or_default(),
                state = state.as_str()
            );
        }
        let blockers = self
            .ask(&json!({ "op": "mission.show", "mission": id }))
            .and_then(|shown| {
                shown
                    .get("blockers")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
            })
            .map_or_else(
                || "unknown (wsd unreachable)".to_owned(),
                |codes| {
                    let codes: Vec<String> = codes
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(escape)
                        .collect();
                    if codes.is_empty() {
                        "none".to_owned()
                    } else {
                        codes.join(", ")
                    }
                },
            );
        let _ = write!(html, "</dd><dt>Blocked by</dt><dd>{blockers}</dd></dl>");
        let requests = store.requests_of(id).unwrap_or_default();
        if !requests.is_empty() {
            html.push_str("<h2>Decision requests</h2><ul>");
            for request in &requests {
                let _ = write!(
                    html,
                    "<li>{} — {} <a href=\"/decisions\">decisions</a></li>",
                    escape(&request.question),
                    escape(&request.state)
                );
            }
            html.push_str("</ul>");
        }
        html.push_str("<h2>Result</h2>");
        match mission.result() {
            Some(result) => {
                let _ = write!(
                    html,
                    "<dl><dt>Commit</dt><dd><code>{}</code></dd><dt>Evidence digest</dt><dd><code>{}</code></dd><dt>Summary</dt><dd>{}</dd></dl>",
                    result.commit().as_str(),
                    result.evidence().to_hex(),
                    escape(result.summary())
                );
            }
            None => html.push_str("<p>No result submitted.</p>"),
        }
        if let Some(verdict) = mission.verdict() {
            let _ = write!(
                html,
                "<h2>Decision</h2><p>{}: {}</p>",
                verdict.state().as_str(),
                escape(verdict.reason())
            );
        }
        html.push_str("<h2>Runs</h2><table><thead><tr><th>Run</th><th>State</th><th>Output bytes</th><th>Output digest</th><th>Inputs</th><th>Exit</th></tr></thead><tbody>");
        for run in store.runs_of(id).unwrap_or_default() {
            let exit = match (run.exit_code, run.signal, run.budget) {
                (_, _, Some(budget)) => format!("budget {budget}"),
                (Some(code), _, None) => format!("code {code}"),
                (None, Some(signal), None) => format!("signal {signal}"),
                (None, None, None) => String::new(),
            };
            let _ = write!(
                html,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                run.run.get(..8).unwrap_or_default(),
                escape(&run.state),
                run.output_bytes,
                run.output_digest.unwrap_or_default(),
                run.inputs,
                escape(&exit)
            );
        }
        html.push_str("</tbody></table><h2>Notes</h2><ul>");
        for (at, text) in store.notes_of(id).unwrap_or_default() {
            let _ = write!(
                html,
                "<li><time>{}</time> {}</li>",
                escape(&at),
                escape(&text)
            );
        }
        let _ = write!(
            html,
            "</ul><h2>Decide</h2><form method=\"post\" action=\"/missions/{id}/decide\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><select name=\"decision\"><option value=\"accept\">accept</option><option value=\"reject\">reject</option><option value=\"abandon\">abandon</option><option value=\"cancel\">cancel</option></select> <textarea name=\"reason\" required></textarea> <button>Decide</button></form><h2>Add a note</h2><form method=\"post\" action=\"/missions/{id}/note\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><textarea name=\"text\" required></textarea> <button>Add</button></form>"
        );
        Page::new(200, layout(mission.title(), &html))
    }

    fn forward(&self, id: &str, request: serde_json::Value) -> Page {
        if work_supervision_domain_id(id).is_err() {
            return Page::refused(404, "request.not_found");
        }
        let socket = self.root.join("run").join("wsd.sock");
        let outcome = Client::connect(&socket).and_then(|mut client| client.request(&request));
        match outcome {
            Ok(_) => Page::redirect(&format!("/missions/{id}")),
            Err(error) => Page::refused(409, error.code()),
        }
    }
}

/// Label of a mission run by the fake agent: a simulation, never B′.
fn simulation_badge(mission: &work_supervision_domain::Mission) -> &'static str {
    match mission.executor() {
        work_supervision_domain::ExecutorProfile::Fake => {
            "<p class=\"badge\"><strong>Simulation — fake agent.</strong> Runs of this mission do not count toward B′.</p>"
        }
    }
}

fn work_supervision_domain_id(id: &str) -> Result<MissionId, ()> {
    MissionId::parse(id).map_err(|_| ())
}

/// 32 lowercase hexadecimal characters: the form of every identifier in a path.
fn is_identifier(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn nav() -> &'static str {
    "<nav><a href=\"/\">Missions</a> · <a href=\"/decisions\">Decisions</a> · <a href=\"/ideas\">Ideas</a> · <a href=\"/sessions\">Sessions</a></nav>"
}

const SECURITY_HEADERS: [(&str, &str); 5] = [
    (
        "Content-Security-Policy",
        "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    ("Cache-Control", "no-store"),
    ("X-Frame-Options", "DENY"),
];

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn layout(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title} — Work Supervision</title><style>:root{{--text:#1d1d1f;--muted:#5f6368;--line:#d0d4da;--accent:#0b57d0;--surface:#ffffff;--warn:#a50e0e;--caution:#8a5300}}@media (prefers-color-scheme: dark){{:root{{--text:#e8eaed;--muted:#9aa0a6;--line:#3c4043;--accent:#8ab4f8;--surface:#1f1f1f;--warn:#f28b82;--caution:#fdd663}}}}.muted{{color:var(--muted)}}.rev-irreversible{{color:var(--warn);font-weight:600}}.rev-costly{{color:var(--caution)}}section.request{{border-top:1px solid var(--line);margin-top:1rem}}nav{{margin-bottom:1rem}}body{{font:1rem/1.5 system-ui,sans-serif;color:var(--text);background:var(--surface);max-width:60rem;margin:0 auto;padding:1rem}}a{{color:var(--accent)}}table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid var(--line);padding:.25rem .5rem;text-align:left}}pre{{white-space:pre-wrap;border:1px solid var(--line);padding:.5rem}}dt{{color:var(--muted)}}textarea{{width:100%;min-height:3rem}}</style></head><body>{body}</body></html>",
        title = escape(title)
    )
}

fn login_page() -> String {
    layout(
        "Sign in",
        "<h1>Sign in</h1><p>Paste the token from <code>run/cockpit.token</code> in the root.</p><form method=\"post\" action=\"/login\"><input name=\"token\" type=\"password\" autocomplete=\"off\" required> <button>Sign in</button></form>",
    )
}

/// `application/x-www-form-urlencoded` → map (last value wins; invalid escapes refuse the pair).
fn parse_form(body: &[u8]) -> HashMap<String, String> {
    let Ok(text) = std::str::from_utf8(body) else {
        return HashMap::new();
    };
    text.split('&')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            Some((decode(name)?, decode(value)?))
        })
        .collect()
}

fn decode(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut iterator = text.bytes();
    while let Some(byte) = iterator.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = char::from(iterator.next()?).to_digit(16)?;
                let low = char::from(iterator.next()?).to_digit(16)?;
                bytes.push(u8::try_from(high * 16 + low).ok()?);
            }
            _ => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::{decode, escape, parse_form, same};

    #[test]
    fn forms_decode_escapes_and_html_is_escaped() {
        let form = parse_form(b"reason=checked+at+the+cockpit%21&csrf=ab&broken=%G1");
        assert_eq!(
            form.get("reason").map(String::as_str),
            Some("checked at the cockpit!")
        );
        assert_eq!(form.get("csrf").map(String::as_str), Some("ab"));
        assert!(!form.contains_key("broken"));
        assert_eq!(decode("%C3%A9"), Some("é".to_owned()));
        assert_eq!(
            escape("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }
}
