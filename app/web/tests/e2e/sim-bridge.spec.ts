// End-to-end smoke test for the main → sim-worker control path.
//
// Drives the actual UI (clicks the pause button, selects TPS values, fires
// input events on sliders, clicks the profile checkbox, presses "r" to
// restart). Verifies observable downstream consequences:
//   - Pause: tick counter stops within the deadline; resume restarts it.
//   - TPS:   observed ticks-per-second tracks the dropdown setting.
//   - Slider: the bridge does not reject the message + the change applies.
//   - Profile toggle: all 4 stacked tables populate.
//   - Restart "r": tick counter resets, no console errors.
//
// IMPORTANT: every test forces target-TPS=1000 before the interaction it
// cares about. That's the regime where the v1.6 Wave D loop's `timeoutMs`
// drops to 0, `Atomics.waitAsync(.., 0)` returns synchronously, and the
// loop spins without yielding to the event loop. A test at default TPS=60
// would have *passed* on the buggy commit because step_n(1) leaves a
// comfortably-positive timeoutMs and the await branch fires normally.
//
// If a future regression makes the loop yield-less, every test in this
// file should fail.

import { test, expect, type Page } from "@playwright/test";

async function readStatus(page: Page): Promise<string> {
  // v1.13 Wave 2 removed the top-bar `#status` span; the live status line is
  // now `#perf-status-line` in the bottom perf panel (perf-panel.ts), updated
  // once per painted frame as "seed: … · tick N · pop M · TPS · FPS".
  return await page.evaluate(
    () => document.getElementById("perf-status-line")?.textContent ?? "",
  );
}

async function readTick(page: Page): Promise<number> {
  const txt = await readStatus(page);
  const m = /tick (\d+)/.exec(txt);
  return m ? Number(m[1]) : NaN;
}

async function readPopulation(page: Page): Promise<number> {
  const txt = await readStatus(page);
  const m = /pop (\d+)/.exec(txt);
  return m ? Number(m[1]) : NaN;
}

async function waitForBoot(page: Page): Promise<void> {
  // The status line flips to "seed: … · tick N · pop M …" once boot_ready
  // lands and the first SAB snapshot is read.
  await expect
    .poll(async () => (await readStatus(page)).match(/tick \d+/) !== null, {
      timeout: 15_000,
    })
    .toBe(true);
}

// v1.13 moved the TPS control from a top-bar `<select id="target-tps-input">`
// to the perf-panel numeric `<input id="target-tps-input">` (perf-panel.ts).
// `selectOption` is wrong for an <input>; set the value and dispatch the
// "change" event the widget listens for so the TPS actually applies.
async function setTargetTps(page: Page, tps: number): Promise<void> {
  await page.evaluate((v) => {
    const el = document.getElementById("target-tps-input") as
      | HTMLInputElement
      | null;
    if (!el) throw new Error("#target-tps-input not found");
    el.value = String(v);
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }, tps);
}

// v2.1 P4 follow-up: the profiler lives in a top-level Profiler rail tab.
async function ensureProfilerVisible(page: Page): Promise<void> {
  const collapsed = await page.evaluate(
    () => document.getElementById("app-shell")?.classList.contains("rail-collapsed") ?? true,
  );
  if (collapsed) {
    await page.locator("body").focus();
    await page.keyboard.press("~");
  }
  await page.evaluate(() => {
    const btn = document.querySelector<HTMLButtonElement>('.rail-tab[data-tab="profiler"]');
    if (btn && !btn.classList.contains("is-active")) btn.click();
  });
  await expect(page.locator("#rail-profiler")).toBeVisible();
  await expect(page.locator("#settings-profiler-pane")).toBeVisible();
}

