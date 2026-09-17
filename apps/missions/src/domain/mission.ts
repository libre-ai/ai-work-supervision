// Missions v1 domain — the mission aggregate and its fail-closed, revisioned
// state machine (docs/apps/missions.md, contracts/openapi/missions.v1.yaml,
// contracts/schemas/mission-record.v1.schema.json). This is the human-approver
// baseline; the v2 two-agent-reviewer protocol is a locked, UNIMPLEMENTED
// contract and is deliberately absent here.
//
// The domain is pure: `decide` takes the current aggregate (or null for
// creation), a command and an issuer-resolved context, and returns either the
// emitted events + the next aggregate, or a typed refusal. It never performs
// I/O; persistence, RLS, authorization and orchestration live in later layers.

export type MissionState =
  | "proposed"
  | "assessed"
  | "approved"
  | "refused"
  | "running"
  | "blocked"
  | "paused"
  | "cancelled"
  | "result-submitted"
  | "accepted"
  | "rejected"
  | "abandoned";

const TERMINAL: ReadonlySet<MissionState> = new Set<MissionState>([
  "refused",
  "cancelled",
  "accepted",
  "abandoned",
]);

export type RiskLevel = "low" | "medium" | "high" | "critical";

export type RefusalCode =
  | "mission.spec_unaccepted"
  | "mission.handoff_not_planning_only"
  | "mission.risk_unassessed"
  | "mission.approval_required"
  | "mission.transition_forbidden"
  | "mission.revision_stale"
  | "mission.event_untrusted"
  | "mission.budget_exceeded"
  | "mission.evidence_missing"
  | "mission.tenant_mismatch";

export interface Budgets {
  readonly maxDurationSeconds: number;
  readonly maxToolCalls: number;
  readonly network: "none" | "allowlisted";
}

export interface ResultRef {
  readonly artifact: string;
  readonly evidence: string;
  readonly submittedAt: string;
}

// The final decision on a mission. mission-record.v1 requires it exactly when
// the state is `accepted`, `rejected` or `abandoned` (the verdict-status enum);
// `refused` and `cancelled` carry no verdict.
export interface Verdict {
  readonly status: "accepted" | "rejected" | "abandoned";
  readonly reasonCode: string;
  readonly decidedAt: string;
}

export interface Mission {
  readonly id: string;
  readonly tenantId: string;
  readonly revision: number;
  readonly state: MissionState;
  readonly handoffId: string;
  readonly handoffDigest: string;
  readonly risk?: { readonly level: RiskLevel; readonly policyVersion: string };
  readonly budgets: Budgets;
  readonly acceptanceCriteria: readonly string[];
  readonly approvals: readonly string[];
  readonly eventCursor: number;
  readonly result?: ResultRef;
  readonly verdict?: Verdict;
  readonly createdAt: string;
}

export type MissionEventType =
  | "MissionProposed"
  | "MissionRiskAssessed"
  | "MissionApproved"
  | "MissionRefused"
  | "MissionStarted"
  | "MissionBlocked"
  | "HumanDecisionRequested"
  | "MissionPaused"
  | "MissionCancelled"
  | "MissionResultSubmitted"
  | "MissionResultAccepted"
  | "MissionResultRejected"
  | "MissionAbandoned";

export interface MissionEvent {
  readonly type: MissionEventType;
  readonly missionId: string;
  readonly tenantId: string;
  readonly sequence: number;
}

// Issuer-resolved facts. The app layer resolves the referenced handoff, the
// human approval and the orchestrator binding before calling `decide`; the
// domain never trusts the raw request for these.
export interface CommandContext {
  readonly tenantId: string;
  readonly now: string;
  readonly expectedRevision: number;
}

