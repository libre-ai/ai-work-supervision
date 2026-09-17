import { describe, expect, test } from "bun:test";
import { type Budgets, type Command, type CommandContext, decide, type Mission } from "./mission";

const TENANT = "ten_alpha000000000";
const NOW = "2030-01-01T00:00:00Z";
const BUDGETS: Budgets = { maxDurationSeconds: 3600, maxToolCalls: 100, network: "none" };

function ctx(expectedRevision: number, tenantId = TENANT): CommandContext {
  return { tenantId, now: NOW, expectedRevision };
}

const PROPOSE: Command = {
  type: "ProposeMission",
  id: "urn:libre-ai:mission:0001",
  handoffId: "urn:libre-ai:handoff:0001",
  handoffDigest: "a".repeat(64),
  handoffAccepted: true,
  handoffPlanningOnly: true,
  budgets: BUDGETS,
  acceptanceCriteria: ["criterion-a"],
};

// Drive the aggregate forward through a list of commands, asserting each step
// succeeds, and return the resulting mission.
function drive(commands: readonly Command[]): Mission {
  let state: Mission | null = null;
  let revision = 0;
  for (const command of commands) {
    const decision = decide(state, command, ctx(revision));
    if (!decision.ok) throw new Error(`unexpected refusal ${decision.refusal} for ${command.type}`);
    state = decision.next;
    revision = state.revision;
  }
  if (state === null) throw new Error("no commands");
  return state;
}

