// SPDX-FileCopyrightText: 2026 Libre AI contributors
// SPDX-License-Identifier: EUPL-1.2
//
// The Pi bridge extension: unit behaviour with a fake `pi`, then end to end
// with the real `ws` and `wsd` built by `cargo build` (run `check:native`
// first; a missing binary fails this test, it is never skipped).

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { Subprocess } from "bun";
import {
  type BridgeApi,
  type BridgeContext,
  type BridgeEvent,
  hookArguments,
  installBridge,
  REPORT_TIMEOUT_MS,
} from "./ws-bridge";

type Handler = (event: unknown, ctx: BridgeContext) => Promise<void>;

interface Call {
  command: string;
  args: string[];
  timeout: number | undefined;
}

/** A fake `pi` recording subscriptions; `exec` is given by the test. */
function fakePi(
  exec: (command: string, args: string[]) => Promise<{ code: number; stderr: string }>,
) {
  const handlers = new Map<string, Handler>();
  const calls: Call[] = [];
  const pi: BridgeApi = {
    on(event: BridgeEvent, handler: Handler): void {
      handlers.set(event, handler);
    },
    async exec(command, args, options) {
      calls.push({ command, args, timeout: options?.timeout });
      return exec(command, args);
    },
  } as BridgeApi;
  return { pi, handlers, calls };
}

function context(sessionId: string, cwd: string): BridgeContext {
  return { cwd, sessionManager: { getSessionId: () => sessionId } };
}

describe("the bridge with a fake pi", () => {
  test("subscribes to the four lifecycle events and reports each one", async () => {
    const { pi, handlers, calls } = fakePi(async () => ({ code: 0, stderr: "" }));
    installBridge(pi, { WS_ROOT: "/srv/ws-root", WS_BIN: "/opt/ws" });
    expect([...handlers.keys()].sort()).toEqual([
      "agent_settled",
      "agent_start",
      "session_shutdown",
      "session_start",
    ]);
    await handlers.get("agent_settled")?.({}, context("pi-session-1", "/w/repo"));
    expect(calls).toHaveLength(1);
    expect(calls[0]?.command).toBe("/opt/ws");
    expect(calls[0]?.timeout).toBe(REPORT_TIMEOUT_MS);
    expect(calls[0]?.args.slice(0, 4)).toEqual(["--root", "/srv/ws-root", "hook", "pi"]);
    expect(JSON.parse(calls[0]?.args[4] ?? "")).toEqual({
      event: "agent_settled",
      session_id: "pi-session-1",
      cwd: "/w/repo",
    });
  });

  test("does nothing without an absolute WS_ROOT", () => {
    for (const environment of [{}, { WS_ROOT: "" }, { WS_ROOT: "relative/root" }]) {
      const { pi, handlers } = fakePi(async () => ({ code: 0, stderr: "" }));
      installBridge(pi, environment);
      expect(handlers.size).toBe(0);
    }
  });

  test("never fails the session when ws cannot be run", async () => {
    const { pi, handlers } = fakePi(async () => {
      throw new Error("spawn ENOENT");
    });
    installBridge(pi, { WS_ROOT: "/srv/ws-root" });
    await expect(
      handlers.get("session_start")?.({}, context("s", "/")) ?? Promise.resolve(),
    ).resolves.toBeUndefined();
  });

  test("the payload is the last argument, as ws hook reads it", () => {
    const args = hookArguments("/r", { event: "session_start", session_id: "a b", cwd: "/c" });
    expect(args).toEqual([
      "--root",
      "/r",
      "hook",
      "pi",
      '{"event":"session_start","session_id":"a b","cwd":"/c"}',
    ]);
  });
});

describe("the bridge end to end with ws and wsd", () => {
  const binaries = resolve(import.meta.dir, "../../target/debug");
  const ws = join(binaries, "ws");
  const wsd = join(binaries, "wsd");
  const fakeAgent = join(binaries, "ws-fake-agent");
  let base = "";
  let root = "";
  let daemon: Subprocess | undefined;

  function run(args: string[]): { code: number; stdout: string; stderr: string } {
    const result = Bun.spawnSync([ws, "--root", root, ...args]);
    return {
      code: result.exitCode,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    };
  }

  beforeAll(async () => {
    for (const binary of [ws, wsd, fakeAgent]) {
      expect(existsSync(binary)).toBe(true);
    }
    base = mkdtempSync(join(tmpdir(), "ws-pi-bridge-"));
    root = join(base, "root");
    expect(Bun.spawnSync([ws, "init", "--root", root]).exitCode).toBe(0);
    writeFileSync(
      join(root, "config.toml"),
      `[repositories]\n\n[executor]\nprofile = "fake"\nfake_agent = "${fakeAgent}"\n\n[runs]\nidle_after_ms = 300\ngrace_ms = 500\n`,
    );
    daemon = Bun.spawn([wsd, "--root", root], { stdout: "ignore", stderr: "ignore" });
    const deadline = Date.now() + 20_000;
    while (run(["status"]).code !== 0) {
      expect(Date.now()).toBeLessThan(deadline);
      await Bun.sleep(20);
    }
  });

  afterAll(() => {
    daemon?.kill();
    if (base !== "") {
      rmSync(base, { recursive: true, force: true });
    }
  });

  test("a Pi session is registered, reported and ended through ws hook pi", async () => {
    // The fake pi runs the real program, as pi.exec does: no shell, argv only.
    const { pi, handlers } = fakePi(async (command, args) => {
      const result = Bun.spawnSync([command, ...args]);
      expect(result.stdout.toString()).toBe("");
      return { code: result.exitCode, stderr: result.stderr.toString() };
    });
    installBridge(pi, { WS_ROOT: root, WS_BIN: ws });
    const ctx = context("pi-session-secret-7", base);
    await handlers.get("session_start")?.({}, ctx);
    let sessions = JSON.parse(run(["session", "list"]).stdout);
    expect(sessions).toHaveLength(1);
    expect(sessions[0].harness).toBe("pi");
    expect(sessions[0].reported_state).toBe("working");
    await handlers.get("agent_settled")?.({}, ctx);
    sessions = JSON.parse(run(["session", "list"]).stdout);
    expect(sessions[0].reported_state).toBe("waiting-input");
    expect(sessions[0].note).toBe("turn-complete");
    await handlers.get("session_shutdown")?.({}, ctx);
    sessions = JSON.parse(run(["session", "list"]).stdout);
    expect(sessions[0].state).toBe("ended");
    // Only the digest of Pi's session identifier reached the journal.
    const journal = await Bun.file(join(root, "journal", "journal.v0.jsonl")).text();
    expect(journal).not.toContain("pi-session-secret-7");
  });
});
