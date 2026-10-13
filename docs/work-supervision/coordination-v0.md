# Work Supervision v0 — coordination primitives

Crates: `crates/work-supervision-domain` (pure rules, modules `coordination`
and `scope`), `crates/work-supervision-store` (migration 0004, projection),
`crates/work-supervision-daemon` (operations and guards),
`crates/work-supervision-cli` (`ws` commands, `ws report`, `ws hook`) and
`crates/work-supervision-cockpit` (read and decide).

These primitives make several missions, and the agent sessions that work on
them, followable without reconstructing what happened by hand. They are
harness-agnostic: nothing here names a model provider, and nothing here widens
the executor profiles — the C0 guard (`mission-v0.md`) is unchanged.

## Actors and authority

Every request carries an actor, `owner` (the default) or `session:<id>` (an
agent session declared through the bridge, below). The socket admits only the
daemon's own UID (`daemon-v0.md`), so **the actor is declarative, not
authenticated**: a process of the same user can claim to be the owner. The
boundary that makes it hold is the C0 confinement, which keeps an agent from
reaching the socket; until then the attribution is a record of what each
client declared, and the rules below are what the daemon enforces on that
declaration.

| Operation | `owner` | `session:<id>` |
| --- | --- | --- |
| capture, qualify an idea | yes | yes (its own session must be active) |
| promote, dismiss an idea | yes | refused `actor.owner_only` |
| open a decision request | yes | yes |
| answer a decision request | yes | refused `actor.owner_only` |
| withdraw a decision request | yes | only one it opened |
| declare dependency, scope, check | yes | refused `actor.owner_only` |
| split an approved outline into missions | yes | refused `actor.owner_only` |
| run checks, decide on a mission | yes (unchanged) | not exposed to sessions |
| register, report, end a session | — | its own session |

## Ideas — deferred capture

An idea is recorded without interrupting the current mission, qualified later,
then promoted to a mission or dismissed.

| From | Command | To | Event |
| --- | --- | --- | --- |
| — | capture | `captured` | `idea.captured` |
| `captured`, `qualified` | qualify | `qualified` | `idea.qualified` |
| `captured`, `qualified` | promote (intent) | `promoting` | `idea.promotion.intent` |
| `promoting` | promote (confirmed) | `promoted` | `idea.promoted` |
| `promoting` | promote (aborted) | `qualified` if ever qualified, else `captured` | `idea.promotion.aborted` |
| `captured`, `qualified` | dismiss | `dismissed` | `idea.dismissed` |

Promotion writes three entries: the intent naming the new mission identifier,
`mission.created` with that identifier, then `idea.promoted`. At start (and in
`ws doctor`) an idea left `promoting` is confirmed when its mission exists and
aborted otherwise, so a crash never leaves a duplicate mission nor an idea
stuck in `promoting`. The brief of the promoted mission is the idea text unless
another brief is given; its repository is the one given, or the one the idea
was qualified with (`idea.repository_required` otherwise). The mission is
validated before the intent is written. Crash points `idea-promotion-intended`
and `idea-mission-created` (debug builds, `daemon-v0.md`) cover both windows.

**Qualifying asks two questions before any brief.** Is what the idea asks for
already implemented? Was it already refused? The answers go into the
qualification context, with the `path:line` or the decision they rest on. A
dismissal names its **ground** — `implemented` (it exists already), `refused`
(the owner decided against it) or `other` (a duplicate, an idea overtaken by
events) — besides its reason. The two first grounds are kept apart on purpose:
an idea dismissed because the code already holds it was never refused, and a
later reader who takes one for the other turns a done thing into a forbidden
one, or a refused one into a thing to build again. A dismissal journalled
before grounds existed keeps no ground (`null`); none is guessed.

## Decision requests — structured arbitration

A request asks the owner one question with two to four options. Each option
carries a label, its consequence and its reversibility (`reversible`,
`costly`, `irreversible`); one option may be marked recommended. The options
are shown side by side (CLI, cockpit), so alternatives are compared on the same
fields rather than read from a paragraph.

| From | Command | To | Event |
| --- | --- | --- | --- |
| — | open | `open` | `request.opened` |
| `open` | answer (owner) | `answered` | `request.answered` |
| `open` | withdraw | `withdrawn` | `request.withdrawn` |

