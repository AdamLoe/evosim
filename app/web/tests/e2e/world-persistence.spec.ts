import { test, expect, type Page } from "@playwright/test";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

interface E2EHarness {
  requestWorldArtifact?: () => Promise<string | null>;
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

// The saved-world actions moved into the General rail tab. The rail starts
// collapsed, so open it via the top-bar hamburger, switch to General, and
// wait for the Export button to be actionable.
async function openMenuTab(page: Page): Promise<void> {
  await page.locator("#settings-rail-btn").click();
  await page.evaluate(() => {
    const btn = document.querySelector<HTMLButtonElement>('.rail-tab[data-tab="general"]');
    if (btn && !btn.classList.contains("is-active")) btn.click();
  });
  await expect(page.locator("#world-export-btn")).toBeVisible();
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

async function latestSave(page: Page): Promise<{
  tick: number;
  artifactJson: string;
  identity: { lineage: string; run_id: string; parent_run_id?: string };
}> {
  return await page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const req = indexedDB.open("evosim.world-saves", 1);
      req.onerror = () => reject(req.error);
      req.onsuccess = () => resolve(req.result);
    });
    const records = await new Promise<Array<{ tick: number; artifactJson: string; updatedAt: number }>>(
      (resolve, reject) => {
        const tx = db.transaction("saves", "readonly");
        const req = tx.objectStore("saves").getAll();
        req.onerror = () => reject(req.error);
        req.onsuccess = () => resolve(req.result as Array<{ tick: number; artifactJson: string; updatedAt: number }>);
      },
    );
    records.sort((a, b) => b.updatedAt - a.updatedAt);
    const latest = records[0];
    if (!latest) throw new Error("no save records");
    const parsed = JSON.parse(latest.artifactJson) as {
      identity: { lineage: string; run_id: string; parent_run_id?: string };
    };
    return { ...latest, identity: parsed.identity };
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
  await setTargetTps(page, 1000);
});

test.afterEach(async ({ page }) => {
  const errs = (page as unknown as { _consoleErrors?: string[] })._consoleErrors ?? [];
  const fatal = errs.filter(
    (e) => !/initThreadPool/.test(e) && !/GL Driver Message/.test(e),
  );
  expect(fatal, "no fatal console errors during test").toEqual([]);
});

test("export and import world artifacts", async ({ page }) => {
  await page.waitForTimeout(500);
  await openMenuTab(page);

  // Capture current world artifact via the e2e hook (same underlying call as
  // the Export button, bypassing the file-download UI).
  const artifactJson = await page.evaluate(async () => {
    const ns = (window as unknown as { __evosimE2E?: E2EHarness }).__evosimE2E;
    if (!ns?.requestWorldArtifact) throw new Error("requestWorldArtifact e2e hook not installed");
    return await ns.requestWorldArtifact();
  });
  expect(artifactJson, "world artifact must be non-null").not.toBeNull();

  const savedMeta = await page.evaluate((json: string) => {
    const parsed = JSON.parse(json) as {
      state?: { tick?: number };
      identity?: { lineage?: string; run_id?: string };
    };
    return {
      tick: Number(parsed.state?.tick ?? 0),
      lineage: parsed.identity?.lineage ?? "",
      run_id: parsed.identity?.run_id ?? "",
    };
  }, artifactJson!);
  expect(savedMeta.tick).toBeGreaterThan(0);
  expect(savedMeta.lineage).toBe("original");

  // Export via the Export button (downloads + updates the save-status line).
  await page.locator("#world-export-btn").click();
  await expect(page.locator("#save-status")).toContainText(/exported t\d+/, { timeout: 10_000 });

  // Let the world run further so the tick advances past the captured snapshot.
  await page.waitForTimeout(500);
  expect(await readTick(page)).toBeGreaterThan(savedMeta.tick);

  // Pause and import the captured artifact — the sim should resume at the
  // saved tick (within a small tolerance for the boot handshake step).
  await page.locator("#playpause-btn").click();
  const tmp = path.join(os.tmpdir(), `evosim-import-${Date.now()}.json`);
  await fs.writeFile(tmp, artifactJson!, "utf8");
  await page.locator("#world-import-input").setInputFiles(tmp);
  await expect(page.locator("#save-status")).toContainText(/imported t\d+|resumed/, {
    timeout: 15_000,
  });
  await waitForBoot(page);
  expect(await readTick(page)).toBeLessThanOrEqual(savedMeta.tick + 3);

  // The import should have written a record to IndexedDB (kind="imported").
  const imported = await latestSave(page);
  expect(imported.tick).toBeGreaterThan(0);
  expect(imported.identity.lineage).toBe("original");
});
