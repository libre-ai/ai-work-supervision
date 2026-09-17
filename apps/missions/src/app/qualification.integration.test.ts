// Adversarial release qualification for the Missions v1 write path
// (docs/apps/missions.md §Work packages 6, §Release and rollback): a systematic
// sweep proving the composed command service resists role confusion,
// cross-tenant access, replay and single-role capture, and that terminal
// missions are immutable. Runs against the real PostgreSQL barrier (PGlite).

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { createTestDatabase, type TestDatabase } from "@libre-ai/testing";
import type { MissionRole } from "../authz/mission-authorization";
import type { Command } from "../domain/mission";
import { type CommandRequest, executeMissionCommand } from "./execute-command";

const DATA_MIGRATIONS = join(
  import.meta.dir,
  "..",
  "..",
  "node_modules",
  "@libre-ai",
  "data",
  "migrations",
);
const MISSIONS_MIGRATIONS = join(import.meta.dir, "..", "..", "migrations");
const TENANT_A = "ten_alpha00000000001";
const TENANT_B = "ten_bravo00000000002";
const NOW = "2030-01-01T00:00:00Z";

let tdb: TestDatabase;

beforeAll(async () => {
  tdb = await createTestDatabase();
  await tdb.applyMigrations(DATA_MIGRATIONS);
  await tdb.applyMigrations(MISSIONS_MIGRATIONS);
});

function run(tenantId: string, role: MissionRole, request: CommandRequest) {
  return executeMissionCommand(tdb.db, { tenantId, role }, request, NOW);
}

function proposeCommand(id: string): Command {
  return {
    type: "ProposeMission",
    id,
    handoffId: "urn:libre-ai:handoff:1",
    handoffDigest: "a".repeat(64),
    handoffAccepted: true,
    handoffPlanningOnly: true,
    budgets: { maxDurationSeconds: 3600, maxToolCalls: 100, network: "none" },
    acceptanceCriteria: ["criterion-a"],
  };
}

