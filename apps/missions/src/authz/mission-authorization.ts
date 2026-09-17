// Missions v1 application-side authorization matrix. The Biscuit token proves
// the caller's tenant and role (verified by the authz-biscuit authorizer against
// the locked contracts/authz/missions-v1.datalog); this module is the app-side
// half: which Biscuit `operation` a domain command requires, and which roles the
// policy authorizes for it. A conformance test pins this matrix to the locked
// datalog, so the two never drift. Actual token verification and revocation are
// wired at the request boundary (a later increment).

import type { Command } from "../domain/mission";

export type MissionRole = "requester" | "approver" | "operator" | "reviewer" | "orchestrator";

export type MissionOperation =
  | "propose"
  | "assess-risk"
  | "approve"
  | "refuse"
  | "start"
  | "pause"
  | "resume"
  | "cancel"
  | "report-event"
  | "answer-decision"
  | "submit-result"
  | "accept-result"
  | "reject-result"
  | "abandon"
  | "read"
  | "export";

// The Biscuit operation each command maps to. The `Record<Command["type"], …>`
// type makes this exhaustive at compile time: a new domain command cannot be
// added without a mapping here, so no command is ever left un-authorized —
// typecheck fails until it is mapped.
export const COMMAND_OPERATION: Readonly<Record<Command["type"], MissionOperation>> = {
  ProposeMission: "propose",
  AssessMissionRisk: "assess-risk",
  ApproveMission: "approve",
  RefuseMission: "refuse",
  StartMission: "start",
  PauseMission: "pause",
  ResumeMission: "resume",
  CancelMission: "cancel",
  RecordOrchestratorEvent: "report-event",
  AnswerDecisionRequest: "answer-decision",
  SubmitMissionResult: "submit-result",
  AcceptMissionResult: "accept-result",
  RejectMissionResult: "reject-result",
  AbandonMission: "abandon",
  ExportMissionRecord: "export",
};

// The role → authorized operations matrix, faithful to missions-v1.datalog.
export const ROLE_OPERATIONS: Readonly<Record<MissionRole, readonly MissionOperation[]>> = {
  requester: ["propose", "read", "export"],
  approver: [
    "assess-risk",
    "approve",
    "refuse",
    "accept-result",
    "reject-result",
    "abandon",
    "read",
    "export",
  ],
  operator: ["start", "pause", "resume", "cancel", "answer-decision", "read", "export"],
  reviewer: ["accept-result", "reject-result", "abandon", "read", "export"],
  orchestrator: ["report-event", "submit-result"],
};

export interface AuthorizationOutcome {
  readonly authorized: boolean;
  readonly operation: MissionOperation;
}

/**
 * Whether `role` is authorized for `operation` under the locked missions-v1
 * policy. Fail-closed: an unknown role authorizes nothing.
 */
export function isOperationAuthorized(role: MissionRole, operation: MissionOperation): boolean {
  return ROLE_OPERATIONS[role]?.includes(operation) ?? false;
}

/**
 * Authorize a domain command for a role: resolves the command's Biscuit
 * operation and checks the policy matrix. This is the app-side gate that runs
 * after the token's tenant/role are verified and before the domain `decide`.
 */
export function authorizeCommand(
  role: MissionRole,
  commandType: Command["type"],
): AuthorizationOutcome {
  const operation = COMMAND_OPERATION[commandType];
  return { authorized: isOperationAuthorized(role, operation), operation };
}
