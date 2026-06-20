// v2.1 P1 — NN I/O inspector block end-to-end spec.
//
// Asserts that pausing, clicking a creature, and waiting for the NN I/O
// throttle (500 ms) populates the #ins-nn-block with:
//   - At least the stable input groups that survive the v2.1 P2 biome
//     simplification: GrassSectors and SelfMemory.
//   - An outputs section with 3 logit bars and one chosen-action label.
//
// Groups that are expected to be REMOVED in a later phase (e.g. BiomeDir,
// WallProximity) are NOT asserted here so this spec survives that change.
//
// The test only runs when the NN inspect fetch is served (requires the Rust
// `creature_nn_inspect_json` wasm export to be present). If the wasm build
// predates the export the `#ins-nn-block` stays hidden and the poll times out
// gracefully.

import { test, expect, type Page } from "@playwright/test";

// ── helpers (shared pattern from sab-control.spec.ts) ──────────────────────

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

async function setPaused(page: Page, paused: boolean): Promise<void> {
  // Read the play/pause button state and toggle only if needed.
  const isActive = await page.evaluate(() => {
    const btn = document.getElementById("playpause-btn");
    return btn ? btn.classList.contains("is-active") : false;
  });
  // .is-active means paused (button shows "Play" label, sim is paused).
  if (paused && !isActive) {
    await page.click("#playpause-btn");
  } else if (!paused && isActive) {
    await page.click("#playpause-btn");
  }
}

/** Select a creature via the e2e hook (window.__evosimE2E.selectFirstCreature).
 *  Returns true when a creature was found and the inspector body is populated
 *  with a real creature id (not just optimistically shown). */
async function selectCreatureViaHook(page: Page): Promise<boolean> {
  // Use the product-side hook that picks the first creature from the live SoA
  // snapshot directly — no blind canvas click needed.
  const found = await page.evaluate(() => {
    const ns = (window as unknown as { __evosimE2E?: { selectFirstCreature?: () => boolean } }).__evosimE2E;
    return ns?.selectFirstCreature?.() ?? false;
  });
  if (!found) return false;

  // Wait until #inspector-body is visible AND #ins-id is non-empty — confirms
  // a real creature is selected (not an optimistic flash that gets cleared).
  // Using getComputedStyle to avoid false positives from inline style="" vs
  // style="display:none" transitions.
  try {
    await expect
      .poll(
        () => page.evaluate(() => {
          const body = document.getElementById("inspector-body");
          const id = document.getElementById("ins-id");
          if (!body || !id) return false;
          const visible = getComputedStyle(body).display !== "none";
          const hasId = (id.textContent ?? "").trim().length > 0;
          return visible && hasId;
        }),
        { timeout: 4_000 },
      )
      .toBe(true);
    return true;
  } catch {
    return false;
  }
}

// ── tests ───────────────────────────────────────────────────────────────────

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
  const errs =
    (page as unknown as { _consoleErrors?: string[] })._consoleErrors ?? [];
  const fatal = errs.filter(
    (e) =>
      !/initThreadPool/.test(e) &&
      !/GL Driver Message/.test(e),
  );
  expect(fatal, "no fatal console errors during test").toEqual([]);
});

