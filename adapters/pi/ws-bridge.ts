// SPDX-FileCopyrightText: 2026 Libre AI contributors
// SPDX-License-Identifier: EUPL-1.2
//
// Pi extension of the Work Supervision session bridge
// (docs/work-supervision/harness-adapters-v0.md): it reports the session's
// lifecycle to `ws hook pi`. It launches no agent and changes nothing in Pi's
// behaviour; a supervision that cannot be reached is ignored.
//
// Load it with `pi -e adapters/pi/ws-bridge.ts` or from `~/.pi/agent/extensions/`,
// with `WS_ROOT` set to the Work Supervision root (absolute path). `WS_BIN`
// names the `ws` program when it is not on `PATH`. Without `WS_ROOT` the
// extension does nothing.
//
// Written against the extension API of Pi 0.84.2; `BridgeApi` is the subset it
// uses, so the factory accepts Pi's `ExtensionAPI` without depending on it.

/** The bridge payload `ws hook pi` reads (`crates/work-supervision-cli/src/hook.rs`). */
export interface BridgePayload {
  event: BridgeEvent;
  session_id: string;
  cwd: string;
}

/** Pi events the bridge reports. */
export type BridgeEvent = "session_start" | "agent_start" | "agent_settled" | "session_shutdown";

/** What the bridge reads from Pi's extension context. */
export interface BridgeContext {
  cwd: string;
  sessionManager: { getSessionId(): string };
}

type Handler = (event: unknown, ctx: BridgeContext) => Promise<void>;

/** The subset of Pi's `ExtensionAPI` the bridge uses. */
export interface BridgeApi {
  on(event: "session_start", handler: Handler): void;
  on(event: "agent_start", handler: Handler): void;
  on(event: "agent_settled", handler: Handler): void;
  on(event: "session_shutdown", handler: Handler): void;
  exec(
    command: string,
    args: string[],
    options?: { timeout?: number },
  ): Promise<{ code: number; stderr: string }>;
}

/** The environment the bridge reads. */
export interface BridgeEnvironment {
  WS_ROOT?: string;
  WS_BIN?: string;
}

/** Longest a report may take: a hung supervision never holds Pi up for long. */
export const REPORT_TIMEOUT_MS = 5_000;

/** Builds the arguments of one `ws hook pi` call; the payload is the last one. */
export function hookArguments(root: string, payload: BridgePayload): string[] {
  return ["--root", root, "hook", "pi", JSON.stringify(payload)];
}

/** Installs the bridge on `pi`, reading `environment`. */
export function installBridge(pi: BridgeApi, environment: BridgeEnvironment): void {
  const root = environment.WS_ROOT;
  if (root === undefined || !root.startsWith("/")) {
    return;
  }
  const program = environment.WS_BIN ?? "ws";
  const report = (event: BridgeEvent): Handler => {
    return async (_event, ctx) => {
      const payload: BridgePayload = {
        event,
        session_id: ctx.sessionManager.getSessionId(),
        cwd: ctx.cwd,
      };
      try {
        await pi.exec(program, hookArguments(root, payload), { timeout: REPORT_TIMEOUT_MS });
      } catch {
        // The supervision is never allowed to fail the session it observes.
      }
    };
  };
  pi.on("session_start", report("session_start"));
  pi.on("agent_start", report("agent_start"));
  pi.on("agent_settled", report("agent_settled"));
  pi.on("session_shutdown", report("session_shutdown"));
}

/** Pi's extension entry point. */
export default function wsBridge(pi: BridgeApi): void {
  installBridge(pi, { WS_ROOT: process.env.WS_ROOT, WS_BIN: process.env.WS_BIN });
}
