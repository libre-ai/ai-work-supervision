# @libre-ai/missions

> **Frozen reference — no authority over Work Supervision v0.** Kept as the
> reference of the multi-tenant model (V6) by owner decision Y31
> (2026-10-09). The v0 state lives in the native core under `crates/`; see
> [`AGENTS.md`](AGENTS.md).

Layer-2 human cockpit for proposing, risk-assessing, reviewing, observing and
validating bounded agent missions, keeping reported activity distinct from a
validated result. Spec: `docs/apps/missions.md`. Work package: `WP-G3-A01`.

## Status

Increment 1 — the **v1 domain** (`src/domain/mission.ts`): the mission
aggregate and its fail-closed, revisioned state machine (15 commands → 13
events, refusal matrix), the human-approver baseline. The v2 two-agent-reviewer
protocol is a locked, **unimplemented** contract and is deliberately absent.

Delivered so far: the domain, tenant-scoped RLS persistence (`packages/data`
barrier, append-only event log, optimistic revision), and the app-side
authorization matrix conformant to `contracts/authz/missions-v1.datalog`. Per
the spec's runtime boundaries, Missions may use contract fixtures for domain/UI
tests but cannot start a real mission or claim orchestrator integration until a
bounded implementation work package and conformance review are approved.

### Next increments (tracked)

1. ~~persistence / RLS via `packages/data`~~ (done);
2. ~~authorization matrix conformant to `missions-v1.datalog`~~ (done);
3. human cockpit UI + accessibility (`Bun.serve`, React 19);
4. runtime Biscuit token verification + revocation at the request boundary
   (TS↔Rust bridge to `authz-biscuit`);
5. adversarial qualification (cross-tenant, self-review, role-confusion, replay).

## Test

```
bun test
```

## Persistence contract

`saveMission(executor, next, events, recordedAt, previous)` requires the position
observed before the domain decision: `{ revision, eventCursor }`, or `null` for
creation. Updates compare both values under the active organization transaction.
A concurrent progress event can change the cursor without changing revision;
its position must never be overwritten by a stale transition. A conflict updates
no row and appends no event. The caller must keep aggregate and event writes in
one transaction, as `executeMissionCommand` does.

`ExportMissionRecord` is an authorized, revision-checked read and never calls
persistence. An empty event list alone does not imply a read: progress advances
the cursor. Rejected exports retain the same authorization and stale-revision
refusals. Cursor gaps in `mission_events` are intentional: raw progress belongs
to the orchestrator stream, while this log records domain transitions.

These checks exercise real SQL and RLS in PGlite. They do not establish a deployed
PostgreSQL service, network authentication, or a real executor connection.

`bun run check:types` checks all Missions sources with strict types.
`bun run test:coverage` writes text and LCOV reports and enforces 95% lines and
90% functions; the root check runs both. The reports measure the loaded Missions
modules, not external orchestrators or an installed product.
