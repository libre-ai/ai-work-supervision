//! `ws attach` — the terminal interface: mission list, the mission's terminal,
//! a decision bar.
//!
//! The interface is a pure state machine ([`App`]) turning keys into daemon
//! requests ([`Action`]), and a renderer ([`render`]); [`run`] wires both to a
//! real terminal (crossterm, raw mode, alternate screen) and to `wsd`. The
//! terminal panel replays the run's log through a VT emulator (`vt100`); the
//! log is read from the root, input goes through the daemon.
//!
//! Keys — list: `↑`/`↓` select, `r` ready, `g` run, `⏎` attach, `u` submit a
//! result (evidence file, then summary), `a` accept, `x` reject, `d` abandon,
//! `c` cancel (each asks for a reason), `q` quit. Attached: typed text is sent
//! with `⏎` (a newline is appended), `Esc` returns to the list.

use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::{Value, json};
use work_supervision_daemon::{Client, ClientError};
use work_supervision_store::BlobStore;

/// What a key asks the caller to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this request to the daemon and give the response to [`App::on_response`].
    Request(Value),
    /// Leave the interface.
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Prompt {
    Reason(&'static str),
    Evidence,
    Summary { evidence: String },
}

impl Prompt {
    const fn label(&self) -> &'static str {
        match self {
            Self::Reason(_) => "reason",
            Self::Evidence => "evidence file",
            Self::Summary { .. } => "summary",
        }
    }
}

#[derive(Debug, Clone)]
struct Row {
    id: String,
    title: String,
    state: String,
}

const TERMINAL_ROWS: u16 = 200;
const TERMINAL_COLS: u16 = 200;

/// State of the interface.
pub struct App {
    root: PathBuf,
    missions: Vec<Row>,
    selected: usize,
    preselect: Option<String>,
    focus: Focus,
    prompt: Option<Prompt>,
    buffer: String,
    status: String,
    terminal: vt100::Parser,
    log: Option<PathBuf>,
    offset: u64,
}

impl std::fmt::Debug for App {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("App")
            .field("selected", &self.selected)
            .field("focus", &self.focus)
            .finish_non_exhaustive()
    }
}

impl App {
    /// An interface over the root `root`, selecting `preselect` once listed.
    #[must_use]
    pub fn new(root: PathBuf, preselect: Option<String>) -> Self {
        Self {
            root,
            missions: Vec::new(),
            selected: 0,
            preselect,
            focus: Focus::List,
            prompt: None,
            buffer: String::new(),
            status: String::new(),
            terminal: vt100::Parser::new(TERMINAL_ROWS, TERMINAL_COLS, 0),
            log: None,
            offset: 0,
        }
    }

