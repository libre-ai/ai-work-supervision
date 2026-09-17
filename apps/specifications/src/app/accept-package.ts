// Specifications acceptance seam — the pure composition that gates accepting a
// spec-workspace into an immutable, content-addressed package. It joins the two
// guarantees the two-layer design keeps deliberately separate:
//   - the workspace LIFECYCLE (workspace.ts `decide` on AcceptSpecPackage): the
//     version is submitted, the caller's revision is current, and ≥2 distinct
//     reviewers approved (separation of powers);
//   - the package BYTES (spec-package.ts `validateSpecPackage`): the submitted
//     document is a well-formed, semantically complete accepted package.
// Acceptance is permitted only if BOTH pass — a workspace can never be accepted
// into an invalid package, and invalid bytes can never slip past the lifecycle.
//
// This is pure: no I/O, no persistence, no tenant context. The persisted
// acceptance path (which loads the workspace, calls this, saves the accepted
// transition, guards the package body's tenant, and stores the content-addressed
// package) is a later increment — it depends on an open design decision (whether
// the accepted package is stored as its own content-addressed row and/or its
// digest is recorded on the workspace).

import { type SpecPackage, validateSpecPackage } from "../domain/spec-package";
import { decide, type Event, type SpecRefusalCode, type WorkspaceState } from "../domain/workspace";

export type AcceptanceDecision =
  | {
      readonly status: "accepted";
      readonly package: SpecPackage;
      readonly events: readonly Event[];
      readonly state: WorkspaceState;
    }
  | { readonly status: "refused"; readonly refusal: SpecRefusalCode }
  // The package bytes are not a well-formed spec package at all — a boundary
  // concern with no matrix code, mirroring the validator's `malformed`.
  | { readonly status: "malformed" }
  // AcceptSpecPackage does not apply to this workspace state (e.g. not submitted)
  // — a boundary concern with no matrix code, mirroring the domain's `invalid`.
  | { readonly status: "invalid" };

/**
 * Decide whether a submitted workspace may be accepted into `packageInput`. The
 * lifecycle gate runs first (`decide` on AcceptSpecPackage): an inapplicable state
 * is `invalid`, a violated invariant (stale revision, fewer than two approvers) is
 * `refused` with its `spec.*` code. Only then is the byte gate run
 * (`validateSpecPackage`): malformed bytes are `malformed`, a well-formed but
 * incomplete package is `refused` with its code. When both pass, `accepted`
 * carries the validated package, the accept event, and the advanced workspace
 * state — none of which is persisted here.
 */
export function decideAcceptance(
  state: WorkspaceState | null,
  packageInput: unknown,
  expectedRevision: number,
): AcceptanceDecision {
  const lifecycle = decide(state, { type: "AcceptSpecPackage", expectedRevision });
  if (lifecycle.status === "invalid") return { status: "invalid" };
  if (lifecycle.status === "refused") return { status: "refused", refusal: lifecycle.refusal };

  const bytes = validateSpecPackage(packageInput);
  if (bytes.status === "malformed") return { status: "malformed" };
  if (bytes.status === "refused") return { status: "refused", refusal: bytes.refusal };

  return {
    status: "accepted",
    package: bytes.value,
    events: lifecycle.events,
    state: lifecycle.state,
  };
}
