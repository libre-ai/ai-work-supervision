// Specifications domain — the pure, revisioned spec-workspace lifecycle
// (docs/apps/specifications.md §Domain protocol). This is the authoring
// counterpart to the accepted-package validator (spec-package.ts): the workspace
// is HOW a package is built and accepted; the validator checks the FINAL bytes.
// A draft is revisioned and mutable; a submitted version is review-frozen; an
// accepted package is immutable; supersession is terminal. `decide` is a pure
// fold — (state, command) → a decision — with optimistic concurrency by revision.
// It imports nothing and does not persist. Fail-closed with three outcomes:
//   - `accepted` — the command applies; events + the advanced state.
//   - `refused`  — a domain invariant is violated, with the exact spec.* code.
//   - `invalid`  — the command does not apply to this state, or carries a
//     malformed id (a boundary concern; the matrix has no code for it).

const IDENTIFIER = /^[a-z][a-z0-9_-]{2,127}$/;
const OPAQUE_REVIEWER = /^rev_[a-z0-9]{16,64}$/;

export type Status = "draft" | "submitted" | "accepted" | "superseded";

// Only the matrix codes this lifecycle enforces (evidence_hash_mismatch is the
// package validator's concern, not the workspace's).
export type SpecRefusalCode =
  | "spec.problem_missing"
  | "spec.contract_missing"
  | "spec.acceptance_unverifiable"
  | "spec.decision_open"
  | "spec.approval_self_only"
  | "spec.revision_stale"
  | "spec.package_immutable"
  | "spec.handoff_execution_right";

export type Command =
  | {
      readonly type: "CreateSpecWorkspace";
      readonly problem: string;
      readonly actors: readonly string[];
    }
  | {
      readonly type: "AddRequirement";
      readonly expectedRevision: number;
      readonly requirementId: string;
    }
  | {
      readonly type: "AttachContract";
      readonly expectedRevision: number;
      readonly contractId: string;
    }
  | {
      readonly type: "DefineAcceptanceCriterion";
      readonly expectedRevision: number;
      readonly criterionId: string;
    }
  | {
      readonly type: "RecordDecision";
      readonly expectedRevision: number;
      readonly decisionId: string;
    }
  | {
      readonly type: "ResolveDecision";
      readonly expectedRevision: number;
      readonly decisionId: string;
    }
  | { readonly type: "SubmitSpecForReview"; readonly expectedRevision: number }
  | {
      readonly type: "ReviewSpec";
      readonly expectedRevision: number;
      readonly reviewerId: string;
      readonly approve: boolean;
    }
  | { readonly type: "AcceptSpecPackage"; readonly expectedRevision: number }
  | { readonly type: "SupersedeSpecPackage"; readonly expectedRevision: number }
  | {
      readonly type: "CreatePlanningHandoff";
      readonly expectedRevision: number;
      readonly requestsExecution: boolean;
    };

export type Event =
  | { readonly type: "SpecWorkspaceCreated" }
  | { readonly type: "RequirementAdded"; readonly requirementId: string }
  | { readonly type: "ContractAttached"; readonly contractId: string }
  | { readonly type: "AcceptanceCriterionDefined"; readonly criterionId: string }
  | { readonly type: "DecisionRecorded"; readonly decisionId: string }
  | { readonly type: "DecisionResolved"; readonly decisionId: string }
  | { readonly type: "SpecSubmitted" }
  | { readonly type: "SpecReviewRecorded"; readonly reviewerId: string; readonly approve: boolean }
  | { readonly type: "SpecPackageAccepted" }
  | { readonly type: "SpecPackageSuperseded" }
  | { readonly type: "PlanningHandoffCreated" };

export interface WorkspaceState {
  readonly status: Status;
  readonly revision: number;
  readonly problem: string;
  readonly actorCount: number;
  readonly requirementIds: readonly string[];
  readonly contractIds: readonly string[];
  readonly criterionIds: readonly string[];
  readonly openDecisionIds: readonly string[];
  readonly resolvedDecisionIds: readonly string[];
  readonly approverIds: readonly string[];
}

