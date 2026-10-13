#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `ws report --markdown`: rendering of a report, without a daemon.

use serde_json::json;
use work_supervision_cli::report::render;

fn sample() -> serde_json::Value {
    json!({
        "schema": "libre-ai.work-supervision.report.v0",
        "mission": { "id": "0123456789abcdef0123456789abcdef", "title": "Cache | parser",
                     "repository": "sample", "state": "result-submitted", "executor": "fake",
                     "simulation": true },
        "intent": {
            "brief": "print hello\n",
            "criteria": [
                { "index": 0, "text": "tests pass", "check": { "argv": ["cargo", "test"] },
                  "last_execution": { "commit": "a", "state": "finished", "exit_code": 0,
                                      "passed": true, "at_submitted_commit": true } },
                { "index": 1, "text": "docs updated", "check": null, "last_execution": null }
            ],
            "dependencies": [{ "mission": "fedcba9876543210fedcba9876543210", "state": "accepted" }],
            "scope": ["src/cache"]
        },
        "result": { "commit": "aaaa", "evidence_digest": "bbbb", "summary": "memoised" },
        "verdict": null,
        "evidence": { "scope_check": { "commit": "aaaa", "changed": 3, "outside": ["README.md"] },
                      "check_executions": [] },
        "runs": [{ "run": "r1", "state": "exited", "exit_code": 0, "signal": null,
                   "budget": null, "output_bytes": 10, "inputs": 1 }],
        "decisions": [{
            "question": "Which storage?", "state": "answered", "recommended": 0, "choice": 1,
            "reason": "needs sharing",
            "options": [
                { "label": "SQLite", "consequence": "No server.", "reversibility": "reversible" },
                { "label": "PostgreSQL", "consequence": "A server.", "reversibility": "costly" }
            ]
        }],
        "sessions": [{ "id": "s1", "harness": "claude-code", "state": "ended",
                       "reported_state": "waiting-input" }],
        "notes": [{ "text": "check the cache size", "at": "2026-10-09T10:00:00.000Z" }],
        "blockers": ["scope.violated"],
        "gaps": ["simulation", "criteria.unchecked", "scope.violated"],
        "timeline": [{ "seq": 1, "at": "2026-10-09T09:00:00.000Z", "kind": "mission.created" }]
    })
}

#[test]
fn a_report_renders_every_section_a_reader_needs() {
    let markdown = render(&sample());
    for expected in [
        "# Cache \\| parser",
        "state **result-submitted**",
        "> Simulation: the fake agent ran this mission. It does not count toward B′.",
        "No decision yet.",
        "Blocked by: `scope.violated`",
        "Gaps: `simulation`, `criteria.unchecked`, `scope.violated`",
        "| 0 | tests pass | `cargo test` | passed (exit 0), at the submitted commit |",
        "| 1 | docs updated | none | — |",
        "- `fedcba9876543210fedcba9876543210` (accepted)",
        "Scope: `src/cache`",
        "memoised",
        "Scope check at `aaaa`: 3 changed path(s), 1 outside the scope: `README.md`.",
        "| 0 | SQLite (recommended) | No server. | reversible |",
        "| 1 | **PostgreSQL** — chosen | A server. | costly |",
        "Reason: needs sharing",
        "## Agent sessions (declared, not verified)",
        "- 2026-10-09T10:00:00.000Z — check the cache size",
        "- 2026-10-09T09:00:00.000Z `#1` mission.created",
    ] {
        assert!(
            markdown.contains(expected),
            "missing {expected:?} in\n{markdown}"
        );
    }
}

#[test]
fn texts_from_agents_cannot_inject_html_or_links() {
    let mut report = sample();
    report["result"]["summary"] =
        json!("done <img src=x onerror=alert(1)> [click](javascript:alert(1))");
    report["notes"] = json!([{ "text": "<script>x</script>", "at": "t" }]);
    let markdown = render(&report);
    assert!(!markdown.contains("<img"));
    assert!(!markdown.contains("<script>"));
    assert!(!markdown.contains("[click](javascript"));
    assert!(
        markdown
            .contains("done &lt;img src=x onerror=alert(1)&gt; \\[click\\](javascript:alert(1))")
    );
}

#[test]
fn a_report_with_missing_sections_still_renders() {
    let markdown = render(&json!({ "mission": { "title": "Empty" } }));
    assert!(markdown.contains("# Empty"));
    assert!(markdown.contains("No result submitted."));
    assert!(markdown.contains("Scope: undeclared (whole repository)"));
    assert!(markdown.contains("Scope check: none recorded."));
}

#[test]
fn phases_render_with_the_governing_artifact_fenced_verbatim() {
    let mut report = sample();
    report["phases"] = json!([
        { "phase": "research", "artifact": { "id": "a1", "state": "approved",
          "digest": "d1", "submitted_by": "session:s1" } },
        { "phase": "outline", "artifact": null }
    ]);
    report["governing"] = json!({ "phase": "research", "decided_at": "2026-10-09T11:00:00.000Z",
                                   "content": "uses ``` and <script>x</script>" });
    let markdown = render(&report);
    for expected in [
        "## Phases",
        "| research | `a1` | approved | `d1` | session:s1 |",
        "| outline | — | missing | — | — |",
        "Governing artifact: **research**, approved at 2026-10-09T11:00:00.000Z",
        "````text\nuses ``` and <script>x</script>\n````",
    ] {
        assert!(
            markdown.contains(expected),
            "missing {expected:?} in\n{markdown}"
        );
    }
    report["governing"] = serde_json::Value::Null;
    assert!(render(&report).contains("No phase is approved yet."));
    report["phases"] = json!([]);
    assert!(!render(&report).contains("## Phases"));
}
