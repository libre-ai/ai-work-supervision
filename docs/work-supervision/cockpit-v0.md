# Work Supervision v0 — web cockpit

Crate: `crates/work-supervision-cockpit` (`ws-cockpit --root <dir> [--port <n>]`).
Owner decision Y30 (D-2, 2026-10-09): a Rust server rendering HTML, one stack;
the React cockpit of the frozen `apps/missions` is not reused.

## What it does — read and decide

- Listens on `127.0.0.1` only (`tiny_http` 0.12.0); port 0 picks an ephemeral one.
- Reads the projection read-only: mission list; per mission the brief,
  criteria, result (commit, evidence digest, summary), decision, runs (state,
  output bytes and digest, inputs, exit) and notes.
- Writes only through `wsd`: a decision (`accept`, `reject`, `abandon`,
  `cancel`, with a reason) and a note, journalled as `ws decide` / `ws note`.
- Coordination (`coordination-v0.md`): `/decisions` shows the open decision
  requests with their options side by side — label, consequence,
  reversibility, recommendation — and answers or withdraws them; `/ideas`
  lists deferred ideas and promotes or dismisses the open ones; `/sessions`
  lists declared agent sessions, labelled as declared, never verified. A
  mission page shows its scope, dependencies, checks with their last
  execution, requests and blockers. Blockers are asked of `wsd` (they depend
  on running checks and other worktrees) and shown unknown when it cannot be
  reached.
- **No bridge from the browser to a terminal**: no route reads a run log, a
  PTY or sends input; no page contains terminal output (tested with an output
  marker that only the agent's terminal holds).

## Request policy

| Check | Refusal |
| --- | --- |
| `Host` is `127.0.0.1:<port>` or `localhost:<port>` (DNS rebinding) | 421 `request.host_refused` |
| POST: `Origin` is the cockpit's own | 403 `request.origin_refused` |
| POST: `Sec-Fetch-Site`, when present, is `same-origin` | 403 `request.cross_site` |
| a session cookie (`HttpOnly`, `SameSite=Strict`) from posting the token of `run/cockpit.token` (0600, new at each start) | GET → 303 `/login`; POST → 403 `session.required` |
| POST: the session's CSRF token | 403 `session.csrf_refused` |

Tokens are compared in constant time. Every response carries
`Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline';
form-action 'self'; frame-ancestors 'none'; base-uri 'none'` (no script at
all), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
`Cache-Control: no-store`, `X-Frame-Options: DENY`. All texts are HTML-escaped.