A request may be attached to a mission; it cannot be opened on a terminal
mission (`request.mission_closed`). While a request on a mission is `open`, the
mission can neither start a run nor be accepted (`request.pending`): the
question is answered before the work it conditions goes on.

## Mission contract refinements

Declared by the owner while the mission is `draft`, frozen with the brief by
`ready` (`mission.not_draft` otherwise).

- **Dependency** (`dependency.declared`): the mission waits for another one.
  Refusals: unknown mission, self-dependency, a cycle (`dependency.cycle`),
  a duplicate (`dependency.duplicate`), more than 32.
- **Scope** (`scope.declared`, once): repository-relative path prefixes, 1 to
  32, each made of `/`-separated segments without `.`, `..`, empty segments, a
  leading `/`, a backslash or a control character, 256 bytes at most. A prefix
  covers itself and everything under it. A mission without a declared scope
  covers its whole repository.
- **Check** (`check.declared`, one per criterion): an argument vector, 1 to 64
  arguments, run by the daemon to verify that criterion.

## Guards

Starting a run (`ws run`) is refused, in this order:

1. `dependency.unsatisfiable` — a dependency was abandoned or cancelled;
2. `dependency.pending` — a dependency is not accepted yet;
3. `dependency.unmerged` — a dependency of the same repository is accepted,
   but its result commit is not in the history the run starts from (below,
   « Parallel frontier »);
4. `request.pending` — a decision request on the mission is open;
5. `phase.unapproved` — a declared phase has no approved artifact
   (`phases-v0.md`);
6. `scope.conflict` — another mission of the same repository holds a worktree
   (`provisioned`, `running`, `waiting-input`, `exited`, `result-submitted`,
   `rejected`) and the two scopes overlap. Two scopes overlap when a prefix of
   one covers a prefix of the other; an undeclared scope overlaps everything.

Accepting (`ws decide <id> accept`) is refused, in this order, besides the
existing rules:

0. `worktree.head_moved` — the worktree's HEAD is not the submitted commit
   (a commit made after the submission, by a check or by code a check ran,
   would otherwise be kept on `ws/<mission>` unverified);
1. `request.pending`;
2. `phase.unapproved` (`phases-v0.md`);
3. `check.running` — checks of the mission are running;
4. `criteria.unverified` — a criterion carries a check whose last finished
   execution at the submitted commit is missing or did not exit 0 within its
   budgets;
5. `criteria.not_red_at_base` — a criterion carries a check that was not seen
   failing at the mission's base commit (below, « Checks »): a criterion
   already true before the work proves nothing about it;
6. `scope.violated` — a file changed between the base and the submitted commit
   lies outside the declared scope (`scope.checked` with `outside > 0`);
7. `scope.unchecked` — a scope is declared but no scope check exists at the
   submitted commit and none can be computed (the worktree is gone): nothing
   proves the changes stayed inside.

While checks of a mission run, every decision on it is refused
(`check.running`): a decision could release the worktree under them.

`ws mission show` and the cockpit list the current blockers of each mission
with these codes, so a blocked mission says what it waits for.

## Scope check

On result submission the daemon lists the paths changed between the mission's
base commit and the submitted commit (`git diff --name-only -z`) and appends
`scope.checked` with the commit, the number of changed paths, the number
outside the scope and the digest of the list of outside paths (a blob). When a
crash leaves a submitted result without its check, acceptance computes it
first. A path that is not UTF-8 is kept with its invalid bytes replaced. A
scope check that fails never undoes a journalled submission (the response
carries `scope_check_failed`) and never stops a restart (`ws doctor` counts
`scope_checks_failed`); a declared scope then blocks with `scope.unchecked`.

Cost: a request reads once the missions holding a worktree and every declared
scope (two queries), then compares in memory; `mission.list` shares that read
across all missions. The comparison itself stays proportional to missions ×
worktree holders, under the daemon's lock — to revisit before the
multi-tenant phase.

## Checks — verified criteria

`ws check <id>` runs every declared check one after the other, on a thread of
the daemon, at one of two commits:

