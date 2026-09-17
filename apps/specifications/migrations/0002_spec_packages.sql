-- Specifications v1 persistence — content-addressed accepted packages
-- (docs/apps/specifications.md §Acceptance path). An accepted package is
-- immutable and identified by its SHA256 digest (content-addressing). Multiple
-- workspaces may accept the same package digest (idempotent re-acceptance),
-- but a workspace may transition to only ONE digest (enforced by the
-- unique constraint on workspace_id). The package bytes are stored as JSONB
-- for validation and retrieval; the digest is the PRIMARY identity.
-- Depends on the libre_ai_app role (packages/data 0000_app_role.sql).

CREATE TABLE spec_packages (
  tenant_id text NOT NULL
    CONSTRAINT spec_packages_tenant_format CHECK (tenant_id ~ '^ten_[a-z0-9]{16,64}$'),
  digest text NOT NULL
    CONSTRAINT spec_packages_digest_sha256 CHECK (digest ~ '^[a-f0-9]{64}$'),
  package_id text NOT NULL,
  workspace_id text NOT NULL,
  accepted_at timestamptz NOT NULL,
  accepted_by text,
  package_data jsonb NOT NULL,
  PRIMARY KEY (tenant_id, digest)
);

-- A workspace may accept at most one digest. Re-accepting the same digest
-- for the same workspace is idempotent (INSERT ... ON CONFLICT ... DO NOTHING);
-- accepting a different digest for an already-accepted workspace is refused
-- by the application layer, guarded by the unique constraint.
CREATE UNIQUE INDEX spec_packages_workspace_id_unique
  ON spec_packages (tenant_id, workspace_id)
  WHERE workspace_id IS NOT NULL;

ALTER TABLE spec_packages ENABLE ROW LEVEL SECURITY;
ALTER TABLE spec_packages FORCE ROW LEVEL SECURITY;

CREATE POLICY spec_packages_tenant_isolation ON spec_packages
  USING (tenant_id = current_setting('app.tenant_id', true))
  WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- Packages are immutable once accepted: the grant excludes UPDATE and DELETE.
GRANT SELECT, INSERT ON spec_packages TO libre_ai_app;
