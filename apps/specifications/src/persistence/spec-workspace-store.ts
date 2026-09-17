// Specifications v1 persistence adapter. Persists the spec-workspace aggregate
// (a revisioned snapshot) and appends its causal events inside the caller's
// tenant transaction (packages/data withTenantDbTransaction): the tenant is read
// from the active context (never the request), and RLS scopes every row.
// Optimistic concurrency is enforced by the aggregate revision — a stale write
// updates no row and is rejected, never silently lost. The domain is
// command-sourced (workspace.ts `decide` exposes no event reducer), so the
// snapshot row is the source of truth and the event table is an append-only audit
// trail; each accepted `decide` advances the revision by one and emits one event,
// so a persisted event's sequence equals the revision at which it occurred.

import { requireTenantContext, type SqlExecutor } from "@libre-ai/data";
import type { Event, Status, WorkspaceState } from "../domain/workspace";

export class SpecWorkspaceRevisionConflictError extends Error {
  constructor(readonly workspaceId: string) {
    super(`spec workspace revision conflict for ${workspaceId}`);
    this.name = "SpecWorkspaceRevisionConflictError";
  }
}

export interface RecordedWorkspaceEvent {
  readonly sequence: number;
  readonly type: string;
  readonly recordedAt: string;
}

interface WorkspaceRow {
  readonly tenant_id: string;
  readonly id: string;
  readonly revision: number;
  readonly status: string;
  readonly state: unknown;
  readonly created_at: string | Date;
}

interface EventRow {
  readonly sequence: number;
  readonly type: string;
  readonly recorded_at: string | Date;
}

// PGlite returns jsonb already parsed; a text-typed driver would hand back a
// string. Accept both so the adapter is driver-agnostic.
function asJson(value: unknown): unknown {
  return typeof value === "string" ? JSON.parse(value) : value;
}

function asIsoString(value: string | Date): string {
  return value instanceof Date ? value.toISOString() : value;
}

// Rebuild a frozen WorkspaceState from the snapshot jsonb, re-freezing the arrays
// so a loaded aggregate is as immutable as a freshly decided one.
function rowToState(row: WorkspaceRow): WorkspaceState {
  const state = asJson(row.state) as WorkspaceState;
  return Object.freeze({
    status: state.status as Status,
    revision: state.revision,
    problem: state.problem,
    actorCount: state.actorCount,
    requirementIds: Object.freeze([...state.requirementIds]),
    contractIds: Object.freeze([...state.contractIds]),
    criterionIds: Object.freeze([...state.criterionIds]),
    openDecisionIds: Object.freeze([...state.openDecisionIds]),
    resolvedDecisionIds: Object.freeze([...state.resolvedDecisionIds]),
    approverIds: Object.freeze([...state.approverIds]),
  });
}

/**
 * Persist the next aggregate snapshot and append its events, atomically within
 * the caller's tenant transaction. `revision === 1` inserts; otherwise the update
 * is guarded by the previous revision (`revision - 1`) and throws
 * `SpecWorkspaceRevisionConflictError` if it matched no row (a concurrent writer
 * won). Each event is appended with `sequence` equal to the revision at which it
 * occurred; the guarded update runs before any event insert, so a lost race
 * writes nothing.
 */
export async function saveWorkspace(
  executor: SqlExecutor,
  workspaceId: string,
  state: WorkspaceState,
  events: readonly Event[],
  recordedAt: string,
): Promise<void> {
  const tenantId = requireTenantContext();
  const stateJson = JSON.stringify(state);

  if (state.revision === 1) {
    await executor.query(
      `INSERT INTO spec_workspaces (tenant_id, id, revision, status, state, created_at)
       VALUES ($1,$2,$3,$4,$5,$6)`,
      [tenantId, workspaceId, state.revision, state.status, stateJson, recordedAt],
    );
  } else {
    // tenant_id in the WHERE is defense in depth above FORCE RLS (the USING clause
    // already scopes the row); the guarded revision is the optimistic concurrency
    // check.
    const updated = await executor.query(
      `UPDATE spec_workspaces SET revision = $1, status = $2, state = $3
       WHERE tenant_id = $4 AND id = $5 AND revision = $6`,
      [state.revision, state.status, stateJson, tenantId, workspaceId, state.revision - 1],
    );
    if ((updated.affectedRows ?? 0) !== 1) {
      throw new SpecWorkspaceRevisionConflictError(workspaceId);
    }
  }

  const baseSequence = state.revision - events.length + 1;
  for (const [index, domainEvent] of events.entries()) {
    await executor.query(
      `INSERT INTO spec_workspace_events (tenant_id, workspace_id, sequence, type, recorded_at)
       VALUES ($1,$2,$3,$4,$5)`,
      [tenantId, workspaceId, baseSequence + index, domainEvent.type, recordedAt],
    );
  }
}

/**
 * Load one workspace by id. Tenant scoping is by RLS alone: the read runs under
 * the active tenant context, so a foreign-tenant id simply returns no row (never
 * another tenant's workspace).
 */
export async function loadWorkspace(
  executor: SqlExecutor,
  id: string,
): Promise<WorkspaceState | null> {
  const { rows } = await executor.query<WorkspaceRow>(
    "SELECT * FROM spec_workspaces WHERE id = $1",
    [id],
  );
  const row = rows[0];
  return row === undefined ? null : rowToState(row);
}

/** Read a workspace's append-only event log in causal order (audit/verification). */
export async function loadWorkspaceEvents(
  executor: SqlExecutor,
  id: string,
): Promise<readonly RecordedWorkspaceEvent[]> {
  const { rows } = await executor.query<EventRow>(
    "SELECT sequence, type, recorded_at FROM spec_workspace_events WHERE workspace_id = $1 ORDER BY sequence",
    [id],
  );
  return rows.map((row) =>
    Object.freeze({
      sequence: row.sequence,
      type: row.type,
      recordedAt: asIsoString(row.recorded_at),
    }),
  );
}
