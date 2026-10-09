// Builds the native release binaries of Work Supervision with the machine-path
// remap of owner decision Y40, then fails if any of them still embeds a machine
// path: the scan of the bytes, not the exit code of cargo, is the proof.

import { spawnSync } from "node:child_process";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { assertNoMachinePaths, machinePathContext, rustflagOverrides } from "./machine-paths";

// Shipped binaries only: `ws-fake-agent` is a test fixture, never distributed.
const RELEASE_BINARIES = [
  { package: "work-supervision-cli", binary: "ws" },
  { package: "work-supervision-daemon", binary: "wsd" },
  { package: "work-supervision-cockpit", binary: "ws-cockpit" },
  { package: "work-supervision-journal-verifier", binary: "ws-journal-verify" },
] as const;

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const overrides = rustflagOverrides();
if (overrides.length > 0) {
  throw new Error(
    `Release build refuses rustflag overrides that would drop the path remap: ${overrides.join(", ")}`,
  );
}
const context = machinePathContext(repositoryRoot);
const result = spawnSync(
  "cargo",
  [
    "build",
    "--locked",
    "--release",
    ...RELEASE_BINARIES.flatMap(({ package: name, binary }) => ["-p", name, "--bin", binary]),
  ],
  { cwd: repositoryRoot, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
);
if (result.status !== 0) {
  process.stderr.write(result.stderr ?? "");
  throw new Error("release build failed");
}
const targetDirectory = resolve(repositoryRoot, "target/release");
await assertNoMachinePaths(
  context,
  RELEASE_BINARIES.map(({ binary }) => resolve(targetDirectory, binary)),
  (path) => relative(repositoryRoot, path),
);