const approvedPath: readonly Command[] = [
  PROPOSE,
  { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
  { type: "ApproveMission", approval: "urn:libre-ai:approval:1", humanApproved: true },
];

const runningPath: readonly Command[] = [...approvedPath, { type: "StartMission" }];

describe("ProposeMission (creation)", () => {
  test("creates a proposed mission at revision 1", () => {
    const decision = decide(null, PROPOSE, ctx(0));
    expect(decision.ok).toBe(true);
    if (!decision.ok) return;
    expect(decision.next.state).toBe("proposed");
    expect(decision.next.revision).toBe(1);
    expect(decision.next.tenantId).toBe(TENANT);
    expect(decision.events.map((e) => e.type)).toEqual(["MissionProposed"]);
  });

  test("refuses an unaccepted handoff", () => {
    const decision = decide(null, { ...PROPOSE, handoffAccepted: false }, ctx(0));
    expect(decision).toEqual({ ok: false, refusal: "mission.spec_unaccepted" });
  });

  test("refuses a handoff that is not planning-only", () => {
    const decision = decide(null, { ...PROPOSE, handoffPlanningOnly: false }, ctx(0));
    expect(decision).toEqual({ ok: false, refusal: "mission.handoff_not_planning_only" });
  });

  test("refuses proposing onto an existing aggregate", () => {
    const mission = drive([PROPOSE]);
    const decision = decide(mission, PROPOSE, ctx(mission.revision));
    expect(decision).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });
});

describe("optimistic revision and tenant isolation", () => {
  test("refuses a stale revision", () => {
    const mission = drive([PROPOSE]);
    const decision = decide(
      mission,
      { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
      ctx(mission.revision + 1),
    );
    expect(decision).toEqual({ ok: false, refusal: "mission.revision_stale" });
  });

  test("refuses a cross-tenant command", () => {
    const mission = drive([PROPOSE]);
    const decision = decide(
      mission,
      { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
      ctx(mission.revision, "ten_beta0000000000"),
    );
    expect(decision).toEqual({ ok: false, refusal: "mission.tenant_mismatch" });
  });
});

describe("intake lifecycle", () => {
  test("assess then approve reaches approved with the approval recorded", () => {
    const mission = drive(approvedPath);
    expect(mission.state).toBe("approved");
    expect(mission.risk?.level).toBe("low");
    expect(mission.approvals).toEqual(["urn:libre-ai:approval:1"]);
  });

  test("approve refuses without risk assessment (from proposed)", () => {
    const mission = drive([PROPOSE]);
    const decision = decide(
      mission,
      { type: "ApproveMission", approval: "urn:libre-ai:approval:1", humanApproved: true },
      ctx(mission.revision),
    );
    // Approve is only valid from assessed; from proposed it is a forbidden transition.
    expect(decision).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });

  test("approve refuses without human approval", () => {
    const mission = drive([
      PROPOSE,
      { type: "AssessMissionRisk", level: "high", policyVersion: "1.0.0" },
    ]);
    const decision = decide(
      mission,
      { type: "ApproveMission", approval: "urn:libre-ai:approval:1", humanApproved: false },
      ctx(mission.revision),
    );
    expect(decision).toEqual({ ok: false, refusal: "mission.approval_required" });
  });

  test("refuse ends the mission from assessed", () => {
    const mission = drive([
      PROPOSE,
      { type: "AssessMissionRisk", level: "critical", policyVersion: "1.0.0" },
      { type: "RefuseMission" },
    ]);
    expect(mission.state).toBe("refused");
  });
});

describe("execution lifecycle", () => {
  test("start, pause, resume", () => {
    const paused = drive([...runningPath, { type: "PauseMission" }]);
    expect(paused.state).toBe("paused");
    const resumed = decide(paused, { type: "ResumeMission" }, ctx(paused.revision));
    expect(resumed.ok && resumed.next.state).toBe("running");
  });

  test("a bound orchestrator progress event advances the cursor without a transition", () => {
    const running = drive(runningPath);
    const decision = decide(
      running,
      {
        type: "RecordOrchestratorEvent",
        orchestratorBound: true,
        budgetExceeded: false,
        requiresDecision: false,
      },
      ctx(running.revision),
    );
    expect(decision.ok).toBe(true);
    if (!decision.ok) return;
    expect(decision.next.state).toBe("running");
    expect(decision.next.eventCursor).toBe(running.eventCursor + 1);
    expect(decision.events).toEqual([]);
  });

  test("an unbound orchestrator event is untrusted", () => {
    const running = drive(runningPath);
    const decision = decide(
      running,
      {
        type: "RecordOrchestratorEvent",
        orchestratorBound: false,
        budgetExceeded: false,
        requiresDecision: false,
      },
      ctx(running.revision),
    );
    expect(decision).toEqual({ ok: false, refusal: "mission.event_untrusted" });
  });

  test("a budget-exceeded event blocks the mission", () => {
    const running = drive(runningPath);
    const decision = decide(
      running,
      {
        type: "RecordOrchestratorEvent",
        orchestratorBound: true,
        budgetExceeded: true,
        requiresDecision: false,
      },
      ctx(running.revision),
    );
    expect(decision.ok).toBe(true);
    if (!decision.ok) return;
    expect(decision.next.state).toBe("blocked");
    expect(decision.events.map((e) => e.type)).toEqual(["MissionBlocked"]);
  });

  test("a decision request blocks then an answer resumes running", () => {
    const running = drive(runningPath);
    const blocked = decide(
      running,
      {
        type: "RecordOrchestratorEvent",
        orchestratorBound: true,
        budgetExceeded: false,
        requiresDecision: true,
      },
      ctx(running.revision),
    );
    expect(blocked.ok && blocked.next.state).toBe("blocked");
    expect(blocked.ok && blocked.events.map((e) => e.type)).toEqual(["HumanDecisionRequested"]);
    if (!blocked.ok) return;
    const answered = decide(
      blocked.next,
      { type: "AnswerDecisionRequest" },
      ctx(blocked.next.revision),
    );
    expect(answered.ok && answered.next.state).toBe("running");
  });

  test("cancel from running is terminal", () => {
    const mission = drive([...runningPath, { type: "CancelMission" }]);
    expect(mission.state).toBe("cancelled");
  });
});

describe("result lifecycle", () => {
  const submittedPath: readonly Command[] = [
    ...runningPath,
    {
      type: "SubmitMissionResult",
      artifact: "urn:libre-ai:artifact:1",
      evidence: "urn:libre-ai:evidence:1",
    },
  ];

  test("submit requires artifact and evidence", () => {
    const running = drive(runningPath);
    const decision = decide(
      running,
      { type: "SubmitMissionResult", artifact: "", evidence: "urn:libre-ai:evidence:1" },
      ctx(running.revision),
    );
    expect(decision).toEqual({ ok: false, refusal: "mission.evidence_missing" });
  });

  test("submit then accept reaches accepted", () => {
    const submitted = drive(submittedPath);
    expect(submitted.state).toBe("result-submitted");
    const accepted = decide(
      submitted,
      { type: "AcceptMissionResult", approval: "urn:libre-ai:approval:2", humanApproved: true },
      ctx(submitted.revision),
    );
    expect(accepted.ok && accepted.next.state).toBe("accepted");
  });

  test("accept refuses without human approval", () => {
    const submitted = drive(submittedPath);
    const decision = decide(
      submitted,
      { type: "AcceptMissionResult", approval: "urn:libre-ai:approval:2", humanApproved: false },
      ctx(submitted.revision),
    );
    expect(decision).toEqual({ ok: false, refusal: "mission.approval_required" });
  });

  test("reject then remediate (resubmit) then accept", () => {
    const submitted = drive(submittedPath);
    const rejected = decide(submitted, { type: "RejectMissionResult" }, ctx(submitted.revision));
    expect(rejected.ok && rejected.next.state).toBe("rejected");
    if (!rejected.ok) return;
    const resubmitted = decide(
      rejected.next,
      {
        type: "SubmitMissionResult",
        artifact: "urn:libre-ai:artifact:2",
        evidence: "urn:libre-ai:evidence:2",
      },
      ctx(rejected.next.revision),
    );
    expect(resubmitted.ok && resubmitted.next.state).toBe("result-submitted");
  });
});

describe("terminal and audit invariants", () => {
  test("no command advances a terminal mission", () => {
    const accepted = drive([
      ...runningPath,
      {
        type: "SubmitMissionResult",
        artifact: "urn:libre-ai:artifact:1",
        evidence: "urn:libre-ai:evidence:1",
      },
      { type: "AcceptMissionResult", approval: "urn:libre-ai:approval:2", humanApproved: true },
    ]);
    const decision = decide(accepted, { type: "CancelMission" }, ctx(accepted.revision));
    expect(decision).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });

  test("export is a no-op that emits nothing and keeps the state", () => {
    const running = drive(runningPath);
    const decision = decide(running, { type: "ExportMissionRecord" }, ctx(running.revision));
    expect(decision.ok).toBe(true);
    if (!decision.ok) return;
    expect(decision.next).toBe(running);
    expect(decision.events).toEqual([]);
  });

  test("start is forbidden from proposed (skipping assessment/approval)", () => {
    const mission = drive([PROPOSE]);
    const decision = decide(mission, { type: "StartMission" }, ctx(mission.revision));
    expect(decision).toEqual({ ok: false, refusal: "mission.transition_forbidden" });
  });
});

describe("verdict (mission-record.v1 conformance)", () => {
  const submittedPath: readonly Command[] = [
    ...runningPath,
    {
      type: "SubmitMissionResult",
      artifact: "urn:libre-ai:artifact:1",
      evidence: "urn:libre-ai:evidence:1",
    },
  ];

  test("accepted carries an accepted verdict", () => {
    const accepted = drive([
      ...submittedPath,
      { type: "AcceptMissionResult", approval: "urn:libre-ai:approval:2", humanApproved: true },
    ]);
    expect(accepted.state).toBe("accepted");
    expect(accepted.verdict?.status).toBe("accepted");
    expect(accepted.verdict?.reasonCode).toMatch(/^mission\./);
    expect(accepted.verdict?.decidedAt).toBe(NOW);
  });

  test("rejected carries a rejected verdict, cleared on remediation", () => {
    const rejected = drive([...submittedPath, { type: "RejectMissionResult" }]);
    expect(rejected.verdict?.status).toBe("rejected");
    const resubmitted = decide(
      rejected,
      {
        type: "SubmitMissionResult",
        artifact: "urn:libre-ai:artifact:2",
        evidence: "urn:libre-ai:evidence:2",
      },
      ctx(rejected.revision),
    );
    expect(resubmitted.ok && resubmitted.next.verdict).toBeUndefined();
  });

  test("abandoned carries an abandoned verdict", () => {
    const abandoned = drive([...runningPath, { type: "AbandonMission" }]);
    expect(abandoned.state).toBe("abandoned");
    expect(abandoned.verdict?.status).toBe("abandoned");
  });

  test("refused and cancelled carry no verdict", () => {
    const refused = drive([
      PROPOSE,
      { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" },
      { type: "RefuseMission" },
    ]);
    expect(refused.verdict).toBeUndefined();
    const cancelled = drive([...runningPath, { type: "CancelMission" }]);
    expect(cancelled.verdict).toBeUndefined();
  });
});
