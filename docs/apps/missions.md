# Missions

- **Path:** `apps/missions`
- **Owner:** Experiences / Missions
- **Runtime:** Bun.serve, React 19, PostgreSQL/RLS; orchestrator is separate specialized Rust capability
- **Tenant model:** organization

## Purpose and actors

Missions lets requesters propose, risk-assess, review, observe and validate bounded agent missions while keeping reported activity distinct from validated results. The locked v1 baseline uses human approvers/verdicts. The locked, unimplemented v2 contract requires two reviewer agents, distinct from every contributor, to approve the same immutable plan digest and then the same immutable result digest. Protected canonical contracts, auth, migrations, releases and deployments retain an additional human control gate.

## Journeys

1. **Propose/review plan:** requester creates a mission from an accepted planning handoff; a deterministic plan body is reviewed blindly by two eligible agents on the same digest.
2. **Authorize/observe:** Missions verifies the plan quorum and emits an expiring authorization; the orchestrator reports causal events while an operator may pause/cancel within policy.
3. **Review result:** the executor submits artifact, evidence and contributor lineage; two other agents independently review the same digest. Any rejection or changed digest requires remediation and two fresh reviews.
4. **Validate/audit:** a valid two-agent quorum transitions the technical result to `validated`; authorized actors export immutable records and digested evidence, never hidden chain-of-thought or secrets.

## Non-goals

- orchestrating processes/tools inside the web app ;
- agent marketplace/profile, general project management, self-review or single-agent approval ;
- equating event volume, agent status or claimed completion with success ;
- editing event history ;
- accepting handoff that grants execution rights.

## Domain protocol

**Commands v1:** `ProposeMission`, `AssessMissionRisk`, `ApproveMission`, `RefuseMission`, `StartMission`, `PauseMission`, `ResumeMission`, `CancelMission`, `RecordOrchestratorEvent`, `AnswerDecisionRequest`, `SubmitMissionResult`, `AcceptMissionResult`, `RejectMissionResult`, `AbandonMission`, `ExportMissionRecord`.

**Queries v1:** `GetMission`, `ListMissions`, `GetMissionEvents`, `GetOpenDecisionRequests`, `GetResultEvidence`, `GetApprovalHistory`, `GetMissionExport`.

The v2 protocol below is a locked contract and remains unimplemented.

**Commands v2:** `ProposeMission`, `AssessMissionRisk`, `SubmitExecutionPlan`, `SubmitAgentReview`, `StartMission`, `PauseMission`, `ResumeMission`, `CancelMission`, `RecordOrchestratorEvent`, `AnswerDecisionRequest`, `SubmitMissionResult`, `AbandonMission`, `ExportMissionRecord`.

**Queries v2:** `GetMission`, `ListMissions`, `GetMissionEvents`, `GetOpenDecisionRequests`, `GetResultEvidence`, `GetReviewQuorum`, `GetMissionExport`.

**Events:** `MissionProposed`, `MissionRiskAssessed`, `MissionApproved`, `MissionRefused`, `MissionStarted`, `MissionBlocked`, `HumanDecisionRequested`, `MissionPaused`, `MissionCancelled`, `MissionResultSubmitted`, `MissionResultAccepted`, `MissionResultRejected`, `MissionAbandoned`.

Both state machines are fail-closed and revisioned. Only the orchestrator adapter can report execution events. In v2, only Missions can compute quorum from two signed, unexpired, one-shot reviews whose agent identities are distinct from each other and from the harness-derived contributor lineage. `result-submitted` is never terminal success.

## Refusal matrix

| Code | Refusal |
| --- | --- |
| `mission.spec_unaccepted` | referenced SpecPackage/handoff not accepted/verified |
| `mission.handoff_not_planning_only` | handoff contains execution capability |
| `mission.risk_unassessed` | approval/start requested before risk policy result |
| `mission.approval_required` | operation lacks required human approval |
| `mission.transition_forbidden` | state/actor cannot perform transition |
| `mission.revision_stale` | command targets stale aggregate revision |
| `mission.event_untrusted` | event source/token is not bound orchestrator instance |
| `mission.budget_exceeded` | time/tool/network/cost budget exceeded; mission blocks/stops |
| `mission.evidence_missing` | result lacks required artifact/evidence reference |
| `mission.review_ineligible` | reviewer is a contributor, duplicate, untrusted or not isolated |
| `mission.review_invalid` | signature, nonce, expiry, subject or evidence digest is invalid |
| `mission.review_rejected` | at least one eligible reviewer rejects the digest |
| `mission.quorum_missing` | two favorable reviews on the exact digest are absent |
| `mission.quorum_stale` | subject, evidence or lineage changed after review |
| `mission.tenant_mismatch` | mission/spec/event/review tenant differs |

