// Missions v1 persistence adapter. Persists the domain aggregate and appends
// its causal events inside the caller's tenant transaction
// (packages/data withTenantDbTransaction): the tenant is read from the active
// context (never the request), and RLS scopes every row. Optimistic
// concurrency is enforced by the aggregate revision — a stale write updates no
// row and is rejected, never silently lost.

import { requireTenantContext, type SqlExecutor } from "@libre-ai/data";
import type { Mission, MissionEvent, MissionState, RiskLevel } from "../domain/mission";

export class MissionRevisionConflictError extends Error {
  constructor(readonly missionId: string) {
    super(`mission revision conflict for ${missionId}`);
    this.name = "MissionRevisionConflictError";
  }
}

export class MissionTenantMismatchError extends Error {
  constructor() {
    super("aggregate tenant differs from the active tenant context");
    this.name = "MissionTenantMismatchError";
  }
}

interface MissionRow {
  readonly tenant_id: string;
  readonly id: string;
  readonly revision: number;
  readonly state: string;
  readonly handoff_id: string;
  readonly handoff_digest: string;
  readonly risk: unknown;
  readonly budgets: unknown;
  readonly acceptance_criteria: unknown;
  readonly approvals: unknown;
  readonly event_cursor: number;
  readonly result: unknown;
  readonly verdict: unknown;
  readonly created_at: string | Date;
}

function asJson(value: unknown): unknown {
  return typeof value === "string" ? JSON.parse(value) : value;
}

function asIsoString(value: string | Date): string {
  return value instanceof Date ? value.toISOString() : value;
}

function rowToMission(row: MissionRow): Mission {
  return {
    id: row.id,
    tenantId: row.tenant_id,
    revision: row.revision,
    state: row.state as MissionState,
    handoffId: row.handoff_id,
    handoffDigest: row.handoff_digest,
    risk: (asJson(row.risk) as { level: RiskLevel; policyVersion: string } | null) ?? undefined,
    budgets: asJson(row.budgets) as Mission["budgets"],
    acceptanceCriteria: asJson(row.acceptance_criteria) as string[],
    approvals: asJson(row.approvals) as string[],
    eventCursor: row.event_cursor,
    result: (asJson(row.result) as Mission["result"] | null) ?? undefined,
    verdict: (asJson(row.verdict) as Mission["verdict"] | null) ?? undefined,
    createdAt: asIsoString(row.created_at),
  };
}

/**
 * Persist the next aggregate and append its events, atomically within the
 * caller's tenant transaction. `revision === 1` inserts; otherwise the update
 * is guarded by the previous revision (`revision - 1`) and throws
 * `MissionRevisionConflictError` if it matched no row (a concurrent writer won).
 */
export async function saveMission(
  executor: SqlExecutor,
  mission: Mission,
  events: readonly MissionEvent[],
  recordedAt: string,
): Promise<void> {
  const tenantId = requireTenantContext();
  if (mission.tenantId !== tenantId) throw new MissionTenantMismatchError();

  const risk = mission.risk === undefined ? null : JSON.stringify(mission.risk);
  const result = mission.result === undefined ? null : JSON.stringify(mission.result);
  const verdict = mission.verdict === undefined ? null : JSON.stringify(mission.verdict);

  if (mission.revision === 1) {
    await executor.query(
      `INSERT INTO missions (
         tenant_id, id, revision, state, handoff_id, handoff_digest, risk, budgets,
         acceptance_criteria, approvals, event_cursor, result, verdict, created_at
       ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)`,
      [
        tenantId,
        mission.id,
        mission.revision,
        mission.state,
        mission.handoffId,
        mission.handoffDigest,
        risk,
        JSON.stringify(mission.budgets),
        JSON.stringify(mission.acceptanceCriteria),
        JSON.stringify(mission.approvals),
        mission.eventCursor,
        result,
        verdict,
        mission.createdAt,
      ],
    );
  } else {
    // tenant_id in the WHERE is defense in depth above FORCE RLS (the USING
    // clause already scopes the row); the guarded revision is the optimistic
    // concurrency check.
    const updated = await executor.query(
      `UPDATE missions SET
         revision = $1, state = $2, risk = $3, approvals = $4,
         event_cursor = $5, result = $6, verdict = $7
       WHERE tenant_id = $8 AND id = $9 AND revision = $10`,
      [
        mission.revision,
        mission.state,
        risk,
        JSON.stringify(mission.approvals),
        mission.eventCursor,
        result,
        verdict,
        tenantId,
        mission.id,
        mission.revision - 1,
      ],
    );
    if ((updated.affectedRows ?? 0) !== 1) throw new MissionRevisionConflictError(mission.id);
  }

  for (const domainEvent of events) {
    await executor.query(
      `INSERT INTO mission_events (tenant_id, mission_id, sequence, type, recorded_at)
       VALUES ($1,$2,$3,$4,$5)`,
      [tenantId, domainEvent.missionId, domainEvent.sequence, domainEvent.type, recordedAt],
    );
  }
}

/**
 * Load one mission by id. Tenant scoping is by RLS alone here: the read runs
 * under the active tenant context, so a foreign-tenant id simply returns no row
 * (never another tenant's mission). No explicit tenant column is compared —
 * unlike the write path, where `saveMission` also asserts the aggregate's
 * tenant matches the context as defense in depth before mutating.
 */
export async function loadMission(executor: SqlExecutor, id: string): Promise<Mission | null> {
  const { rows } = await executor.query<MissionRow>("SELECT * FROM missions WHERE id = $1", [id]);
  const row = rows[0];
  return row === undefined ? null : rowToMission(row);
}