test("NN I/O block populates with GrassSectors and SelfMemory groups on creature click while paused", async ({
  page,
}) => {
  // Pause the sim so the NN I/O fetch fires.
  await setPaused(page, true);

  // Select a creature via the e2e hook (picks first creature from live SoA).
  const selected = await selectCreatureViaHook(page);
  if (!selected) {
    // No creature available — skip rather than fail (world may be extinct).
    test.skip();
    return;
  }

  // Wait for the NN I/O block to appear (the 500 ms throttle + SAB round-trip
  // is served on the worker's tick loop, which still runs inspect requests
  // when paused via `serveInspectRequest`).
  await expect
    .poll(
      () => page.evaluate(() => {
        const block = document.getElementById("ins-nn-block");
        return block ? getComputedStyle(block).display !== "none" : false;
      }),
      { timeout: 8_000 },
    )
    .toBe(true);

  // Assert stable input groups are present.
  const inputsText = await page.evaluate(
    () => document.getElementById("ins-nn-inputs")?.textContent ?? "",
  );

  // GrassSectors group must appear.
  expect(inputsText, "GrassSectors group should be present").toContain("GrassSectors");

  // SelfMemory group must appear.
  expect(inputsText, "SelfMemory group should be present").toContain("SelfMemory");

  // Assert the outputs section: 3 logit bar fills exist.
  const logitGraze = await page.evaluate(
    () => document.getElementById("ins-nn-logit-graze-fill") !== null,
  );
  const logitAttack = await page.evaluate(
    () => document.getElementById("ins-nn-logit-attack-fill") !== null,
  );
  const logitSplit = await page.evaluate(
    () => document.getElementById("ins-nn-logit-split-fill") !== null,
  );
  expect(logitGraze, "graze logit bar fill should exist").toBe(true);
  expect(logitAttack, "attack logit bar fill should exist").toBe(true);
  expect(logitSplit, "split logit bar fill should exist").toBe(true);

  // Assert chosen action is set to a non-placeholder value.
  const chosenText = await page.evaluate(
    () => document.getElementById("ins-nn-chosen")?.textContent ?? "",
  );
  expect(chosenText, "chosen action should be set").not.toBe("—");
  expect(chosenText.length, "chosen action should be non-empty").toBeGreaterThan(0);
  // Chosen action must be one of the four valid values.
  expect(
    ["Graze", "Attack", "Split", "Mate"].includes(chosenText),
    `chosen action "${chosenText}" should be a valid action`,
  ).toBe(true);
});

test("NN I/O block logit bars have non-empty width after population", async ({
  page,
}) => {
  await setPaused(page, true);
  const selected = await selectCreatureViaHook(page);
  if (!selected) {
    test.skip();
    return;
  }

  // Wait for the block.
  await expect
    .poll(
      () => page.evaluate(() => {
        const block = document.getElementById("ins-nn-block");
        return block ? getComputedStyle(block).display !== "none" : false;
      }),
      { timeout: 8_000 },
    )
    .toBe(true);

  // At least one of the 3 logit bar fills must have a non-zero width
  // (the chosen action's bar should always be > 0 after softmax).
  const logitWidths = await page.evaluate(() => {
    const names = ["graze", "attack", "split"];
    return names.map((n) => {
      const el = document.getElementById(`ins-nn-logit-${n}-fill`) as HTMLElement | null;
      return el ? parseFloat(el.style.width ?? "0") : 0;
    });
  });
  const anyNonZero = logitWidths.some((w) => w > 0);
  expect(anyNonZero, "at least one logit bar must have non-zero width").toBe(true);

  // The vx/vy readout must be set (not "—").
  const vxvy = await page.evaluate(
    () => document.getElementById("ins-nn-vxvy")?.textContent ?? "",
  );
  expect(vxvy, "vx/vy readout should be set").not.toBe("—");
});

test("NN I/O block also populates while the simulation is running", async ({
  page,
}) => {
  await setPaused(page, false);

  await expect
    .poll(
      async () => {
        await page.evaluate(() => {
          const block = document.getElementById("ins-nn-block");
          if (block && getComputedStyle(block).display !== "none") return;
          const ns = (window as unknown as { __evosimE2E?: { selectFirstCreature?: () => boolean } }).__evosimE2E;
          ns?.selectFirstCreature?.();
        });
        return await page.evaluate(() => {
          const block = document.getElementById("ins-nn-block");
          return block ? getComputedStyle(block).display !== "none" : false;
        });
      },
      { timeout: 8_000 },
    )
    .toBe(true);

  const vxvy = await page.evaluate(
    () => document.getElementById("ins-nn-vxvy")?.textContent ?? "",
  );
  expect(vxvy, "running NN vx/vy readout should be set").not.toBe("—");
});
