import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { withTenantDbTransaction } from "@libre-ai/data";
import { createTestDatabase, type TestDatabase } from "@libre-ai/testing";
import { type Command, decide } from "../domain/workspace";
import {
  loadWorkspace,
  loadWorkspaceEvents,
  SpecWorkspaceRevisionConflictError,
  saveWorkspace,
} from "./spec-workspace-store";

// The spec-workspace persistence exercised against the real PostgreSQL barrier
// (PGlite): the tables, FORCE RLS policies and least-privilege grants from
// 0001_specifications.sql, on top of the libre_ai_app role from packages/data.
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
const TENANT_B = "ten_bbbbbbbbbbbbbbbb";
const NOW = "2030-01-01T00:00:00Z";

let tdb: TestDatabase;

beforeAll(async () => {
  tdb = await createTestDatabase();
  await tdb.applyMigrations(DATA_MIGRATIONS);
  await tdb.applyMigrations(SPEC_MIGRATIONS);
});

function createCmd(): Command {
  return {
    type: "CreateSpecWorkspace",
    problem: "ship the thing",
    actors: ["author-a", "reviewer-b"],
  };
}
function addRequirementCmd(expectedRevision: number): Command {
  return { type: "AddRequirement", expectedRevision, requirementId: "req-one" };
}
function attachContractCmd(expectedRevision: number): Command {
  return { type: "AttachContract", expectedRevision, contractId: "contract-x" };
}

// Run raw SQL under the app role and (optionally) a tenant GUC, rolled back so
// tests do not leak rows. Mirrors the packages/data barrier test.
async function asRawTenant<T>(tenant: string | null, fn: () => Promise<T>): Promise<T> {
  await tdb.db.exec("BEGIN");
  try {
    await tdb.db.exec("SET LOCAL ROLE libre_ai_app");
    if (tenant !== null) {
      await tdb.db.query("SELECT set_config('app.tenant_id', $1, true)", [tenant]);
    }
    return await fn();
  } finally {
    await tdb.db.exec("ROLLBACK");
  }
}

// Standalone workspaces surface what the hub run masked: an unclosed
// PGlite instance makes bun test exit non-zero even with every test green.
afterAll(async () => {
  await tdb.close();
});

describe("spec-workspace store round-trip and tenant isolation", () => {
  test("saves a draft workspace and loads it back within the tenant", async () => {
    const wid = "urn:libre-ai:spec-workspace:rt1";
    const created = decide(null, createCmd());
    expect(created.status).toBe("accepted");
    if (created.status !== "accepted") return;

    const loaded = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      await saveWorkspace(tx, wid, created.state, created.events, NOW);
      return loadWorkspace(tx, wid);
    });
    expect(loaded?.status).toBe("draft");
    expect(loaded?.revision).toBe(1);
    expect(loaded?.problem).toBe("ship the thing");
    expect(loaded?.actorCount).toBe(2);
  });

  test("advances the snapshot under optimistic concurrency by revision", async () => {
    const wid = "urn:libre-ai:spec-workspace:adv1";
    const created = decide(null, createCmd());
    if (created.status !== "accepted") throw new Error("create refused");
    const next = decide(created.state, addRequirementCmd(1));
    if (next.status !== "accepted") throw new Error("add refused");

    const loaded = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      await saveWorkspace(tx, wid, created.state, created.events, NOW);
      await saveWorkspace(tx, wid, next.state, next.events, NOW);
      return loadWorkspace(tx, wid);
    });
    expect(loaded?.revision).toBe(2);
    expect(loaded?.requirementIds).toEqual(["req-one"]);
  });

  test("a different tenant cannot read another tenant's workspace", async () => {
    const wid = "urn:libre-ai:spec-workspace:iso1";
    const created = decide(null, createCmd());
    if (created.status !== "accepted") throw new Error("create refused");
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveWorkspace(tx, wid, created.state, created.events, NOW),
    );

    const crossTenant = await withTenantDbTransaction(tdb.db, TENANT_B, (tx) =>
      loadWorkspace(tx, wid),
    );
    expect(crossTenant).toBeNull();
  });

  test("a different tenant's raw UPDATE matches no row (RLS USING)", async () => {
    const wid = "urn:libre-ai:spec-workspace:iso2";
    const created = decide(null, createCmd());
    if (created.status !== "accepted") throw new Error("create refused");
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveWorkspace(tx, wid, created.state, created.events, NOW),
    );

    // Tenant B, in raw SQL under the app role, cannot mutate tenant A's row: the
    // RLS USING clause makes it invisible, so the UPDATE affects 0 rows.
    const updated = await asRawTenant(TENANT_B, async () => {
      const u = await tdb.db.query(
        "UPDATE spec_workspaces SET status = 'superseded' WHERE id = $1",
        [wid],
      );
      return u.affectedRows ?? 0;
    });
    expect(updated).toBe(0);

    const stillDraft = await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      loadWorkspace(tx, wid),
    );
    expect(stillDraft?.status).toBe("draft");
  });
});

