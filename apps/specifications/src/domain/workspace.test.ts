import { describe, expect, test } from "bun:test";

import { type Command, decide, type SpecRefusalCode, type WorkspaceState } from "./workspace";

const REV_A = `rev_${"a".repeat(16)}`;
const REV_B = `rev_${"b".repeat(16)}`;

function apply(state: WorkspaceState | null, command: Command): WorkspaceState {
  const decision = decide(state, command);
  if (decision.status !== "accepted")
    throw new Error(`fixture failed: ${JSON.stringify(decision)}`);
  return decision.state;
}

function created(): WorkspaceState {
  return apply(null, {
    type: "CreateSpecWorkspace",
    problem: "A clear problem.",
    actors: ["owner-role", "reviewer-role"],
  });
}

// A draft with one requirement, one contract, one criterion and no open decisions
// (ready to submit). Returns the state at revision 4.
function submittableDraft(): WorkspaceState {
  let s = created(); // rev 1
  s = apply(s, { type: "AddRequirement", expectedRevision: s.revision, requirementId: "req-one" });
  s = apply(s, {
    type: "AttachContract",
    expectedRevision: s.revision,
    contractId: "contract-one",
  });
  s = apply(s, {
    type: "DefineAcceptanceCriterion",
    expectedRevision: s.revision,
    criterionId: "crit-one",
  });
  return s;
}

