// v2.0.5 S4: grass_size restart gate.
//
// Proves that changing the `grass_size` construction setting and restarting
// produces a world with a different (and correctly-sized) `grass_dim`. This
// closes the question: "is grass_size actually applied on restart, or does the
// boot path ignore it?"
//
// Observable: the `boot_ready` message carries `grass_dim` (cells per axis).
// At default cell_size=20 and world_size=9600: grass_dim = round(9600/20) = 480.
// At cell_size=10:                              grass_dim = round(9600/10) = 960.
// We intercept the Worker.onmessage to capture successive grass_dim values.
//
// Root-cause recap (proved by this spec):
//   The prior inert-grass_size symptom was that `newWithFounderCount` never
//   received a `grass_cell_size` argument — it used `..Default::default()` for
//   all un-listed DevSliders fields, including grass_cell_size. The fix
//   (v2.0.4 S2) routes grass_size through `initial_sliders` (name→value map)
//   which the worker applies via `world.set_slider("grass_size", value)`.
//   Because `grass_size` is now in `CONSTRUCTION_ONLY_SLIDERS`, that slider
//   call runs BEFORE the World is constructed (actually it stores the value in
//   `inner.sliders.grass_cell_size`), and the construction call
//   `World::new_with_sliders_topology` reads `sliders.grass_cell_size` when
//   computing `WorldDims`. Hence grass_dim now differs on restart.
//
// What this spec asserts:
//   (a) Default boot: grass_dim = 480 (cell_size=20, world_size=9600).
//   (b) After changing grass_size to 10 and restarting: grass_dim = 960.
//   (c) grass_dim(a) ≠ grass_dim(b) — the change is observable end-to-end.

import { test, expect, type Page } from "@playwright/test";

// ── Helpers ───────────────────────────────────────────────────────────────────

async function readStatus(page: Page): Promise<string> {
  return await page.evaluate(
    () => document.getElementById("perf-status-line")?.textContent ?? "",
  );
}

async function waitForBoot(page: Page): Promise<void> {
  await expect
    .poll(async () => (await readStatus(page)).match(/tick \d+/) !== null, {
      timeout: 25_000,
    })
    .toBe(true);
}

async function waitForBootReady(page: Page, afterCount: number): Promise<void> {
  // Wait until __bootReadyCount > afterCount (i.e. a new boot_ready was captured).
  await expect
    .poll(
      async () =>
        await page.evaluate(
          () =>
            (window as unknown as Record<string, unknown>).__bootReadyCount as
              | number
              | undefined ?? 0,
        ),
      { timeout: 20_000 },
    )
    .toBeGreaterThan(afterCount);
}

// ── Test ──────────────────────────────────────────────────────────────────────

