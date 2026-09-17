import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { withTenantDbTransaction } from "@libre-ai/data";
import { createTestDatabase, type TestDatabase } from "@libre-ai/testing";
import type { Command } from "../domain/workspace";
import { loadAcceptedPackage } from "../persistence/spec-package-store";
import { loadWorkspace } from "../persistence/spec-workspace-store";
import { executeSpecCommand } from "./execute-command";
import { persistAcceptance } from "./persist-acceptance";

const DATA_MIGRATIONS = join(
  import.meta.dir,
  "..",
  "..",
  "node_modules",
  "@libre-ai",
  "data",
  "migrations",
);
const SPEC_MIGRATIONS = join(import.meta.dir, "..", "..", "migrations");
const TENANT_A = "ten_aaaaaaaaaaaaaaaa";
const NOW = "2030-01-01T00:00:00Z";
const REVIEWER_1 = "rev_aaaaaaaaaaaaaaaa";
const REVIEWER_2 = "rev_bbbbbbbbbbbbbbbb";

let tdb: TestDatabase;

beforeAll(async () => {
  tdb = await createTestDatabase();
  await tdb.applyMigrations(DATA_MIGRATIONS);
  await tdb.applyMigrations(SPEC_MIGRATIONS);
});

// Execute a command through the full persistence pipeline
async function executeCommand(workspaceId: string, command: Command) {
  return executeSpecCommand(tdb.db, TENANT_A, workspaceId, command, NOW);
}

// Build up a workspace to the `submitted` state via persistence
async function setupSubmittedWorkspace(workspaceId: string) {
  const commands: Command[] = [
    { type: "CreateSpecWorkspace", problem: "ship the thing", actors: ["author-a"] },
    { type: "AddRequirement", expectedRevision: 1, requirementId: "req-one" },
    { type: "AttachContract", expectedRevision: 2, contractId: "contract-one" },
    { type: "DefineAcceptanceCriterion", expectedRevision: 3, criterionId: "crit-one" },
    { type: "SubmitSpecForReview", expectedRevision: 4 },
    { type: "ReviewSpec", expectedRevision: 5, reviewerId: REVIEWER_1, approve: true },
    { type: "ReviewSpec", expectedRevision: 6, reviewerId: REVIEWER_2, approve: true },
  ];

  for (const cmd of commands) {
    const result = await executeCommand(workspaceId, cmd);
    if (!result.ok) throw new Error(`command ${cmd.type} failed: ${result.refusal}`);
  }

  // Load and return the final submitted state
  const state = await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
    loadWorkspace(tx, workspaceId),
  );
  if (!state) throw new Error(`workspace ${workspaceId} was not persisted`);
  return state;
}

