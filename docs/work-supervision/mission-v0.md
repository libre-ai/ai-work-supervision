# Work Supervision v0 — mission state machine

Crate: `crates/work-supervision-domain` (pure: no I/O, no clock, no random
number). Single user, no tenant, no role, no third-party approval (ADR-0042 §5):
`accept`, `reject` and `abandon` are the owner's decisions.

## Transitions

| From | Command | To |
| --- | --- | --- |
| — | create | draft |
| draft | ready | ready |
| ready | provision | provisioned |
| provisioned, rejected | start-run | running |
| running | await-input | waiting-input |
| waiting-input | resume-input | running |
| running, waiting-input | exit-run | exited |
| exited | submit-result | result-submitted |
| result-submitted | accept | accepted |
| result-submitted | reject | rejected |
| draft, ready, result-submitted, rejected | abandon | abandoned |
| provisioned, running, waiting-input, exited | cancel | cancelled |
| any | note | unchanged (no revision) |

Terminal states: `accepted`, `abandoned`, `cancelled` (only notes).

## Refusals (closed)

| Code | When |
| --- | --- |
| `mission.not_found` | a command other than create on an absent mission |
| `mission.already_exists` | create on an existing mission |
| `mission.revision_stale` | the command's expected revision is not the mission's (checked before the transition) |
| `mission.transition_forbidden` | any pair not in the table above |
| `mission.result_incomplete` | a result without commit, evidence or a non-blank summary — acceptance is therefore impossible without proof |
| `mission.field_invalid` | title, repository name, brief, criteria, budgets, note, reason or identifier out of its rule |
| `agent.real_forbidden_until_c0` | an executor profile other than `fake` (C0 guard) |

## C0 guard

`ExecutorProfile` has a single variant, `Fake`. A configuration naming any
other profile — an agent name, an executable path, another spelling — is
refused with `agent.real_forbidden_until_c0`. Adding a real profile is a
reviewed code change after the C0 confinement qualification is green.

## Write path

`Supervisor::execute` (`crates/work-supervision-store`) reads the mission from
the projection, lets the domain decide, stores the texts of the event as
blobs, appends the event to the journal and then applies the durable entry to
the projection. A refusal writes nothing. The event catalogue is in
`events-v0.md`.

Semantics ported without code from the recovered `apps/missions` domain
(frozen reference, no authority over v0): closed refusals, optimistic
revision, result and evidence before acceptance, terminal states without exit,
reported activity kept apart from a validated result.
