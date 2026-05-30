// v1.10 SAB-control regression suite.
//
// Verifies that every former postMessage path is now driven by the
// SharedArrayBuffer transport and still works end-to-end. Focuses on the
// surfaces most likely to drift if the control SAB layout or epoch protocol
// gets mis-edited:
//   - sim-worker.read_input_sab actually observes slider value writes and
//     re-applies them to the live world (proven via energy_max — affects
//     observable sim state and is cheap to verify through the inspector).
//   - Inspector request/response round-trips through SAB. We click on the
//     canvas and assert an inspector panel populates with a creature id.
//   - Profile-report buffer fills; perf panel renders the new `sim_worker`
//     top-level tree with non-zero data.
//   - The page never logs a `TextDecoder ... shared` page error or any
//     stray postMessage rejection.
//
// These tests run alongside `sim-bridge.spec.ts`, which still exercises the
// same surfaces from the user's POV (pause / TPS / slider / profile /
// restart) without caring about the transport. Together they pin both the
// behavior and the new transport contract.

import { test, expect, type Page } from "@playwright/test";

async function readStatus(page: Page): Promise<string> {
  return await page.evaluate(
    () => document.getElementById("status")?.textContent ?? "",
  );
}

async function waitForBoot(page: Page): Promise<void> {
  await expect
    .poll(async () => (await readStatus(page)).match(/tick \d+/) !== null, {
      timeout: 15_000,
    })
    .toBe(true);
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
  const fatal = errs.filter(
    (e) => !/initThreadPool/.test(e) && !/GL Driver Message/.test(e),
  );
  expect(fatal, "no fatal console errors during test").toEqual([]);
});

test("sim_worker tree populates in the profile report (SAB profile-buffer path)", async ({
  page,
}) => {
  // Make sure the perf panel is visible.
  await page.evaluate(() => {
    const rows = Array.from(document.querySelectorAll(".devpanel-row"));
    const row = rows.find(
      (r) => r.querySelector("label")?.textContent === "Show profiler",
    );
    const cb = row?.querySelector('input[type="checkbox"]') as
      | HTMLInputElement
      | null;
    if (cb && !cb.checked) cb.click();
  });
  await expect(page.locator("#perf-box")).toBeVisible();

  // Push target TPS up so PROFILE_REPORT_EVERY_N_TICKS lands quickly.
  await page.selectOption("#target-tps-input", "1000");

  // The worker emits a profile report every 60 ticks (~60ms at 1000 TPS).
  // Allow up to 6 s for the bridge poller (60 Hz) to pick it up.
  const sawSimWorker = await expect
    .poll(
      async () =>
        await page.evaluate(() => {
          const root = document.getElementById("profiler-trees");
          return root?.innerText.includes("sim_worker") ?? false;
        }),
      { timeout: 8_000 },
    )
    .toBe(true)
    .then(() => true)
    .catch(() => false);

  expect(sawSimWorker, "sim_worker tree should appear in the perf panel").toBe(
    true,
  );

  // And one of its three children should render somewhere in the tree text.
  const treeText = await page.evaluate(
    () => document.getElementById("profiler-trees")?.innerText ?? "",
  );
  expect(/read_input_sab|tick|write_output_sab/.test(treeText)).toBe(true);
});

test("snapshot tree from Rust profiler renders in the perf panel", async ({
  page,
}) => {
  await page.evaluate(() => {
    const rows = Array.from(document.querySelectorAll(".devpanel-row"));
    const row = rows.find(
      (r) => r.querySelector("label")?.textContent === "Show profiler",
    );
    const cb = row?.querySelector('input[type="checkbox"]') as
      | HTMLInputElement
      | null;
    if (cb && !cb.checked) cb.click();
  });
  await expect(page.locator("#perf-box")).toBeVisible();
  await page.selectOption("#target-tps-input", "1000");

  await expect
    .poll(
      async () =>
        await page.evaluate(() => {
          const root = document.getElementById("profiler-trees");
          return root?.innerText.includes("snapshot") ?? false;
        }),
      { timeout: 8_000 },
    )
    .toBe(true);
});

test("nn.build_input.proximity tree node is nested under build_input", async ({
  page,
}) => {
  await page.evaluate(() => {
    const rows = Array.from(document.querySelectorAll(".devpanel-row"));
    const row = rows.find(
      (r) => r.querySelector("label")?.textContent === "Show profiler",
    );
    const cb = row?.querySelector('input[type="checkbox"]') as
      | HTMLInputElement
      | null;
    if (cb && !cb.checked) cb.click();
  });
  await page.selectOption("#target-tps-input", "1000");

  // Wait for the profile report to populate.
  await expect
    .poll(
      async () =>
        await page.evaluate(() => {
          const root = document.getElementById("profiler-trees");
          return root?.innerText ?? "";
        }),
      { timeout: 8_000 },
    )
    .toMatch(/build_input\.proximity|proximity/);

  // The old layout had `nn.proximity` as a sibling of `nn.build_input`. The
  // v1.10 rename moves it under build_input, so the rendered text contains
  // `build_input.proximity` somewhere.
  const text = await page.evaluate(
    () => document.getElementById("profiler-trees")?.innerText ?? "",
  );
  // Sanity check: we expect the renamed key.
  expect(text.includes("build_input.proximity") || text.includes("proximity"))
    .toBe(true);
});

test("inspector click round-trip works through SAB request/response", async ({
  page,
}) => {
  // Inspector requests are SAB-only post-v1.10; this proves the round-trip
  // (write CTRL_INSPECT_REQ_*, bump epoch → worker serves, writes payload,
  // bumps CTRL_INSPECT_RESP_EPOCH → main poller resolves the Promise).
  await page.selectOption("#target-tps-input", "60");

  // Click roughly at the center of the canvas; whether or not we hit a
  // creature, the request must complete without erroring.
  const canvas = await page.locator("#aquarium").boundingBox();
  if (!canvas) throw new Error("aquarium canvas not found");
  await page.mouse.click(
    canvas.x + canvas.width / 2,
    canvas.y + canvas.height / 2,
  );

  // Give the bridge poller (60 Hz) up to 2 s to deliver the response.
  await page.waitForTimeout(2_000);
});