export type Decision =
  | {
      readonly status: "accepted";
      readonly events: readonly Event[];
      readonly state: WorkspaceState;
    }
  | { readonly status: "refused"; readonly refusal: SpecRefusalCode }
  | { readonly status: "invalid" };

const INVALID: Decision = { status: "invalid" };
function refuse(refusal: SpecRefusalCode): Decision {
  return { status: "refused", refusal };
}
function accept(events: readonly Event[], state: WorkspaceState): Decision {
  return { status: "accepted", events: Object.freeze([...events]), state };
}

function isId(value: unknown): value is string {
  return typeof value === "string" && IDENTIFIER.test(value);
}

function advanced(state: WorkspaceState, patch: Partial<WorkspaceState>): WorkspaceState {
  return Object.freeze({ ...state, ...patch, revision: state.revision + 1 });
}

// A content mutation is draft-only. On an immutable version (accepted/superseded)
// it is `package_immutable`; on a review-frozen submitted version it does not
// apply (`invalid`); on a stale revision it is `revision_stale`.
function guardDraftMutation(state: WorkspaceState, expectedRevision: number): Decision | undefined {
  if (state.status === "accepted" || state.status === "superseded")
    return refuse("spec.package_immutable");
  if (state.status !== "draft") return INVALID;
  if (expectedRevision !== state.revision) return refuse("spec.revision_stale");
  return undefined;
}

/**
 * Fold a command onto the workspace state. Pass `null` for the first command,
 * which must be `CreateSpecWorkspace`.
 */
export function decide(state: WorkspaceState | null, command: Command): Decision {
  if (state === null) {
    if (command.type !== "CreateSpecWorkspace") return INVALID;
    if (!command.actors.every(isId) || new Set(command.actors).size !== command.actors.length)
      return INVALID;
    if (command.problem.trim().length === 0 || command.actors.length === 0)
      return refuse("spec.problem_missing");
    return accept(
      [{ type: "SpecWorkspaceCreated" }],
      Object.freeze({
        status: "draft",
        revision: 1,
        problem: command.problem,
        actorCount: command.actors.length,
        requirementIds: Object.freeze([]),
        contractIds: Object.freeze([]),
        criterionIds: Object.freeze([]),
        openDecisionIds: Object.freeze([]),
        resolvedDecisionIds: Object.freeze([]),
        approverIds: Object.freeze([]),
      }),
    );
  }

  switch (command.type) {
    case "CreateSpecWorkspace":
      return INVALID; // already created

    case "AddRequirement":
    case "AttachContract":
    case "DefineAcceptanceCriterion":
    case "RecordDecision":
    case "ResolveDecision": {
      const blocked = guardDraftMutation(state, command.expectedRevision);
      if (blocked) return blocked;
      return decideContent(state, command);
    }

    case "SubmitSpecForReview": {
      if (state.status === "accepted" || state.status === "superseded")
        return refuse("spec.package_immutable");
      if (state.status !== "draft") return INVALID;
      if (command.expectedRevision !== state.revision) return refuse("spec.revision_stale");
      if (state.openDecisionIds.length > 0) return refuse("spec.decision_open");
      // The matrix has no requirement-specific code; a package with no requirement
      // lacks the "outcome" half of problem/actor/outcome, so it maps to
      // spec.problem_missing (docs/apps/specifications.md §Refusal matrix).
      if (state.requirementIds.length === 0) return refuse("spec.problem_missing");
      if (state.contractIds.length === 0) return refuse("spec.contract_missing");
      if (state.criterionIds.length === 0) return refuse("spec.acceptance_unverifiable");
      return accept([{ type: "SpecSubmitted" }], advanced(state, { status: "submitted" }));
    }

    case "ReviewSpec": {
      if (state.status !== "submitted") return INVALID;
      if (command.expectedRevision !== state.revision) return refuse("spec.revision_stale");
      if (!OPAQUE_REVIEWER.test(command.reviewerId)) return INVALID;
      const approverIds =
        command.approve && !state.approverIds.includes(command.reviewerId)
          ? Object.freeze([...state.approverIds, command.reviewerId])
          : state.approverIds;
      return accept(
        [{ type: "SpecReviewRecorded", reviewerId: command.reviewerId, approve: command.approve }],
        advanced(state, { approverIds }),
      );
    }

    case "AcceptSpecPackage": {
      if (state.status !== "submitted") return INVALID;
      if (command.expectedRevision !== state.revision) return refuse("spec.revision_stale");
      if (state.approverIds.length < 2) return refuse("spec.approval_self_only");
      return accept([{ type: "SpecPackageAccepted" }], advanced(state, { status: "accepted" }));
    }

    case "SupersedeSpecPackage": {
      if (state.status !== "accepted") return INVALID;
      if (command.expectedRevision !== state.revision) return refuse("spec.revision_stale");
      return accept([{ type: "SpecPackageSuperseded" }], advanced(state, { status: "superseded" }));
    }

    case "CreatePlanningHandoff": {
      if (state.status !== "accepted") return INVALID;
      if (command.expectedRevision !== state.revision) return refuse("spec.revision_stale");
      // A planning handoff is read-only by construction; requesting an executable
      // capability is a separation-of-powers violation. A handoff is a projection,
      // not a state mutation — status stays "accepted"; the revision advances only
      // to distinguish successive handoff events on the immutable package.
      if (command.requestsExecution) return refuse("spec.handoff_execution_right");
      return accept([{ type: "PlanningHandoffCreated" }], advanced(state, {}));
    }
    default:
      // Exhaustiveness guard: a new Command variant without a case above fails to
      // compile here; at runtime an unknown command is refused fail-closed.
      command satisfies never;
      return INVALID;
  }
}

