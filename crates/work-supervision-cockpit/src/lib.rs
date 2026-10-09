//! `ws-cockpit` — the web cockpit of Work Supervision v0 (owner decision Y30:
//! a Rust server rendering HTML, one stack).
//!
//! The cockpit **reads and decides**, nothing else:
//!
//! - it listens on `127.0.0.1` only and opens the projection read-only;
//! - pages show missions, results, evidence digests, run counters and digests,
//!   verdicts and notes — never terminal output, and no route reaches a PTY,
//!   a run log or an input;
//! - the two writes, a decision and a note, are sent to `wsd` over its socket,
//!   which journals them exactly as `ws decide` and `ws note` do.
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
            _ => Page::refused(404, "request.not_found"),
        }
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
        let mut rows = String::new();
        for mission in &missions {
            let worktree = store
                .worktree(mission.id().as_str())
                .ok()
                .flatten()
                .map(|row| row.state)
                .unwrap_or_default();
            let _ = write!(
                rows,
                "<tr><td><a href=\"/missions/{id}\">{short}</a></td><td>{title}</td><td class=\"state\">{state}</td><td>{worktree}</td><td>{revision}</td></tr>",
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
                    "<h1>Missions</h1><p>{count} mission(s).</p><table><thead><tr><th>Id</th><th>Title</th><th>State</th><th>Worktree</th><th>Revision</th></tr></thead><tbody>{rows}</tbody></table>",
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
        for criterion in mission.criteria() {
            let _ = write!(html, "<li>{}</li>", escape(criterion));
        }
        html.push_str("</ul><h2>Result</h2>");
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
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title} — Work Supervision</title><style>:root{{--text:#1d1d1f;--muted:#5f6368;--line:#d0d4da;--accent:#0b57d0;--surface:#ffffff}}@media (prefers-color-scheme: dark){{:root{{--text:#e8eaed;--muted:#9aa0a6;--line:#3c4043;--accent:#8ab4f8;--surface:#1f1f1f}}}}body{{font:1rem/1.5 system-ui,sans-serif;color:var(--text);background:var(--surface);max-width:60rem;margin:0 auto;padding:1rem}}a{{color:var(--accent)}}table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid var(--line);padding:.25rem .5rem;text-align:left}}pre{{white-space:pre-wrap;border:1px solid var(--line);padding:.5rem}}dt{{color:var(--muted)}}textarea{{width:100%;min-height:3rem}}</style></head><body>{body}</body></html>",
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