// Settings holds the staged sliders + the `#settings-apply` footer. The rail
// defaults to General, so explicitly switch to Settings after opening it.
async function ensureSettingsRailOpen(page: Page): Promise<void> {
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
  await expect(page.locator("#settings-apply")).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  const consoleErrors: string[] = [];
  page.on("pageerror", (e) => consoleErrors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") consoleErrors.push(`console.error: ${m.text()}`);
  });
  (page as unknown as { _consoleErrors: string[] })._consoleErrors =
    consoleErrors;
  await page.goto("/");
  await waitForBoot(page);
});

test.afterEach(async ({ page }) => {
  const errs =
    (page as unknown as { _consoleErrors?: string[] })._consoleErrors ?? [];
  // initThreadPool warning is OK on non-isolated browsers; everything else fails.
  const fatal = errs.filter(
    (e) => !/initThreadPool/.test(e) && !/GL Driver Message/.test(e),
  );
  expect(fatal, "no fatal console errors during test").toEqual([]);
});

test("fresh default world stays alive below the population cap during startup", async ({
  page,
}) => {
  await setTargetTps(page, 1000);
  await page.waitForTimeout(3000);

  const tick = await readTick(page);
  const pop = await readPopulation(page);
  expect(tick, "default world should keep ticking after boot").toBeGreaterThan(50);
  expect(pop, "default world should still have living creatures").toBeGreaterThan(0);
  expect(pop, "default world should remain below the default population cap").toBeLessThan(8000);
});

test("pause + resume — stops and starts the tick counter at high TPS", async ({
  page,
}) => {
  // Push target-TPS up so the sim under-runs its per-tick budget and the loop
  // hits the timeoutMs<=0 path. This is the bug regime; a test at default 60
  // would have passed on the buggy commit.
  await setTargetTps(page, 1000);
  // Let the worker pick up the new TPS.
  await page.waitForTimeout(500);

  const beforePause = await readTick(page);
  await page.click("#playpause-btn");
  // Sample the moment we paused so the "running ticks observed after pause"
  // delta is measured from a known stop point.
  await page.waitForTimeout(300);
  const atPause = await readTick(page);
  await page.waitForTimeout(1500);
  const afterPaused = await readTick(page);

  // Within ~300 ms of the click, ticks should have stopped advancing.
  // Allow a small grace (≤ 5) for one in-flight step_n(1) after the click.
  expect(
    afterPaused - atPause,
    `tick should not advance while paused (was ${atPause}, now ${afterPaused})`,
  ).toBeLessThanOrEqual(5);

  // Now resume and confirm we start advancing again.
  await page.click("#playpause-btn");
  await page.waitForTimeout(800);
  const afterResume = await readTick(page);
  expect(
    afterResume - afterPaused,
    `tick should advance after resume (was ${afterPaused}, now ${afterResume})`,
  ).toBeGreaterThan(10);

  // sanity: the whole sequence saw forward progress overall
  expect(afterResume).toBeGreaterThan(beforePause);
});

test("target TPS dropdown — observed throughput tracks the selected value", async ({
  page,
}) => {
  // Start at the broken regime, then drop to 10 — if onmessage stops firing,
  // the worker stays at TPS=1000 and we'd see ~2000 ticks in 2 s instead of
  // ~20.
  await setTargetTps(page, 1000);
  await page.waitForTimeout(1000);

  await setTargetTps(page, 10);
  // Allow up to 1 s for the message to apply.
  await page.waitForTimeout(1000);

  const t0 = await readTick(page);
  await page.waitForTimeout(2000);
  const t1 = await readTick(page);
  const dticks = t1 - t0;

  // 10 TPS × 2 s = 20 ticks (with some tolerance for boot/timing slop).
  // The bug would leave us at 1000 TPS → hundreds of ticks.
  expect(
    dticks,
    `at TPS=10 expect ~20 ticks in 2 s, got ${dticks}`,
  ).toBeLessThan(100);
  expect(dticks).toBeGreaterThan(5);
});