function validPackage(overrides = {}) {
  return {
    schemaVersion: "libre-ai.spec-package.v1",
    id: "urn:libre-ai:spec-package:p1",
    tenantId: TENANT_A,
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

// Standalone workspaces surface what the hub run masked: an unclosed
// PGlite instance makes bun test exit non-zero even with every test green.
afterAll(async () => {
  await tdb.close();
});

describe("persist-acceptance — composition of decideAcceptance + persistence", () => {
  test("persists an accepted workspace and stores the content-addressed package", async () => {
    const workspaceId = "urn:libre-ai:workspace:accept-1";
    const submittedState = await setupSubmittedWorkspace(workspaceId);
    const pkg = validPackage();

    const result = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      // Act: persist acceptance (load, decide, persist + store package)
      const persisted = await persistAcceptance(tx, workspaceId, pkg, submittedState.revision);

      // Assert: decision was accepted
      if (persisted.status !== "accepted") return persisted;

      // Assert: workspace is now accepted
      const loadedWorkspace = await loadWorkspace(tx, workspaceId);
      expect(loadedWorkspace?.status).toBe("accepted");
      expect(loadedWorkspace?.revision).toBe(submittedState.revision + 1);

      // Assert: package is stored
      const storedPackage = await loadAcceptedPackage(tx, pkg.digest);
      expect(storedPackage?.digest).toBe(pkg.digest);
      expect(storedPackage?.workspaceId).toBe(workspaceId);

      return persisted;
    });

    expect(result.status).toBe("accepted");
  });

  test("a refused decision does not persist anything", async () => {
    const workspaceId = "urn:libre-ai:workspace:refuse-1";
    const submittedState = await setupSubmittedWorkspace(workspaceId);
    const pkg = validPackage({ digest: "1".repeat(64) });

    const result = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      // Act: try to accept with a stale revision — should be refused
      const persisted = await persistAcceptance(tx, workspaceId, pkg, submittedState.revision - 1);

      // Assert: decision was refused
      if (persisted.status !== "refused") return persisted;

      // Assert: workspace is unchanged
      const loadedWorkspace = await loadWorkspace(tx, workspaceId);
      expect(loadedWorkspace?.status).toBe("submitted");
      expect(loadedWorkspace?.revision).toBe(submittedState.revision);

      // Assert: package is NOT stored
      const storedPackage = await loadAcceptedPackage(tx, pkg.digest);
      expect(storedPackage).toBeNull();

      return persisted;
    });

    expect(result.status).toBe("refused");
  });

  test("malformed package bytes are not persisted", async () => {
    const workspaceId = "urn:libre-ai:workspace:malformed-1";
    const submittedState = await setupSubmittedWorkspace(workspaceId);

    const result = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      // Act: try to accept with malformed package bytes — should be malformed at byte gate
      // (Note: we pass {} which is malformed, and the persistAcceptance will fail-closed)
      const persisted = await persistAcceptance(tx, workspaceId, {}, submittedState.revision);

      // Assert: decision was malformed, not persisted
      expect(persisted.status).toBe("malformed");

      // Assert: workspace is unchanged
      const loadedWorkspace = await loadWorkspace(tx, workspaceId);
      expect(loadedWorkspace?.status).toBe("submitted");

      return persisted;
    });

    expect(result.status).toBe("malformed");
  });

  test("a workspace once accepted cannot be re-accepted (immutable)", async () => {
    const workspaceId = "urn:libre-ai:workspace:immutable-1";
    const submittedState = await setupSubmittedWorkspace(workspaceId);
    const pkg1 = validPackage({ digest: "a".repeat(64) });
    const pkg2 = validPackage({ digest: "b".repeat(64) });

    const result = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      // Accept once
      const firstAccept = await persistAcceptance(tx, workspaceId, pkg1, submittedState.revision);
      expect(firstAccept.status).toBe("accepted");

      // Load the now-accepted workspace
      const acceptedState = await loadWorkspace(tx, workspaceId);
      if (acceptedState === null) throw new Error("Accepted workspace is missing");
      expect(acceptedState.status).toBe("accepted");

      // Try to accept again — should be invalid (state is now "accepted", not "submitted")
      const secondAttempt = await persistAcceptance(tx, workspaceId, pkg2, acceptedState.revision);

      return secondAttempt;
    });

    // The second attempt should be invalid at the lifecycle gate
    expect(result.status).toBe("invalid");
  });

  test("accepts once then loads the stored package back by its digest", async () => {
    const workspaceId = "urn:libre-ai:workspace:idempotent-1";
    const submittedState = await setupSubmittedWorkspace(workspaceId);
    const pkg = validPackage({ digest: "d".repeat(64) });

    const result = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      // Accept once
      const firstAccept = await persistAcceptance(tx, workspaceId, pkg, submittedState.revision);
      expect(firstAccept.status).toBe("accepted");

      // Verify the package is stored
      const storedPackage = await loadAcceptedPackage(tx, pkg.digest);
      expect(storedPackage?.digest).toBe(pkg.digest);

      return firstAccept;
    });

    expect(result.status).toBe("accepted");
  });
});
