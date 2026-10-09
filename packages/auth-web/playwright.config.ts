import { defineConfig, devices } from "@playwright/test";

const HOST = "127.0.0.1";
const PORT = 4187;

export default defineConfig({
  expect: { timeout: 5_000 },
  fullyParallel: false,
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    {
      name: "firefox",
      use: {
        ...devices["Desktop Firefox"],
        // TODO(playwright-1.64): remove once @playwright/test >= 1.64.0 (microsoft/playwright#42731) — Juggler loses a message on COOP-triggered context replacement.
        launchOptions: {
          firefoxUserPrefs: { "browser.tabs.remote.useCrossOriginOpenerPolicy": false },
        },
      },
    },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  testDir: "./e2e",
  testMatch: /.*\.e2e\.ts/,
  use: {
    baseURL: `https://${HOST}:${PORT}`,
    ignoreHTTPSErrors: true,
    trace: "retain-on-failure",
  },
  webServer: {
    command: "bun e2e/serve-e2e.ts",
    env: { HOST, PORT: String(PORT) },
    ignoreHTTPSErrors: true,
    reuseExistingServer: false,
    timeout: 30_000,
    url: `https://${HOST}:${PORT}/`,
  },
  workers: 1,
});
