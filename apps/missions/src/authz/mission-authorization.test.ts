import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Command } from "../domain/mission";
import {
  authorizeCommand,
  COMMAND_OPERATION,
  isOperationAuthorized,
  type MissionOperation,
  type MissionRole,
  ROLE_OPERATIONS,
} from "./mission-authorization";

const POLICY = join(
  import.meta.dir,
  "..",
  "..",
  "node_modules",
  "@libre-ai",
  "contracts-authority",
  "contracts",
  "authz",
  "missions-v1.datalog",
);

describe("command authorization", () => {
  test("a requester may propose but not approve or start", () => {
    expect(authorizeCommand("requester", "ProposeMission")).toEqual({
      authorized: true,
      operation: "propose",
    });
    expect(authorizeCommand("requester", "ApproveMission").authorized).toBe(false);
    expect(authorizeCommand("requester", "StartMission").authorized).toBe(false);
  });

  test("an approver may approve/refuse and accept/reject a result, an operator drives execution", () => {
    expect(authorizeCommand("approver", "ApproveMission").authorized).toBe(true);
    expect(authorizeCommand("approver", "AcceptMissionResult").authorized).toBe(true);
    expect(authorizeCommand("approver", "StartMission").authorized).toBe(false);
    expect(authorizeCommand("operator", "StartMission").authorized).toBe(true);
    expect(authorizeCommand("operator", "AnswerDecisionRequest").authorized).toBe(true);
    expect(authorizeCommand("operator", "ApproveMission").authorized).toBe(false);
  });

  test("only the orchestrator may report events or submit a result", () => {
    expect(authorizeCommand("orchestrator", "RecordOrchestratorEvent").authorized).toBe(true);
    expect(authorizeCommand("orchestrator", "SubmitMissionResult").authorized).toBe(true);
    expect(authorizeCommand("requester", "RecordOrchestratorEvent").authorized).toBe(false);
    expect(authorizeCommand("operator", "SubmitMissionResult").authorized).toBe(false);
  });

  test("an unknown role authorizes nothing (fail-closed)", () => {
    expect(isOperationAuthorized("phantom" as MissionRole, "read")).toBe(false);
  });

  test("every domain command maps to an operation", () => {
    const commandTypes: Command["type"][] = [
      "ProposeMission",
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
      "ExportMissionRecord",
    ];
    for (const type of commandTypes) {
      expect(COMMAND_OPERATION[type]).toBeDefined();
    }
    expect(Object.keys(COMMAND_OPERATION)).toHaveLength(commandTypes.length);
  });
});

describe("conformance to the locked missions-v1.datalog policy", () => {
  // Parse the locked policy's allow rules into a role -> sorted operations map.
  function policyMatrix(): Record<string, string[]> {
    const source = readFileSync(POLICY, "utf8");
    const matrix: Record<string, string[]> = {};
    for (const line of source.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed.startsWith("allow if")) continue;
      const role = trimmed.match(/role\(\$user,\s*"([a-z-]+)"\)/)?.[1];
      const opsBlock = trimmed.match(/\[([^\]]*)\]\.contains\(\$operation\)/)?.[1];
      if (role === undefined || opsBlock === undefined) continue;
      const ops = [...opsBlock.matchAll(/"([a-z-]+)"/g)].map((m) => m[1] as string).sort();
      matrix[role] = ops;
    }
    return matrix;
  }

  test("the TS role→operations matrix equals the datalog policy", () => {
    const fromPolicy = policyMatrix();
    const fromCode: Record<string, string[]> = Object.fromEntries(
      Object.entries(ROLE_OPERATIONS).map(([role, ops]) => [role, [...ops].sort()]),
    );
    expect(fromCode).toEqual(fromPolicy);
  });

  test("every mapped command operation is one the policy authorizes for some role", () => {
    const policyOperations = new Set(Object.values(policyMatrix()).flat());
    for (const operation of Object.values(COMMAND_OPERATION) as MissionOperation[]) {
      expect(policyOperations.has(operation)).toBe(true);
    }
  });
});
