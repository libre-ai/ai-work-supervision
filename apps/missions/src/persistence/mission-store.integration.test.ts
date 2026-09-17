import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { withTenantDbTransaction } from "@libre-ai/data";
import { createTestDatabase, type TestDatabase } from "@libre-ai/testing";
import { type Command, decide, type Mission } from "../domain/mission";
import { loadMission, MissionRevisionConflictError, saveMission } from "./mission-store";

// The mission persistence exercised against the real PostgreSQL barrier
// (PGlite): the tables, FORCE RLS policies and least-privilege grants from
// 0001_missions.sql, on top of the libre_ai_app role from packages/data.
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
const TENANT_A = "ten_aaaaaaaaaaaaaaaa";
const TENANT_B = "ten_bbbbbbbbbbbbbbbb";
const NOW = "2030-01-01T00:00:00Z";

let tdb: TestDatabase;

beforeAll(async () => {
  tdb = await createTestDatabase();
  await tdb.applyMigrations(DATA_MIGRATIONS);
  await tdb.applyMigrations(MISSIONS_MIGRATIONS);
});

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

// Run raw SQL under the app role and (optionally) a tenant GUC, rolled back so
// tests do not leak rows. Mirrors the D01 barrier test.
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

describe("mission store round-trip and tenant isolation", () => {
  test("saves a proposed mission and loads it back within the tenant", async () => {
    const decision = decide(null, proposeCommand("urn:libre-ai:mission:rt1"), {
      tenantId: TENANT_A,
      now: NOW,
      expectedRevision: 0,
    });
    expect(decision.ok).toBe(true);
    if (!decision.ok) return;

    const loaded = await withTenantDbTransaction(tdb.db, TENANT_A, async (tx) => {
      await saveMission(tx, decision.next, decision.events, NOW);
      return loadMission(tx, "urn:libre-ai:mission:rt1");
    });
    expect(loaded?.state).toBe("proposed");
    expect(loaded?.tenantId).toBe(TENANT_A);
    expect(loaded?.acceptanceCriteria).toEqual(["criterion-a"]);
  });

  test("a different tenant cannot read another tenant's mission", async () => {
    const decision = decide(null, proposeCommand("urn:libre-ai:mission:iso1"), {
      tenantId: TENANT_A,
      now: NOW,
      expectedRevision: 0,
    });
    if (!decision.ok) throw new Error("propose refused");
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveMission(tx, decision.next, decision.events, NOW),
    );

    const crossTenant = await withTenantDbTransaction(tdb.db, TENANT_B, (tx) =>
      loadMission(tx, "urn:libre-ai:mission:iso1"),
    );
    expect(crossTenant).toBeNull();
  });

  test("a different tenant's raw UPDATE and DELETE match no row (RLS USING)", async () => {
    const decision = decide(null, proposeCommand("urn:libre-ai:mission:iso2"), {
      tenantId: TENANT_A,
      now: NOW,
      expectedRevision: 0,
    });
    if (!decision.ok) throw new Error("propose refused");
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveMission(tx, decision.next, decision.events, NOW),
    );

    // Tenant B, in raw SQL under the app role, cannot mutate tenant A's row:
    // the RLS USING clause makes it invisible, so both statements affect 0 rows.
    const [updated, deleted] = await asRawTenant(TENANT_B, async () => {
      const u = await tdb.db.query("UPDATE missions SET state = 'cancelled' WHERE id = $1", [
        "urn:libre-ai:mission:iso2",
      ]);
      const d = await tdb.db.query("DELETE FROM missions WHERE id = $1", [
        "urn:libre-ai:mission:iso2",
      ]);
      return [u.affectedRows ?? 0, d.affectedRows ?? 0];
    });
    expect(updated).toBe(0);
    expect(deleted).toBe(0);

    // The row is untouched for its owner.
    const stillProposed = await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      loadMission(tx, "urn:libre-ai:mission:iso2"),
    );
    expect(stillProposed?.state).toBe("proposed");
  });
});

describe("optimistic revision concurrency", () => {
  test("a stale-revision write updates no row and is rejected", async () => {
    const created = decide(null, proposeCommand("urn:libre-ai:mission:cc1"), {
      tenantId: TENANT_A,
      now: NOW,
      expectedRevision: 0,
    });
    if (!created.ok) throw new Error("propose refused");
    const proposed: Mission = created.next;

    const assess: Command = { type: "AssessMissionRisk", level: "low", policyVersion: "1.0.0" };
    const winner = decide(proposed, assess, { tenantId: TENANT_A, now: NOW, expectedRevision: 1 });
    const loser = decide(proposed, assess, { tenantId: TENANT_A, now: NOW, expectedRevision: 1 });
    if (!winner.ok || !loser.ok) throw new Error("assess refused");

    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveMission(tx, proposed, created.events, NOW),
    );
    // The winner advances revision 1 -> 2.
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveMission(tx, winner.next, winner.events, NOW),
    );
    // The loser holds the same stale revision-1 aggregate: its guarded update
    // matches no row (current revision is already 2) and is rejected.
    await expect(
      withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
        saveMission(tx, loser.next, loser.events, NOW),
      ),
    ).rejects.toBeInstanceOf(MissionRevisionConflictError);
  });
});

describe("append-only event log and fail-closed barrier", () => {
  test("the app role may not update or delete recorded events", async () => {
    const decision = decide(null, proposeCommand("urn:libre-ai:mission:ap1"), {
      tenantId: TENANT_A,
      now: NOW,
      expectedRevision: 0,
    });
    if (!decision.ok) throw new Error("propose refused");
    await withTenantDbTransaction(tdb.db, TENANT_A, (tx) =>
      saveMission(tx, decision.next, decision.events, NOW),
    );

    await asRawTenant(TENANT_A, async () => {
      await expect(
        tdb.db.exec("UPDATE mission_events SET type = 'MissionRefused'"),
      ).rejects.toThrow();
      await expect(tdb.db.exec("DELETE FROM mission_events")).rejects.toThrow();
    });
  });

  test("without a tenant context the barrier denies reads and writes", async () => {
    await asRawTenant(null, async () => {
      const read = await tdb.db.query("SELECT * FROM missions");
      expect(read.rows).toHaveLength(0);
      await expect(
        tdb.db.query(
          `INSERT INTO missions (
             tenant_id, id, revision, state, handoff_id, handoff_digest, budgets,
             acceptance_criteria, approvals, event_cursor, created_at
           ) VALUES ($1,'urn:libre-ai:mission:x',1,'proposed','h','${"a".repeat(64)}','{}','[]','[]',1,$2)`,
          [TENANT_A, NOW],
        ),
      ).rejects.toThrow();
    });
  });
});
