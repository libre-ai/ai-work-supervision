import { describe, expect, test } from "bun:test";
import { type Command, decide, type WorkspaceState } from "../domain/workspace";
import { decideAcceptance } from "./accept-package";

const NOW = "2030-01-01T00:00:00Z";
const REVIEWER_1 = "rev_aaaaaaaaaaaaaaaa";
const REVIEWER_2 = "rev_bbbbbbbbbbbbbbbb";

// Fold a command sequence through `decide`, asserting each step is accepted, and
// return the final state. A refused/invalid step throws (the fixtures are valid).
function fold(commands: readonly Command[]): WorkspaceState {
  let state: WorkspaceState | null = null;
  for (const command of commands) {
    const decision = decide(state, command);
    if (decision.status !== "accepted") {
      throw new Error(`fixture command ${command.type} was ${decision.status}`);
    }
    state = decision.state;
  }
  if (state === null) throw new Error("empty command sequence");
  return state;
}

// A workspace folded to `submitted`, one review per approver. Its revision after
// N reviews is 5 + N (create=1, requirement=2, contract=3, criterion=4, submit=5).
function submittedWorkspace(approvers: readonly string[]): WorkspaceState {
  const base: Command[] = [
    { type: "CreateSpecWorkspace", problem: "ship the thing", actors: ["author-a"] },
    { type: "AddRequirement", expectedRevision: 1, requirementId: "req-one" },
    { type: "AttachContract", expectedRevision: 2, contractId: "contract-one" },
    { type: "DefineAcceptanceCriterion", expectedRevision: 3, criterionId: "crit-one" },
    { type: "SubmitSpecForReview", expectedRevision: 4 },
  ];
  const reviews: Command[] = approvers.map((reviewerId, index) => ({
    type: "ReviewSpec",
    expectedRevision: 5 + index,
    reviewerId,
    approve: true,
  }));
  return fold([...base, ...reviews]);
}

function validPackage(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    schemaVersion: "libre-ai.spec-package.v1",
    id: "urn:libre-ai:spec-package:p1",
    tenantId: "ten_aaaaaaaaaaaaaaaa",
    version: 1,
    status: "accepted",
    problem: "ship the thing",
    actors: ["author-a"],
    requirements: [{ id: "req-one", text: "do x", priority: "must" }],
    decisions: [],
    contracts: ["urn:libre-ai:contract:c1"],
    risks: [],
    acceptanceCriteria: [{ id: "crit-one", observable: "x happens", evidenceRule: "rule-one" }],
    approvals: [
      {
        role: "reviewer",
        approvedAt: NOW,
        reference: "urn:libre-ai:approval:a1",
        subjectDigest: "b".repeat(64),
      },
      {
        role: "reviewer",
        approvedAt: NOW,
        reference: "urn:libre-ai:approval:a2",
        subjectDigest: "c".repeat(64),
      },
    ],
    acceptedAt: NOW,
    digest: "a".repeat(64),
    ...overrides,
  };
}

describe("decideAcceptance — both gates must pass", () => {
  test("a submitted workspace with two approvers and valid bytes is accepted", () => {
    const state = submittedWorkspace([REVIEWER_1, REVIEWER_2]);
    const result = decideAcceptance(state, validPackage(), 7);
    expect(result.status).toBe("accepted");
    if (result.status !== "accepted") return;
    expect(result.state.status).toBe("accepted");
    expect(result.package.id).toBe("urn:libre-ai:spec-package:p1");
    expect(result.events).toEqual([{ type: "SpecPackageAccepted" }]);
  });

  test("a draft workspace does not apply (invalid), even with valid bytes", () => {
    const draft = fold([{ type: "CreateSpecWorkspace", problem: "p", actors: ["author-a"] }]);
    expect(decideAcceptance(draft, validPackage(), 1)).toEqual({ status: "invalid" });
  });

  test("fewer than two approvers is refused approval_self_only (lifecycle gate)", () => {
    const state = submittedWorkspace([REVIEWER_1]);
    expect(decideAcceptance(state, validPackage(), 6)).toEqual({
      status: "refused",
      refusal: "spec.approval_self_only",
    });
  });

  test("a stale revision is refused revision_stale (lifecycle gate)", () => {
    const state = submittedWorkspace([REVIEWER_1, REVIEWER_2]);
    expect(decideAcceptance(state, validPackage(), 6)).toEqual({
      status: "refused",
      refusal: "spec.revision_stale",
    });
  });

  test("malformed package bytes are malformed (byte gate)", () => {
    const state = submittedWorkspace([REVIEWER_1, REVIEWER_2]);
    expect(decideAcceptance(state, {}, 7)).toEqual({ status: "malformed" });
  });

  test("a well-formed but incomplete package is refused with its code (byte gate)", () => {
    const state = submittedWorkspace([REVIEWER_1, REVIEWER_2]);
    // no contracts → the validator refuses contract_missing.
    expect(decideAcceptance(state, validPackage({ contracts: [] }), 7)).toEqual({
      status: "refused",
      refusal: "spec.contract_missing",
    });
  });

  test("the lifecycle gate runs before the byte gate", () => {
    const state = submittedWorkspace([REVIEWER_1, REVIEWER_2]);
    // A stale revision with malformed bytes surfaces the lifecycle refusal, not
    // the byte one — proving the order.
    expect(decideAcceptance(state, {}, 6)).toEqual({
      status: "refused",
      refusal: "spec.revision_stale",
    });
  });
});