export type Command =
  | {
      readonly type: "ProposeMission";
      readonly id: string;
      readonly handoffId: string;
      readonly handoffDigest: string;
      readonly handoffAccepted: boolean;
      readonly handoffPlanningOnly: boolean;
      readonly budgets: Budgets;
      readonly acceptanceCriteria: readonly string[];
    }
  | {
      readonly type: "AssessMissionRisk";
      readonly level: RiskLevel;
      readonly policyVersion: string;
    }
  | { readonly type: "ApproveMission"; readonly approval: string; readonly humanApproved: boolean }
  | { readonly type: "RefuseMission" }
  | { readonly type: "StartMission" }
  | { readonly type: "PauseMission" }
  | { readonly type: "ResumeMission" }
  | { readonly type: "CancelMission" }
  | {
      readonly type: "RecordOrchestratorEvent";
      readonly orchestratorBound: boolean;
      readonly budgetExceeded: boolean;
      readonly requiresDecision: boolean;
    }
  | { readonly type: "AnswerDecisionRequest" }
  | {
      readonly type: "SubmitMissionResult";
      readonly artifact: string;
      readonly evidence: string;
    }
  | {
      readonly type: "AcceptMissionResult";
      readonly approval: string;
      readonly humanApproved: boolean;
    }
  | { readonly type: "RejectMissionResult" }
  | { readonly type: "AbandonMission" }
  | { readonly type: "ExportMissionRecord" };

export type Decision =
  | { readonly ok: true; readonly events: readonly MissionEvent[]; readonly next: Mission }
  | { readonly ok: false; readonly refusal: RefusalCode };

function refuse(refusal: RefusalCode): Decision {
  return { ok: false, refusal };
}

function event(mission: Mission, type: MissionEventType, sequence: number): MissionEvent {
  return { type, missionId: mission.id, tenantId: mission.tenantId, sequence };
}

// Commands that only read/audit: allowed in any non-terminal state, no
// transition, no event. Export is the sole v1 example.
function isPureAudit(command: Command): boolean {
  return command.type === "ExportMissionRecord";
}

/**
 * Apply one command to the mission aggregate. `state` is null only for
 * `ProposeMission` (creation). Fail-closed: an unmodelled transition, a stale
 * revision or a tenant mismatch always refuses; the aggregate is never mutated
 * in place (a new frozen value is returned).
 */