    /// Replaces the list with the `data` of a `mission.list` response.
    pub fn set_missions(&mut self, data: &Value) {
        let selected = self.selected_id().map(str::to_owned);
        self.missions = data
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(Row {
                            id: item.get("id")?.as_str()?.to_owned(),
                            title: item.get("title")?.as_str()?.to_owned(),
                            state: item.get("state")?.as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let wanted = self.preselect.take().or(selected);
        if let Some(index) = wanted.and_then(|id| self.missions.iter().position(|row| row.id == id))
        {
            self.selected = index;
        }
        self.selected = self.selected.min(self.missions.len().saturating_sub(1));
    }

    /// Follows the current run of the `data` of a `mission.show` response.
    pub fn set_detail(&mut self, data: &Value) {
        let log = data
            .get("current_run")
            .and_then(Value::as_str)
            .map(|run| self.root.join("runs").join(run).join("pty.log"));
        if log != self.log {
            self.log = log;
            self.offset = 0;
            self.terminal = vt100::Parser::new(TERMINAL_ROWS, TERMINAL_COLS, 0);
        }
    }

    /// Feeds the terminal panel with the log bytes not read yet.
    ///
    /// # Errors
    ///
    /// The log could not be read (an absent log is not an error: the run has
    /// not written yet).
    pub fn tail_log(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.log else {
            return Ok(());
        };
        let mut file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        self.offset += u64::try_from(bytes.len()).unwrap_or(0);
        self.feed_terminal(&bytes);
        Ok(())
    }

    /// Feeds raw terminal bytes to the panel's emulator.
    pub fn feed_terminal(&mut self, bytes: &[u8]) {
        self.terminal.process(bytes);
    }

    /// Text of the terminal panel, as displayed.
    #[must_use]
    pub fn terminal_text(&self) -> String {
        self.terminal.screen().contents()
    }

    /// Identifier of the selected mission.
    #[must_use]
    pub fn selected_id(&self) -> Option<&str> {
        self.missions.get(self.selected).map(|row| row.id.as_str())
    }

    /// State of the selected mission.
    #[must_use]
    pub fn selected_state(&self) -> Option<&str> {
        self.missions
            .get(self.selected)
            .map(|row| row.state.as_str())
    }

    /// The last refusal code, empty when the last request succeeded.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Records the outcome of a request.
    pub fn on_response(&mut self, request: &Value, response: Result<Value, ClientError>) {
        let _ = request;
        match response {
            Ok(_) => self.status.clear(),
            Err(error) => error.code().clone_into(&mut self.status),
        }
    }

    /// Handles one key.
    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if key.kind != KeyEventKind::Press {
            return Vec::new();
        }
        if self.prompt.is_some() {
            return self.on_prompt_key(key);
        }
        match self.focus {
            Focus::Terminal => self.on_terminal_key(key),
            Focus::List => self.on_list_key(key),
        }
    }

    fn mission_request(&self, op: &str) -> Vec<Action> {
        self.selected_id()
            .map(|id| vec![Action::Request(json!({ "op": op, "mission": id }))])
            .unwrap_or_default()
    }

    fn on_list_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.missions.len() {
                    self.selected += 1;
                }
                Vec::new()
            }
            KeyCode::Char('r') => self.mission_request("mission.ready"),
            KeyCode::Char('g') => self.mission_request("run"),
            KeyCode::Enter => {
                if self.selected_id().is_some() {
                    self.focus = Focus::Terminal;
                    self.buffer.clear();
                }
                Vec::new()
            }
            KeyCode::Char('u') => self.open(Prompt::Evidence),
            KeyCode::Char('a') => self.open(Prompt::Reason("accept")),
            KeyCode::Char('x') => self.open(Prompt::Reason("reject")),
            KeyCode::Char('d') => self.open(Prompt::Reason("abandon")),
            KeyCode::Char('c') => self.open(Prompt::Reason("cancel")),
            KeyCode::Char('q') => vec![Action::Quit],
            _ => Vec::new(),
        }
    }

    fn open(&mut self, prompt: Prompt) -> Vec<Action> {
        if self.selected_id().is_some() {
            self.prompt = Some(prompt);
            self.buffer.clear();
        }
        Vec::new()
    }

    fn on_terminal_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::List;
                self.buffer.clear();
                Vec::new()
            }
            KeyCode::Enter => {
                let text = format!("{}\n", std::mem::take(&mut self.buffer));
                self.selected_id()
                    .map(|id| {
                        vec![Action::Request(
                            json!({ "op": "send", "mission": id, "text": text }),
                        )]
                    })
                    .unwrap_or_default()
            }
            KeyCode::Backspace => {
                self.buffer.pop();
                Vec::new()
            }
            KeyCode::Char(character) => {
                self.buffer.push(character);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.buffer.clear();
                Vec::new()
            }
            KeyCode::Backspace => {
                self.buffer.pop();
                Vec::new()
            }
            KeyCode::Char(character) => {
                self.buffer.push(character);
                Vec::new()
            }
            KeyCode::Enter => {
                let Some(id) = self.selected_id().map(str::to_owned) else {
                    self.prompt = None;
                    return Vec::new();
                };
                let value = std::mem::take(&mut self.buffer);
                match self.prompt.take() {
                    Some(Prompt::Reason(decision)) => vec![Action::Request(json!({
                        "op": "decide", "mission": id, "decision": decision, "reason": value
                    }))],
                    Some(Prompt::Evidence) => {
                        self.prompt = Some(Prompt::Summary { evidence: value });
                        Vec::new()
                    }
                    Some(Prompt::Summary { evidence }) => {
                        match self.store_evidence(Path::new(&evidence)) {
                            Ok(digest) => vec![Action::Request(json!({
                                "op": "result.submit", "mission": id, "evidence_digest": digest, "summary": value
                            }))],
                            Err(code) => {
                                code.clone_into(&mut self.status);
                                Vec::new()
                            }
                        }
                    }
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn store_evidence(&self, path: &Path) -> Result<String, &'static str> {
        let bytes = fs::read(path).map_err(|_| "evidence.unreadable")?;
        let store = BlobStore::open(&self.root.join("evidence")).map_err(|error| error.code())?;
        Ok(store.put(&bytes).map_err(|error| error.code())?.to_hex())
    }
}