Unknown event types are quarantined, never projected as success.

## Data

PostgreSQL owns the mission aggregate, review references, verified quorum state, append-only events, decision requests, budgets and validation projection. Detailed review findings and other evidence remain owned by Proof/Artifact as tenant-private, retention-bounded objects referenced by digest. Orchestrator runtime state is not Missions authority. Retention follows ADR-0002 section 3; reviewer/contributor identities never enter operational logs or OTEL.

## Authentication and authorization

Browser uses opaque session. Biscuit resources are exact mission, run, review-subject and artifact/evidence refs. Authority includes user, mandatory tenant, `role(user, role)`, root token ID and expiration. V2 reviewer tokens are attenuated to one subject digest and can only read that subject then submit one review; they cannot modify, execute or see a sibling verdict before submission. Author, reviewer, orchestrator and harness tokens cannot individually fabricate quorum. Revocation and RLS are checked for each command.

## Runtime boundaries

TypeScript owns the mission domain, review-quorum workflow, persistence and projection. WP-G2-S01 does not implement an orchestrator or harness. The agent-orchestration Specification Lock fixes the execution-plan/control protocol, attenuated authorization and sandbox boundaries. Any future Rust orchestration owns only process/tool scheduling and budget enforcement behind those authorities and emits authorized protocol events without shared DB. Until a bounded implementation work package and conformance review are approved, Missions may use contract fixtures for UI/domain tests but cannot start a real mission or claim orchestrator integration.

## Accessibility and degraded mode

Timeline has ordered textual/table view, filters and live announcements that do not steal focus. Risk, block and verdict never rely on color. Orchestrator outage marks control status unknown and disables start/resume/accept based solely on stale report; audit/export remains. Database outage fails commands closed. Evidence outage allows viewing recorded refs but prevents acceptance when criterion requires retrieval.

## Contracts

- MissionRecord v1 — `contracts/schemas/mission-record.v1.schema.json` ;
- Agent Handoff v1 — `contracts/schemas/agent-handoff.v1.schema.json` ;
- Orchestrator Event v1 — `contracts/schemas/orchestrator-event.v1.schema.json` ;
- locked, unimplemented v2 plan/review/quorum/control/harness family — cataloged schemas headed by `contracts/schemas/execution-plan-body.v1.schema.json` and `contracts/agent-orchestration/SEMANTICS.md` ;
- locked, unimplemented MissionRecord v2 — `contracts/schemas/mission-record.v2.schema.json` ;
- Evidence/Artifact refs — canonical schemas ;
- Missions APIs — `contracts/openapi/missions.v1.yaml` and locked, unimplemented `contracts/openapi/missions.v2.yaml` ;
- Biscuit policies — `contracts/authz/missions-v1.datalog` and locked, unimplemented `contracts/authz/agent-runs-v1.datalog`.

## Evidence

Unit tests exhaust every state/role transition, revisions and idempotency. V2 contract tests additionally reject self-review, duplicate reviewers, omitted contributors, stale digests, disclosed sibling verdicts, replayed nonces and invalid signatures. Future implementation integration must run a simulated orchestrator, PostgreSQL RLS, revocation and budget stop. Its security qualification must prove no individual author/reviewer/orchestrator/harness identity can fabricate quorum.

## Work packages

1. mission/handoff/event/authz contracts and transition table — Canonical Core ;
2. mission persistence/RLS/domain API — Experiences ;
3. orchestrator adapter and simulated event producer — Specialized Rust ;
4. human cockpit/accessibility — Experiences + Web Platform ;
5. Proof/Artifact verification integration — Specialized Rust ;
6. adversarial authorization/budget/replay qualification — Infrastructure and Release.

Future bounded domain/UI and orchestrator work packages may proceed in parallel against protocol fixtures after their own approval; integration additionally requires transition/authz conformance and unchanged locked hashes.

## Release and rollback

Release requires a simulated and one bounded real mission with attributable two-agent plan/result quorums, enforced budget, block/decision and evidence; protected domains also require their canonical human gate. Cross-tenant, self-review, duplicate-reviewer and role-confusion attacks must fail. Rollback first prevents new starts, preserves event ingestion compatibility and restores app/orchestrator pair. Event history and accepted deletion are never rewritten.
