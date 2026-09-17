// Missions v1 command service — the write path that composes the three layers:
// authorization (missions-v1 matrix) → domain (fail-closed state machine) →
// persistence (tenant-scoped RLS, append-only events, optimistic revision), all
// inside one tenant transaction. The verified principal (tenant + role) is an
// input; the runtime Biscuit token verification and revocation that produce it
// are a later increment (the TS↔Rust bridge). Per the spec's runtime boundary
// this runs against contract fixtures / a provisioned database, never a real
// orchestrator.

import { type SqlExecutor, withTenantDbTransaction } from "@libre-ai/data";
import { authorizeCommand, type MissionRole } from "../authz/mission-authorization";
import {
  type Command,
  decide,
  type Mission,
  type MissionEvent,
  type RefusalCode,
} from "../domain/mission";
import {
  loadMission,
  MissionRevisionConflictError,
  saveMission,
} from "../persistence/mission-store";

export interface Principal {
  readonly tenantId: string;
  readonly role: MissionRole;
}

export interface CommandRequest {
  readonly command: Command;
  /** Target aggregate for every command except ProposeMission (which carries its own id). */
  readonly missionId?: string;
  /** Optimistic revision the caller holds; 0 for a creation. */
  readonly expectedRevision: number;
}

// App-level refusals sit alongside the domain refusal codes; all are `mission.*`.
export type CommandRefusal =
  | RefusalCode
  | "mission.unauthorized"
  | "mission.not_found"
  | "mission.revision_conflict"
  | "mission.request_invalid";

export type CommandOutcome =
  | { readonly ok: true; readonly mission: Mission; readonly events: readonly MissionEvent[] }
  | { readonly ok: false; readonly refusal: CommandRefusal };

function refuse(refusal: CommandRefusal): CommandOutcome {
  return { ok: false, refusal };
}

/**
 * Execute one mission command for a verified principal. Fail-closed at every
 * layer: an unauthorized role, a missing aggregate, a forbidden transition, a
 * tenant mismatch or a stale revision each refuse without mutating state.
 */
export async function executeMissionCommand(
  executor: SqlExecutor,
  principal: Principal,
  request: CommandRequest,
  now: string,
): Promise<CommandOutcome> {
  // 1. Authorization: the role must be allowed the command's operation under the
  //    locked missions-v1 policy, before any I/O.
  if (!authorizeCommand(principal.role, request.command.type).authorized) {
    return refuse("mission.unauthorized");
  }

  const isCreation = request.command.type === "ProposeMission";
  if (!isCreation && request.missionId === undefined) return refuse("mission.request_invalid");

  return withTenantDbTransaction(executor, principal.tenantId, async (tx) => {
    // 2. Load the current aggregate (creation starts from null).
    let current: Mission | null = null;
    if (!isCreation) {
      current = await loadMission(tx, request.missionId as string);
      if (current === null) return refuse("mission.not_found");
    }

    // 3. Domain decision (fail-closed state machine).
    const decision = decide(current, request.command, {
      tenantId: principal.tenantId,
      now,
      expectedRevision: request.expectedRevision,
    });
    if (!decision.ok) return refuse(decision.refusal);

    // 4. Persist the next aggregate + append its events under optimistic
    //    concurrency. A lost race refuses; the transaction wrote nothing.
    try {
      await saveMission(tx, decision.next, decision.events, now);
    } catch (error) {
      if (error instanceof MissionRevisionConflictError) return refuse("mission.revision_conflict");
      throw error;
    }
    return { ok: true, mission: decision.next, events: decision.events };
  });
}
