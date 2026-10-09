# apps/missions — frozen reference

Status: **FROZEN** (owner decision Y31, 2026-10-09).

- This TypeScript domain is kept as the **reference of the multi-tenant model
  (V6)**: organization-scoped missions, row-level security per `tenant_id`,
  the `missions-v1.datalog` authorization matrix and the React cockpit fixture.
- It has **no authority over Work Supervision v0**. The v0 state lives in the
  native core under `crates/` (single user, no tenant, ADR-0042 §5): its
  journal is the authority and its SQLite database a projection of it.
- Do not extend, rewire or delete this package. Keep its tests passing under
  `bun run check`; a change other than a security fix or a toolchain bump is a
  new owner decision.
- Two mission state machines exist in this repository on purpose; the one
  here is not the v0 one and must not be read as such.
