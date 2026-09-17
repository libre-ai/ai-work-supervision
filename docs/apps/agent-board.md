# Agent Board

- **Path:** `apps/agent-board`
- **Owner:** Experiences / Agent Board
- **Runtime:** Bun.serve, React 19, WebSockets, PostgreSQL/RLS for materialized mission state; read-only projections from Missions events
- **Tenant model:** organization

## Purpose and actors

Agent Board is the human surface for the Polaris agent orchestration fleet (couche-2). It visualizes authorized missions, agent capability/status, task boards with assignment workflows, squad organization, runtime progress and execution audits — all without orchestrating agents or modifying mission state. Actors are organization owner, operations facilitator, agent operator, observer and auditor.

## Journeys

1. **View fleet dashboard:** operator inspects agent availability, capability taxonomy, operational status and historical success rate per agent/squad; surfaces capacity and skill utilization.
2. **Manage mission board:** operator views all authorized missions (authorized by Missions authority), assigns pending missions to eligible agents/squads, observes run lifecycle (queued→claimed→executing→completed/failed).
3. **Observe run progress:** operator follows a live mission execution via streaming events (orchestrator-reported), comments, decision requests (human approval gates within Missions), blockers and estimated time-to-completion.
4. **Inspect run audit:** operator exports an immutable audit trail for a completed mission: execution plan (hash-verified), agent identity, runtime events, evidence references, approval history and two-agent review quorum proof.
5. **Configure autopilots:** operations facilitator defines recurring mission templates, eligibility criteria for auto-assignment, escalation rules and approval thresholds; previews the impact on fleet utilization.
6. **Triage decisions and escalations:** operator reviews decision requests from a running mission, verifies bounded choices in Missions-enforced policy, answers blocking questions and observes conditional resumption.

## Non-goals

- orchestrating process/tool execution inside or outside the web app (that is Orchestrator authority) ;
- modifying, cancelling or re-planning authorized missions (mutations are Missions-only authority) ;
- creating or altering agent capability/credential (harness and identity service authority) ;
- storing or evaluating evidence details (Proof/Artifact authority; agent-board only references/links) ;
- general agent marketplace, personal profiles or open-ended discovery ;
- real-time metrics dashboards or performance analytics (distinct from operational status) ;
- bidirectional sync or polling external systems (read-only projection only) ;
- agent self-management, self-review or individual approval ;
- bypassing or weakening Missions' two-agent review quorum ;
- accepting or rejecting mission results (only observing verdicts delivered by Missions).

## Domain protocol

**Commands v1:** `CreateAutopilot`, `UpdateAutopilotEligibility`, `DeleteAutopilot`, `AssignMissionToAgent`, `AssignMissionToSquad`, `AnswerDecisionRequest`, `AcknowledgeBlocker`.

Commands do not author or modify missions; they only route authorized missions to eligible agents and confirm read-only events. All commands are gated by operator role and mission authorization state from Missions. Decision request answers are forwarded to Missions as tributary events; agent-board remains non-authoritative for acceptance/rejection.

**Queries v1:** `GetFleetStatus`, `GetAgentCapabilities`, `ListAgentsBySquad`, `GetMissionBoard`, `GetMissionProgressStream`, `GetRunEvents`, `GetRunAuditTrail`, `GetAutopilotConfig`, `ListPendingDecisionRequests`, `PreviewAutopilotImpact`.

**Events (from Missions, consumed read-only):** `MissionAuthorized`, `MissionStarted`, `MissionPaused`, `MissionBlocked`, `MissionCancelled`, `MissionResultSubmitted`, `MissionResultAccepted`, `MissionResultRejected`, `HumanDecisionRequested`, `DecisionAnswered`.

**Events (from Orchestrator, via Missions adapter):** `ExecutorAssigned`, `ExecutionStarted`, `ToolInvoked`, `BudgetCheck`, `ExecutionProgressed`, `ExecutionPaused`, `ExecutionFailed`, `ExecutionSucceeded`, `ProofRequested`.

Agent-board is a read-only projection over Missions and Orchestrator event streams. No mutation alters Missions state or execution authorization; all operators see a consistent view of the same authorized fleet and running missions.

## Refusal matrix

| Code                                    | Refusal                                                                             |
| --------------------------------------- | ----------------------------------------------------------------------------------- |
| `agent-board.operator_unauthorized`     | user lacks operator or observer role for the organization                           |
| `agent-board.mission_not_authorized`    | referenced mission not in authorized state per Missions authority                   |
| `agent-board.agent_not_eligible`        | target agent lacks required capability or is in maintenance state                   |
| `agent-board.squad_not_found`           | referenced squad not configured in fleet policy                                     |
| `agent-board.autopilot_conflict`        | proposed autopilot rule conflicts with active rules or mission constraints          |
| `agent-board.assignment_stale`          | mission state changed; reassignment requires fresh authorization from Missions      |
| `agent-board.decision_already_answered` | decision request was already answered or timed out                                  |
| `agent-board.audit_trail_incomplete`    | run evidence or review quorum not yet available (still pending from Proof/Artifact) |
| `agent-board.tenant_mismatch`           | referenced mission, agent or squad tenant differs from request tenant               |
| `agent-board.query_timeout`             | event stream or audit projection unavailable (Missions/Orchestrator outage)         |

