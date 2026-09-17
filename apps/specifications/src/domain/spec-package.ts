// Specifications domain — the pure validator for an accepted SpecPackage
// (docs/apps/specifications.md; contracts/schemas/spec-package.v1.schema.json).
// An accepted package is immutable and content-addressed. This module imports
// nothing, persists nothing, transmits nothing. It validates untrusted input
// into a typed, contract-conformant SpecPackage and reports, fail-closed, WHY it
// is not acceptable, distinguishing three outcomes:
//   - `malformed`  — it is not a well-formed spec package at all (identity or
//     item structure fails the schema); a boundary concern, not a domain refusal.
//   - `refused`    — it is well-formed but violates a semantic completeness or
//     separation invariant, reported with the exact spec.* matrix code.
//   - `valid`      — a typed, conformant accepted package.
// Patterns reuse the LOCKED common.v1 $defs verbatim.

const URN = /^urn:libre-ai:[a-z][a-z0-9-]*:[A-Za-z0-9._~-]+$/;
const TENANT_ID = /^ten_[a-z0-9]{16,64}$/;
const IDENTIFIER = /^[a-z][a-z0-9_-]{2,127}$/;
const SHA256 = /^[a-f0-9]{64}$/;
// common.v1 approvalReference.role
const ROLE = /^[a-z][a-z0-9-]{1,63}$/;
const TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/;

const PRIORITIES = ["must", "should", "could"] as const;
const SEVERITIES = ["low", "medium", "high", "critical"] as const;

export interface Requirement {
  readonly id: string;
  readonly text: string;
  readonly priority: (typeof PRIORITIES)[number];
}
export interface Decision {
  readonly id: string;
  readonly status: "accepted";
  readonly decision: string;
}
export interface Risk {
  readonly id: string;
  readonly severity: (typeof SEVERITIES)[number];
  readonly control: string;
}
export interface AcceptanceCriterion {
  readonly id: string;
  readonly observable: string;
  readonly evidenceRule: string;
}
export interface ApprovalReference {
  readonly role: string;
  readonly approvedAt: string;
  readonly reference: string;
  readonly subjectDigest: string;
}
export interface SpecPackage {
  readonly schemaVersion: "libre-ai.spec-package.v1";
  readonly id: string;
  readonly tenantId: string;
  readonly version: number;
  readonly status: "accepted";
  readonly problem: string;
  readonly actors: readonly string[];
  readonly requirements: readonly Requirement[];
  readonly decisions: readonly Decision[];
  readonly contracts: readonly string[];
  readonly risks: readonly Risk[];
  readonly acceptanceCriteria: readonly AcceptanceCriterion[];
  readonly approvals: readonly ApprovalReference[];
  readonly acceptedAt: string;
  readonly digest: string;
}

// Only the codes this increment actually enforces (declaring-but-never-returning
// a code is a fail-closed lie). evidence_hash_mismatch (content-digest) and the
// command-level codes (revision_stale, package_immutable, handoff_execution_right)
// are deferred: the spec-package digest preimage is not defined in a reusable
// tool, so verifying it here would be a guess.
export type SpecRefusalCode =
  | "spec.problem_missing"
  | "spec.contract_missing"
  | "spec.acceptance_unverifiable"
  | "spec.decision_open"
  | "spec.approval_self_only";

export type SpecValidation =
  | { readonly status: "valid"; readonly value: SpecPackage }
  | { readonly status: "malformed" }
  | { readonly status: "refused"; readonly refusal: SpecRefusalCode };