describe("optimistic revision concurrency", () => {
  test("a stale-revision write updates no row and is rejected", async () => {
    const wid = "urn:libre-ai:spec-workspace:cc1";
    const created = decide(null, createCmd());
    if (created.status !== "accepted") throw new Error("create refused");
    // Two commands both computed from the revision-1 draft: each advances to
    // revision 2 and guards on revision 1.
    const winner = decide(created.state, addRequirementCmd(1));
    const loser = decide(created.state, attachContractCmd(1));
    if (winner.status !== "accepted" || loser.status !== "accepted") throw new Error("refused");

    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveWorkspace(tx, wid, created.state, created.events, NOW),
    );
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveWorkspace(tx, wid, winner.state, winner.events, NOW),
    );
    // The loser holds the same stale revision-1 aggregate: its guarded update
    // matches no row (current revision is already 2) and is rejected before any
    // event is appended.
    await expect(
      withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
        saveWorkspace(tx, wid, loser.state, loser.events, NOW),
      ),
    ).rejects.toBeInstanceOf(SpecWorkspaceRevisionConflictError);
  });
});

describe("append-only event log and fail-closed barrier", () => {
  test("the append-only log records each event in causal order", async () => {
    const wid = "urn:libre-ai:spec-workspace:log1";
    const created = decide(null, createCmd());
    if (created.status !== "accepted") throw new Error("create refused");
    const next = decide(created.state, addRequirementCmd(1));
    if (next.status !== "accepted") throw new Error("add refused");

    const events = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      await saveWorkspace(tx, wid, created.state, created.events, NOW);
      await saveWorkspace(tx, wid, next.state, next.events, NOW);
      return loadWorkspaceEvents(tx, wid);
    });
    expect(events.map((e) => [e.sequence, e.type])).toEqual([
      [1, "SpecWorkspaceCreated"],
      [2, "RequirementAdded"],
    ]);
  });

  test("the app role may not update or delete recorded events", async () => {
    const wid = "urn:libre-ai:spec-workspace:ap1";
    const created = decide(null, createCmd());
    if (created.status !== "accepted") throw new Error("create refused");
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveWorkspace(tx, wid, created.state, created.events, NOW),
    );

    await asRawTenant(TENANT_A, async () => {
      await expect(
        tdb.db.exec("UPDATE spec_workspace_events SET type = 'RequirementAdded'"),
      ).rejects.toThrow();
      await expect(tdb.db.exec("DELETE FROM spec_workspace_events")).rejects.toThrow();
    });
  });

  test("the app role may not delete a workspace (no DELETE grant)", async () => {
    const wid = "urn:libre-ai:spec-workspace:del1";
    const created = decide(null, createCmd());
    if (created.status !== "accepted") throw new Error("create refused");
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveWorkspace(tx, wid, created.state, created.events, NOW),
    );

    await asRawTenant(TENANT_A, async () => {
      await expect(tdb.db.exec("DELETE FROM spec_workspaces")).rejects.toThrow();
    });
  });

  test("without a tenant context the barrier denies reads and writes", async () => {
    await asRawTenant(null, async () => {
      const read = await tdb.db.query("SELECT * FROM spec_workspaces");
      expect(read.rows).toHaveLength(0);
      await expect(
        tdb.db.query(
          `INSERT INTO spec_workspaces (tenant_id, id, revision, status, state, created_at)
           VALUES ($1,'urn:libre-ai:spec-workspace:x',1,'draft','{}',$2)`,
          [TENANT_A, NOW],
        ),
      ).rejects.toThrow();
    });
  });
});
