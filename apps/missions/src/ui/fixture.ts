// A small, deterministic set of missions used to render the read-only cockpit
// in tests and local development. Per the spec's runtime boundaries the cockpit
// uses contract fixtures; it cannot start a real mission or claim orchestrator
// integration.

import type { Mission } from "../domain/mission";

const TENANT = "ten_alpha000000000";
const BUDGETS = { maxDurationSeconds: 3600, maxToolCalls: 100, network: "none" } as const;

export const COCKPIT_FIXTURE: readonly Mission[] = [
  {
    id: "urn:libre-ai:mission:0001",
    tenantId: TENANT,
    revision: 1,
    state: "proposed",
    handoffId: "urn:libre-ai:handoff:0001",
    handoffDigest: "a".repeat(64),
    budgets: BUDGETS,
    acceptanceCriteria: ["criterion-a"],
    approvals: [],
    eventCursor: 1,
    createdAt: "2030-01-01T00:00:00Z",
  },
  {
    id: "urn:libre-ai:mission:0002",
    tenantId: TENANT,
    revision: 3,
    state: "approved",
    handoffId: "urn:libre-ai:handoff:0002",
    handoffDigest: "b".repeat(64),
    risk: { level: "medium", policyVersion: "1.0.0" },
    budgets: BUDGETS,
    acceptanceCriteria: ["criterion-a", "criterion-b"],
    approvals: ["urn:libre-ai:approval:1"],
    eventCursor: 3,
    createdAt: "2030-01-02T00:00:00Z",
  },
  {
    id: "urn:libre-ai:mission:0003",
    tenantId: TENANT,
    revision: 4,
    state: "running",
    handoffId: "urn:libre-ai:handoff:0003",
    handoffDigest: "c".repeat(64),
    risk: { level: "high", policyVersion: "1.0.0" },
    budgets: BUDGETS,
    acceptanceCriteria: ["criterion-a"],
    approvals: ["urn:libre-ai:approval:2"],
    eventCursor: 5,
    createdAt: "2030-01-03T00:00:00Z",
  },
  {
    id: "urn:libre-ai:mission:0004",
    tenantId: TENANT,
    revision: 7,
    state: "accepted",
    handoffId: "urn:libre-ai:handoff:0004",
    handoffDigest: "d".repeat(64),
    risk: { level: "low", policyVersion: "1.0.0" },
    budgets: BUDGETS,
    acceptanceCriteria: ["criterion-a"],
    approvals: ["urn:libre-ai:approval:3", "urn:libre-ai:approval:4"],
    eventCursor: 9,
    result: {
      artifact: "urn:libre-ai:artifact:1",
      evidence: "urn:libre-ai:evidence:1",
      submittedAt: "2030-01-04T00:00:00Z",
    },
    verdict: {
      status: "accepted",
      reasonCode: "mission.result_accepted",
      decidedAt: "2030-01-04T01:00:00Z",
    },
    createdAt: "2030-01-04T00:00:00Z",
  },
];
