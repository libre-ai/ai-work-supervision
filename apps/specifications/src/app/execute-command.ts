// Specifications command service — the write path that composes the spec-workspace
// vertical for one command inside the caller's tenant transaction
// (packages/data withTenantDbTransaction): load the aggregate, fold the command
// through the pure domain (`decide`), then persist the next snapshot and append
// its events under optimistic concurrency. Fail-closed at every layer: a missing
// aggregate, a domain refusal, an inapplicable command, or a lost concurrency race
// each refuse without mutating state.
//
// There is no authorization layer here: unlike missions/sessions, specifications
// has no locked authorizer datalog, so no role matrix is composed (inventing one
// would not be contract-faithful). The verified tenant is an input; the session
// authentication that produces it is a server-layer concern.
//
// `AcceptSpecPackage` is intentionally NOT reachable through this entrypoint:
// acceptance must additionally validate the submitted content-addressed package
// bytes (the `validateSpecPackage` seam), so routing it here — where only the
// lifecycle gate would run — would be fail-open against that invariant. It is
// refused `spec.request_invalid` until the dedicated acceptance path exists.

import { type SqlExecutor, withTenantDbTransaction } from "@libre-ai/data";
import {
  type Command,
  decide,
  type Event,
  type SpecRefusalCode,
  type WorkspaceState,
} from "../domain/workspace";
import {
  loadWorkspace,
  SpecWorkspaceRevisionConflictError,
  saveWorkspace,
} from "../persistence/spec-workspace-store";

// App-level refusals sit alongside the domain refusal codes; all are `spec.*`.
// `request_invalid` is the boundary code for a command that does not apply to the
// current state (the domain's `invalid`) or that is not reachable here;
// `revision_conflict` is a lost persistence race (distinct from the domain's
// `revision_stale`, which is the caller's own optimistic check failing).
export type SpecCommandRefusal =
  | SpecRefusalCode
  | "spec.not_found"
  | "spec.request_invalid"
  | "spec.revision_conflict";

export type SpecCommandOutcome =
  | { readonly ok: true; readonly state: WorkspaceState; readonly events: readonly Event[] }
  | { readonly ok: false; readonly refusal: SpecCommandRefusal };

function refuse(refusal: SpecCommandRefusal): SpecCommandOutcome {
  return { ok: false, refusal };
}

/**
 * Execute one spec-workspace command for a verified tenant. In order: guard the
 * acceptance seam out (fail-closed), then within one tenant transaction load the
 * current aggregate (creation starts from null; a missing target is
 * `spec.not_found`), fold the command through `decide` (a domain refusal returns
 * its `spec.*` code; an inapplicable command is `spec.request_invalid`), and
 * persist the next snapshot + events under optimistic concurrency (a lost race is
 * `spec.revision_conflict`, and the transaction wrote nothing).
 */
export async function executeSpecCommand(
  executor: SqlExecutor,
  tenantId: string,
  workspaceId: string,
  command: Command,
  now: string,
): Promise<SpecCommandOutcome> {
  if (command.type === "AcceptSpecPackage") return refuse("spec.request_invalid");

  const isCreation = command.type === "CreateSpecWorkspace";

  return withTenantDbTransaction(executor, tenantId, async (tx) => {
    let current: WorkspaceState | null = null;
    if (!isCreation) {
      current = await loadWorkspace(tx, workspaceId);
      if (current === null) return refuse("spec.not_found");
    }

    const decision = decide(current, command);
    if (decision.status === "refused") return refuse(decision.refusal);
    if (decision.status === "invalid") return refuse("spec.request_invalid");

    try {
      await saveWorkspace(tx, workspaceId, decision.state, decision.events, now);
    } catch (error) {
      if (error instanceof SpecWorkspaceRevisionConflictError) {
        return refuse("spec.revision_conflict");
      }
      throw error;
    }
    return { ok: true, state: decision.state, events: decision.events };
  });
}
