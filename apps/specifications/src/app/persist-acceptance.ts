// Persisted acceptance path — composes the pure decideAcceptance gate with
// persistence. Loads the workspace, runs the pure decideAcceptance, and on
// accept: persists the workspace transition AND stores the content-addressed
// package. Fail-closed: a refused or malformed decision persists nothing.
// Runs inside the caller's tenant transaction (packages/data withTenantDbTransaction).

import type { SqlExecutor } from "@libre-ai/data";
import { saveAcceptedPackage } from "../persistence/spec-package-store";
import { loadWorkspace, saveWorkspace } from "../persistence/spec-workspace-store";
import { type AcceptanceDecision, decideAcceptance } from "./accept-package";

/**
 * Persist an acceptance decision. Load the workspace, run the pure decideAcceptance,
 * and on accept: save the workspace transition and store the content-addressed package.
 * Fail-closed: a refused, malformed, or invalid decision persists nothing.
 *
 * ATOMICITY: both writes run on the provided `executor`. Call this within a
 * transaction (the app wraps it in `withTenantDbTransaction`) so the two writes
 * commit together — if either throws (a stale `SpecWorkspaceRevisionConflictError`
 * or a `SpecPackageDigestConflictError`), the whole transaction rolls back and
 * NOTHING is persisted. There is no partial-write window.
 *
 * DIGEST TRUST (documented residual): the package `digest` is format-validated
 * (SHA-256 hex) and stored as the content address, but is NOT recomputed from the
 * package bytes here — the spec-package digest preimage is not yet a defined
 * canonical serialization (see `domain/spec-package.ts`). Content-address
 * verification against the bytes is deferred to that preimage definition; today
 * the caller (the post-`decideAcceptance` app path) is trusted for it.
 */
export async function persistAcceptance(
  executor: SqlExecutor,
  workspaceId: string,
  packageInput: unknown,
  expectedRevision: number,
  acceptedBy?: string,
  recordedAt?: string,
): Promise<AcceptanceDecision> {
  const now = recordedAt ?? new Date().toISOString();

  // Load the workspace. If not found, decide with null (invalid state).
  const state = await loadWorkspace(executor, workspaceId);

  // Run the pure decision gate.
  const decision = decideAcceptance(state, packageInput, expectedRevision);

  // Fail-closed: if not accepted, persist nothing.
  if (decision.status !== "accepted") {
    return decision;
  }

  // Persist the workspace transition.
  await saveWorkspace(executor, workspaceId, decision.state, decision.events, now);

  // Persist the content-addressed package. This may throw SpecPackageDigestConflictError
  // if the workspace already has a different digest (a safeguard, though the workspace
  // state machine prevents re-acceptance of an already-accepted workspace).
  const pkg = decision.package;
  await saveAcceptedPackage(executor, pkg, workspaceId, now, acceptedBy);

  return decision;
}