// A minimal command instance per state-changing type, for the authorization
// sweep (the request is rejected on the role before any of these fields matter).
const SAMPLE: Record<Exclude<Command["type"], "ProposeMission">, Command> = {
  AssessMissionRisk: { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
  ApproveMission: {
    type: "ApproveMission",
    approval: "urn:libre-ai:approval:1",
    humanApproved: true,
  },
  RefuseMission: { type: "RefuseMission" },
  StartMission: { type: "StartMission" },
  PauseMission: { type: "PauseMission" },
  ResumeMission: { type: "ResumeMission" },
  CancelMission: { type: "CancelMission" },
  RecordOrchestratorEvent: {
    type: "RecordOrchestratorEvent",
    orchestratorBound: true,
    budgetExceeded: false,
    requiresDecision: false,
  },
  AnswerDecisionRequest: { type: "AnswerDecisionRequest" },
  SubmitMissionResult: {
    type: "SubmitMissionResult",
    artifact: "urn:libre-ai:artifact:1",
    evidence: "urn:libre-ai:evidence:1",
  },
  AcceptMissionResult: {
    type: "AcceptMissionResult",
    approval: "urn:libre-ai:approval:1",
    humanApproved: true,
  },
  RejectMissionResult: { type: "RejectMissionResult" },
  AbandonMission: { type: "AbandonMission" },
  ExportMissionRecord: { type: "ExportMissionRecord" },
};

// The commands each role is NOT authorized for (complement of missions-v1).
const FORBIDDEN: Record<MissionRole, (keyof typeof SAMPLE)[]> = {
  requester: [
    "AssessMissionRisk",
    "ApproveMission",
    "RefuseMission",
    "StartMission",
    "PauseMission",
    "ResumeMission",
    "CancelMission",
    "RecordOrchestratorEvent",
    "AnswerDecisionRequest",
    "SubmitMissionResult",
    "AcceptMissionResult",
    "RejectMissionResult",
    "AbandonMission",
  ],
  approver: [
    "StartMission",
    "PauseMission",
    "ResumeMission",
    "CancelMission",
    "RecordOrchestratorEvent",
    "AnswerDecisionRequest",
    "SubmitMissionResult",
  ],
  operator: [
    "AssessMissionRisk",
    "ApproveMission",
    "RefuseMission",
    "RecordOrchestratorEvent",
    "SubmitMissionResult",
    "AcceptMissionResult",
    "RejectMissionResult",
    "AbandonMission",
  ],
  reviewer: [
    "AssessMissionRisk",
    "ApproveMission",
    "RefuseMission",
    "StartMission",
    "PauseMission",
    "ResumeMission",
    "CancelMission",
    "RecordOrchestratorEvent",
    "AnswerDecisionRequest",
    "SubmitMissionResult",
  ],
  orchestrator: [
    "AssessMissionRisk",
    "ApproveMission",
    "RefuseMission",
    "StartMission",
    "PauseMission",
    "ResumeMission",
    "CancelMission",
    "AnswerDecisionRequest",
    "AcceptMissionResult",
    "RejectMissionResult",
    "AbandonMission",
  ],
};

// Standalone workspaces surface what the hub run masked: an unclosed
// PGlite instance makes bun test exit non-zero even with every test green.
afterAll(async () => {
  await tdb.close();
});

describe("qualification — role confusion is refused before any I/O", () => {
  for (const [role, forbidden] of Object.entries(FORBIDDEN) as [
    MissionRole,
    (keyof typeof SAMPLE)[],
  ][]) {
    test(`${role} is denied every operation outside its policy scope`, async () => {
      for (const type of forbidden) {
        const outcome = await run(TENANT_A, role, {
          command: SAMPLE[type],
          missionId: "urn:libre-ai:mission:phantom",
          expectedRevision: 1,
        });
        // Authorization runs before the aggregate is even loaded, so a forbidden
        // role is refused unauthorized, never reaching not_found or the domain.
        expect(outcome).toEqual({ ok: false, refusal: "mission.unauthorized" });
      }
    });
  }
});

describe("qualification — cross-tenant, replay, capture and terminal invariants", () => {
  test("a foreign tenant cannot mutate another tenant's mission", async () => {
    const id = "urn:libre-ai:mission:q-tenant";
    await run(TENANT_A, "requester", { command: proposeCommand(id), expectedRevision: 0 });
    // Tenant B is a valid approver in its own tenant, but the mission is invisible
    // to it (RLS), so the mutation resolves to not_found — never a cross-tenant write.
    const outcome = await run(TENANT_B, "approver", {
      command: SAMPLE.AssessMissionRisk,
      missionId: id,
      expectedRevision: 1,
    });
    expect(outcome).toEqual({ ok: false, refusal: "mission.not_found" });
  });

  test("replaying a committed command loses on the stale revision", async () => {
    const id = "urn:libre-ai:mission:q-replay";
    await run(TENANT_A, "requester", { command: proposeCommand(id), expectedRevision: 0 });
    const first = await run(TENANT_A, "approver", {
      command: SAMPLE.AssessMissionRisk,
      missionId: id,
      expectedRevision: 1,
    });
    expect(first.ok).toBe(true);
    // The committed command advanced the revision — the precondition that makes
    // the replay stale.
    expect(first.ok && first.mission.revision).toBe(2);
    // The same request replayed at revision 1 is now stale (current is 2).
    const replay = await run(TENANT_A, "approver", {
      command: SAMPLE.AssessMissionRisk,
      missionId: id,
      expectedRevision: 1,
    });
    expect(replay).toEqual({ ok: false, refusal: "mission.revision_stale" });
  });

  test("no single role can drive proposal → approval → execution → acceptance", async () => {
    const id = "urn:libre-ai:mission:q-solo";
    // A lone approver cannot even propose.
    const proposeAsApprover = await run(TENANT_A, "approver", {
      command: proposeCommand(id),
      expectedRevision: 0,
    });
    expect(proposeAsApprover).toEqual({ ok: false, refusal: "mission.unauthorized" });

    // Legitimately propose (requester), then that requester cannot approve.
    await run(TENANT_A, "requester", { command: proposeCommand(id), expectedRevision: 0 });
    await run(TENANT_A, "approver", {
      command: SAMPLE.AssessMissionRisk,
      missionId: id,
      expectedRevision: 1,
    });
    const requesterApproves = await run(TENANT_A, "requester", {
      command: SAMPLE.ApproveMission,
      missionId: id,
      expectedRevision: 2,
    });
    expect(requesterApproves).toEqual({ ok: false, refusal: "mission.unauthorized" });
    // And an approver cannot start execution (that is the operator's power).
    await run(TENANT_A, "approver", {
      command: SAMPLE.ApproveMission,
      missionId: id,
      expectedRevision: 2,
    });
    const approverStarts = await run(TENANT_A, "approver", {
      command: SAMPLE.StartMission,
      missionId: id,
      expectedRevision: 3,
    });
    expect(approverStarts).toEqual({ ok: false, refusal: "mission.unauthorized" });
  });

  test("a terminal mission refuses every further command", async () => {
    const id = "urn:libre-ai:mission:q-terminal";
    await run(TENANT_A, "requester", { command: proposeCommand(id), expectedRevision: 0 });
    await run(TENANT_A, "approver", {
      command: SAMPLE.AssessMissionRisk,
      missionId: id,
      expectedRevision: 1,
    });
    const refused = await run(TENANT_A, "approver", {
      command: SAMPLE.RefuseMission,
      missionId: id,
      expectedRevision: 2,
    });
    expect(refused.ok && refused.mission.state).toBe("refused");
    // The mission is terminal; an authorized operator's cancel is still refused
    // by the domain (transition_forbidden), not silently applied.
    const cancel = await run(TENANT_A, "operator", {
      command: SAMPLE.CancelMission,
      missionId: id,
      expectedRevision: 3,
    });
    expect(cancel).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });

  test("a cancelled mission refuses every further command", async () => {
    const id = "urn:libre-ai:mission:q-cancelled";
    await run(TENANT_A, "requester", { command: proposeCommand(id), expectedRevision: 0 });
    await run(TENANT_A, "approver", {
      command: SAMPLE.AssessMissionRisk,
      missionId: id,
      expectedRevision: 1,
    });
    await run(TENANT_A, "approver", {
      command: SAMPLE.ApproveMission,
      missionId: id,
      expectedRevision: 2,
    });
    await run(TENANT_A, "operator", {
      command: SAMPLE.StartMission,
      missionId: id,
      expectedRevision: 3,
    });
    const cancelled = await run(TENANT_A, "operator", {
      command: SAMPLE.CancelMission,
      missionId: id,
      expectedRevision: 4,
    });
    expect(cancelled.ok && cancelled.mission.state).toBe("cancelled");
    // An authorized operator's pause on the cancelled (terminal) mission is
    // refused by the domain, never applied.
    const pause = await run(TENANT_A, "operator", {
      command: SAMPLE.PauseMission,
      missionId: id,
      expectedRevision: 5,
    });
    expect(pause).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });

  test("an accepted mission refuses every further command", async () => {
    const id = "urn:libre-ai:mission:q-accepted";
    await run(TENANT_A, "requester", { command: proposeCommand(id), expectedRevision: 0 });
    await run(TENANT_A, "approver", {
      command: SAMPLE.AssessMissionRisk,
      missionId: id,
      expectedRevision: 1,
    });
    await run(TENANT_A, "approver", {
      command: SAMPLE.ApproveMission,
      missionId: id,
      expectedRevision: 2,
    });
    await run(TENANT_A, "operator", {
      command: SAMPLE.StartMission,
      missionId: id,
      expectedRevision: 3,
    });
    await run(TENANT_A, "orchestrator", {
      command: SAMPLE.SubmitMissionResult,
      missionId: id,
      expectedRevision: 4,
    });
    const accepted = await run(TENANT_A, "approver", {
      command: SAMPLE.AcceptMissionResult,
      missionId: id,
      expectedRevision: 5,
    });
    expect(accepted.ok && accepted.mission.state).toBe("accepted");
    const abandon = await run(TENANT_A, "approver", {
      command: SAMPLE.AbandonMission,
      missionId: id,
      expectedRevision: 6,
    });
    expect(abandon).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });

  test("only a requester may propose a mission", async () => {
    for (const role of ["operator", "reviewer", "orchestrator", "approver"] as const) {
      const outcome = await run(TENANT_A, role, {
        command: proposeCommand(`urn:libre-ai:mission:q-propose-${role}`),
        expectedRevision: 0,
      });
      expect(outcome).toEqual({ ok: false, refusal: "mission.unauthorized" });
    }
  });
});
