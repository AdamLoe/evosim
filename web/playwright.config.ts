import { defineConfig, devices } from "@playwright/test";

// Playwright config for the evosim e2e suite.
//
// The suite drives the dev server (Vite) and exercises the main → sim-worker
// control path: pause / TPS / sliders / profile toggle / restart. It exists
// to catch any regression to the v1.6 Wave D async loop's "yield to the
// event loop so onmessage fires" property — the bug class that has bitten
// us twice now (synchronous Atomics.wait in v1.6 plan-review C1, and
// Atomics.waitAsync returning sync on timeout=0 in the post-v1.7 patch).
//
// Browser binary is the pinned one already cached on disk
// (~/.cache/ms-playwright/chromium-1223) so CI doesn't need to download
// anything. If you need a fresh browser, run `npx playwright install chromium`.

export default defineConfig({
  testDir: "./tests/e2e",
  // Tests are throughput- not parallelism-sensitive; running serially keeps
  // the single Vite dev server happy and matches how a human would interact.
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "line" : "list",
  timeout: 60_000,
  use: {
    baseURL: "http://localhost:47821",
    headless: true,
    // Trace on first retry only — keeps CI artifacts small.
    trace: "on-first-retry",
  },
  webServer: {
    command: "pnpm dev",
    url: "http://localhost:47821/",
    reuseExistingServer: true,
    timeout: 30_000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