function decideContent(
  state: WorkspaceState,
  command:
    | { readonly type: "AddRequirement"; readonly requirementId: string }
    | { readonly type: "AttachContract"; readonly contractId: string }
    | { readonly type: "DefineAcceptanceCriterion"; readonly criterionId: string }
    | { readonly type: "RecordDecision"; readonly decisionId: string }
    | { readonly type: "ResolveDecision"; readonly decisionId: string },
): Decision {
  switch (command.type) {
    case "AddRequirement": {
      if (!isId(command.requirementId) || state.requirementIds.includes(command.requirementId))
        return INVALID;
      return accept(
        [{ type: "RequirementAdded", requirementId: command.requirementId }],
        advanced(state, {
          requirementIds: Object.freeze([...state.requirementIds, command.requirementId]),
        }),
      );
    }
    case "AttachContract": {
      if (!isId(command.contractId) || state.contractIds.includes(command.contractId))
        return INVALID;
      return accept(
        [{ type: "ContractAttached", contractId: command.contractId }],
        advanced(state, { contractIds: Object.freeze([...state.contractIds, command.contractId]) }),
      );
    }
    case "DefineAcceptanceCriterion": {
      if (!isId(command.criterionId) || state.criterionIds.includes(command.criterionId))
        return INVALID;
      return accept(
        [{ type: "AcceptanceCriterionDefined", criterionId: command.criterionId }],
        advanced(state, {
          criterionIds: Object.freeze([...state.criterionIds, command.criterionId]),
        }),
      );
    }
    case "RecordDecision": {
      if (
        !isId(command.decisionId) ||
        state.openDecisionIds.includes(command.decisionId) ||
        state.resolvedDecisionIds.includes(command.decisionId)
      ) {
        return INVALID;
      }
      return accept(
        [{ type: "DecisionRecorded", decisionId: command.decisionId }],
        advanced(state, {
          openDecisionIds: Object.freeze([...state.openDecisionIds, command.decisionId]),
        }),
      );
    }
    case "ResolveDecision": {
      if (!state.openDecisionIds.includes(command.decisionId)) return INVALID;
      return accept(
        [{ type: "DecisionResolved", decisionId: command.decisionId }],
        advanced(state, {
          openDecisionIds: Object.freeze(
            state.openDecisionIds.filter((id) => id !== command.decisionId),
          ),
          resolvedDecisionIds: Object.freeze([...state.resolvedDecisionIds, command.decisionId]),
        }),
      );
    }
  }
}
