import { test, expect, type Page } from "@playwright/test";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

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

test("save, resume, fork, export, and import world artifacts", async ({ page }) => {
  await page.waitForTimeout(500);
  await page.locator("#world-save-btn").click();
  await expect(page.locator("#save-status")).toContainText(/saved t\d+/, { timeout: 10_000 });
  const saved = await latestSave(page);
  expect(saved.tick).toBeGreaterThan(0);
  expect(saved.identity.lineage).toBe("original");

  await page.locator("#world-export-btn").click();
  await expect(page.locator("#save-status")).toContainText(/exported t\d+/, { timeout: 10_000 });

  await page.waitForTimeout(500);
  expect(await readTick(page)).toBeGreaterThan(saved.tick);
  await page.locator("#playpause-btn").click();
  await page.locator("#world-resume-btn").click();
  await expect(page.locator("#save-status")).toContainText("resumed", { timeout: 15_000 });
  await waitForBoot(page);
  expect(await readTick(page)).toBeLessThanOrEqual(saved.tick + 2);

  await page.locator("#world-fork-btn").click();
  await expect(page.locator("#save-status")).toContainText("forked", { timeout: 15_000 });
  await page.locator("#world-save-btn").click();
  await expect(page.locator("#save-status")).toContainText(/saved t\d+/, { timeout: 10_000 });
  const forked = await latestSave(page);
  expect(forked.identity.lineage).toBe("fork");
  expect(forked.identity.parent_run_id).toBe(saved.identity.run_id);
  expect(forked.identity.run_id).not.toBe(saved.identity.run_id);

  const tmp = path.join(os.tmpdir(), `evosim-import-${Date.now()}.json`);
  await fs.writeFile(tmp, saved.artifactJson, "utf8");
  await page.locator("#world-import-input").setInputFiles(tmp);
  await expect(page.locator("#save-status")).toContainText(/imported t\d+|resumed/, {
    timeout: 15_000,
  });
  await waitForBoot(page);
  expect(await readTick(page)).toBeLessThanOrEqual(saved.tick + 3);
});