- **at the base**, for a mission in `ready` or `provisioned`: a `ready`
  mission is first guarded as a run (every run guard above applies, since its
  base is the one a run would get) and provisioned exactly as a run would
  provision it; the checks then run in its worktree, at its base commit. This
  is where each criterion is shown red before the work. While these checks
  run, a run of the mission is refused (`check.running`).
- **at the submitted commit**, for a mission in `result-submitted`.

Any other state is refused (`check.not_submitted`), and so is a mission
without a declared check (`check.none_declared`) or whose worktree is not
clean at the commit of the series (`check.worktree_changed`).

A criterion is **red at the base** when the last finished execution of its
check at the base commit exited with a code other than 0, or by a signal,
within its budgets. A program that could not be started, or a check stopped
by a budget, says nothing about the criterion and is not red. The acceptance
guard `criteria.not_red_at_base` requires every criterion carrying a check to
be red at the base; a criterion meant to hold before and after the work (no
regression) belongs to the repository's own required checks, not to a mission
criterion. The rule, shown red against the base before the code exists, is the
one a test added with a fix is held to.

A program that cannot be started is
recorded as a finished execution without exit code: it never passes. Before
each check the worktree must still be clean and at the commit of the series;
otherwise the series stops, nothing more is journalled, and the remaining
criteria stay unverified. Everything that can fail is prepared before
`check.started`, so only a crash leaves a start without its end. A criterion
is verified when the **last finished** execution of its check passed at the
submitted commit — the same rule for the guard, `ws report` (`verified` per
criterion) and the cockpit. Each execution
uses the run machinery (`pty-v0.md`): a fresh terminal in the worktree, an
environment cleared down to `HOME` (inside the root), `LANG`, `PATH` (the
configured one), `SHELL` and `TERM`, the mission's budgets, its group killed at
the end, its output in `runs/<check>/pty.log`. Events: `check.started`
(check, criterion, commit, digest of the argument vector) and `check.finished`
(exit code or signal, bytes, output digest, overrun budget); a check left
started by a dead daemon is recorded `check.interrupted` at start (crash point
`check-started`), and an interrupted execution never passes.

A check executes what the worktree contains with the user's rights, exactly as
the owner running the same command by hand would; it is not confined until C0
is. Only the owner declares and runs checks.

## Outline split — from an approved outline to linked missions

The `outline` phase (`phases-v0.md`) cuts the work into vertical slices. Once
the owner approved it, `ws outline split <mission> <artifact> <plan file>`
(owner only) creates one **draft** mission per slice, in the repository of the
mission whose outline it is, and declares their dependencies. Nothing is run,
readied or approved: each draft is refined (scope, checks) and readied by the
owner, as any other mission.

The methods of this section, of « Parallel frontier » below, of the base
checks and of the dismissal grounds come from the delivery skills of
`mattpocock/skills` (MIT), read on 2026-10-13; no text or code of theirs is
reused.

The plan is JSON, read strictly (an unknown key refuses it):

```json
{ "missions": [
  { "key": "lexer", "title": "Lexer", "brief": "…", "demonstrates": "a file is tokenised" },
  { "key": "parser", "title": "Parser", "brief": "…", "demonstrates": "a file is parsed",
    "criteria": ["errors carry a position"], "after": ["lexer"],
    "max_duration_seconds": 600, "max_output_bytes": 10485760 }
] }
```

- 1 to 32 missions; `key` is `[a-z0-9][a-z0-9-]*`, 1 to 32 bytes, unique in
  the plan (`outline.key_invalid`, `outline.key_duplicate`).
- `demonstrates` answers « what can I demonstrate once this slice is done? »
  and is required (`coordination.field_invalid: demonstrates`): it becomes the
  mission's **first criterion**, the one its first check is declared on. A
  slice that demonstrates nothing is not a vertical slice.
- `after` lists keys of the same plan (`outline.dependency_unknown`); a slice
  waiting for itself or twice for the same slice is `dependency.duplicate`, a
  cycle of any length `dependency.cycle`. Dependencies are the owner's
  declaration: the plan is the owner's request, never inferred from the
  outline's text, which no rule reads.
- Every mission is validated as `mission.new` validates one
  (`mission.field_invalid`) before anything is written.

