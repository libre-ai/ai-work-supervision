//! `ws hook <harness>`: translates a harness's own hook payload into the
//! declarative session bridge (`docs/work-supervision/harness-adapters-v0.md`).
//!
//! The translation is pure: [`translate`] reads a payload and says what to
//! report; `main` looks the session up, registers it when needed and sends
//! the report. The harness's session identifier never leaves this process:
//! only its SHA-256 is sent to the daemon.

use serde_json::Value;

/// What a hook event means for the session it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Report a state (`working`, `waiting-input`, `blocked`), with a short note.
    Report {
        /// Reported state.
        state: &'static str,
        /// Note naming why, never a message of the agent.
        note: Option<String>,
    },
    /// Declare the end of the session.
    End,
    /// Nothing to record for this event.
    Ignore,
}

/// A translated hook event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    /// The harness's own session identifier (hashed before it is sent).
    pub external_id: String,
    /// Working directory the harness reported, if any.
    pub cwd: Option<String>,
    /// What to record.
    pub action: Action,
}

fn text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

fn working() -> Action {
    Action::Report {
        state: "working",
        note: None,
    }
}

fn waiting(note: &str) -> Action {
    Action::Report {
        state: "waiting-input",
        note: Some(note.to_owned()),
    }
}

/// Keeps a note to a short machine word: hook payloads may carry messages
/// written by the agent, which are never recorded.
fn word(value: Option<&str>) -> String {
    value
        .filter(|word| {
            !word.is_empty()
                && word.len() <= 64
                && word
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
        .unwrap_or("notification")
        .to_owned()
}

/// The common lifecycle events of Claude Code and Codex hooks.
fn lifecycle(event: &str, payload: &Value) -> Action {
    match event {
        "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => working(),
        "Notification" => waiting(&word(text(payload, "notification_type"))),
        "PermissionRequest" => waiting("permission"),
        "Stop" => waiting("turn-complete"),
        "StopFailure" => Action::Report {
            state: "blocked",
            note: Some("stop-failure".to_owned()),
        },
        "SessionEnd" => Action::End,
        _ => Action::Ignore,
    }
}

/// Translates the payload of `harness` (`claude-code`, `codex`, `pi`).
///
/// # Errors
///
/// `hook.harness_unknown`, `hook.payload_invalid` (no session identifier).
pub fn translate(harness: &str, payload: &Value) -> Result<HookEvent, &'static str> {
    let invalid = "hook.payload_invalid";
    let cwd = text(payload, "cwd").map(str::to_owned);
    let (external_id, action) = match harness {
        "claude-code" => (
            text(payload, "session_id").ok_or(invalid)?,
            lifecycle(text(payload, "hook_event_name").ok_or(invalid)?, payload),
        ),
        // Codex: its lifecycle hooks carry the same fields as Claude Code's;
        // its `notify` program receives `{"type": "agent-turn-complete",
        // "thread-id": …}` as its last argument.
        "codex" => match text(payload, "type") {
            Some("agent-turn-complete") => (
                text(payload, "thread-id").ok_or(invalid)?,
                waiting("turn-complete"),
            ),
            Some(_) => (text(payload, "thread-id").ok_or(invalid)?, Action::Ignore),
            None => (
                text(payload, "session_id").ok_or(invalid)?,
                lifecycle(text(payload, "hook_event_name").ok_or(invalid)?, payload),
            ),
        },
        // Pi: the bridge extension sends `{"event", "session_id", "cwd"}`
        // with the names of Pi's extension events.
        "pi" => {
            let action = match text(payload, "event").ok_or(invalid)? {
                "session_start" | "agent_start" => working(),
                "agent_settled" => waiting("turn-complete"),
                "session_shutdown" => Action::End,
                _ => Action::Ignore,
            };
            (text(payload, "session_id").ok_or(invalid)?, action)
        }
        _ => return Err("hook.harness_unknown"),
    };
    if external_id.is_empty() {
        return Err(invalid);
    }
    Ok(HookEvent {
        external_id: external_id.to_owned(),
        cwd,
        action,
    })
}

/// The repository whose configured path contains `cwd`, by name: the longest
/// matching path wins, segment by segment.
#[must_use]
pub fn repository_of(cwd: &str, repositories: &[(String, std::path::PathBuf)]) -> Option<String> {
    let cwd = std::path::Path::new(cwd);
    repositories
        .iter()
        .filter(|(_, path)| cwd.starts_with(path))
        .max_by_key(|(_, path)| path.components().count())
        .map(|(name, _)| name.clone())
}
