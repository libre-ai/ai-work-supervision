# Work Supervision v0 — `wsd` and `ws`

Crates: `crates/work-supervision-daemon` (`wsd`, protocol client) and
`crates/work-supervision-cli` (`ws`).

## Root

`ws init --root <dir>` creates the root (mode 0700): `journal/`, `blobs/`,
`evidence/`, `worktrees/`, `runs/`, `run/` and a `config.toml` template (mode
0600). The root is always a parameter (`--root` or `WS_ROOT`); a relative path,
an existing path or a path inside a git working tree is refused
(`root.not_absolute`, `root.exists`, `root.inside_repository`).

`config.toml` is private and lives only in the root; it is read with a strict
TOML subset (`crates/work-supervision-daemon/src/config.rs`). A profile other
than `fake` stops the daemon before anything else
(`agent.real_forbidden_until_c0`).

## Daemon

`wsd --root <dir>` is the single writer of the root. Startup order (plan §2.7):

1. the journal is verified by the independent verifier (an invalid journal stops the start);
2. the writer opens it, quarantining a torn tail (`journal.recovered`);
3. the projection catches up;
4. unconfirmed worktree intents are reconciled;
5. runs still `running` are recorded `run.interrupted`; a mission still running
   leaves with `exit-run` (interrupted unless its run had exited); a ready
   mission whose worktree was created is provisioned; a terminal mission whose
   worktree remains is released (kept if accepted, archived otherwise).

Counts are written to standard error; no title, brief, criterion, note,
summary, reason or terminal output ever is.

Socket `run/wsd.sock`: mode 0600, in a 0700 directory, peer effective UID read
from the kernel (`SO_PEERCRED` on Linux, `getpeereid` on macOS) and compared
with the daemon's (`socket.peer_refused`). No network port is opened. Threads,
not an async runtime: one per connection and one per run, behind one lock.

Protocol: one JSON object per line, at most 4 MiB; responses
`{"data": …, "meta": {"op": …}}` or `{"error": {"code": …}}`.

## Mission flow

`mission.new` → `mission.ready` → `run` (on a ready mission: worktree at the
repository HEAD, then `mission.provisioned`; then `run-started` and a PTY with
the fake agent playing the brief) → idle output → `waiting-input`, `send` →
`running` → `run.exited` and `exited` → `result.submit` (commit = worktree
HEAD; the evidence file is stored by `ws` in `evidence/`) → `decide` accept
(refused while the worktree has changes), reject (worktree kept, `run` resumes),
abandon, cancel (stops the run first).

## Crash injection

In debug builds only, `WSD_FAULT=<point>` makes the daemon `SIGKILL` itself at
one of 14 points (`FAULT_POINTS`). The recovery test runs a mission to each
point, restarts the daemon and finishes the mission: the verifier is green, the
projection equals its reconstruction, no worktree is orphaned and the accepted
branch holds the result. Three more points cover the coordination primitives
(`idea-promotion-intended`, `idea-mission-created`, `check-started`); their
recovery is tested in `crates/work-supervision-daemon/tests/coordination.rs`
(`coordination-v0.md`). Startup recovery also interrupts checks left running,
confirms or aborts pending idea promotions and records missing scope checks;
`ws doctor` reports their counts.

## `ws`

Commands are listed in `crates/work-supervision-cli/src/main.rs`. `ws journal
verify` exits 0 / 1 / 2 / 3 like `ws-journal-verify`. `ws rebuild [--check]`
refuses while the daemon runs (`daemon.running`), rebuilds the projection from
the journal and the blobs, reports whether it equals the live one, and (without
`--check`) replaces it, keeping the previous one as `state.sqlite.previous`.

## `ws attach` (terminal interface)

`ratatui` 0.30.2 with its crossterm backend; the mission's terminal is replayed
from `runs/<run>/pty.log` through the `vt100` 0.16.2 emulator (VT choice of
the plan: MIT, like `ratatui`). Panels: mission list, terminal of the selected
mission, decision bar. Keys — list: `↑`/`↓` select, `r` ready, `g` run, `⏎`
attach, `u` result (evidence file, then summary), `a` accept, `x` reject,
`d` abandon, `c` cancel (each asks for a reason), `q` quit; attached: typed
text is sent with `⏎`, `Esc` returns to the list. A refusal is shown by its
code. The interface only sends requests to `wsd`; it never writes the journal.

## Head anchoring

The hash chain cannot see a complete, consistent rewrite of the journal. With
`[anchor] path = "<file outside the root>"` in the configuration, `wsd`
records the head (`seq` + digest, one JSON line, schema
`libre-ai.work-supervision.anchor.v0`, mode 0600, atomic rename) after every
request and every run event. At start, after the independent verification and
before opening the writer, it refuses:

- a journal whose entry at the anchored `seq` is missing or has another digest
  (`journal.anchor_mismatch`) — shorter, longer or same-length rewrites alike;
- a written journal without its anchor (`journal.anchor_missing`);
- an anchor path inside the root (`config.anchor_inside_root`).

`ws journal verify` reports the anchor (`matched`, `not-configured`) and exits 1
on a mismatch or a missing anchor.
