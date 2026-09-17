import { describe, expect, test } from "bun:test";

import { validateSpecPackage } from "./spec-package";

const VALID = {
  schemaVersion: "libre-ai.spec-package.v1",
  id: "urn:libre-ai:spec:pkg-alpha",
  tenantId: "ten_aaaaaaaaaaaaaaaa",
  version: 1,
  status: "accepted",
  problem: "We need an accessible civic tool.",
  actors: ["owner-role", "reviewer-role"],
  requirements: [{ id: "req-one", text: "must do X", priority: "must" }],
  decisions: [{ id: "dec-one", status: "accepted", decision: "chose approach A" }],
  contracts: ["urn:libre-ai:contract:c-one"],
  risks: [{ id: "risk-one", severity: "high", control: "documented mitigation" }],
  acceptanceCriteria: [
    { id: "ac-one", observable: "the flow works offline", evidenceRule: "gate-one" },
  ],
  approvals: [
    {
      role: "methodological-review",
      approvedAt: "2026-07-21T10:00:00Z",
      reference: "urn:libre-ai:review:r-1",
      subjectDigest: "a".repeat(64),
    },
    {
      role: "legal-privacy-review",
      approvedAt: "2026-07-21T11:00:00Z",
      reference: "urn:libre-ai:review:r-2",
      subjectDigest: "b".repeat(64),
    },
  ],
  acceptedAt: "2026-07-21T12:00:00Z",
  digest: "c".repeat(64),
} as const;

function raw(overrides: Record<string, unknown>): Record<string, unknown> {
  return { ...VALID, ...overrides };
}

describe("validateSpecPackage — accepts a conformant accepted package", () => {
  test("a fully conformant package is valid", () => {
    const result = validateSpecPackage(VALID);
    expect(result.status).toBe("valid");
    if (result.status !== "valid") return;
    expect(result.value.actors).toEqual(["owner-role", "reviewer-role"]);
    expect(Object.isFrozen(result.value)).toBe(true);
    expect(Object.isFrozen(result.value.approvals[0])).toBe(true);
  });

  test("empty decisions and risks are allowed (schema has no minItems)", () => {
    expect(validateSpecPackage(raw({ decisions: [], risks: [] })).status).toBe("valid");
  });
});

describe("validateSpecPackage — malformed (structural / identity)", () => {
  test.each([
    ["unknown top-level key", { extra: 1 }],
    ["wrong schemaVersion", { schemaVersion: "libre-ai.spec-package.v2" }],
    ["id not a urn", { id: "pkg-alpha" }],
    ["tenantId without ten_", { tenantId: "org-example" }],
    ["version below 1", { version: 0 }],
    ["status not accepted", { status: "draft" }],
    ["acceptedAt malformed", { acceptedAt: "2026-07-21" }],
    ["digest not sha256", { digest: "z".repeat(64) }],
    ["requirements empty", { requirements: [] }],
    [
      "requirement bad priority",
      { requirements: [{ id: "req-one", text: "x", priority: "maybe" }] },
    ],
    ["risk bad severity", { risks: [{ id: "risk-one", severity: "apocalyptic", control: "x" }] }],
    [
      "approval missing subjectDigest",
      {
        approvals: [
          { role: "r", approvedAt: "2026-07-21T10:00:00Z", reference: "urn:libre-ai:review:r-1" },
        ],
      },
    ],
    [
      "decision structurally bad",
      { decisions: [{ id: "Dec-One", status: "accepted", decision: "x" }] },
    ],
    [
      "requirements over 1000",
      {
        requirements: Array.from({ length: 1001 }, () => ({
          id: "req-x",
          text: "x",
          priority: "must",
        })),
      },
    ],
    [
      "requirement text over 5000",
      { requirements: [{ id: "req-one", text: "x".repeat(5001), priority: "must" }] },
    ],
  ])("is malformed: %s", (_label, override) => {
    expect(validateSpecPackage(raw(override))).toEqual({ status: "malformed" });
  });

  test("a non-object input is malformed", () => {
    expect(validateSpecPackage(null)).toEqual({ status: "malformed" });
    expect(validateSpecPackage([])).toEqual({ status: "malformed" });
  });
});

describe("validateSpecPackage — refused (semantic completeness)", () => {
  test.each([
    ["empty problem", { problem: "" }],
    ["problem over 5000 chars", { problem: "x".repeat(5001) }],
    ["no actors", { actors: [] }],
    ["actors over 100", { actors: Array.from({ length: 101 }, (_v, i) => `actor-${i}`) }],
    ["actor malformed", { actors: ["Owner-Role"] }],
    ["duplicate actors", { actors: ["owner-role", "owner-role"] }],
  ])("problem_missing: %s", (_label, override) => {
    expect(validateSpecPackage(raw(override))).toEqual({
      status: "refused",
      refusal: "spec.problem_missing",
    });
  });

  test.each([
    ["no contracts", { contracts: [] }],
    ["contract not a urn", { contracts: ["c-one"] }],
    [
      "duplicate contracts",
      { contracts: ["urn:libre-ai:contract:c-one", "urn:libre-ai:contract:c-one"] },
    ],
    [
      "contracts over 1000",
      { contracts: Array.from({ length: 1001 }, (_v, i) => `urn:libre-ai:contract:c-${i}`) },
    ],
  ])("contract_missing: %s", (_label, override) => {
    expect(validateSpecPackage(raw(override))).toEqual({
      status: "refused",
      refusal: "spec.contract_missing",
    });
  });

  test.each([
    ["no criteria", { acceptanceCriteria: [] }],
    ["criterion missing evidenceRule", { acceptanceCriteria: [{ id: "ac-one", observable: "x" }] }],
    [
      "criteria over 1000",
      {
        acceptanceCriteria: Array.from({ length: 1001 }, () => ({
          id: "ac-x",
          observable: "x",
          evidenceRule: "gate-x",
        })),
      },
    ],
  ])("acceptance_unverifiable: %s", (_label, override) => {
    expect(validateSpecPackage(raw(override))).toEqual({
      status: "refused",
      refusal: "spec.acceptance_unverifiable",
    });
  });

  test("decision_open: a decision whose status is not accepted", () => {
    expect(
      validateSpecPackage(
        raw({ decisions: [{ id: "dec-one", status: "open", decision: "pending" }] }),
      ),
    ).toEqual({ status: "refused", refusal: "spec.decision_open" });
  });

  test.each([
    ["a single approver", { approvals: [VALID.approvals[0]] }],
    [
      "two approvals with the same reference",
      {
        approvals: [
          VALID.approvals[0],
          { ...VALID.approvals[1], reference: "urn:libre-ai:review:r-1" },
        ],
      },
    ],
  ])("approval_self_only: %s", (_label, override) => {
    expect(validateSpecPackage(raw(override))).toEqual({
      status: "refused",
      refusal: "spec.approval_self_only",
    });
  });
});
