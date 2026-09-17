// A small, deterministic set of spec-workspace views used to render the read-only
// cockpit in tests and local development. Per the spec's runtime boundaries the
// cockpit uses contract fixtures; it cannot author a real specification or claim
// persistence/orchestrator integration. WorkspaceState is identity-free, so each
// state is paired with its workspace id (as the persistence layer would return).

import type { SpecWorkspaceView } from "./specifications-cockpit";

export const COCKPIT_FIXTURE: readonly SpecWorkspaceView[] = [
  {
    id: "urn:libre-ai:spec-workspace:0001",
    state: {
      status: "draft",
      revision: 2,
      problem: "réduire le délai de première réponse",
      actorCount: 2,
      requirementIds: ["req-a"],
      contractIds: [],
      criterionIds: [],
      openDecisionIds: ["dec-a"],
      resolvedDecisionIds: [],
      approverIds: [],
    },
  },
  {
    id: "urn:libre-ai:spec-workspace:0002",
    state: {
      status: "submitted",
      revision: 5,
      problem: "unifier les exports de conformité",
      actorCount: 3,
      requirementIds: ["req-a", "req-b"],
      contractIds: ["contract-a"],
      criterionIds: ["crit-a"],
      openDecisionIds: [],
      resolvedDecisionIds: ["dec-a"],
      approverIds: [],
    },
  },
  {
    id: "urn:libre-ai:spec-workspace:0003",
    state: {
      status: "accepted",
      revision: 8,
      problem: "tracer la provenance des synthèses",
      actorCount: 3,
      requirementIds: ["req-a", "req-b", "req-c"],
      contractIds: ["contract-a", "contract-b"],
      criterionIds: ["crit-a", "crit-b"],
      openDecisionIds: [],
      resolvedDecisionIds: ["dec-a", "dec-b"],
      approverIds: ["rev_aaaaaaaaaaaaaaaa", "rev_bbbbbbbbbbbbbbbb"],
    },
  },
  {
    id: "urn:libre-ai:spec-workspace:0004",
    state: {
      status: "superseded",
      revision: 9,
      problem: "ancienne politique d'audience",
      actorCount: 2,
      requirementIds: ["req-a"],
      contractIds: ["contract-a"],
      criterionIds: ["crit-a"],
      openDecisionIds: [],
      resolvedDecisionIds: ["dec-a"],
      approverIds: ["rev_aaaaaaaaaaaaaaaa", "rev_cccccccccccccccc"],
    },
  },
];
