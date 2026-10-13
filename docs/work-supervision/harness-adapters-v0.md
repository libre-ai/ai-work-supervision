# Work Supervision v0 — harness adapters (Claude Code, Codex, Pi)

Two different integrations, with two different authorities:

1. **The declarative bridge** (delivered): sessions the owner launches
   *outside* the factory report to `wsd` through `ws hook`. The factory
   launches nothing; a reported state is never a verified result; these
   sessions do not count toward B′ (`coordination-v0.md`).
2. **The executor adapter contract** (specified, not enabled): what a real
   executor profile must provide for the factory to launch an agent itself.
   `ExecutorProfile` keeps its single `Fake` variant; enabling a profile is a
   reviewed code change made after the C0 confinement qualification is green
   (ADR-0042 §3 in `libre-ai/project-governance`, `mission-v0.md`). The tests
   assert that `claude-code`, `codex` and `pi` are refused as executor names
   (`agent.real_forbidden_until_c0`).

Facts below were read on 2026-10-09 from the installed binaries (`--help`,
`--version`) and their published documentation; they were not observed by
running an agent. Versions read: Claude Code 2.1.295, Codex CLI 0.160.1, Pi
0.84.2. Re-read them before relying on a flag: harness interfaces change
between versions.

## 1. The bridge: `ws hook <harness>`

`ws hook` reads the harness's payload on standard input (or as its last
argument, for Codex's `notify`), looks the session up by the SHA-256 of the
harness's own session identifier, registers it on first sight (repository
from the configured path containing `cwd`, mission when `cwd` is a mission
worktree), then reports. It writes nothing on standard output — Claude Code
adds a hook's standard output to the model's context on some events — and
always exits 0, so a harness is never stopped by its supervision. A
notification keeps its type (a short machine word), never the agent's message.

| Harness event | Reported |
| --- | --- |
| session start, prompt submitted, tool use | `working` |
| notification (Claude Code) | `waiting-input`, note = `notification_type` |
| permission request | `waiting-input`, note `permission` |
| turn complete (Stop, `agent-turn-complete`, `agent_settled`) | `waiting-input`, note `turn-complete` |
| stop failure (Claude Code) | `blocked`, note `stop-failure` |
| session end / shutdown | session ended |
| anything else | nothing |

The root is given to `ws` by `--root` or `WS_ROOT`; the examples below use
`--root "$WS_ROOT"` with the variable set in the harness's environment.

### Claude Code

Hooks are commands configured under `hooks` in a settings file
(`~/.claude/settings.json`, a project's `.claude/settings.json` or
`.claude/settings.local.json`, or a file passed with `--settings`). Each
receives a JSON object on standard input with `session_id`, `cwd` and
`hook_event_name`; `Notification` adds `notification_type`. `--bare`
disables hooks.

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "ws --root \"$WS_ROOT\" hook claude-code", "timeout": 10 }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "ws --root \"$WS_ROOT\" hook claude-code", "timeout": 10 }] }],
    "Notification": [{ "hooks": [{ "type": "command", "command": "ws --root \"$WS_ROOT\" hook claude-code", "timeout": 10 }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "ws --root \"$WS_ROOT\" hook claude-code", "timeout": 10 }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "ws --root \"$WS_ROOT\" hook claude-code", "timeout": 10 }] }]
  }
}
```

Installing this is a change of the owner's harness configuration: it is the
owner's act, not something `ws` does.

### Codex

Two mechanisms, both understood by `ws hook codex`:

- lifecycle hooks (`~/.codex/hooks.json`, `[hooks]` in `~/.codex/config.toml`,
  or the same files under a repository's `.codex/`), whose standard input
  carries `session_id`, `cwd` and `hook_event_name` (`SessionStart`,
  `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `PostToolUse`,
  `Stop`, `SessionEnd`, …). Codex requires each hook to be trusted by its hash
  (`/hooks`) before it runs;
- `notify`, a top-level argument vector in `config.toml`, called at the end of
  each turn with one JSON argument `{"type": "agent-turn-complete",
  "thread-id": …, "cwd": …}`:

```toml
notify = ["ws", "--root", "/absolute/path/to/the/root", "hook", "codex"]
```

`notify` sees turn ends only; approvals are seen through the
`PermissionRequest` hook.

### Pi

Pi has no hook file: it is extended by TypeScript extensions that subscribe
to events with `pi.on(…)` and can run a program with `pi.exec(…)`. The bridge
contract for Pi is a payload `{"event": <Pi event name>, "session_id": …,
"cwd": …}` sent to `ws hook pi` on standard input, with the events
`session_start`, `agent_start`, `agent_settled` (no automatic continuation
left: the agent waits for its user) and `session_shutdown`, passed as the last
argument (`pi.exec` has no standard input).

The extension is `adapters/pi/ws-bridge.ts`: load it with `pi -e
adapters/pi/ws-bridge.ts` or from `~/.pi/agent/extensions/`, with `WS_ROOT`
set (and `WS_BIN` when `ws` is not on `PATH`); without `WS_ROOT` it does
nothing, and a report that fails or hangs (bounded to 5 s) never fails the
session. Its tests (`bun run check:adapters`) drive it with a fake `pi` and,
end to end, against the real `ws` and `wsd`. It is typed against the subset of
the API it uses; on 2026-10-09 that subset was checked to accept the
`ExtensionAPI` of Pi 0.84.2 with TypeScript 7.0.2 (a misuse of the same types
was refused by the same check). It has not been run inside Pi. Pi includes no
sandbox and no approval mechanism; on this machine its RPC mode is reserved to
the gondolin launcher, which is the confinement C0 qualifies.

## 2. The executor adapter contract (after C0)

What a real executor profile must define before the factory may launch it,
in addition to the C0 qualification of the confinement it runs in:

| Element | Requirement | Claude Code | Codex | Pi |
| --- | --- | --- | --- | --- |
| Launch | a non-interactive or PTY form taking the brief, started by the confinement launcher, never by `wsd` directly | `claude -p <prompt>` (stream-json input and output available) | `codex exec [PROMPT]` (stdin with `-`), `--json` events | `pi -p`, `--mode json`; `--mode rpc` only through the gondolin launcher |
| Environment | cleared, then an allowlist; provider credentials through the confinement's secret substitution, never in the PTY environment | `ANTHROPIC_API_KEY` or a settings `apiKeyHelper` | `OPENAI_API_KEY` / `CODEX_API_KEY` (semantics of the latter not verified) | per provider (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …) |
| Waiting for input | observable without reading the agent's text: the PTY idle signal, and the harness's own events through the bridge | `Notification`, `PermissionRequest`, `Stop` | `PermissionRequest`, `Stop`, `notify` | `agent_settled`, RPC `extension_ui_request` |
| Approvals | the harness's own approval flow either disabled in favour of the confinement, or routed to a decision request — never auto-approved | `--permission-mode`, `--permission-prompts none` refuses | `--sandbox`, hooks may answer `PermissionRequest` | none built in |
| Result | a commit in the mission worktree and an evidence file; the summary is the owner's to read, the checks are what verifies | — | `-o <file>` writes the last message | — |
| Identity | the harness's session identifier hashed into the run, so bridge reports and runs meet | `session_id` | `session_id` / `thread-id` | session file / `session_id` |

The profile, its argument template and its environment allowlist will be
code in `crates/work-supervision-pty` and `crates/work-supervision-domain`,
reviewed with the C0 evidence; never a configuration value.