test("grass_dim changes on restart when grass_size is changed", async ({ page }) => {
  // Collect console errors.
  const consoleErrors: string[] = [];
  page.on("pageerror", (e) => consoleErrors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") consoleErrors.push(`console.error: ${m.text()}`);
  });

  // Intercept Worker.onmessage to capture grass_dim from boot_ready.
  // We record the sequence of grass_dim values from successive boots so we can
  // compare before-restart vs after-restart.
  await page.addInitScript(() => {
    const w = window as unknown as Record<string, unknown>;
    w.__bootReadyGrassDims = [] as number[];
    w.__bootReadyCount = 0;

    const proto = Worker.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(proto, "onmessage");
    if (!descriptor) return;
    const origSet = descriptor.set!;
    Object.defineProperty(proto, "onmessage", {
      ...descriptor,
      set(fn: ((ev: MessageEvent) => void) | null) {
        if (typeof fn !== "function") {
          origSet.call(this, fn);
          return;
        }
        const wrapped = (ev: MessageEvent) => {
          const data = ev?.data;
          if (data && data.kind === "boot_ready" && typeof data.grass_dim === "number") {
            (w.__bootReadyGrassDims as number[]).push(data.grass_dim);
            (w.__bootReadyCount as number);
            w.__bootReadyCount = (w.__bootReadyCount as number) + 1;
          }
          fn(ev);
        };
        origSet.call(this, wrapped);
      },
    });
  });

  // ── 1. Navigate and wait for first boot ──────────────────────────────────
  await page.goto("/");
  await waitForBoot(page);
  await waitForBootReady(page, 0); // wait for first boot_ready

  const grassDimAfterFirstBoot = await page.evaluate(
    () => {
      const dims = (window as unknown as Record<string, unknown>).__bootReadyGrassDims as number[];
      return dims[dims.length - 1] ?? null;
    },
  );
  console.log(`[grass-size-restart] first boot grass_dim=${grassDimAfterFirstBoot}`);

  // (a) At default cell_size=20, world_size=9600: grass_dim = round(9600/20) = 480.
  expect(
    grassDimAfterFirstBoot,
    "first boot grass_dim must be 480 (default cell_size=20, world_size=9600)",
  ).toBe(480);

  // ── 2. Open Settings, change grass_size to 10, Apply, Restart ────────────
  // Open the rail and switch to Settings.
  await page.locator("body").focus();
  await page.keyboard.press("~");
  await page.evaluate(() => {
    const btn = document.querySelector<HTMLButtonElement>('.rail-tab[data-tab="settings"]');
    if (btn && !btn.classList.contains("is-active")) btn.click();
  });
  await expect(page.locator("#settings-apply")).toBeVisible();

  // Find the "Grass cell size" slider row by its label text and set value to 10.
  const changeStatus = await page.evaluate(() => {
    const rows = Array.from(document.querySelectorAll(".devpanel-row"));
    const row = rows.find(
      (r) => r.querySelector("label")?.textContent?.trim() === "Grass cell size",
    );
    if (!row) return "row-not-found";
    const numInput = row.querySelector(
      'input[type="number"]',
    ) as HTMLInputElement | null;
    if (!numInput) return "input-not-found";
    numInput.value = "10";
    numInput.dispatchEvent(new Event("input", { bubbles: true }));
    return "ok";
  });
  expect(changeStatus, "grass_size row must be found and editable").toBe("ok");

  // Apply the staged change.
  await page.click("#settings-apply");
  await page.waitForTimeout(200);

  // Restart (press 'r').
  const bootCountBeforeRestart = await page.evaluate(
    () => (window as unknown as Record<string, unknown>).__bootReadyCount as number ?? 0,
  );
  await page.evaluate(() => (document.activeElement as HTMLElement)?.blur?.());
  await page.locator("body").focus();
  await page.keyboard.press("r");

  // Wait for the new worker's boot_ready.
  await waitForBootReady(page, bootCountBeforeRestart);
  // Also wait for the tick counter to reset (confirms the new world is running).
  await expect
    .poll(async () => {
      const txt = await readStatus(page);
      const m = /tick (\d+)/.exec(txt);
      return m ? Number(m[1]) : NaN;
    }, { timeout: 15_000 })
    .toBeGreaterThan(0);

  const grassDimAfterRestart = await page.evaluate(
    () => {
      const dims = (window as unknown as Record<string, unknown>).__bootReadyGrassDims as number[];
      return dims[dims.length - 1] ?? null;
    },
  );
  console.log(`[grass-size-restart] post-restart grass_dim=${grassDimAfterRestart}`);

  // (b) At cell_size=10, world_size=9600: grass_dim = round(9600/10) = 960.
  expect(
    grassDimAfterRestart,
    "post-restart grass_dim must be 960 (cell_size=10, world_size=9600)",
  ).toBe(960);

  // (c) The two dims must differ.
  expect(
    grassDimAfterRestart,
    "grass_dim must change after changing grass_size and restarting",
  ).not.toBe(grassDimAfterFirstBoot);

  // Final console error check.
  const fatal = consoleErrors.filter(
    (e) => !/initThreadPool/.test(e) && !/GL Driver Message/.test(e),
  );
  expect(fatal, "no fatal console errors during test").toEqual([]);
});