export function decide(state: Mission | null, command: Command, ctx: CommandContext): Decision {
  if (command.type === "ProposeMission") {
    if (state !== null) return refuse("mission.transition_forbidden");
    if (!command.handoffAccepted) return refuse("mission.spec_unaccepted");
    if (!command.handoffPlanningOnly) return refuse("mission.handoff_not_planning_only");
    const mission: Mission = Object.freeze({
      id: command.id,
      tenantId: ctx.tenantId,
      revision: 1,
      state: "proposed",
      handoffId: command.handoffId,
      handoffDigest: command.handoffDigest,
      budgets: command.budgets,
      acceptanceCriteria: Object.freeze([...command.acceptanceCriteria]),
      approvals: Object.freeze([] as string[]),
      eventCursor: 1,
      createdAt: ctx.now,
    });
    return { ok: true, events: [event(mission, "MissionProposed", 1)], next: mission };
  }

  // Every non-creation command targets an existing aggregate under an optimistic
  // revision and the same tenant.
  if (state === null) return refuse("mission.transition_forbidden");
  if (state.tenantId !== ctx.tenantId) return refuse("mission.tenant_mismatch");
  if (state.revision !== ctx.expectedRevision) return refuse("mission.revision_stale");

  if (isPureAudit(command)) {
    return { ok: true, events: [], next: state };
  }
  if (TERMINAL.has(state.state)) return refuse("mission.transition_forbidden");

  const advance = (
    nextState: MissionState,
    type: MissionEventType,
    patch: Partial<Mission> = {},
  ): Decision => {
    const sequence = state.eventCursor + 1;
    const next: Mission = Object.freeze({
      ...state,
      ...patch,
      state: nextState,
      revision: state.revision + 1,
      eventCursor: sequence,
    });
    return { ok: true, events: [event(next, type, sequence)], next };
  };

  switch (command.type) {
    case "AssessMissionRisk":
      if (state.state !== "proposed") return refuse("mission.transition_forbidden");
      return advance("assessed", "MissionRiskAssessed", {
        risk: { level: command.level, policyVersion: command.policyVersion },
      });

    case "ApproveMission":
      if (state.state !== "assessed") return refuse("mission.transition_forbidden");
      if (state.risk === undefined) return refuse("mission.risk_unassessed");
      if (!command.humanApproved) return refuse("mission.approval_required");
      return advance("approved", "MissionApproved", {
        approvals: Object.freeze([...state.approvals, command.approval]),
      });

    case "RefuseMission":
      if (state.state !== "assessed") return refuse("mission.transition_forbidden");
      return advance("refused", "MissionRefused");

    // Start and Resume both transition to `running` and emit `MissionStarted`:
    // v1 has no distinct `MissionResumed` in the locked event set, and the prior
    // event sequence (a preceding MissionPaused) already distinguishes a resume
    // from a first start for any projection that needs it.
    case "StartMission":
      if (state.state !== "approved" && state.state !== "paused")
        return refuse("mission.transition_forbidden");
      return advance("running", "MissionStarted");

    case "PauseMission":
      if (state.state !== "running") return refuse("mission.transition_forbidden");
      return advance("paused", "MissionPaused");

    case "ResumeMission":
      if (state.state !== "paused") return refuse("mission.transition_forbidden");
      return advance("running", "MissionStarted");

    case "CancelMission":
      if (state.state !== "running" && state.state !== "paused" && state.state !== "blocked")
        return refuse("mission.transition_forbidden");
      return advance("cancelled", "MissionCancelled");

    case "RecordOrchestratorEvent":
      if (state.state !== "running") return refuse("mission.transition_forbidden");
      // Only a bound orchestrator instance may report execution events.
      if (!command.orchestratorBound) return refuse("mission.event_untrusted");
      // A budget stop and a human-decision request both block; the two are
      // distinguished by the emitted event (MissionBlocked vs
      // HumanDecisionRequested), not by aggregate state — the open decision is
      // owned by the decision-requests projection, not the mission record.
      if (command.budgetExceeded) return advance("blocked", "MissionBlocked");
      if (command.requiresDecision) return advance("blocked", "HumanDecisionRequested");
      // A plain progress event advances the cursor without changing state and
      // without emitting a mission-domain event: the mission event log records
      // state transitions, while raw orchestrator progress lives in the
      // orchestrator's own causal stream (Missions is not its authority). The
      // cursor keeps the mission's position in that stream.
      {
        const sequence = state.eventCursor + 1;
        const next: Mission = Object.freeze({ ...state, eventCursor: sequence });
        return { ok: true, events: [], next };
      }

    case "AnswerDecisionRequest":
      if (state.state !== "blocked") return refuse("mission.transition_forbidden");
      return advance("running", "MissionStarted");

    case "SubmitMissionResult":
      if (state.state !== "running" && state.state !== "rejected")
        return refuse("mission.transition_forbidden");
      if (command.artifact.length === 0 || command.evidence.length === 0)
        return refuse("mission.evidence_missing");
      // Remediating a rejected result clears the prior rejection verdict; the
      // resubmitted, non-terminal mission carries none.
      return advance("result-submitted", "MissionResultSubmitted", {
        result: { artifact: command.artifact, evidence: command.evidence, submittedAt: ctx.now },
        verdict: undefined,
      });

    case "AcceptMissionResult":
      if (state.state !== "result-submitted") return refuse("mission.transition_forbidden");
      if (state.result === undefined) return refuse("mission.evidence_missing");
      if (!command.humanApproved) return refuse("mission.approval_required");
      return advance("accepted", "MissionResultAccepted", {
        approvals: Object.freeze([...state.approvals, command.approval]),
        verdict: { status: "accepted", reasonCode: "mission.result_accepted", decidedAt: ctx.now },
      });

    case "RejectMissionResult":
      if (state.state !== "result-submitted") return refuse("mission.transition_forbidden");
      return advance("rejected", "MissionResultRejected", {
        verdict: { status: "rejected", reasonCode: "mission.result_rejected", decidedAt: ctx.now },
      });

    case "AbandonMission":
      return advance("abandoned", "MissionAbandoned", {
        verdict: { status: "abandoned", reasonCode: "mission.abandoned", decidedAt: ctx.now },
      });

    default:
      return refuse("mission.transition_forbidden");
  }
}
