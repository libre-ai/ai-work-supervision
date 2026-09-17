import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { withTenantDbTransaction } from "@libre-ai/data";
import { createTestDatabase, type TestDatabase } from "@libre-ai/testing";
import type { MissionRole } from "../authz/mission-authorization";
import type { Command } from "../domain/mission";
import { loadMission } from "../persistence/mission-store";
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
const TENANT = "ten_alpha00000000001";
const NOW = "2030-01-01T00:00:00Z";

let tdb: TestDatabase;

beforeAll(async () => {
  tdb = await createTestDatabase();
  await tdb.applyMigrations(DATA_MIGRATIONS);
  await tdb.applyMigrations(MISSIONS_MIGRATIONS);
});

function run(role: MissionRole, request: CommandRequest) {
  return executeMissionCommand(tdb.db, { tenantId: TENANT, role }, request, NOW);
}

const PROPOSE: Command = {
  type: "ProposeMission",
  id: "urn:libre-ai:mission:j1",
  handoffId: "urn:libre-ai:handoff:1",
  handoffDigest: "a".repeat(64),
  handoffAccepted: true,
  handoffPlanningOnly: true,
  budgets: { maxDurationSeconds: 3600, maxToolCalls: 100, network: "none" },
  acceptanceCriteria: ["criterion-a"],
};

// Standalone workspaces surface what the hub run masked: an unclosed
// PGlite instance makes bun test exit non-zero even with every test green.
afterAll(async () => {
  await tdb.close();
});

describe("mission command service — full authorized journey", () => {
  test("propose → assess → approve → start → submit → accept across roles", async () => {
    const propose = await run("requester", { command: PROPOSE, expectedRevision: 0 });
    expect(propose.ok && propose.mission.state).toBe("proposed");

    const id = "urn:libre-ai:mission:j1";
    const assess = await run("approver", {
      command: { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
      missionId: id,
      expectedRevision: 1,
    });
    expect(assess.ok && assess.mission.state).toBe("assessed");

    const approve = await run("approver", {
      command: { type: "ApproveMission", approval: "urn:libre-ai:approval:1", humanApproved: true },
      missionId: id,
      expectedRevision: 2,
    });
    expect(approve.ok && approve.mission.state).toBe("approved");

    const start = await run("operator", {
      command: { type: "StartMission" },
      missionId: id,
      expectedRevision: 3,
    });
    expect(start.ok && start.mission.state).toBe("running");

    const submit = await run("orchestrator", {
      command: {
        type: "SubmitMissionResult",
        artifact: "urn:libre-ai:artifact:1",
        evidence: "urn:libre-ai:evidence:1",
      },
      missionId: id,
      expectedRevision: 4,
    });
    expect(submit.ok && submit.mission.state).toBe("result-submitted");

    const accept = await run("approver", {
      command: {
        type: "AcceptMissionResult",
        approval: "urn:libre-ai:approval:2",
        humanApproved: true,
      },
      missionId: id,
      expectedRevision: 5,
    });
    expect(accept.ok && accept.mission.state).toBe("accepted");

    // The persisted aggregate reflects the final validated state with a verdict.
    const persisted = await withTenantDbTransaction(tdb.db, TENANT, (tx) => loadMission(tx, id));
    expect(persisted?.state).toBe("accepted");
    expect(persisted?.verdict?.status).toBe("accepted");
    expect(persisted?.revision).toBe(6);
  });
});

describe("mission command service — fail-closed at every layer", () => {
  test("authorization: a requester may not approve", async () => {
    await run("requester", { command: PROPOSE, expectedRevision: 0 }).catch(() => undefined);
    const outcome = await run("requester", {
      command: { type: "ApproveMission", approval: "urn:libre-ai:approval:1", humanApproved: true },
      missionId: "urn:libre-ai:mission:j1",
      expectedRevision: 1,
    });
    expect(outcome).toEqual({ ok: false, refusal: "mission.unauthorized" });
  });

  test("domain: an operator cannot start a mission still proposed", async () => {
    const propose = await run("requester", {
      command: { ...PROPOSE, id: "urn:libre-ai:mission:j2" },
      expectedRevision: 0,
    });
    expect(propose.ok).toBe(true);
    const outcome = await run("operator", {
      command: { type: "StartMission" },
      missionId: "urn:libre-ai:mission:j2",
      expectedRevision: 1,
    });
    expect(outcome).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });

  test("persistence: an unknown mission is not found", async () => {
    const outcome = await run("approver", {
      command: { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
      missionId: "urn:libre-ai:mission:absent",
      expectedRevision: 1,
    });
    expect(outcome).toEqual({ ok: false, refusal: "mission.not_found" });
  });

  test("concurrency: a stale-revision command loses to the committed writer", async () => {
    const id = "urn:libre-ai:mission:j3";
    await run("requester", { command: { ...PROPOSE, id }, expectedRevision: 0 });
    const winner = await run("approver", {
      command: { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
      missionId: id,
      expectedRevision: 1,
    });
    expect(winner.ok).toBe(true);
    // A second writer still holding revision 1 loses: the service reloads the
    // now-current aggregate (revision 2) and the domain's optimistic check
    // refuses the stale expected revision before any write. (The persistence
    // layer's revision_conflict guards a true interleaved race; it is exercised
    // directly by the mission-store integration test.)
    const loser = await run("approver", {
      command: { type: "AssessMissionRisk", level: "high", policyVersion: "1.0.0" },
      missionId: id,
      expectedRevision: 1,
    });
    expect(loser).toEqual({ ok: false, refusal: "mission.revision_stale" });
  });

  test("request: a non-creation command without a mission id is invalid", async () => {
    const outcome = await run("operator", {
      command: { type: "StartMission" },
      expectedRevision: 1,
    });
    expect(outcome).toEqual({ ok: false, refusal: "mission.request_invalid" });
  });
});