function acceptedPackage(): WorkspaceState {
  let s = submittableDraft();
  s = apply(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
  s = apply(s, {
    type: "ReviewSpec",
    expectedRevision: s.revision,
    reviewerId: REV_A,
    approve: true,
  });
  s = apply(s, {
    type: "ReviewSpec",
    expectedRevision: s.revision,
    reviewerId: REV_B,
    approve: true,
  });
  return apply(s, { type: "AcceptSpecPackage", expectedRevision: s.revision });
}

describe("decide — creation", () => {
  test("creates a draft at revision 1", () => {
    const decision = decide(null, {
      type: "CreateSpecWorkspace",
      problem: "P",
      actors: ["owner-role"],
    });
    expect(decision.status).toBe("accepted");
    if (decision.status !== "accepted") return;
    expect(decision.state.status).toBe("draft");
    expect(decision.state.revision).toBe(1);
    expect(decision.events).toEqual([{ type: "SpecWorkspaceCreated" }]);
    expect(Object.isFrozen(decision.state)).toBe(true);
  });

  test("a first command other than create is invalid", () => {
    expect(decide(null, { type: "SubmitSpecForReview", expectedRevision: 1 })).toEqual({
      status: "invalid",
    });
  });

  test.each([
    ["empty problem", { problem: "  ", actors: ["owner-role"] }],
    ["no actors", { problem: "P", actors: [] }],
  ])("problem_missing on create: %s", (_label, over) => {
    expect(decide(null, { type: "CreateSpecWorkspace", ...over } as Command)).toEqual({
      status: "refused",
      refusal: "spec.problem_missing",
    });
  });

  test("a malformed or duplicate actor id is invalid", () => {
    expect(decide(null, { type: "CreateSpecWorkspace", problem: "P", actors: ["Owner"] })).toEqual({
      status: "invalid",
    });
    expect(
      decide(null, {
        type: "CreateSpecWorkspace",
        problem: "P",
        actors: ["owner-role", "owner-role"],
      }),
    ).toEqual({ status: "invalid" });
  });

  test("creating again is invalid", () => {
    expect(
      decide(created(), { type: "CreateSpecWorkspace", problem: "P", actors: ["owner-role"] }),
    ).toEqual({
      status: "invalid",
    });
  });
});

describe("decide — draft mutations", () => {
  test("adds content and advances the revision", () => {
    const decision = decide(created(), {
      type: "AddRequirement",
      expectedRevision: 1,
      requirementId: "req-one",
    });
    expect(decision.status).toBe("accepted");
    if (decision.status !== "accepted") return;
    expect(decision.state.revision).toBe(2);
    expect(decision.state.requirementIds).toEqual(["req-one"]);
  });

  test("a stale revision is refused", () => {
    expect(
      decide(created(), { type: "AddRequirement", expectedRevision: 99, requirementId: "req-one" }),
    ).toEqual({
      status: "refused",
      refusal: "spec.revision_stale",
    });
  });

  test.each([
    [
      "malformed requirement id",
      { type: "AddRequirement", expectedRevision: 1, requirementId: "R" },
    ],
    [
      "unknown decision to resolve",
      { type: "ResolveDecision", expectedRevision: 1, decisionId: "dec-x" },
    ],
  ])("invalid draft command: %s", (_label, command) => {
    expect(decide(created(), command as Command)).toEqual({ status: "invalid" });
  });

  test("a duplicate requirement id is invalid", () => {
    const s = apply(created(), {
      type: "AddRequirement",
      expectedRevision: 1,
      requirementId: "req-one",
    });
    expect(
      decide(s, { type: "AddRequirement", expectedRevision: 2, requirementId: "req-one" }),
    ).toEqual({
      status: "invalid",
    });
  });

  test("resolving an open decision moves it to resolved", () => {
    let s = apply(created(), {
      type: "RecordDecision",
      expectedRevision: 1,
      decisionId: "dec-one",
    });
    expect(s.openDecisionIds).toEqual(["dec-one"]);
    s = apply(s, { type: "ResolveDecision", expectedRevision: s.revision, decisionId: "dec-one" });
    expect(s.openDecisionIds).toEqual([]);
    expect(s.resolvedDecisionIds).toEqual(["dec-one"]);
  });
});

describe("decide — submission gating", () => {
  test("submits a complete draft", () => {
    const s = submittableDraft();
    const decision = decide(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
    expect(decision.status).toBe("accepted");
    if (decision.status !== "accepted") return;
    expect(decision.state.status).toBe("submitted");
  });

  test("an open decision blocks submission", () => {
    let s = submittableDraft();
    s = apply(s, { type: "RecordDecision", expectedRevision: s.revision, decisionId: "dec-open" });
    expect(decide(s, { type: "SubmitSpecForReview", expectedRevision: s.revision })).toEqual({
      status: "refused",
      refusal: "spec.decision_open",
    });
  });

  test.each<[string, string, SpecRefusalCode]>([
    ["no requirement", "req-one", "spec.problem_missing"],
    ["no contract", "contract-one", "spec.contract_missing"],
    ["no criterion", "crit-one", "spec.acceptance_unverifiable"],
  ])("submission refused when missing %s", (_label, _omitted, refusal) => {
    // Build a draft missing exactly one required section.
    let s = created();
    if (refusal !== "spec.problem_missing") {
      s = apply(s, {
        type: "AddRequirement",
        expectedRevision: s.revision,
        requirementId: "req-one",
      });
    }
    if (refusal !== "spec.contract_missing") {
      s = apply(s, {
        type: "AttachContract",
        expectedRevision: s.revision,
        contractId: "contract-one",
      });
    }
    if (refusal !== "spec.acceptance_unverifiable") {
      s = apply(s, {
        type: "DefineAcceptanceCriterion",
        expectedRevision: s.revision,
        criterionId: "crit-one",
      });
    }
    expect(decide(s, { type: "SubmitSpecForReview", expectedRevision: s.revision })).toEqual({
      status: "refused",
      refusal,
    });
  });
});

describe("decide — review and acceptance", () => {
  test("accepts after two distinct approvers", () => {
    const accepted = acceptedPackage();
    expect(accepted.status).toBe("accepted");
  });

  test("acceptance is refused with fewer than two approvers", () => {
    let s = submittableDraft();
    s = apply(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
    s = apply(s, {
      type: "ReviewSpec",
      expectedRevision: s.revision,
      reviewerId: REV_A,
      approve: true,
    });
    expect(decide(s, { type: "AcceptSpecPackage", expectedRevision: s.revision })).toEqual({
      status: "refused",
      refusal: "spec.approval_self_only",
    });
  });

  test("the same reviewer approving twice counts once", () => {
    let s = submittableDraft();
    s = apply(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
    s = apply(s, {
      type: "ReviewSpec",
      expectedRevision: s.revision,
      reviewerId: REV_A,
      approve: true,
    });
    s = apply(s, {
      type: "ReviewSpec",
      expectedRevision: s.revision,
      reviewerId: REV_A,
      approve: true,
    });
    expect(s.approverIds).toEqual([REV_A]);
    expect(decide(s, { type: "AcceptSpecPackage", expectedRevision: s.revision })).toEqual({
      status: "refused",
      refusal: "spec.approval_self_only",
    });
  });

  test("review before submission is invalid", () => {
    const s = submittableDraft();
    expect(
      decide(s, {
        type: "ReviewSpec",
        expectedRevision: s.revision,
        reviewerId: REV_A,
        approve: true,
      }),
    ).toEqual({
      status: "invalid",
    });
  });

  test("a malformed reviewer id is invalid", () => {
    let s = submittableDraft();
    s = apply(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
    expect(
      decide(s, {
        type: "ReviewSpec",
        expectedRevision: s.revision,
        reviewerId: "reviewer",
        approve: true,
      }),
    ).toEqual({
      status: "invalid",
    });
  });
});

describe("decide — immutability and supersession", () => {
  test("content mutation of an accepted package is refused as immutable", () => {
    const accepted = acceptedPackage();
    expect(
      decide(accepted, {
        type: "AddRequirement",
        expectedRevision: accepted.revision,
        requirementId: "req-late",
      }),
    ).toEqual({ status: "refused", refusal: "spec.package_immutable" });
  });

  test("submitting an accepted package is refused as immutable", () => {
    const accepted = acceptedPackage();
    expect(
      decide(accepted, { type: "SubmitSpecForReview", expectedRevision: accepted.revision }),
    ).toEqual({
      status: "refused",
      refusal: "spec.package_immutable",
    });
  });

  test("content mutation of a submitted (review-frozen) version is invalid", () => {
    let s = submittableDraft();
    s = apply(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
    expect(
      decide(s, {
        type: "AddRequirement",
        expectedRevision: s.revision,
        requirementId: "req-late",
      }),
    ).toEqual({
      status: "invalid",
    });
  });

  test("supersedes an accepted package", () => {
    const accepted = acceptedPackage();
    const decision = decide(accepted, {
      type: "SupersedeSpecPackage",
      expectedRevision: accepted.revision,
    });
    expect(decision.status).toBe("accepted");
    if (decision.status !== "accepted") return;
    expect(decision.state.status).toBe("superseded");
  });

  test("superseding a non-accepted workspace is invalid", () => {
    expect(decide(created(), { type: "SupersedeSpecPackage", expectedRevision: 1 })).toEqual({
      status: "invalid",
    });
  });
});

describe("decide — stale revision on every state transition", () => {
  const STALE = { status: "refused", refusal: "spec.revision_stale" } as const;

  test("SubmitSpecForReview", () => {
    expect(
      decide(submittableDraft(), { type: "SubmitSpecForReview", expectedRevision: 99 }),
    ).toEqual(STALE);
  });

  test("ReviewSpec", () => {
    let s = submittableDraft();
    s = apply(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
    expect(
      decide(s, { type: "ReviewSpec", expectedRevision: 99, reviewerId: REV_A, approve: true }),
    ).toEqual(STALE);
  });

  test("AcceptSpecPackage", () => {
    let s = submittableDraft();
    s = apply(s, { type: "SubmitSpecForReview", expectedRevision: s.revision });
    s = apply(s, {
      type: "ReviewSpec",
      expectedRevision: s.revision,
      reviewerId: REV_A,
      approve: true,
    });
    s = apply(s, {
      type: "ReviewSpec",
      expectedRevision: s.revision,
      reviewerId: REV_B,
      approve: true,
    });
    expect(decide(s, { type: "AcceptSpecPackage", expectedRevision: 99 })).toEqual(STALE);
  });

  test("SupersedeSpecPackage", () => {
    expect(
      decide(acceptedPackage(), { type: "SupersedeSpecPackage", expectedRevision: 99 }),
    ).toEqual(STALE);
  });

  test("CreatePlanningHandoff", () => {
    expect(
      decide(acceptedPackage(), {
        type: "CreatePlanningHandoff",
        expectedRevision: 99,
        requestsExecution: false,
      }),
    ).toEqual(STALE);
  });
});

describe("decide — planning handoff", () => {
  test("creates a read-only handoff from an accepted package", () => {
    const accepted = acceptedPackage();
    const decision = decide(accepted, {
      type: "CreatePlanningHandoff",
      expectedRevision: accepted.revision,
      requestsExecution: false,
    });
    expect(decision.status).toBe("accepted");
    if (decision.status !== "accepted") return;
    expect(decision.events).toEqual([{ type: "PlanningHandoffCreated" }]);
  });

  test("a handoff requesting execution is refused", () => {
    const accepted = acceptedPackage();
    expect(
      decide(accepted, {
        type: "CreatePlanningHandoff",
        expectedRevision: accepted.revision,
        requestsExecution: true,
      }),
    ).toEqual({ status: "refused", refusal: "spec.handoff_execution_right" });
  });

  test("a handoff from a draft is invalid", () => {
    expect(
      decide(created(), {
        type: "CreatePlanningHandoff",
        expectedRevision: 1,
        requestsExecution: false,
      }),
    ).toEqual({
      status: "invalid",
    });
  });
});
