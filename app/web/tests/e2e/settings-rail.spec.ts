import { test, expect, type Page } from "@playwright/test";
import { inflateSync } from "node:zlib";

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

function pngHasNonBlackPixels(buffer: Buffer): boolean {
  const signature = "89504e470d0a1a0a";
  if (buffer.subarray(0, 8).toString("hex") !== signature) {
    throw new Error("canvas screenshot was not a PNG");
  }
  let offset = 8;
  let width = 0;
  let height = 0;
  let colorType = 0;
  const idat: Buffer[] = [];
  while (offset < buffer.length) {
    const length = buffer.readUInt32BE(offset);
    const type = buffer.toString("ascii", offset + 4, offset + 8);
    const data = buffer.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      const bitDepth = data[8];
      colorType = data[9];
      if (bitDepth !== 8 || (colorType !== 2 && colorType !== 6)) {
        throw new Error(`unsupported PNG format bitDepth=${bitDepth} colorType=${colorType}`);
      }
    } else if (type === "IDAT") {
      idat.push(Buffer.from(data));
    } else if (type === "IEND") {
      break;
    }
    offset += 12 + length;
  }

  const channels = colorType === 6 ? 4 : 3;
  const stride = width * channels;
  const inflated = inflateSync(Buffer.concat(idat));
  let prev = Buffer.alloc(stride);
  let pos = 0;
  for (let y = 0; y < height; y++) {
    const filter = inflated[pos++];
    const row = Buffer.from(inflated.subarray(pos, pos + stride));
    pos += stride;
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? row[x - channels] : 0;
      const up = prev[x] ?? 0;
      const upLeft = x >= channels ? prev[x - channels] : 0;
      if (filter === 1) row[x] = (row[x] + left) & 0xff;
      else if (filter === 2) row[x] = (row[x] + up) & 0xff;
      else if (filter === 3) row[x] = (row[x] + Math.floor((left + up) / 2)) & 0xff;
      else if (filter === 4) {
        const p = left + up - upLeft;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - upLeft);
        row[x] = (row[x] + (pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft)) & 0xff;
      } else if (filter !== 0) {
        throw new Error(`unsupported PNG filter ${filter}`);
      }
    }
    for (let x = 0; x < stride; x += channels) {
      if (row[x] > 0 || row[x + 1] > 0 || row[x + 2] > 0) return true;
    }
    prev = row;
  }
  return false;
}

// Read one centre pixel directly from the WebGL framebuffer via the e2e hook
// (bypasses the compositor, which can clear the WebGL backing store in headless
// Chromium before a DOM screenshot is captured).  Falls back to a DOM screenshot
// + PNG parse only when the hook is not yet installed.
async function expectCanvasNonBlank(page: Page): Promise<void> {
  await expect
    .poll(
      async () => {
        const fromHook = await page.evaluate(() => {
          const ns = (
            window as unknown as {
              __evosimE2E?: { canvasHasNonBlackPixels?: () => boolean };
            }
          ).__evosimE2E;
          return ns?.canvasHasNonBlackPixels?.() ?? null;
        });
        if (fromHook !== null) return fromHook;
        // fallback: DOM screenshot
        return pngHasNonBlackPixels(await page.locator("#aquarium").screenshot());
      },
      { timeout: 5_000 },
    )
    .toBe(true);
}

async function railIsOpen(page: Page): Promise<boolean> {
  return await page.evaluate(
    () => !(document.getElementById("app-shell")?.classList.contains("rail-collapsed") ?? true),
  );
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try { localStorage.clear(); } catch { /* ignore */ }
  });
  await page.goto("/");
  await waitForBoot(page);
  await setTargetTps(page, 1000);
});

test("paused Settings rail toggles keep the canvas painted", async ({ page }) => {
  await expectCanvasNonBlank(page);
  await page.click("#playpause-btn");
  await expect(page.locator("#playpause-btn")).toHaveClass(/is-active/);
  await expectCanvasNonBlank(page);

  await page.locator("body").focus();
  await page.keyboard.press("Escape");
  await expect(page.locator("#rail-settings")).toBeVisible();
  await expectCanvasNonBlank(page);

  await page.keyboard.press("Escape");
  await expect.poll(() => railIsOpen(page)).toBe(false);
  await expectCanvasNonBlank(page);
});

test("Escape opens Settings, closes Settings, and ignores text inputs", async ({ page }) => {
  await page.locator("body").focus();
  await page.keyboard.press("Escape");
  await expect(page.locator("#rail-settings")).toBeVisible();

  await page.keyboard.press("Escape");
  await expect.poll(() => railIsOpen(page)).toBe(false);

  await page.click("#settings-rail-btn");
  await expect(page.locator("#rail-general")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator("#rail-settings")).toBeVisible();

  const focusedNumberInput = page.locator("#rail-settings input[type='number']").first();
  await focusedNumberInput.focus();
  await page.keyboard.press("Escape");
  await expect(page.locator("#rail-settings")).toBeVisible();
});

test("General tab omits named save controls and badge is bottom-left", async ({ page }) => {
  await page.click("#settings-rail-btn");
  await expect(page.locator("#rail-general")).toBeVisible();
  await expect(page.locator("#world-save-btn")).toHaveCount(0);
  await expect(page.locator("#world-resume-btn")).toHaveCount(0);
  await expect(page.locator("#world-fork-btn")).toHaveCount(0);

  const badge = page.locator("#app-badge");
  await expect(badge).toHaveText(/^evosim v/);
  await expect(badge).toHaveCSS("border-radius", "0px");
  const placement = await page.evaluate(() => {
    const badgeRect = document.getElementById("app-badge")!.getBoundingClientRect();
    const wrapRect = document.getElementById("canvas-wrap")!.getBoundingClientRect();
    return {
      left: Math.round(badgeRect.left - wrapRect.left),
      bottom: Math.round(wrapRect.bottom - badgeRect.bottom),
    };
  });
  expect(placement.left).toBeGreaterThanOrEqual(6);
  expect(placement.left).toBeLessThanOrEqual(10);
  expect(placement.bottom).toBeGreaterThanOrEqual(6);
  expect(placement.bottom).toBeLessThanOrEqual(10);
});
