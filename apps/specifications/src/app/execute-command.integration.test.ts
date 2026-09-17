import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { createTestDatabase, type TestDatabase } from "@libre-ai/testing";
import type { Command } from "../domain/workspace";
import { executeSpecCommand } from "./execute-command";

// The specifications command service exercised end-to-end against the real
// PostgreSQL barrier (PGlite): load -> decide -> persist (snapshot + append-only
// events, optimistic revision), each command in its own tenant transaction, on
// top of 0001_specifications.sql and the packages/data app role.
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

function run(workspaceId: string, command: Command) {
  return executeSpecCommand(tdb.db, TENANT_A, workspaceId, command, NOW);
}

// Standalone workspaces surface what the hub run masked: an unclosed
// PGlite instance makes bun test exit non-zero even with every test green.
afterAll(async () => {
  await tdb.close();
});

describe("spec command service — full authoring journey", () => {
  test("create → requirement → contract → acceptance → submit → two reviews", async () => {
    const wid = "urn:libre-ai:spec-workspace:journey";
    const created = await run(wid, {
      type: "CreateSpecWorkspace",
      problem: "ship the thing",
      actors: ["author-a", "reviewer-b"],
    });
    expect(created.ok).toBe(true);
    if (!created.ok) return;
    expect(created.state.revision).toBe(1);

    const steps = [
      await run(wid, { type: "AddRequirement", expectedRevision: 1, requirementId: "req-one" }),
      await run(wid, { type: "AttachContract", expectedRevision: 2, contractId: "contract-one" }),
      await run(wid, {
        type: "DefineAcceptanceCriterion",
        expectedRevision: 3,
        criterionId: "crit-one",
      }),
      await run(wid, { type: "SubmitSpecForReview", expectedRevision: 4 }),
      await run(wid, {
        type: "ReviewSpec",
        expectedRevision: 5,
        reviewerId: REVIEWER_1,
        approve: true,
      }),
      await run(wid, {
        type: "ReviewSpec",
        expectedRevision: 6,
        reviewerId: REVIEWER_2,
        approve: true,
      }),
    ];
    for (const step of steps) expect(step.ok).toBe(true);

    const last = steps[steps.length - 1];
    expect(last?.ok).toBe(true);
    if (last?.ok !== true) return;
    expect(last.state.status).toBe("submitted");
    expect(last.state.revision).toBe(7);
    expect(last.state.approverIds).toEqual([REVIEWER_1, REVIEWER_2]);
  });
});

describe("spec command service — fail-closed at every layer", () => {
  test("domain: submitting a spec with no requirement is problem_missing", async () => {
    const wid = "urn:libre-ai:spec-workspace:fc-submit";
    await run(wid, { type: "CreateSpecWorkspace", problem: "p", actors: ["author-a"] });
    const submitted = await run(wid, { type: "SubmitSpecForReview", expectedRevision: 1 });
    expect(submitted).toEqual({ ok: false, refusal: "spec.problem_missing" });
  });

  test("persistence: a command on an unknown workspace is not_found", async () => {
    const outcome = await run("urn:libre-ai:spec-workspace:ghost", {
      type: "AddRequirement",
      expectedRevision: 1,
      requirementId: "req-x",
    });
    expect(outcome).toEqual({ ok: false, refusal: "spec.not_found" });
  });

  test("concurrency: a stale-revision command loses to the committed writer", async () => {
    const wid = "urn:libre-ai:spec-workspace:fc-cc";
    await run(wid, { type: "CreateSpecWorkspace", problem: "p", actors: ["author-a"] });
    const winner = await run(wid, {
      type: "AddRequirement",
      expectedRevision: 1,
      requirementId: "req-one",
    });
    expect(winner.ok).toBe(true);
    // A second writer still holding revision 1 loses: the service reloads the
    // now-current aggregate (revision 2) and the domain optimistic check refuses
    // the stale expected revision before any write. (The persistence layer's
    // revision_conflict guards a true interleaved race; it is exercised directly
    // by the spec-workspace-store integration test.)
    const loser = await run(wid, {
      type: "AttachContract",
      expectedRevision: 1,
      contractId: "contract-x",
    });
    expect(loser).toEqual({ ok: false, refusal: "spec.revision_stale" });
  });

  test("request: a mutation of a review-frozen workspace does not apply (request_invalid)", async () => {
    const wid = "urn:libre-ai:spec-workspace:fc-frozen";
    await run(wid, { type: "CreateSpecWorkspace", problem: "p", actors: ["author-a"] });
    await run(wid, { type: "AddRequirement", expectedRevision: 1, requirementId: "req-one" });
    await run(wid, { type: "AttachContract", expectedRevision: 2, contractId: "contract-one" });
    await run(wid, {
      type: "DefineAcceptanceCriterion",
      expectedRevision: 3,
      criterionId: "crit-one",
    });
    await run(wid, { type: "SubmitSpecForReview", expectedRevision: 4 });
    // status is now "submitted": a content mutation does not apply → invalid,
    // surfaced at the boundary as request_invalid.
    const frozen = await run(wid, {
      type: "AddRequirement",
      expectedRevision: 5,
      requirementId: "req-two",
    });
    expect(frozen).toEqual({ ok: false, refusal: "spec.request_invalid" });
  });

  test("request: acceptance is not reachable here — it requires the package seam", async () => {
    const outcome = await run("urn:libre-ai:spec-workspace:fc-accept", {
      type: "AcceptSpecPackage",
      expectedRevision: 5,
    });
    expect(outcome).toEqual({ ok: false, refusal: "spec.request_invalid" });
  });
});
