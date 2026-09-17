// Specifications v1 persistence adapter — content-addressed accepted packages.
// An accepted package is immutable and identified by its SHA256 digest. The
// package is stored as a row in spec_packages, tenant-scoped by RLS. Idempotent
// on digest: re-saving the same digest for the same workspace is a no-op.
// A workspace may accept at most one digest; attempting to accept a different
// digest for a workspace that already has one throws SpecPackageDigestConflictError.
// Persists the metadata (workspace_id, accepted_at) and full package bytes,
// inside the caller's tenant transaction (packages/data withTenantDbTransaction).

import { requireTenantContext, type SqlExecutor } from "@libre-ai/data";
import type { SpecPackage } from "../domain/spec-package";

export class SpecPackageDigestConflictError extends Error {
  constructor(
    readonly workspaceId: string,
    readonly newDigest: string,
    readonly existingDigest: string,
  ) {
    super(
      `spec workspace ${workspaceId} already accepts digest ${existingDigest}, ` +
        `cannot accept different digest ${newDigest}`,
    );
    this.name = "SpecPackageDigestConflictError";
  }
}

export interface AcceptedPackageMetadata {
  readonly packageId: string;
  readonly digest: string;
  readonly workspaceId: string;
  readonly acceptedAt: string;
}

interface PackageRow {
  readonly tenant_id: string;
  readonly digest: string;
  readonly package_id: string;
  readonly workspace_id: string;
  readonly accepted_at: string | Date;
  readonly accepted_by: string | null;
  readonly package_data: unknown;
}

interface WorkspaceDigestRow {
  readonly digest: string;
}

function asIsoString(value: string | Date): string {
  return value instanceof Date ? value.toISOString() : value;
}

/**
 * Save an accepted package, content-addressed by digest. Idempotent on digest:
 * re-saving the same digest for the same workspace is a no-op. A workspace may
 * accept at most one digest; attempting to accept a different digest for a
 * workspace that already has one throws SpecPackageDigestConflictError.
 */
export async function saveAcceptedPackage(
  executor: SqlExecutor,
  pkg: SpecPackage,
  workspaceId: string,
  acceptedAt: string,
  acceptedBy?: string,
): Promise<void> {
  const tenantId = requireTenantContext();
  const packageData = JSON.stringify(pkg);

  // Check if this workspace already has an accepted package with a DIFFERENT digest.
  const { rows: existing } = await executor.query<WorkspaceDigestRow>(
    `SELECT digest FROM spec_packages WHERE tenant_id = $1 AND workspace_id = $2`,
    [tenantId, workspaceId],
  );

  const existingRow = existing?.[0];
  if (existingRow) {
    if (existingRow.digest !== pkg.digest) {
      throw new SpecPackageDigestConflictError(workspaceId, pkg.digest, existingRow.digest);
    }
    // Same digest, same workspace — idempotent, no-op
    return;
  }

  // New workspace or new digest for a workspace without a prior acceptance.
  // Use INSERT ... ON CONFLICT ... DO NOTHING for digest-level idempotence
  // (multiple workspaces may accept the same digest).
  await executor.query(
    `INSERT INTO spec_packages (tenant_id, digest, package_id, workspace_id, accepted_at, accepted_by, package_data)
     VALUES ($1, $2, $3, $4, $5, $6, $7)
     ON CONFLICT (tenant_id, digest) DO NOTHING`,
    [tenantId, pkg.digest, pkg.id, workspaceId, acceptedAt, acceptedBy ?? null, packageData],
  );
}

/**
 * Load one accepted package by digest. Tenant scoping is by RLS alone: the read
 * runs under the active tenant context, so a foreign-tenant digest simply returns
 * no row (never another tenant's package).
 */
export async function loadAcceptedPackage(
  executor: SqlExecutor,
  digest: string,
): Promise<AcceptedPackageMetadata | null> {
  const { rows } = await executor.query<PackageRow>(
    `SELECT tenant_id, digest, package_id, workspace_id, accepted_at, accepted_by, package_data
     FROM spec_packages WHERE digest = $1`,
    [digest],
  );
  const row = rows[0];
  if (row === undefined) return null;

  return {
    packageId: row.package_id,
    digest: row.digest,
    workspaceId: row.workspace_id,
    acceptedAt: asIsoString(row.accepted_at),
  };
}
