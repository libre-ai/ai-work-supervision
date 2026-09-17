-- Specifications v1 persistence (docs/apps/specifications.md §Domain protocol).
-- PostgreSQL owns the spec-workspace aggregate (a revisioned snapshot) and its
-- append-only event log, both tenant-scoped behind FORCE row-level security keyed
-- on the app.tenant_id GUC set by the withTenantDbTransaction barrier
-- (packages/data). The tenant-format CHECK and the least-privilege grants are the
-- structural floor that holds even for a caller that bypasses the application
-- helpers. Depends on the libre_ai_app role (packages/data 0000_app_role.sql).
--
-- The workspace state is stored as one snapshot (`state` jsonb) rather than an
-- event-sourced fold: the domain (workspace.ts `decide`) is command-sourced and
-- exposes no event reducer, so the aggregate row is the source of truth and the
-- event table is an append-only audit trail. `status` is lifted to its own column
-- as a queryable DB-floor enum; optimistic concurrency is by `revision`. Each
-- accepted `decide` advances the revision by one and emits one event, so a
-- persisted event's sequence equals the revision at which it occurred (WP-G3-F01).

CREATE TABLE spec_workspaces (
  tenant_id text NOT NULL
    CONSTRAINT spec_workspaces_tenant_format CHECK (tenant_id ~ '^ten_[a-z0-9]{16,64}$'),
  id text NOT NULL,
  revision integer NOT NULL
    CONSTRAINT spec_workspaces_revision_positive CHECK (revision >= 1),
  status text NOT NULL CONSTRAINT spec_workspaces_status_enum CHECK (status IN (
    'draft', 'submitted', 'accepted', 'superseded'
  )),
  state jsonb NOT NULL,
  created_at timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, id)
);

ALTER TABLE spec_workspaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE spec_workspaces FORCE ROW LEVEL SECURITY;

CREATE POLICY spec_workspaces_tenant_isolation ON spec_workspaces
  USING (tenant_id = current_setting('app.tenant_id', true))
  WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- A workspace is created, advanced and superseded in place (supersession is a
-- status transition, not a row deletion); it is never physically removed, so the
-- grant excludes DELETE.
GRANT SELECT, INSERT, UPDATE ON spec_workspaces TO libre_ai_app;

-- Append-only causal event log. The authoring history is never rewritten: the
-- grant excludes UPDATE and DELETE, so the log is immutable even to the
-- application role.
CREATE TABLE spec_workspace_events (
  tenant_id text NOT NULL
    CONSTRAINT spec_workspace_events_tenant_format CHECK (tenant_id ~ '^ten_[a-z0-9]{16,64}$'),
  workspace_id text NOT NULL,
  sequence integer NOT NULL
    CONSTRAINT spec_workspace_events_sequence_positive CHECK (sequence >= 1),
  type text NOT NULL CONSTRAINT spec_workspace_events_type_enum CHECK (type IN (
    'SpecWorkspaceCreated', 'RequirementAdded', 'ContractAttached',
    'AcceptanceCriterionDefined', 'DecisionRecorded', 'DecisionResolved',
    'SpecSubmitted', 'SpecReviewRecorded', 'SpecPackageAccepted',
    'SpecPackageSuperseded', 'PlanningHandoffCreated'
  )),
  recorded_at timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, workspace_id, sequence)
);

ALTER TABLE spec_workspace_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE spec_workspace_events FORCE ROW LEVEL SECURITY;

CREATE POLICY spec_workspace_events_tenant_isolation ON spec_workspace_events
  USING (tenant_id = current_setting('app.tenant_id', true))
  WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

GRANT SELECT, INSERT ON spec_workspace_events TO libre_ai_app;
