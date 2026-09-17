-- Missions v1 persistence (docs/apps/missions.md §Data). PostgreSQL owns the
-- mission aggregate and its append-only event log, both tenant-scoped behind
-- FORCE row-level security keyed on the app.tenant_id GUC set by the
-- withTenantDbTransaction barrier (packages/data). The tenant format CHECK and
-- the least-privilege grants are the structural floor that holds even for a
-- caller that bypasses the application helpers. Depends on the libre_ai_app
-- role (packages/data migration 0000_app_role.sql).

CREATE TABLE missions (
  tenant_id text NOT NULL
    CONSTRAINT missions_tenant_format CHECK (tenant_id ~ '^ten_[a-z0-9]{16,64}$'),
  id text NOT NULL,
  revision integer NOT NULL CONSTRAINT missions_revision_positive CHECK (revision >= 1),
  state text NOT NULL CONSTRAINT missions_state_enum CHECK (state IN (
    'proposed', 'assessed', 'approved', 'refused', 'running', 'blocked',
    'paused', 'cancelled', 'result-submitted', 'accepted', 'rejected', 'abandoned'
  )),
  handoff_id text NOT NULL,
  handoff_digest text NOT NULL
    CONSTRAINT missions_handoff_digest_format CHECK (handoff_digest ~ '^[a-f0-9]{64}$'),
  -- risk, result and verdict are conditionally required by mission-record.v1
  -- (allOf on state); the aggregate/domain enforces those conditions, so the
  -- columns are nullable here.
  risk jsonb,
  budgets jsonb NOT NULL,
  acceptance_criteria jsonb NOT NULL,
  approvals jsonb NOT NULL,
  event_cursor integer NOT NULL CONSTRAINT missions_event_cursor_nonneg CHECK (event_cursor >= 0),
  result jsonb,
  verdict jsonb,
  created_at timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, id)
);

ALTER TABLE missions ENABLE ROW LEVEL SECURITY;
ALTER TABLE missions FORCE ROW LEVEL SECURITY;

CREATE POLICY missions_tenant_isolation ON missions
  USING (tenant_id = current_setting('app.tenant_id', true))
  WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

GRANT SELECT, INSERT, UPDATE, DELETE ON missions TO libre_ai_app;

-- Append-only causal event log. Reported activity is never rewritten
-- (docs/apps/missions.md non-goals): the grant excludes UPDATE and DELETE, so
-- the log is immutable even to the application role.
CREATE TABLE mission_events (
  tenant_id text NOT NULL
    CONSTRAINT mission_events_tenant_format CHECK (tenant_id ~ '^ten_[a-z0-9]{16,64}$'),
  mission_id text NOT NULL,
  sequence integer NOT NULL
    CONSTRAINT mission_events_sequence_positive CHECK (sequence >= 1),
  type text NOT NULL CONSTRAINT mission_events_type_enum CHECK (type IN (
    'MissionProposed', 'MissionRiskAssessed', 'MissionApproved', 'MissionRefused',
    'MissionStarted', 'MissionBlocked', 'HumanDecisionRequested', 'MissionPaused',
    'MissionCancelled', 'MissionResultSubmitted', 'MissionResultAccepted',
    'MissionResultRejected', 'MissionAbandoned'
  )),
  recorded_at timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, mission_id, sequence)
);

ALTER TABLE mission_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE mission_events FORCE ROW LEVEL SECURITY;

CREATE POLICY mission_events_tenant_isolation ON mission_events
  USING (tenant_id = current_setting('app.tenant_id', true))
  WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

GRANT SELECT, INSERT ON mission_events TO libre_ai_app;