Refused also: `artifact.not_found`; `outline.unapproved` (the artifact is not
the **approved** `outline` artifact of that mission — a superseded or returned
one included); `outline.already_split` (a split of this artifact is under way
or done — an aborted one may be given again).

**Write path.** `outline.split.intent` (the plan as a blob, the identifiers of
the missions to create, drawn before), then `mission.created` per mission,
`dependency.declared` per edge, then `outline.split.confirmed`. At start, a
split left `splitting` is **finished**, not undone: the missions and edges not
recorded yet are recorded and the split confirmed. A split recovery cannot
finish (its repository left the configuration) is recorded
`outline.split.aborted`; the missions it did create stay as drafts the owner
sees. Crash points `outline-split-intended` and `outline-missions-created`
cover both windows. `ws mission show` adds `outline`: `planned_by` (artifact,
mission and position of the split that planned it) and `splits` (the splits of
the mission's own outlines).

## Parallel frontier — merged, not closed

Missions whose dependencies are satisfied may run side by side, each in its own
worktree. What satisfies a dependency is that its result is **under the work
that depends on it**, not that it is closed:

- acceptance keeps a result on `ws/<mission>` (`worktree-v0.md`), and a run
  starts from its repository's `HEAD`. Until the owner merges the accepted
  branch into the branch the repository has checked out — the integration
  branch — a dependent mission would start from a base without it. The run
  guard `dependency.unmerged` refuses that, and so does a base check (above),
  since it provisions the same base. Containment is
  `git merge-base --is-ancestor <result> <base>`, with the base the mission's
  worktree base once provisioned and the repository's `HEAD` before. A
  containment git cannot establish (an unknown commit, git failing) counts as
  unmerged: blocked, never waved through.
- a dependency in another repository has no commit to look for in this one:
  it is held by its acceptance only.
- two missions of the frontier that would change the same files are kept
  apart by `scope.conflict`, or ordered by a dependency.

Cost: each run guard (and each blocker listing, for every mission shown) asks
git one containment per accepted dependency of the same repository — at most
32 per mission (the dependency bound), each a separate `git` process.

**Worktree cleanup** is not left to the end of a happy path: every worktree is
created and released by an intent journalled before the git effect and a
confirmation after it, startup reconciles any intent a crash left open, and
`ws worktree gc` removes what no live record explains (`worktree-v0.md`). That
is the guarantee a `finally` gives a process, kept across a `SIGKILL`.

**Publishing a result.** Work Supervision never pushes. `ws/<mission>` is a
local branch: the fleet's branch rulesets refuse to create, update or delete a
`ws/*` branch on the forge (measured on 2026-10-13 with
`gh api repos/libre-ai/ai-work-supervision/rules/branches/ws%2F<name>`:
`creation`, `deletion`, `update`), while `work/**` is open — for a **flat**
name only, since `work/**` does not match `work/a/b` in the same measure. An
accepted result that goes to the forge is pushed by the owner under a flat
name, for example `git push origin ws/<mission>:refs/heads/work/ws-<mission>`.
Re-measure the rules of the target repository before relying on them.

## Agent sessions — the declarative bridge

Agents launched by the owner outside the factory (Claude Code, Codex, Pi or
any other harness) can declare their session, so that the supervision shows
them next to the missions. The factory launches nothing: this is a record of
what the sessions report, it does not count toward B′, and a session's
reported state is never a verified result.

| From | Command | To | Event |
| --- | --- | --- | --- |
| — | register | `active` | `session.registered` |
| `active` | report | `active` | `session.reported` |
| `active` | end | `ended` | `session.ended` |

`session.registered` carries the harness (`claude-code`, `codex`, `pi`,
`other`), the repository and mission it works on when known, the digest of its
label and the SHA-256 of the harness's own session identifier (never the
identifier itself). A report carries a state (`working`, `waiting-input`,
`blocked`, `idle`) and an optional note. The age of a session's last report is
computed at read time; a session silent for more than 15 minutes is shown
`silent`, never inferred as ended.

`ws hook <harness>` translates a harness's own hook payload, read on standard
input (or given as its last argument), into these operations; the per-harness
wiring is in `harness-adapters-v0.md`. The open decision requests of every
mission are listed by the operation `decisions.pending` (`ws request list
--open`), so a supervisor asks one question to know what waits for the owner.

