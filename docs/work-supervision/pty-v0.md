# Work Supervision v0 — runs, PTY and fake agent

Crates: `crates/work-supervision-pty` (sessions) and
`crates/work-supervision-fake-agent` (`ws-fake-agent`, the only executor
before the C0 confinement qualification).

## Session

- One fresh terminal per run (`portable-pty` 0.9.0), started in the mission
  worktree; a missing working directory is refused (`pty.cwd_invalid`), never
  replaced by another one.
- Environment **cleared**, then only `HOME` (inside the root), `LANG`
  (`C.UTF-8`), `PATH` (given by the caller), `SHELL` (`/bin/sh`, set because
  `portable-pty` always sets it) and `TERM` (`xterm-256color`).
- The child is a session and process-group leader (`setsid`).
- Output: every byte read from the terminal is appended to `runs/<run>/pty.log`
  (created, never reused) with a rolling SHA-256. A checkpoint — cumulative
  bytes and the digest of the whole log prefix, log synchronised first — is
  reported every 1 MiB or 2 s of output, and at the end.
- Input: written to the terminal, reported by length and SHA-256 only.
- Idle: with an idle delay set, `Idle` is reported after that delay without
  output, `Active` when output resumes.
- Budgets: wall-clock duration or output bytes overrun → `SIGTERM` to the
  group, then `SIGKILL` after the grace delay. When the leader exits by
  itself, what is left in its group is terminated the same way. A session
  ends only once the group is empty and the log is drained.
- Exit: exit code or signal number, bytes, final digest, overrun budget,
  whether `SIGKILL` was needed, whether it was cancelled.

## C0 guard

`executor_program` matches exhaustively over `ExecutorProfile`, whose only
variant is `Fake`; configuration names other than `fake` are refused upstream
with `agent.real_forbidden_until_c0` (`mission-v0.md`).

## Fake agent scenario

The scenario is the mission brief, one step per line: `print`, `flood <n>`,
`read-line`, `sleep <ms>`, `block`, `write-file <relative path> <text>`,
`commit <message>` (fixed fake identity, hooks disabled, no global or system
git configuration), `spawn-stubborn-child` (a child ignoring `SIGTERM` and
`SIGHUP`), `env`, `print-cwd`, `crash` (`SIGABRT`), `exit <code>`. Unknown
steps and paths leaving the working directory exit with code 2.
