# ai-work-supervision Canonical Agent Rules

## Authority

Reserved couche-2 application home for Libre AI Work Supervision: keep track
of work entrusted to AI agents, what is blocked and what was actually done.
Doctrine lives upstream: https://raw.githubusercontent.com/libre-ai/project-governance/HEAD/AGENTS.md
Project state has one authority, `project.v1.yaml`; the README "Project
status" section is generated from it — never edit that section by hand.

## Boundaries

- Contract shapes are canonical in `libre-ai/schemas-and-contracts`, consumed
  through the pinned composition, never redefined here.
- Shared UI, web platform and testing bricks live in
  `libre-ai/application-development-toolkit`; data lifecycle bricks live in
  `libre-ai/organization-data-lifecycle`.
- The Work Supervision v0 native core lives in `crates/` (Cargo workspace at
  the root, single user, no tenant: ADR-0042 §5). Its journal format is
  specified in `docs/work-supervision/journal-v0.md`; the writer and the
  verifier crates must never share code. No executor other than the fake
  agent may be added before the C0 confinement qualification is green.
- Recovered code (`apps/missions`, `apps/specifications`,
  `packages/auth-web`) is not product qualification: admission criteria are
  in the card, not in historical documents.
- `apps/missions` is frozen as the reference of the multi-tenant model (V6)
  and has no authority over the v0 state (`apps/missions/AGENTS.md`).

## Quality gates

- Prepare the pinned composition first:
  https://raw.githubusercontent.com/libre-ai/project-governance/HEAD/docs/LOCAL-COMPOSITION.md
  (target `ai-work-supervision`); installation is an explicit step.
- Then run `bun run check` from this repository's root in the composition;
  never hide a red test. Browser suites sharing a port run sequentially.

## Agents

- Security > quality > performance > completeness, in that order on conflict.
- Check real state before editing: `git status --short` and the check above.
- English for code, comments and this file.
- Never commit a machine-local absolute filesystem path, a secret or a
  personal identifier; legacy deployment scripts are not run for local tests.
