//! Markdown rendering of a mission report (`ws report --markdown`).
//!
//! The JSON report (`libre-ai.work-supervision.report.v0`) is the contract;
//! this is its rendering for a reader who has not followed the history. It
//! adds nothing the JSON does not hold.

use std::fmt::Write as _;

use serde_json::Value;

fn text(value: &Value) -> String {
    match value {
        Value::Null => "—".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Escapes a text for a Markdown table cell or a list item. Texts come from
/// agents and sessions: raw HTML and link syntax are neutralised, so a
/// renderer that does not sanitise shows them as text.
fn cell(value: &Value) -> String {
    let mut escaped = String::new();
    for character in text(value).chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '|' | '[' | ']' | '`' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\n' | '\r' => escaped.push(' '),
            other => escaped.push(other),
        }
    }
    escaped
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// The value at `keys` under `value`, `null` when a key is missing.
fn path<'a>(value: &'a Value, keys: &[&str]) -> &'a Value {
    static NULL: Value = Value::Null;
    keys.iter()
        .try_fold(value, |value, key| value.get(key))
        .unwrap_or(&NULL)
}

/// A fenced code block holding `content` verbatim: the fence is longer than
/// any backtick run inside, so the content can neither close it nor be read as
/// Markdown or HTML.
fn fenced(content: &str) -> String {
    let longest = content
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let content = content.strip_suffix('\n').unwrap_or(content);
    format!("{fence}text\n{content}\n{fence}")
}

/// The declared phases, their current artifacts and the governing one
/// (`docs/work-supervision/phases-v0.md`).
fn phases(out: &mut String, report: &Value) {
    let declared = list(&report["phases"]);
    if declared.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n## Phases\n");
    let _ = writeln!(out, "| Phase | Artifact | State | SHA-256 | By |");
    let _ = writeln!(out, "| --- | --- | --- | --- | --- |");
    for phase in declared {
        let artifact = &phase["artifact"];
        if artifact.is_null() {
            let _ = writeln!(out, "| {} | — | missing | — | — |", text(&phase["phase"]));
        } else {
            let _ = writeln!(
                out,
                "| {} | `{}` | {} | `{}` | {} |",
                text(&phase["phase"]),
                text(&artifact["id"]),
                text(&artifact["state"]),
                text(&artifact["digest"]),
                cell(&artifact["submitted_by"]),
            );
        }
    }
    let governing = &report["governing"];
    if governing.is_null() {
        let _ = writeln!(out, "\nNo phase is approved yet.");
    } else {
        let _ = writeln!(
            out,
            "\nGoverning artifact: **{}**, approved at {} (a later phase prevails over an earlier one).\n",
            text(&governing["phase"]),
            text(&governing["decided_at"]),
        );
        let _ = writeln!(out, "{}", fenced(&text(&governing["content"])));
    }
}

