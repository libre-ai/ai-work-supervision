# Work Supervision v0 — mission worktrees

Crate: `crates/work-supervision-worktree`.

## Mechanism (owner decision Y29, D-1, 2026-10-09)

The **system `git`, in a separate process**: the executable already installed
on the machine, neither linked nor redistributed. Every call:

- has a fixed argv, no shell;
- runs with an environment cleared to `GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_CONFIG_NOSYSTEM=1`, `LC_ALL=C`, `PATH=/usr/bin:/bin:/usr/local/bin`;
- disables hooks and fsmonitor (`-c core.hooksPath=/dev/null -c core.fsmonitor=false`).

## Lifecycle

Path `worktrees/<mission>` under the root, branch `ws/<mission>`.

| Step | Intent (journal) | Effect (git) | Confirmation (journal) |
| --- | --- | --- | --- |
| provision | `worktree.create.intent` | `worktree add -b ws/<mission> <path> <base>` | `worktree.created` with the HEAD read back (must be the base) |
| release, keep | `worktree.remove.intent` (`delete_branch: false`) | `worktree remove` | `worktree.removed`; the branch keeps the accepted result |
| release, abandon | `worktree.remove.intent` (`delete_branch: true`) | archive, `worktree remove`, `branch -D` | `worktree.archived`, `worktree.removed` |

- The base is verified before any intent (`worktree.base_unknown`).
- A worktree with changes cannot be released with *keep* (`worktree.dirty`;
  nothing is written). *Abandon* (also used for cancel) first archives the full
  diff from the base — commits and uncommitted, untracked included
  (`git add -A`, `git diff --cached --binary <base>`) — into the evidence blob
  store (`evidence/<sha256>`); only the digest and size enter the journal.

## Reconciliation (`reconcile`)

For each worktree projected in `creating` or `releasing`:

- `creating`: confirmed (`worktree.created`) if git lists the worktree, its
  HEAD is the base and the branch exists; otherwise every residue is removed
  (registered worktree, unregistered directory, the branch if it still points
  at the base) and the intent is closed with `worktree.create.aborted`;
- `releasing`: an abandon not yet archived is archived first, then the removal
  is finished (idempotent) and `worktree.removed` recorded.

The report counts confirmations, aborts, finished removals and intents still
pending (0 expected).

## Garbage collection (`gc`)

In each configured repository: worktrees registered under `worktrees/` without
a live record (`creating`, `created`, `releasing`) are removed, `ws/*` branches
are deleted unless a live record or a kept release (accepted) explains them,
and directories under `worktrees/` without a live record are removed. The
report counts removed worktrees and branches.

## Tests

`tests/lifecycle.rs`: 100 provision/release cycles with a crash injected at
each of the six points in turn (`AfterCreateIntent`, `DuringCreate` — a
directory left unregistered —, `AfterCreateEffect`, `AfterRemoveIntent`,
`AfterArchive`, `AfterRemoveEffect`), the process "dying" with its journal
lock after each crash, followed by reconciliation: 0 orphan worktree, 0 orphan
`ws/*` branch, empty `worktrees/` after every cycle (checked with the host git).
