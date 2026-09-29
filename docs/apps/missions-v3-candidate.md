<!-- SPDX-FileCopyrightText: 2026 Libre AI contributors -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Missions v3 — product protocol candidate

Status: candidate, unimplemented, pending separate technical reviews and
promotion. This product-owned document introduces the proposed v3 protocol.
It neither replaces `docs/apps/missions.md` nor changes the locked v1/v2
protocols or their historical archives. Its publication is not implementation,
runtime, provider, migration or deployment admission.

## Purpose and ownership

Missions v3 proposes a mission from an independently verified Build Brief v2
planning handoff. The server derives a canonical binding to the exact handoff,
accepted package and detached acceptance receipt. A client assertion of
acceptance, an arbitrary digest or a retained archive cannot grant permission.

Specifications owns the canonical handoff bytes and the minimal historical
proofs of the accepted package. Missions owns its aggregate, derived binding,
digest and lifecycle reference. Each owner controls its own transactions,
authorization, deletion and restoration. No shared application tables or central
proof-storage service are implied.

The owner selected retention of the handoff at Specifications while referenced,
then P5Y after confirmed reference release, with a P35D backup ceiling. This is
the direction of a new retention candidate, not a rewrite of locked policies.
Mission records retain their separate existing lifetime. Reads and retries do
not extend retention; an archived or expired handoff cannot authorize a new
mission, execution or an unqualified audit endpoint.

## Domain protocol

**Commands v3 candidate:** `ProposeMission`, `AssessMissionRisk`, `SubmitExecutionPlan`, `SubmitAgentReview`, `StartMission`, `PauseMission`, `ResumeMission`, `CancelMission`, `RecordOrchestratorEvent`, `AnswerDecisionRequest`, `SubmitMissionResult`, `AbandonMission`, `ExportMissionRecord`.

**Queries v3 candidate:** `GetMission`, `ListMissions`, `GetMissionEvents`, `GetOpenDecisionRequests`, `GetResultEvidence`, `GetReviewQuorum`, `GetMissionExport`.

The operation names remain those of v2. Versioned v3 request/response bodies and
verification rules are new candidates; equal operation names do not make their
payloads interchangeable. The corresponding ten HTTP endpoints are specified
in the Contracts authority, not implemented by this document.

## Planning admission and durable references

1. Resolve the current principal, organization and exact resource permission
   independently of the request. Resolve authenticated Specifications ownership,
   canonical handoff/package bytes, historical acceptance proofs and current
   key revocation. Missing or contradictory observations refuse.
2. Verify strict canonical input, every supplied acceptance receipt, contributor
   independence, the exact accepted criteria, planning-only capability and
   current handoff expiry. Historical membership cannot replace current rights.
3. Prepare stable archive and reference identities for the same idempotent
   proposal. Derive the entire binding and digest server-side; the locator is
   neither a URL nor a bearer capability. A prepared identity retains no bytes.
4. Revalidate at the effect boundary. Specifications atomically reserves the
   archive and joint package-proof protection for the exact organization,
   mission, binding, archive and reference tuple. Missions atomically commits
   its binding/reference, aggregate revision, event cursor and idempotency result.
5. Confirm using authenticated evidence of that exact Missions commit. A crash,
   lost acknowledgment or ambiguous effect is reconciled under the same identity;
   it is neither success nor permission to create a second reference or mission.

Unknown reservations remain protected. Terminal mission status alone does not
release a reference. Release requires authenticated terminal detachment evidence
and a durable tombstone or qualified equivalent for the exact tuple. Released
identities cannot be revived. Restore reconciles both owners and current rights
before exposing data or permitting effects. The archive-to-package historical
pointer must not create an additional active reference or cascading P5Y period.

## Execution remains separately authorized

The binding enables verified planning only. Plan and result each still require
two eligible blind agent reviews on the same immutable subject, with distinct
agents, runs and nonces, independent of every harness-observed contributor.
Signatures, expiry, complete lineage, durable one-shot nonce claims and any
policy-required diversity are mandatory. A rejection or changed subject requires
remediation and fresh reviews. Protected human control remains additional.

ExecutionPlanBody v3 and ExecutionAuthorization v3 name the exact binding and
their source-major discriminants. They preserve the locked v2 graph, generation,
capability, filesystem, network, model-egress, budget and harness restrictions.
Only Missions may derive execution authorization after current authority and
quorum checks. Start requires effective enforced controls; a signed claim without
those controls refuses. No caller-issued authorization is accepted.

Start, pause and resume reject stale revisions. Authorized cancellation remains
monotone for the exact run/plan/authorization despite an older expected revision.
Transfers, decision requests, causal event chains and uncertain external effects
retain their locked barriers. A result-submitted event is not validated success.
Mission projection events and authoritative orchestrator events are separate
streams; one cannot substitute for the other's evidence.

## API and data boundaries

The candidate HTTP surface requires current authorization, bounded canonical
inputs, revision/event-cursor checks, idempotent effects and opaque cursor pages.
Cookie commands additionally require CSRF protection. Service ingress must verify
the issuer and attenuated exact resource/operation; a browser cannot assert an
agent identity. Replays recheck current rights before returning prior outcomes.
Ambiguous external outcomes remain unavailable until reconciled.

Responses are closed `{ data, meta }` envelopes or content-free refusal documents.
No raw payload, signing material, reviewer identity or rejected value enters
operational logs. Possession of a mission/reference digest grants no proof read.
Historical v1/v2 records remain readable under their own policies; no automatic
backfill may fabricate v3 acceptance, signatures, membership history or quorums.

## Canonical contracts and acceptance boundary

Schemas, preimages, refusal codes and endpoint details belong to
`libre-ai/schemas-and-contracts`: `mission-handoff-binding.v1`,
`mission-record.v3`, `execution-plan-body.v3`, `execution-authorization.v3`,
`missions-api.v3`, `contracts/openapi/missions.v3.yaml` and
`contracts/missions-v3/SEMANTICS.md`. Specifications storage mapping and the
retention-v4 candidate complete the cross-owner design. All remain candidates
until their required reviews and promotion succeed.

Before implementation admission, reconcile exact owner-document provenance and
the Contracts resolver by major version; collect separate architecture, security,
cryptography and privacy verdicts as required by the catalog. Before runtime
enablement, prove actual HTTP/CSRF refusals, PostgreSQL multi-connection races and
RLS, crash/restart reference reconciliation, deletion/backup restore, browser
journeys, reviewer isolation/replay defenses and enforced executor controls.
Synthetic contract fixtures and a green root check do not prove these consumers.