const HELP: &str = " ↑↓ select · r ready · g run · ⏎ attach · u result · a accept · x reject";
const HELP_MORE: &str = " d abandon · c cancel · q quit";

/// Draws the interface.
pub fn render(app: &App, frame: &mut Frame<'_>) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(frame.area());
    let [list, terminal] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body);
    let rows: Vec<Line<'_>> = app
        .missions
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let marker = if index == app.selected { '>' } else { ' ' };
            Line::from(format!("{marker} {:<14} {}", row.state, row.title))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(rows).block(Block::default().borders(Borders::ALL).title("Missions")),
        list,
    );
    let title = app.selected_id().map_or_else(
        || "Terminal".to_owned(),
        |id| format!("Terminal {}", id.get(..8).unwrap_or(id)),
    );
    let inner = Block::default().borders(Borders::ALL).inner(terminal);
    let screen = terminal_lines(app, inner);
    frame.render_widget(
        Paragraph::new(screen).block(Block::default().borders(Borders::ALL).title(title)),
        terminal,
    );
    let second = if let Some(prompt) = &app.prompt {
        format!(" {}: {}", prompt.label(), app.buffer)
    } else if app.focus == Focus::Terminal {
        format!(" > {}", app.buffer)
    } else if !app.status.is_empty() {
        format!(" refused: {}", app.status)
    } else {
        HELP_MORE.to_owned()
    };
    frame.render_widget(
        Paragraph::new(vec![Line::from(HELP), Line::from(second)]),
        footer,
    );
}

/// The last rows of the emulated screen that fit in `area`.
fn terminal_lines(app: &App, area: Rect) -> Vec<Line<'static>> {
    let rows: Vec<String> = app.terminal.screen().rows(0, area.width).collect();
    let used = rows
        .iter()
        .rposition(|row| !row.trim().is_empty())
        .map_or(0, |last| last + 1);
    let height = usize::from(area.height);
    let start = used.saturating_sub(height);
    rows.into_iter()
        .skip(start)
        .take(height)
        .map(Line::from)
        .collect()
}

/// Runs the interface on the real terminal until `q`.
///
/// # Errors
///
/// A code: `transport.*` when the daemon cannot be reached, `tui.terminal`
/// when the terminal cannot be driven.
pub fn run(root: &Path, preselect: Option<String>) -> Result<(), String> {
    let socket = root.join("run").join("wsd.sock");
    let mut client = Client::connect(&socket).map_err(|error| error.code().to_owned())?;
    let mut terminal = ratatui::try_init().map_err(|_| "tui.terminal".to_owned())?;
    let outcome = event_loop(
        &mut terminal,
        &mut client,
        App::new(root.to_owned(), preselect),
    );
    ratatui::try_restore().map_err(|_| "tui.terminal".to_owned())?;
    outcome
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    client: &mut Client,
    mut app: App,
) -> Result<(), String> {
    let mut refreshed = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    loop {
        if refreshed.elapsed() >= Duration::from_millis(300) {
            refresh(&mut app, client)?;
            refreshed = Instant::now();
        }
        terminal
            .draw(|frame| render(&app, frame))
            .map_err(|_| "tui.terminal".to_owned())?;
        if !event::poll(Duration::from_millis(100)).map_err(|_| "tui.terminal".to_owned())? {
            continue;
        }
        if let Event::Key(key) = event::read().map_err(|_| "tui.terminal".to_owned())? {
            for action in app.on_key(key) {
                match action {
                    Action::Quit => return Ok(()),
                    Action::Request(request) => {
                        let response = client.request(&request);
                        app.on_response(&request, response);
                        refreshed = Instant::now()
                            .checked_sub(Duration::from_secs(1))
                            .unwrap_or_else(Instant::now);
                    }
                }
            }
        }
    }
}

fn refresh(app: &mut App, client: &mut Client) -> Result<(), String> {
    let list = client
        .request(&json!({ "op": "mission.list" }))
        .map_err(|error| error.code().to_owned())?;
    app.set_missions(&list);
    if let Some(id) = app.selected_id().map(str::to_owned)
        && let Ok(shown) = client.request(&json!({ "op": "mission.show", "mission": id }))
    {
        app.set_detail(&shown);
    }
    app.tail_log().map_err(|_| "tui.log_unreadable".to_owned())
}
