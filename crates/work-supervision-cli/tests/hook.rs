#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `ws hook`: translation of harness payloads, without a daemon.

use std::path::PathBuf;

use serde_json::json;
use work_supervision_cli::hook::{Action, repository_of, translate};

fn report(state: &'static str, note: Option<&str>) -> Action {
    Action::Report {
        state,
        note: note.map(str::to_owned),
    }
}

#[test]
fn claude_code_lifecycle_events_map_to_session_reports() {
    let cases = [
        ("SessionStart", report("working", None)),
        ("UserPromptSubmit", report("working", None)),
        ("PreToolUse", report("working", None)),
        (
            "PermissionRequest",
            report("waiting-input", Some("permission")),
        ),
        ("Stop", report("waiting-input", Some("turn-complete"))),
        ("StopFailure", report("blocked", Some("stop-failure"))),
        ("SessionEnd", Action::End),
        ("PreCompact", Action::Ignore),
    ];
    for (event, expected) in cases {
        let payload =
            json!({ "hook_event_name": event, "session_id": "abc-123", "cwd": "/w/repo" });
        let translated = translate("claude-code", &payload).unwrap();
        assert_eq!(translated.action, expected, "{event}");
        assert_eq!(translated.external_id, "abc-123");
        assert_eq!(translated.cwd.as_deref(), Some("/w/repo"));
    }
}

#[test]
fn a_notification_keeps_its_type_and_never_its_message() {
    let payload = json!({
        "hook_event_name": "Notification", "session_id": "s",
        "notification_type": "permission_prompt",
        "message": "Claude needs your permission to read the secret plan",
    });
    let translated = translate("claude-code", &payload).unwrap();
    assert_eq!(
        translated.action,
        report("waiting-input", Some("permission_prompt"))
    );
    // A type that is not a short machine word is not recorded as such.
    let odd = json!({
        "hook_event_name": "Notification", "session_id": "s",
        "notification_type": "free text with spaces",
    });
    assert_eq!(
        translate("claude-code", &odd).unwrap().action,
        report("waiting-input", Some("notification"))
    );
}

#[test]
fn codex_notify_and_codex_hooks_are_both_understood() {
    let notify = json!({ "type": "agent-turn-complete", "thread-id": "t-1", "cwd": "/w",
                         "last-assistant-message": "done" });
    let translated = translate("codex", &notify).unwrap();
    assert_eq!(
        (translated.external_id.as_str(), translated.action),
        ("t-1", report("waiting-input", Some("turn-complete")))
    );
    let other = json!({ "type": "something-else", "thread-id": "t-1" });
    assert_eq!(translate("codex", &other).unwrap().action, Action::Ignore);
    let hook = json!({ "hook_event_name": "SessionEnd", "session_id": "s-2" });
    assert_eq!(translate("codex", &hook).unwrap().action, Action::End);
}

#[test]
fn pi_bridge_events_map_to_session_reports() {
    let cases = [
        ("session_start", report("working", None)),
        ("agent_start", report("working", None)),
        (
            "agent_settled",
            report("waiting-input", Some("turn-complete")),
        ),
        ("session_shutdown", Action::End),
        ("message_update", Action::Ignore),
    ];
    for (event, expected) in cases {
        let payload = json!({ "event": event, "session_id": "pi-1" });
        assert_eq!(
            translate("pi", &payload).unwrap().action,
            expected,
            "{event}"
        );
    }
}

#[test]
fn payloads_without_an_identifier_and_unknown_harnesses_are_refused() {
    assert_eq!(
        translate("claude-code", &json!({ "hook_event_name": "Stop" })),
        Err("hook.payload_invalid")
    );
    assert_eq!(
        translate(
            "claude-code",
            &json!({ "hook_event_name": "Stop", "session_id": "" })
        ),
        Err("hook.payload_invalid")
    );
    assert_eq!(
        translate("pi", &json!({ "session_id": "x" })),
        Err("hook.payload_invalid")
    );
    assert_eq!(
        translate("gemini", &json!({ "session_id": "x" })),
        Err("hook.harness_unknown")
    );
}

#[test]
fn the_repository_of_a_working_directory_is_the_longest_configured_path() {
    let repositories = vec![
        ("outer".to_owned(), PathBuf::from("/w/repos")),
        ("inner".to_owned(), PathBuf::from("/w/repos/inner")),
        ("other".to_owned(), PathBuf::from("/w/other")),
    ];
    assert_eq!(
        repository_of("/w/repos/inner/src", &repositories).as_deref(),
        Some("inner")
    );
    assert_eq!(
        repository_of("/w/repos/x", &repositories).as_deref(),
        Some("outer")
    );
    // Segment boundaries: `/w/other-repo` is not under `/w/other`.
    assert_eq!(repository_of("/w/other-repo", &repositories), None);
}
