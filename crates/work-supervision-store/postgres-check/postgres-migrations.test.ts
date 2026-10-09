// Second instrument for the Work Supervision v0 projection schema: the
// PostgreSQL migrations are applied in PGlite and their tables, columns,
// nullability and primary keys are compared with migrations/schema.v0.json,
// the description the SQLite migrations are checked against in Rust.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { PGlite } from "@electric-sql/pglite";

interface ColumnDescription {
  name: string;
  type: "integer" | "text";
  nullable: boolean;
}

interface TableDescription {
  name: string;
  columns: ColumnDescription[];
  primary_key: string[];
}

interface SchemaDescription {
  schema: string;
  tables: TableDescription[];
}

const migrations = new URL("../migrations/", import.meta.url);

function migrationNames(dialect: "postgres" | "sqlite"): string[] {
  return readdirSync(new URL(`${dialect}/`, migrations))
    .filter((name) => name.endsWith(".sql"))
    .sort();
}

function postgresType(dataType: string): ColumnDescription["type"] {
  if (dataType === "bigint") return "integer";
  if (dataType === "text") return "text";
  throw new Error(`unexpected PostgreSQL column type ${dataType}`);
}

async function migratedDatabase(): Promise<PGlite> {
  const database = new PGlite();
  for (const name of migrationNames("postgres")) {
    await database.exec(readFileSync(new URL(`postgres/${name}`, migrations), "utf8"));
  }
  return database;
}

async function describePostgres(database: PGlite): Promise<SchemaDescription> {
  const columns = await database.query<{
    table_name: string;
    column_name: string;
    data_type: string;
    is_nullable: string;
  }>(
    `SELECT table_name, column_name, data_type, is_nullable
       FROM information_schema.columns
      WHERE table_schema = 'public'
      ORDER BY table_name, ordinal_position`,
  );
  const keys = await database.query<{ table_name: string; column_name: string }>(
    `SELECT usage.table_name, usage.column_name
       FROM information_schema.table_constraints AS constraints
       JOIN information_schema.key_column_usage AS usage
         ON usage.constraint_name = constraints.constraint_name
        AND usage.table_schema = constraints.table_schema
      WHERE constraints.table_schema = 'public' AND constraints.constraint_type = 'PRIMARY KEY'
      ORDER BY usage.table_name, usage.ordinal_position`,
  );
  const tables = new Map<string, TableDescription>();
  for (const row of columns.rows) {
    const table = tables.get(row.table_name) ?? {
      name: row.table_name,
      columns: [],
      primary_key: [],
    };
    table.columns.push({
      name: row.column_name,
      type: postgresType(row.data_type),
      nullable: row.is_nullable === "YES",
    });
    tables.set(row.table_name, table);
  }
  for (const row of keys.rows) {
    tables.get(row.table_name)?.primary_key.push(row.column_name);
  }
  return {
    schema: "libre-ai.work-supervision.projection.v0",
    tables: [...tables.values()].sort((left, right) => left.name.localeCompare(right.name)),
  };
}

describe("PostgreSQL migrations of the Work Supervision v0 projection", () => {
  test("every SQLite migration has a PostgreSQL counterpart with the same name", () => {
    expect(migrationNames("postgres")).toEqual(migrationNames("sqlite"));
    expect(migrationNames("postgres").length).toBeGreaterThan(0);
  });

  test("apply in PGlite and match the schema description exactly", async () => {
    const database = await migratedDatabase();
    const expected = JSON.parse(
      readFileSync(new URL("schema.v0.json", migrations), "utf8"),
    ) as SchemaDescription;
    const described = await describePostgres(database);
    expect(described.tables.length).toBe(expected.tables.length);
    expect(described).toEqual(expected);
    await database.close();
  });

  test("keep the single-row position and refuse a second one", async () => {
    const database = await migratedDatabase();
    await database.exec(
      `INSERT INTO projection_position (id, seq, digest) VALUES (1, 1, '${"0".repeat(64)}')`,
    );
    await expect(
      database.exec(`INSERT INTO projection_position (id, seq, digest) VALUES (2, 2, 'x')`),
    ).rejects.toThrow();
    await database.close();
  });
});