test("slider change — devpanel slider input fires without rejection at high TPS", async ({
  page,
}) => {
  // v2.0: the Settings rail is collapsed by default — open it so the staged
  // slider + #settings-apply footer are visible/clickable.
  // Same as above: stress-regime so a failing yield manifests.
  await setTargetTps(page, 1000);
  await page.waitForTimeout(500);
  await ensureSettingsRailOpen(page);

  // Edit the "Basic upkeep" row (Rust name: upkeep_multiplier). Sliders now
  // stage; Apply fires the debounced set_slider postMessage.
  const status1 = await page.evaluate(async () => {
    const rows = Array.from(document.querySelectorAll(".devpanel-row"));
    const row = rows.find(
      (r) => r.querySelector("label")?.textContent === "Basic upkeep",
    );
    if (!row) return "row-not-found";
    const num = row.querySelector(
      'input[type="number"]',
    ) as HTMLInputElement | null;
    if (!num) return "input-not-found";
    num.value = "1.75";
    num.dispatchEvent(new Event("input", { bubbles: true }));
    return "ok";
  });
  expect(status1).toBe("ok");

  // Apply commits the staged change.
  await page.click("#settings-apply");
  await page.waitForTimeout(400);

  // If the message arrived, no `[sim] set_slider("upkeep_multiplier", …)
  // rejected` warning should appear in the page console (the bridge wraps
  // wasm errors that way).
  const errs =
    (page as unknown as { _consoleErrors: string[] })._consoleErrors;
  expect(errs.some((e) => /set_slider.*rejected/i.test(e))).toBe(false);

  // Stronger: prove the message path is still functional AFTER the slider
  // burst by issuing a pause and verifying the worker actually pauses. The
  // buggy commit would still pass the "no rejection" check above (the
  // message is silently dropped, not rejected); this follow-up catches it.
  const beforePause = await readTick(page);
  await page.click("#playpause-btn");
  await page.waitForTimeout(800);
  const afterPause = await readTick(page);
  await page.waitForTimeout(800);
  const afterPause2 = await readTick(page);
  expect(
    afterPause2 - afterPause,
    `slider burst should not have wedged the message path; pause must work (before=${beforePause}, after=${afterPause}, after2=${afterPause2})`,
  ).toBeLessThanOrEqual(5);
});

test("profile toggle — all 4 trees populate within 4 s", async ({ page }) => {
  await setTargetTps(page, 1000);
  await page.waitForTimeout(500);

  // v1.9.1: the Rust profiler is always-on (the worker enables it at boot).
  // v2.1 P4 follow-up: profiler lives in the top-level Profiler rail tab.
  // Navigate there so the profiler-trees assertions can find the DOM.
  await ensureProfilerVisible(page);

  // Profiler polls at 1 Hz and needs a few samples to populate. 4 s gives
  // enough cushion even on a slow CI box.
  await expect
    .poll(
      async () => {
        return await page.evaluate(() => {
          const root = document.getElementById("profiler-trees");
          if (!root) return 0;
          return root.querySelectorAll(".profiler-tree-section").length;
        });
      },
      { timeout: 6_000 },
    )
    .toBeGreaterThanOrEqual(4);

  // Confirm at least one tree has real samples — the four sections render
  // an "(no samples yet)" placeholder when empty.
  const text = await page.evaluate(
    () => document.getElementById("profiler-trees")?.innerText ?? "",
  );
  expect(/tick/.test(text)).toBe(true);
  expect(/frame/.test(text)).toBe(true);
});

test("restart 'r' key — tick counter resets, no console errors", async ({
  page,
}) => {
  await setTargetTps(page, 1000);
  await page.waitForTimeout(1000);

  const beforeRestart = await readTick(page);
  expect(beforeRestart).toBeGreaterThan(0);

  // Ensure focus is on the body so "r" isn't swallowed by an input.
  await page.evaluate(() => (document.activeElement as HTMLElement)?.blur?.());
  await page.locator("body").focus();
  await page.keyboard.press("r");

  // Wait long enough for spawnSimWorker + boot_ready + first tick.
  await expect
    .poll(async () => readTick(page), { timeout: 8_000 })
    .toBeLessThan(beforeRestart);
});