## Data

PostgreSQL owns the fleet policy (agent/squad taxonomy, capability index, maintenance states), autopilot templates, assignment journal and audit-trail projections (derived from Missions and Orchestrator events, never authoritative). Real-time mission state is sourced from Missions' event stream via WebSocket; agent-board materializes the projection for fast query response. Evidence references (artifacts, proof objects) are stored as links only, resolved from Proof/Artifact authority on export. Retention for agent-board operational data follows ADR-0002 section 3; operator audit trails are retention-bounded and may not contain internal agent tokens or secrets.

## Authentication and authorization

Browser uses opaque session. Biscuit resources are exact mission, agent, squad and decision-request refs. Authority includes user, mandatory tenant, `role(user, role)`, root token ID and expiration. Operators have read access to all authorized missions and agent fleet for their tenant; facilitators may additionally create/modify autopilot rules. Observers have read-only access to all views except decision requests. Revocation and RLS are checked for every query. Tokens for Missions/Orchestrator event consumption are attenuated to organization tenant and never exposed to browser.

## Runtime boundaries

TypeScript owns the human dashboard, event projection, autopilot logic, accessibility and streaming UI. Missions owns mission state, authorization, decision requests and quorum. Orchestrator owns process/tool scheduling and emits authorized execution events. Agent-board never receives orchestrator credentials, budgets or internal harness state; it observes only sanitized execution events. Until a work package implements real orchestrator integration, agent-board may use contract fixtures and a simulated event stream for UI/domain tests, but cannot observe or claim a real running mission.

## Accessibility and degraded mode

Mission board has ordered table and kanban views, filters (by agent, status, squad, tag) and live announcements that do not steal focus. Agent status (busy, available, maintenance), blockers and decision requests are all textual and never rely on color or animation. Progress indication uses accessible meters with numeric percent. Evidence links are provided as downloadable references; agent-board does not embed external content. Missions outage marks fleet status unknown and disables assignment/autopilot; views remain read-only on cached projection. Orchestrator outage marks mission progress as stalled but preserves historical events.

## Contracts

- Mission Authorized Event — consumed from `contracts/schemas/mission-record.v1.schema.json` and events envelope ;
- Orchestrator Event v2 — `contracts/schemas/orchestrator-event.v2.schema.json` ;
- Evidence/Artifact ref contract — `contracts/schemas/evidence-report.v1.schema.json` ;
- Agent Board API — to be authored as agent-board.v1.yaml OpenAPI contract ;
- Agent Board WebSocket — to be authored as agent-board-events.v1 event schema ;
- Fleet policy schema — to be authored for agent/squad taxonomy and capability index ;
- Autopilot template schema — to be authored for recurring mission rule definitions ;
- Biscuit policy — to be authored as agent-board authorization policy (integrated with Missions authz) ;
- Mission event projection — read-only consumption of Missions.v1 event catalog, transformed into agent-board domain model.

## Evidence

Unit tests cover role-based access, mission state transitions, autopilot rule conflicts and event projection idempotency. Integration tests verify WebSocket reconnect, event ordering and audit-trail completeness. Contract tests validate that agent-board never mutates Missions state and that all assignments are rejected if the mission is not in authorized state. E2E tests cover operator/facilitator/observer journeys: viewing fleet, assigning to squad, observing progress, answering decision requests, inspecting audit trail and exporting run proof.

Security tests verify that read-only projection cannot be exploited to escalate permissions or bypass Missions' quorum; that no operator role can move a mission backward (e.g., claiming a completed mission); that cross-tenant queries return 403, not 404; and that WebSocket presence does not grant authorization (presence is ephemeral, cannot prove participation or review).

## Work packages

1. Fleet policy, agent taxonomy and squad definition — Canonical Core ;
2. Mission authorization projection and event consumer — Experiences + Missions adapter ;
3. Orchestrator event adapter (simulation fixtures, real integration in WP-G3/G4) — Specialized Rust ;
4. Autopilot rules engine and eligibility evaluation — Experiences ;
5. Mission board, fleet dashboard and audit trail UI — Experiences + Web Platform ;
6. Decision request routing and operator answer flow — Experiences + Missions integration ;
7. Accessibility, multi-instance and event ordering qualification — Infrastructure and Release.

UI and projection can proceed in parallel only after fleet policy semantics and Missions event contracts are accepted. Orchestrator event adapter is gated on WP-G3 orchestrator specification work package.

## Release and rollback

Release requires live mission assignment to at least one agent via squad policy, full audit trail export with proof verification, operator/facilitator/observer journeys and a decision request answer that triggers mission resumption. Cross-tenant, unauthorized-mission-view and stale-assignment attacks must fail. Role-based access and event ordering under network restart (WebSocket reconnect) must be proven. Rollback first disables new assignments (autopilot off, manual off), preserves event ingestion from Missions/Orchestrator, and restores app with fresh projection from event stream; assignment journal and cached fleet state are never rewritten.
