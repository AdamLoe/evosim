import { test, expect, type Page } from "@playwright/test";

type WorkerState =
  | "booting"
  | "running"
  | "paused"
  | "stalled"
  | "crashed"
  | "restarting"
  | "failed";

interface E2EHarness {
  getWorkerState?: () => WorkerState;
  simulateWorkerCrash?: () => void;
  simulateWorkerFreeze?: () => void;
}

async function readStatus(page: Page): Promise<string> {
  return await page.evaluate(
    () => document.getElementById("perf-status-line")?.textContent ?? "",
  );
}

async function readTick(page: Page): Promise<number> {
  const txt = await readStatus(page);
  const m = /tick (\d+)/.exec(txt);
  return m ? Number(m[1]) : NaN;
}

async function waitForBoot(page: Page): Promise<void> {
  await expect
    .poll(async () => (await readStatus(page)).match(/tick \d+/) !== null, {
      timeout: 15_000,
    })
    .toBe(true);
}

async function setTargetTps(page: Page, tps: number): Promise<void> {
  await page.evaluate((v) => {
    const el = document.getElementById("target-tps-input") as HTMLInputElement | null;
    if (!el) throw new Error("#target-tps-input not found");
    el.value = String(v);
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }, tps);
}

async function workerState(page: Page): Promise<WorkerState> {
  return await page.evaluate(() => {
    const ns = (window as unknown as { __evosimE2E?: E2EHarness }).__evosimE2E;
    const state = ns?.getWorkerState?.();
    if (!state) throw new Error("worker-state e2e hook not installed");
    return state;
  });
}

async function simulateWorkerCrash(page: Page): Promise<void> {
  await page.evaluate(() => {
    const ns = (window as unknown as { __evosimE2E?: E2EHarness }).__evosimE2E;
    if (!ns?.simulateWorkerCrash) throw new Error("worker-crash e2e hook not installed");
    ns.simulateWorkerCrash();
  });
}

async function simulateWorkerFreeze(page: Page): Promise<void> {
  await page.evaluate(() => {
    const ns = (window as unknown as { __evosimE2E?: E2EHarness }).__evosimE2E;
    if (!ns?.simulateWorkerFreeze) throw new Error("worker-freeze e2e hook not installed");
    ns.simulateWorkerFreeze();
  });
}

test.beforeEach(async ({ page }) => {
  const consoleErrors: string[] = [];
  page.on("pageerror", (e) => consoleErrors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") consoleErrors.push(`console.error: ${m.text()}`);
  });
  (page as unknown as { _consoleErrors: string[] })._consoleErrors = consoleErrors;
  await page.goto("/");
  await waitForBoot(page);
});

test.afterEach(async ({ page }) => {
  const errs = (page as unknown as { _consoleErrors?: string[] })._consoleErrors ?? [];
  const fatal = errs.filter(
    (e) =>
      !/initThreadPool/.test(e) &&
      !/GL Driver Message/.test(e) &&
      !/simulated worker crash/.test(e),
  );
  expect(fatal, "no fatal console errors during watchdog test").toEqual([]);
});

test("simulated worker crash is detected and recovered", async ({ page }) => {
  await setTargetTps(page, 1000);
  await page.waitForTimeout(300);

  await simulateWorkerCrash(page);

  await expect
    .poll(async () => workerState(page), { timeout: 5_000 })
    .not.toBe("running");
  await expect
    .poll(async () => workerState(page), { timeout: 15_000 })
    .toBe("running");

  const recoveredTick = await readTick(page);
  await expect
    .poll(async () => readTick(page), { timeout: 5_000 })
    .toBeGreaterThan(recoveredTick + 10);
  await expect(page.locator("#worker-retry-btn")).toBeHidden();
});

test("simulated unpaused worker freeze is detected and recovered", async ({ page }) => {
  await setTargetTps(page, 1000);
  await page.waitForTimeout(300);

  // Install a freeze-after-boot fault on the next worker and trigger a restart.
  // The spawned frozen worker (W2) boots (tick=1), posts boot_ready, then parks
  // inside Atomics.wait on its own fresh SAB — it never enters simLoop.  From
  // the main thread's perspective W2 looks alive until the stall watchdog fires
  // after WORKER_STALL_TIMEOUT_MS (~3.5 s).  The watchdog spawns a clean
  // replacement (W3) whose simLoop advances tick beyond 1.
  //
  // The recovery cycle can be faster than the Playwright CDP poll interval
  // (~100 ms): the watchdog fires, W3 boots, and W3 is already "running" before
  // the next poll.  A two-step "not running" → "running" sequence would miss
  // the transient entirely in that case.
  //
  // Instead, poll a single combined condition: workerState="running" AND
  // tick > 1.  W2 is frozen at tick=1, so tick > 1 is only true once W3 is
  // running its simLoop.  Timeout covers the full cycle:
  //   W2 boot (~0.5 s) + WORKER_STALL_TIMEOUT_MS (3.5 s) + W3 boot (~0.5 s) + margin.
  await simulateWorkerFreeze(page);

  await expect
    .poll(
      async () => {
        const state = await workerState(page);
        if (state !== "running") return false;
        const tick = await readTick(page);
        // W2 is frozen at tick 1; tick > 1 confirms the replacement worker is live.
        return tick > 1;
      },
      { timeout: 15_000 },
    )
    .toBe(true);

  // The replacement worker is live and already ticking.  Verify simLoop continues
  // to advance (not just a single one-shot tick from boot).
  const recoveredTick = await readTick(page);
  await expect
    .poll(async () => readTick(page), { timeout: 5_000 })
    .toBeGreaterThan(recoveredTick + 10);
});

test("paused worker does not trip the stall watchdog", async ({ page }) => {
  await setTargetTps(page, 1000);
  await page.waitForTimeout(300);

  await page.click("#playpause-btn");
  await expect
    .poll(async () => workerState(page), { timeout: 3_000 })
    .toBe("paused");

  const pausedTick = await readTick(page);
  await page.waitForTimeout(4_500);
  const afterWaitTick = await readTick(page);
  expect(afterWaitTick - pausedTick).toBeLessThanOrEqual(5);
  await expect(workerState(page)).resolves.toBe("paused");
  await expect(page.locator("#worker-retry-btn")).toBeHidden();
});