## Report

`ws report <id> [--markdown]` builds, read-only, a report that stands on its
own: what was asked (brief, criteria and their checks), what was done (result,
runs and their exits), the evidence (evidence digest, scope check, check
executions at the submitted commit), the gaps (codes, below), the decisions
(requests and their answers, verdict), the blockers and a timeline (sequence,
instant and kind of every journal entry of the mission). The JSON form is the
contract; the Markdown form is its rendering for a reader who has not followed
the history; texts from agents and sessions are escaped there (HTML, link
syntax), so a renderer that does not sanitise shows them as text. Gap codes: `simulation`, `result.missing`, `criteria.unchecked`
(criteria without a check), `criteria.unverified`,
`criteria.not_red_at_base`, `scope.undeclared`, `scope.violated`,
`run.budget_exceeded`, `run.interrupted`, `request.pending`,
`phase.unapproved`. Each criterion carrying a check reports `verified` and
`red_at_base`. With a declared workflow the report adds `phases` and
`governing` (`phases-v0.md`).

## Events

Common rules of `events-v0.md` apply: no free text in the journal, texts are
blobs named by `*_digest`; identifiers are 32 lowercase hexadecimal
characters; an event that does not follow from the projected state is refused
(`projection.event_invalid`).

| Kind | Data |
| --- | --- |
| `idea.captured` | `idea`, `text_digest`, `actor` |
| `idea.qualified` | `idea`, `repository` or `null`, `mission` or `null`, `context_digest` or `null`, `actor` |
| `idea.promotion.intent` | `idea`, `mission` |
| `idea.promoted` | `idea`, `mission` |
| `idea.promotion.aborted` | `idea` |
| `idea.dismissed` | `idea`, `ground` (`implemented`, `refused`, `other`; absent before migration 0006), `reason_digest` |
| `request.opened` | `request`, `mission` or `null`, `question_digest`, `options` (array of `{label_digest, consequence_digest, reversibility}`), `recommended` (index or `null`), `actor` |
| `request.answered` | `request`, `choice` (index), `reason_digest` |
| `request.withdrawn` | `request`, `reason_digest`, `actor` |
| `dependency.declared` | `mission`, `on` |
| `scope.declared` | `mission`, `paths_digest` (blob: JSON array of prefixes) |
| `scope.checked` | `mission`, `commit`, `changed`, `outside`, `outside_digest` |
| `check.declared` | `mission`, `criterion` (index), `argv_digest` (blob: JSON array) |
| `check.started` | `mission`, `check`, `criterion`, `commit`, `argv_digest` |
| `check.finished` | `mission`, `check`, `bytes`, `digest`, `exit_code` or `null`, `signal` or `null`, `budget` or `null` |
| `check.interrupted` | `mission`, `check` |
| `session.registered` | `session`, `harness`, `repository` or `null`, `mission` or `null`, `label_digest`, `external_digest` or `null` |
| `session.reported` | `session`, `state`, `note_digest` or `null` |
| `session.ended` | `session`, `outcome` (`completed`, `failed`, `abandoned`), `summary_digest` or `null` |
| `outline.split.intent` | `mission`, `artifact`, `plan_digest` (blob: the plan), `missions` (array of the identifiers to create, in plan order) |
| `outline.split.confirmed` | `mission`, `artifact` |
| `outline.split.aborted` | `mission`, `artifact` |

Projected by migration 0004 in tables `ideas`, `requests`, `request_options`,
`mission_dependencies`, `mission_scopes`, `scope_checks`, `criterion_checks`,
`check_runs`, `sessions` and `mission_events` (the timeline: sequence, mission,
kind and instant of every entry that names a mission). A root created before
this migration has no timeline rows for its earlier entries: `ws rebuild`
fills them from the journal, and `ws rebuild --check` reports the difference
until then.

Migration 0006 adds `ideas.dismiss_ground` and the tables `outline_splits` and
`outline_missions` (the identifiers fixed by the intent, before the missions
exist). The projection refuses an intent on an artifact that is not the
approved outline of its mission, naming an existing mission, or whose
identifiers do not match its plan, and a confirmation while a planned mission
is missing.
