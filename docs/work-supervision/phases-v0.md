# Work Supervision v0 — phases and approved artifacts

Crates: `crates/work-supervision-domain` (pure rules, module `phases`),
`crates/work-supervision-store` (migration 0005, projection),
`crates/work-supervision-daemon` (operations and guards),
`crates/work-supervision-cli` (`ws workflow`, `ws artifact`, `ws report`) and
`crates/work-supervision-cockpit` (read and decide).

A mission can declare, before its brief is frozen, the phases its work goes
through before implementation: what to find out, what the code does today,
what is chosen, how it is cut. Each phase closes on one written artifact the
owner approves. A run starts, and a result is accepted, only when every
declared phase has its approved artifact. The method is the one of
HumanLayer's research / design / outline workflow; the code is not theirs.
Without a declared workflow a mission is unchanged.

Why: the review that matters is the one made before the code. An approved
research says which facts the design stands on; an approved outline says what
the result must contain and how it is verified. Reviewing them costs a fraction
of reviewing the diff they produce, and a wrong fact caught there never becomes
a design assumption.

## Phases

Closed vocabulary, in this canonical order. A workflow lists 1 to 7 of them,
in this order, each once (`coordination.field_invalid: workflow` otherwise).

| Phase | Its artifact holds |
| --- | --- |
| `research-questions` | What to find out. Kept apart from the brief so that the research describes what is, not what the brief hopes for. |
| `research` | The current state: affirmative findings, each with `path:line`; what was read kept apart from what was run. No design choice. |
| `design` | The options considered and the decision taken on each, with what was set aside. |
| `product` | The problem, how success is measured, the solution as the user sees it. |
| `system-design` | Contracts, schemas, stores, queues: what the system exposes and keeps. |
| `program-design` | Call paths, files, types and signatures, test boundaries. |
| `outline` | Vertical slices, each with its testable result, the files it changes and its automated and manual verifications. |

`system-design` and `program-design` are two phases so that the two levels of
a technical design are approved separately.

**Precedence.** When two approved artifacts disagree, the later phase in the
canonical order prevails; the brief comes before every phase. For what the
code does *now*, the code prevails over any artifact. The report names the
**governing artifact**: the approved artifact of the latest declared phase.

## Workflow declaration

`ws workflow <mission> <phase>…` — owner only, mission in `draft`
(`mission.not_draft`), once (`workflow.already_declared`). Event
`workflow.declared`. Frozen with the brief by `ready`.

## Artifacts

An artifact is a UTF-8 text of 1 byte to 1 MiB (`coordination.field_invalid:
content`), stored as a blob. Its states:

| From | Command | To | Event |
| --- | --- | --- | --- |
| — | submit | `submitted` | `artifact.submitted` |
| `submitted` | approve (owner) | `approved` | `artifact.approved` |
| `submitted` | return (owner) | `returned` | `artifact.returned` |
| `submitted`, `approved` | a later submission | `superseded` | (the `artifact.submitted` that supersedes it) |

**Submit** (`ws artifact submit <mission> <phase> <file> [--session <id>]`) —
the owner or an active declared session. Refused, in this order:

1. `mission.not_found`;
2. `artifact.mission_draft` — the mission is `draft`: research starts from a
   frozen brief;
3. `artifact.mission_closed` — the mission is terminal;
4. `workflow.undeclared` — no workflow declared;
5. `phase.undeclared` — the phase is not in the workflow;
6. `phase.previous_unapproved` — an earlier declared phase has no approved
   artifact: a design is not written on an unapproved research.

A submission supersedes the current artifact (`submitted` or `approved`) of
its own phase and of **every later phase**: their basis changed, so their
approval no longer holds. A `returned` artifact stays `returned`.

**Approve** (`ws artifact approve <artifact> --digest <sha256> [--reason <r>]`)
— owner only (`actor.owner_only`). The command carries the SHA-256 of the
content the owner read; it is refused with `artifact.digest_mismatch` when it
is not the artifact's. An approval therefore binds the exact text that was
reviewed, never "whatever this identifier holds now". Refused also with
`artifact.not_found`, `artifact.not_pending` (not `submitted`, so superseded
or decided already) and `artifact.mission_closed`.

**Return** (`ws artifact return <artifact> --reason <r>`) — owner only. The
phase waits for a new submission. Same refusals, without the digest.

The text of an artifact, and of a comment or reason, is data written by an
agent or a person: it is shown escaped, never executed, and no rule of the
daemon reads its content.

**No automatic advance.** The factory launches no phase work and approves
nothing by itself: every phase closes on the owner's approval. HumanLayer
whitelists the transitions its product may chain on its own; Work Supervision
has no such chaining, so there is nothing to whitelist.

## Guards

`phase.unapproved` — a declared phase has no approved artifact. One blocker
per such phase, in workflow order.

- Starting a run is refused with it after `request.pending` and before
  `scope.conflict` (`coordination-v0.md`, Guards).
- Accepting is refused with it after `request.pending` and before
  `check.running`: a submission made after the run started supersedes an
  approval, and the result then stands on an unapproved basis.

`ws mission show`, the cockpit and `ws report` list it with the other blockers;
`ws report` adds the gap code `phase.unapproved`.

## Reads

- `ws artifact list <mission>` — every artifact of the mission, newest first,
  with phase, state, digest, size, author and instants (no content).
- `ws artifact show <artifact>` — one artifact with its content.
- `ws mission show` adds `workflow` and the current artifact of each phase.
- `ws report` adds `phases` (each declared phase and its current artifact)
  and `governing` (the governing artifact, with its content).

## Events

Common rules of `events-v0.md` apply (no free text in the journal; an event
that does not follow from the projected state is refused).

| Kind | Data |
| --- | --- |
| `workflow.declared` | `mission`, `phases` (array of phase names) |
| `artifact.submitted` | `mission`, `artifact`, `phase`, `content_digest`, `bytes`, `actor` |
| `artifact.approved` | `mission`, `artifact`, `content_digest`, `reason_digest` or `null` |
| `artifact.returned` | `mission`, `artifact`, `reason_digest` |

The projection refuses an `artifact.approved` whose `content_digest` is not the
stored one, and an `artifact.submitted` whose blob is not `bytes` long.
Projected by migration 0005 in tables `mission_phases` and `artifacts`.
