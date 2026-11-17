# Work Supervision v0 — steering surfaces

Status: specified, not enabled. In v0 no surface launches a harness: runs are
started by the owner through `wsd`, with the fake agent only
(`harness-adapters-v0.md`, `mission-v0.md`).

A steering surface is what the owner uses to launch or follow a piece of
work: `ws`, `ws attach` (`daemon-v0.md`), `ws-cockpit` (`cockpit-v0.md`), and
any third-party tool to come.

The rule: a surface opens no right that the daemon does not already grant to
the actor, it writes only through `wsd`, and every approval is a decision
request that only `owner` answers (`coordination-v0.md`, « Actors and
authority », « Decision requests »).

| Id | Requirement |
| --- | --- |
| S1 | launches any conforming harness without an identifier of its own |
| S2 | accepts an imposed endpoint profile, which it cannot modify |
| S3 | contacts no domain outside the declared list |
| S4 | its telemetry can be disabled or redirected centrally |
| S5 | its distribution is rebuilt from sources and pinned versions |
| S6 | the session state is exportable, with a settable retention |
| S7 | offers no switch to a personal subscription account |
| S8 | is never the only path to a harness |

These requirements are verified on no surface to date.
