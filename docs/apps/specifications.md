# Specifications

- **Path:** `apps/specifications`
- **Owner:** Experiences / Specifications
- **Runtime:** Bun.serve, React 19, PostgreSQL/RLS
- **Tenant model:** organization

## Purpose and actors

Specifications turns an intent into an explicit, reviewable and immutable SpecPackage with decisions, contracts, risks, acceptance evidence and a planning-only handoff. Authors draft; reviewers decide; approvers accept versions; downstream planners consume packages without execution rights.

## Journeys

1. **Frame:** author creates workspace/problem, actors, constraints and hypotheses; validation exposes missing decisions.
2. **Specify/review:** author adds requirements, contracts, risk controls and tests; reviewers comment and accept/reject attributable decisions. When `collab_enabled`, authors and reviewers co-edit the DRAFT workspace in real time through the sovereign end-to-end-encrypted collaboration brick (MLS RFC 9420 per-epoch keys, ciphertext-only relay); once the package is submitted for review the CRDT is frozen and the review phase uses the append-only comment stream only. Accepted packages stay immutable and content-addressed. The workspace remains fully editable without collaboration enabled.
3. **Accept:** approver freezes a complete package hash and signatures/attestations; accepted version becomes immutable.
4. **Handoff/export:** authorized user emits planning-only handoff referencing accepted package/evidence; consumer verifies hash and capabilities before planning.

## Non-goals

- design canvas clone, generic Markdown editor, issue generator or agent executor ;
- granting repository, shell, network, deployment or mutation capability ;
- accepting a package with unresolved required decisions ;
- mutating accepted package in place ;
- treating AI-generated text as human approval.

## Domain protocol

**Commands:** `CreateSpecWorkspace`, `AddRequirement`, `RecordDecision`, `ResolveDecision`, `AttachContract`, `DefineAcceptanceCriterion`, `SubmitSpecForReview`, `ReviewSpec`, `AcceptSpecPackage`, `SupersedeSpecPackage`, `CreatePlanningHandoff`, `ExportSpecPackage`.

**Queries:** `GetSpecWorkspace`, `ListOpenDecisions`, `ValidateSpecPackage`, `DiffSpecVersions`, `GetAcceptedPackage`, `GetHandoff`, `GetApprovalHistory`.

**Events:** `SpecWorkspaceCreated`, `RequirementAdded`, `DecisionRecorded`, `DecisionResolved`, `SpecSubmitted`, `SpecReviewRecorded`, `SpecPackageAccepted`, `SpecPackageSuperseded`, `PlanningHandoffCreated`.

Draft is revisioned; submitted version is review-frozen; accepted package is immutable/content-addressed. Supersession creates a new lineage edge.

## Refusal matrix

| Code                           | Refusal                                                 |
| ------------------------------ | ------------------------------------------------------- |
| `spec.problem_missing`         | package lacks explicit problem/actor/outcome            |
| `spec.decision_open`           | required cross-module decision unresolved               |
| `spec.contract_missing`        | requirement crosses boundary without canonical contract |
| `spec.acceptance_unverifiable` | criterion has no observable evidence/gate               |
| `spec.approval_self_only`      | author is sole approver where separation required       |
| `spec.revision_stale`          | draft mutation uses stale revision                      |
| `spec.package_immutable`       | mutation targets accepted version                       |
| `spec.handoff_execution_right` | handoff requests executable capability                  |
| `spec.evidence_hash_mismatch`  | evidence/package digest differs                         |

Validation returns stable rule IDs and paths; it never fills missing content automatically.

## Data

PostgreSQL owns organization workspaces, draft revisions, decisions, reviews, accepted package manifests and handoff references. Large attachments live in Cellar only through Artifact manifests; raw secrets and production data are forbidden. Accepted packages follow ADR-0002 section 3 retention and remain immutable while referenced. Migration source is accepted archived specifications/ADRs transformed into v1 contracts with human attribution; historical issue/task state is not imported as authority.

## Authentication and authorization

Opaque browser session maps OIDC subject to organization membership. Biscuit resources are `spec-workspace/<id>`, `spec-package/<id>/<version>` and `handoff/<id>` with author/review/approve/export operations. Handoff token attenuation permits `read` of exact package/evidence only and includes check forbidding operation other than `plan`. RLS protects all tenant data. Private package content never enters token/log.

## Runtime boundaries

TypeScript owns domain workflow, validation orchestration, persistence and UI. Canonical package hashing uses the shared Rust Artifact core with cross-runtime golden vectors; schema validation remains contract-driven in both TS/Rust. Specifications never calls orchestrator execution APIs. It publishes accepted contract bytes/events for planner consumption.

## Accessibility and degraded mode

Forms expose validation summary linked to exact fields/rules; diffs and decisions have table/list alternatives, keyboard review and screen-reader statuses. Draft editing may continue during transient network loss only through explicit queued revisions; conflicting reconnect requires manual merge. Accepted packages remain downloadable when collaboration services are unavailable.

## Contracts

- SpecPackage v1 — `contracts/schemas/spec-package.v1.schema.json` ;
- Agent Handoff v1 — `contracts/schemas/agent-handoff.v1.schema.json` ;
- Evidence Report v1 — `contracts/schemas/evidence-report.v1.schema.json` ;
- Specifications API — `contracts/openapi/specifications.v1.yaml`.

## Evidence

Unit tests cover completeness rules, revisions, lineage and planning-only capabilities. Contract tests use every negative fixture and canonical hash vectors. Integration covers PostgreSQL RLS, concurrent review and artifact references. E2E covers frame→refuse→resolve→review→accept→handoff/export. Security test attempts to insert execution/network/repository capabilities and cross-tenant refs.

## Work packages

1. package/handoff schemas, rule catalog and fixtures — Canonical Core ;
2. workspace/review/acceptance domain and RLS — Experiences ;
3. accessible author/review UI — Experiences + Web Platform ;
4. artifact/evidence/hash integration — Specialized Rust ;
5. planning-only consumer conformance — Missions/orchestrator integration ;
6. concurrency/security/rollback qualification — Infrastructure and Release.

Consumer work starts from fixtures before UI, but production handoff waits accepted package path.

## Release and rollback

Release requires a real project accepted end-to-end, every refusal rule exercised, cross-tenant denial, immutable hash and planning-only consumer proof. Migrations preserve accepted bytes/hashes. Rollback keeps new contract readers or makes app read-only; it never rewrites accepted package or emits handoff from a version the rollback cannot validate.