/// Renders `report` as Markdown.
#[must_use]
pub fn render(report: &Value) -> String {
    let mut out = String::new();
    let mission = &report["mission"];
    let _ = writeln!(out, "# {}", cell(&mission["title"]));
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Mission `{}` · repository `{}` · state **{}** · executor `{}`",
        text(&mission["id"]),
        text(&mission["repository"]),
        text(&mission["state"]),
        text(&mission["executor"]),
    );
    if mission["simulation"] == true {
        let _ = writeln!(
            out,
            "\n> Simulation: the fake agent ran this mission. It does not count toward B′."
        );
    }

    let _ = writeln!(out, "\n## Outcome\n");
    let verdict = &report["verdict"];
    if verdict.is_null() {
        let _ = writeln!(out, "No decision yet.");
    } else {
        let _ = writeln!(
            out,
            "Decision: **{}** — {}",
            text(&verdict["state"]),
            cell(&verdict["reason"])
        );
    }
    let blockers = list(&report["blockers"]);
    if !blockers.is_empty() {
        let codes: Vec<String> = blockers.iter().map(text).collect();
        let _ = writeln!(out, "\nBlocked by: `{}`", codes.join("`, `"));
    }
    let gaps = list(&report["gaps"]);
    if !gaps.is_empty() {
        let codes: Vec<String> = gaps.iter().map(text).collect();
        let _ = writeln!(out, "\nGaps: `{}`", codes.join("`, `"));
    }

    let _ = writeln!(out, "\n## What was asked\n");
    let _ = writeln!(
        out,
        "```text\n{}\n```",
        text(path(report, &["intent", "brief"])).trim_end()
    );
    let criteria = list(path(report, &["intent", "criteria"]));
    if !criteria.is_empty() {
        let _ = writeln!(out, "\n| # | Criterion | Check | Last execution |");
        let _ = writeln!(out, "| --- | --- | --- | --- |");
        for criterion in criteria {
            let check = if criterion["check"].is_null() {
                "none".to_owned()
            } else {
                format!(
                    "`{}`",
                    list(path(criterion, &["check", "argv"]))
                        .iter()
                        .map(text)
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            };
            let last = &criterion["last_execution"];
            let execution = if last.is_null() {
                "—".to_owned()
            } else {
                let verdict = if last["passed"] == true {
                    "passed"
                } else {
                    "not passed"
                };
                let at = if last["at_submitted_commit"] == true {
                    "at the submitted commit"
                } else {
                    "at another commit"
                };
                format!("{verdict} (exit {}), {at}", text(&last["exit_code"]))
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                text(&criterion["index"]),
                cell(&criterion["text"]),
                check.replace('|', "\\|"),
                execution
            );
        }
    }
    let dependencies = list(path(report, &["intent", "dependencies"]));
    if !dependencies.is_empty() {
        let _ = writeln!(out, "\nDepends on:");
        for dependency in dependencies {
            let _ = writeln!(
                out,
                "- `{}` ({})",
                text(&dependency["mission"]),
                text(&dependency["state"])
            );
        }
    }
    let scope = path(report, &["intent", "scope"]);
    let _ = writeln!(
        out,
        "\nScope: {}",
        if scope.is_null() {
            "undeclared (whole repository)".to_owned()
        } else {
            list(scope)
                .iter()
                .map(|path| format!("`{}`", text(path)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );

    phases(&mut out, report);

    let _ = writeln!(out, "\n## What was done\n");
    let result = &report["result"];
    if result.is_null() {
        let _ = writeln!(out, "No result submitted.");
    } else {
        let _ = writeln!(out, "{}", cell(&result["summary"]));
        let _ = writeln!(
            out,
            "\nCommit `{}` · evidence `{}`",
            text(&result["commit"]),
            text(&result["evidence_digest"])
        );
    }
    let runs = list(&report["runs"]);
    if !runs.is_empty() {
        let _ = writeln!(
            out,
            "\n| Run | State | Exit | Signal | Budget overrun | Inputs |"
        );
        let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- |");
        for run in runs {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} |",
                text(&run["run"]),
                text(&run["state"]),
                text(&run["exit_code"]),
                text(&run["signal"]),
                text(&run["budget"]),
                text(&run["inputs"])
            );
        }
    }

    let _ = writeln!(out, "\n## Evidence\n");
    let scope_check = path(report, &["evidence", "scope_check"]);
    if scope_check.is_null() {
        let _ = writeln!(out, "Scope check: none recorded.");
    } else {
        let outside: Vec<String> = list(&scope_check["outside"]).iter().map(text).collect();
        let _ = writeln!(
            out,
            "Scope check at `{}`: {} changed path(s), {} outside the scope{}",
            text(&scope_check["commit"]),
            text(&scope_check["changed"]),
            outside.len(),
            if outside.is_empty() {
                ".".to_owned()
            } else {
                format!(": `{}`.", outside.join("`, `"))
            }
        );
    }

    let decisions = list(&report["decisions"]);
    if !decisions.is_empty() {
        let _ = writeln!(out, "\n## Decisions\n");
        for decision in decisions {
            let _ = writeln!(
                out,
                "### {} ({})\n",
                cell(&decision["question"]),
                text(&decision["state"])
            );
            let _ = writeln!(out, "| # | Option | Consequence | Reversibility |");
            let _ = writeln!(out, "| --- | --- | --- | --- |");
            for (position, option) in list(&decision["options"]).iter().enumerate() {
                let mut label = cell(&option["label"]);
                if decision["recommended"].as_u64() == u64::try_from(position).ok() {
                    label.push_str(" (recommended)");
                }
                if decision["choice"].as_u64() == u64::try_from(position).ok() {
                    label = format!("**{label}** — chosen");
                }
                let _ = writeln!(
                    out,
                    "| {position} | {label} | {} | {} |",
                    cell(&option["consequence"]),
                    text(&option["reversibility"])
                );
            }
            if !decision["reason"].is_null() {
                let _ = writeln!(out, "\nReason: {}", cell(&decision["reason"]));
            }
        }
    }

    let sessions = list(&report["sessions"]);
    if !sessions.is_empty() {
        let _ = writeln!(out, "\n## Agent sessions (declared, not verified)\n");
        for session in sessions {
            let _ = writeln!(
                out,
                "- `{}` · {} · {} · last reported: {}",
                text(&session["id"]),
                text(&session["harness"]),
                text(&session["state"]),
                text(&session["reported_state"])
            );
        }
    }

    let notes = list(&report["notes"]);
    if !notes.is_empty() {
        let _ = writeln!(out, "\n## Notes\n");
        for note in notes {
            let _ = writeln!(out, "- {} — {}", text(&note["at"]), cell(&note["text"]));
        }
    }

    let _ = writeln!(out, "\n## Timeline\n");
    for entry in list(&report["timeline"]) {
        let _ = writeln!(
            out,
            "- {} `#{}` {}",
            text(&entry["at"]),
            text(&entry["seq"]),
            text(&entry["kind"])
        );
    }
    out
}
