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

  await simulateWorkerFreeze(page);

  await expect
    .poll(async () => workerState(page), { timeout: 8_000 })
    .toMatch(/^(stalled|restarting)$/);
  await expect
    .poll(async () => workerState(page), { timeout: 15_000 })
    .toBe("running");

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