const MALFORMED: SpecValidation = { status: "malformed" };
function refused(refusal: SpecRefusalCode): SpecValidation {
  return { status: "refused", refusal };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function hasExactKeys(obj: Record<string, unknown>, allowed: readonly string[]): boolean {
  const permitted = new Set(allowed);
  return Object.keys(obj).every((key) => permitted.has(key));
}
function boundedString(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length >= 1 && value.length <= max;
}
function isArray(value: unknown, min: number, max: number): value is unknown[] {
  return Array.isArray(value) && value.length >= min && value.length <= max;
}
function unique(values: readonly string[]): boolean {
  return new Set(values).size === values.length;
}

function readRequirements(value: unknown): Requirement[] | undefined {
  if (!isArray(value, 1, 1000)) return undefined;
  const out: Requirement[] = [];
  for (const item of value) {
    if (!isObject(item) || !hasExactKeys(item, ["id", "text", "priority"])) return undefined;
    if (typeof item.id !== "string" || !IDENTIFIER.test(item.id)) return undefined;
    if (!boundedString(item.text, 5000)) return undefined;
    if (
      typeof item.priority !== "string" ||
      !(PRIORITIES as readonly string[]).includes(item.priority)
    ) {
      return undefined;
    }
    out.push(
      Object.freeze({
        id: item.id,
        text: item.text,
        priority: item.priority as Requirement["priority"],
      }),
    );
  }
  return out;
}

function readRisks(value: unknown): Risk[] | undefined {
  if (!isArray(value, 0, 1000)) return undefined;
  const out: Risk[] = [];
  for (const item of value) {
    if (!isObject(item) || !hasExactKeys(item, ["id", "severity", "control"])) return undefined;
    if (typeof item.id !== "string" || !IDENTIFIER.test(item.id)) return undefined;
    if (
      typeof item.severity !== "string" ||
      !(SEVERITIES as readonly string[]).includes(item.severity)
    ) {
      return undefined;
    }
    if (!boundedString(item.control, 5000)) return undefined;
    out.push(
      Object.freeze({
        id: item.id,
        severity: item.severity as Risk["severity"],
        control: item.control,
      }),
    );
  }
  return out;
}

function readApprovals(value: unknown): ApprovalReference[] | undefined {
  if (!isArray(value, 1, 100)) return undefined;
  const out: ApprovalReference[] = [];
  for (const item of value) {
    if (
      !isObject(item) ||
      !hasExactKeys(item, ["role", "approvedAt", "reference", "subjectDigest"])
    ) {
      return undefined;
    }
    if (typeof item.role !== "string" || !ROLE.test(item.role)) return undefined;
    if (typeof item.approvedAt !== "string" || !TIMESTAMP.test(item.approvedAt)) return undefined;
    if (typeof item.reference !== "string" || !URN.test(item.reference)) return undefined;
    if (typeof item.subjectDigest !== "string" || !SHA256.test(item.subjectDigest))
      return undefined;
    out.push(
      Object.freeze({
        role: item.role,
        approvedAt: item.approvedAt,
        reference: item.reference,
        subjectDigest: item.subjectDigest,
      }),
    );
  }
  return out;
}

function readAcceptanceCriteria(value: unknown): AcceptanceCriterion[] | undefined {
  if (!isArray(value, 1, 1000)) return undefined;
  const out: AcceptanceCriterion[] = [];
  for (const item of value) {
    if (!isObject(item) || !hasExactKeys(item, ["id", "observable", "evidenceRule"]))
      return undefined;
    if (typeof item.id !== "string" || !IDENTIFIER.test(item.id)) return undefined;
    if (!boundedString(item.observable, 5000)) return undefined;
    if (typeof item.evidenceRule !== "string" || !IDENTIFIER.test(item.evidenceRule))
      return undefined;
    out.push(
      Object.freeze({ id: item.id, observable: item.observable, evidenceRule: item.evidenceRule }),
    );
  }
  return out;
}

const KEYS = [
  "schemaVersion",
  "id",
  "tenantId",
  "version",
  "status",
  "problem",
  "actors",
  "requirements",
  "decisions",
  "contracts",
  "risks",
  "acceptanceCriteria",
  "approvals",
  "acceptedAt",
  "digest",
] as const;

/**
 * Validate untrusted input as an accepted SpecPackage. Structural/identity
 * failures return `malformed`; a well-formed package that violates a semantic
 * completeness invariant returns `refused` with the matching spec.* code:
 * missing problem/actors → problem_missing, contracts → contract_missing,
 * acceptance criteria → acceptance_unverifiable, an unresolved decision →
 * decision_open, and fewer than two distinct approvers → approval_self_only.
 */
export function validateSpecPackage(input: unknown): SpecValidation {
  if (!isObject(input) || !hasExactKeys(input, KEYS)) return MALFORMED;
  for (const key of KEYS) {
    if (!(key in input)) return MALFORMED;
  }
  // Identity fields — malformation means it is not a spec package at all.
  if (input.schemaVersion !== "libre-ai.spec-package.v1") return MALFORMED;
  if (typeof input.id !== "string" || !URN.test(input.id)) return MALFORMED;
  if (typeof input.tenantId !== "string" || !TENANT_ID.test(input.tenantId)) return MALFORMED;
  if (typeof input.version !== "number" || !Number.isInteger(input.version) || input.version < 1) {
    return MALFORMED;
  }
  if (input.status !== "accepted") return MALFORMED;
  if (typeof input.acceptedAt !== "string" || !TIMESTAMP.test(input.acceptedAt)) return MALFORMED;
  if (typeof input.digest !== "string" || !SHA256.test(input.digest)) return MALFORMED;
  const requirements = readRequirements(input.requirements);
  if (requirements === undefined) return MALFORMED;
  const risks = readRisks(input.risks);
  if (risks === undefined) return MALFORMED;

  // Completeness sections — a structural failure maps to its semantic code.
  if (!boundedString(input.problem, 5000)) return refused("spec.problem_missing");
  if (
    !isArray(input.actors, 1, 100) ||
    !input.actors.every((a): a is string => typeof a === "string" && IDENTIFIER.test(a)) ||
    !unique(input.actors)
  ) {
    return refused("spec.problem_missing");
  }
  if (
    !isArray(input.contracts, 1, 1000) ||
    !input.contracts.every((c): c is string => typeof c === "string" && URN.test(c)) ||
    !unique(input.contracts)
  ) {
    return refused("spec.contract_missing");
  }
  const criteria = readAcceptanceCriteria(input.acceptanceCriteria);
  if (criteria === undefined) return refused("spec.acceptance_unverifiable");
  // Decisions: a structurally-bad item is malformed; a well-formed decision
  // whose status is not "accepted" is an unresolved decision → decision_open.
  if (!isArray(input.decisions, 0, 1000)) return MALFORMED;
  const decisions: Decision[] = [];
  for (const item of input.decisions) {
    if (!isObject(item) || !hasExactKeys(item, ["id", "status", "decision"])) return MALFORMED;
    if (typeof item.id !== "string" || !IDENTIFIER.test(item.id)) return MALFORMED;
    if (!boundedString(item.decision, 5000)) return MALFORMED;
    if (item.status !== "accepted") return refused("spec.decision_open");
    decisions.push(Object.freeze({ id: item.id, status: "accepted", decision: item.decision }));
  }
  const approvals = readApprovals(input.approvals);
  if (approvals === undefined) return MALFORMED;
  // Separation of powers: an accepted package needs at least two distinct
  // approvers (by reference), never author-as-sole-approver.
  if (new Set(approvals.map((a) => a.reference)).size < 2)
    return refused("spec.approval_self_only");

  return {
    status: "valid",
    value: Object.freeze({
      schemaVersion: "libre-ai.spec-package.v1",
      id: input.id,
      tenantId: input.tenantId,
      version: input.version,
      status: "accepted",
      problem: input.problem,
      actors: Object.freeze([...input.actors]),
      requirements: Object.freeze(requirements),
      decisions: Object.freeze(decisions),
      contracts: Object.freeze([...input.contracts]),
      risks: Object.freeze(risks),
      acceptanceCriteria: Object.freeze(criteria),
      approvals: Object.freeze(approvals),
      acceptedAt: input.acceptedAt,
      digest: input.digest,
    }),
  };
}
