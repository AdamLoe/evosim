import { test, expect, type Page } from "@playwright/test";

async function readStatus(page: Page): Promise<string> {
  return await page.evaluate(
    () => document.getElementById("perf-status-line")?.textContent ?? "",
  );
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

async function ensureDisplaySettings(page: Page): Promise<void> {
  const collapsed = await page.evaluate(
    () =>
      document.getElementById("app-shell")?.classList.contains("rail-collapsed") ??
      true,
  );
  if (collapsed) {
    await page.locator("body").focus();
    await page.keyboard.press("~");
  }
  await page.evaluate(() => {
    const btn = document.querySelector<HTMLButtonElement>('.rail-tab[data-tab="settings"]');
    if (btn && !btn.classList.contains("is-active")) btn.click();
  });
  await expect(page.locator("#rail-settings")).toBeVisible();
  await page.evaluate(() => {
    document.querySelector<HTMLButtonElement>('.settings-cat-btn[data-cat="display"]')?.click();
  });
  await expect(page.locator("#settings-display-pane")).toBeVisible();
}

async function readSeqs(page: Page): Promise<{ seq: number; consumed: number; fps: number }> {
  return await page.evaluate(() => {
    const ns = (window as unknown as {
      __evosimE2E?: {
        getSnapshotSeq?: () => number;
        getConsumedSeq?: () => number;
        getAppFPS?: () => number;
      };
    }).__evosimE2E;
    if (!ns?.getSnapshotSeq || !ns.getConsumedSeq || !ns.getAppFPS) {
      throw new Error("missing app FPS e2e hooks");
    }
    return {
      seq: ns.getSnapshotSeq(),
      consumed: ns.getConsumedSeq(),
      fps: ns.getAppFPS(),
    };
  });
}

function parseTps(status: string): number {
  const m = /·\s+(\d+) TPS/.exec(status);
  return m ? Number(m[1]) : NaN;
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
    (e) => !/initThreadPool/.test(e) && !/GL Driver Message/.test(e),
  );
  expect(fatal, "no fatal console errors during test").toEqual([]);
});

test("Display exposes exact app FPS choices and persists selection", async ({ page }) => {
  await ensureDisplaySettings(page);
  const options = await page.locator("#app-fps-select option").evaluateAll((opts) =>
    opts.map((o) => (o as HTMLOptionElement).value),
  );
  expect(options).toEqual(["15", "30", "60", "120"]);
  await expect(page.locator("#app-fps-select")).toHaveValue("60");

  await page.selectOption("#app-fps-select", "15");
  await expect
    .poll(async () => (await readSeqs(page)).fps, { timeout: 2_000 })
    .toBe(15);

  await page.reload();
  await waitForBoot(page);
  await ensureDisplaySettings(page);
  await expect(page.locator("#app-fps-select")).toHaveValue("15");
});

test("high target TPS publishes snapshots near configured app FPS", async ({ page }) => {
  await ensureDisplaySettings(page);
  await page.selectOption("#app-fps-select", "15");
  await setTargetTps(page, 1000);

  await expect
    .poll(async () => (await readSeqs(page)).fps, { timeout: 2_000 })
    .toBe(15);
  await page.waitForTimeout(500);

  const start = await readSeqs(page);
  await page.waitForTimeout(2200);
  const end = await readSeqs(page);
  const delta = end.seq - start.seq;

  expect(delta, `snapshot seq delta over 2.2s at 15 FPS was ${delta}`).toBeGreaterThan(12);
  expect(delta, `snapshot seq delta over 2.2s at 15 FPS was ${delta}`).toBeLessThan(50);
  expect(end.seq - end.consumed, "worker should keep at most one unconsumed snapshot").toBeLessThanOrEqual(1);

  const tps = parseTps(await readStatus(page));
  expect(tps, `reported TPS should stay above app FPS, got ${tps}`).toBeGreaterThan(15);
});
