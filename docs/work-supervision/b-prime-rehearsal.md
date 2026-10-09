# B′ rehearsal — SIMULATION with the fake agent

> **Simulation.** B′, the v0 exit criterion of `project.v1.yaml`, is ten
> consecutive missions end to end with a **real** agent behind the C0
> confinement qualification. This rehearsal runs the same campaign with the
> fake agent. It exercises the v0 machinery and **does not count toward B′**;
> the card's criterion stays `pending`.

Test: `crates/work-supervision-cli/tests/b_prime_simulation.rs`, run by
`cargo test --workspace` (so by `bun run check`), through the `ws` command line
against a real `wsd`, with the head anchored outside the root.

Campaign: 10 consecutive missions in three shapes — plain run (4), run with an
input exchange through `ws send` (3), rejection then second run on the kept
worktree (3) — each created, readied, run, given a result with evidence, and
accepted.

End checks, all asserted:

- 10/10 accepted, each `ws/<mission>` branch holding its committed result;
- `ws journal verify` valid, anchor `matched`; `ws rebuild --check` equal;
- 0 worktree registered besides the main one, `worktrees/` empty;
  `ws worktree gc` removes nothing, `ws doctor` finishes no pending removal;
- no handoff file: nothing written outside the root, the repository, the
  private anchor and the test's own inputs; no `notes/` directory.

Every fake-agent mission carries `"simulation": true` in `ws mission show` /
`list`, and the cockpit labels it « Simulation — fake agent ».

Measured on 2026-10-09 (local composition, generation P):

```text
SIMULATION (fake agent, does not count toward B′): 10/10 accepted (3 after a rejection, 3 with input); journal 157 entries, anchor "matched"; projection rebuild equal: true; worktrees registered besides main: 0; ws/* branches: 10 (kept results); gc removed 0+0; doctor pending removals 0; stray files: 0
```
