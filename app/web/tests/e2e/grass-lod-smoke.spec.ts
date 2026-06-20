// Permanent smoke — grass LOD / clipmap snapshot gate.
//
// Verifies the threaded wasm sim boots, ticks, grass density evolves, and the
// snapshot window metadata is sane at default scale (64-byte header, v2.0.3+).
//
// What is checked:
//   (a) page boots — crossOriginIsolated, no fatal console errors
//   (b) tick counter advances >= 5 ticks in ~1 s
//   (c) grass density changes between T0 and T1 (scatter kernel live)
//   (d) window metadata (header bytes [32..64)): mip_level=0, winOriginX=0,
//       win_w=grassDim (full horizontal coverage); win_h=grassDim (full vertical
//       coverage). At default zoom with cell_size=20/grassDim=480 the visible
//       span (~8000 cells) exceeds the full grid, so the window is always the
//       full field: win_w=win_h=grassDim=480, winOriginX=winOriginY=0.
//   (e) population > 0 (world not immediately extinct)

import { test, expect, type Page } from "@playwright/test";

// ── Constants (mirrors sim-bridge.ts v2.0.3) ─────────────────────────────────
// SNAPSHOT_HEADER_BYTES was bumped from 32 → 64 in v2.0.3 (Stream 2b).
const SNAPSHOT_HEADER_BYTES = 64;
const MAX_POP_FOR_SIM = 32_000;
const CREATURE_STRIDE = 8; // f32 lanes per creature
const CREATURE_SOA_BYTES = MAX_POP_FOR_SIM * CREATURE_STRIDE * 4;
// v2.0.4 S1: raised 2048 → 4096. Must match src/wasm_api.rs GRASS_LOD_BUDGET_AXIS
// (single source of truth is the generated web/src/generated/lod-constants.ts).
const GRASS_LOD_BUDGET_AXIS = 4096;

// ── Helpers ───────────────────────────────────────────────────────────────────

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
      timeout: 25_000,
    })
    .toBe(true);
}

async function setTargetTps(page: Page, tps: number): Promise<void> {
  await page.evaluate((v) => {
    const el = document.getElementById(
      "target-tps-input",
    ) as HTMLInputElement | null;
    if (!el) throw new Error("#target-tps-input not found");
    el.value = String(v);
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }, tps);
}

// ── Test ──────────────────────────────────────────────────────────────────────

test("grass-lod-smoke: boots, grass evolves, window metadata sane at default scale", async ({
  page,
}) => {
  // 0. Collect console errors.
  const consoleErrors: string[] = [];
  page.on("pageerror", (e) => consoleErrors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error")
      consoleErrors.push(`console.error: ${m.text()}`);
  });

  // 1. Intercept boot_ready to stash snapshot SAB geometry before navigation.
  await page.addInitScript(() => {
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
          if (data && data.kind === "boot_ready" && data.wasm_memory) {
            const w = window as unknown as Record<string, unknown>;
            w.__smokeSnapshotBuf = data.wasm_memory.buffer;
            w.__smokeSnapshotBase = data.snapshot_buf_byte_offset;
            w.__smokeGrassDim = data.grass_dim;
            w.__smokeReady = true;
          }
          fn(ev);
        };
        origSet.call(this, wrapped);
      },
    });
  });

  // 2. Navigate + wait for boot.
  await page.goto("/");
  await waitForBoot(page);

  // (a) No fatal console errors at boot.
  const bootFatal = consoleErrors.filter(
    (e) => !/initThreadPool/.test(e) && !/GL Driver Message/.test(e),
  );
  expect(bootFatal, "no fatal console errors at boot").toEqual([]);

  // Wait for our snapshot intercept to land.
  await expect
    .poll(
      async () =>
        await page.evaluate(
          () =>
            !!(window as unknown as Record<string, unknown>).__smokeReady,
        ),
      { timeout: 5_000 },
    )
    .toBe(true);

  // Log cross-origin isolation status (SAB required for threading).
  const coi = await page.evaluate(
    () =>
      (window as Window & { crossOriginIsolated?: boolean })
        .crossOriginIsolated,
  );
  console.log(`[lod-smoke] crossOriginIsolated=${coi}`);

  // 3. Speed up sim and let it run.
  await setTargetTps(page, 500);
  await page.waitForTimeout(800);

  // 4. Read T0 — tick + grass density sample + window metadata.
  const tick0 = await readTick(page);

  const constants = {
    HDR: SNAPSHOT_HEADER_BYTES,
    CSOA: CREATURE_SOA_BYTES,
    BUDGET: GRASS_LOD_BUDGET_AXIS,
  };

  const t0 = await page.evaluate((c) => {
    const w = window as unknown as Record<string, unknown>;
    const buf = w.__smokeSnapshotBuf as SharedArrayBuffer | ArrayBuffer | null;
    const base = w.__smokeSnapshotBase as number | undefined;
    const grassDim = w.__smokeGrassDim as number | undefined;
    if (!buf || base === undefined || !grassDim) return null;

    const winAxis = Math.min(grassDim, c.BUDGET);
    const grassRegBytes = winAxis * winAxis;
    const slotSz = c.HDR + c.CSOA + grassRegBytes;

    // Subsample grass density (every 16th cell).
    const grassOff = base + 0 * slotSz + c.HDR + c.CSOA;
    const view8 = new Uint8Array(buf, grassOff, grassRegBytes);
    let grassSum = 0;
    for (let i = 0; i < grassRegBytes; i += 16) grassSum += view8[i];

    // Window metadata lives at header bytes [32..64) relative to the slot start.
    const metaOff = base + 0 * slotSz + 32;
    const dv = new DataView(buf);
    const mipLevel = dv.getUint32(metaOff + 0, true);
    const winOriginX = dv.getUint32(metaOff + 4, true);
    const winOriginY = dv.getUint32(metaOff + 8, true);
    const winW = dv.getUint32(metaOff + 12, true);
    const winH = dv.getUint32(metaOff + 16, true);

    return { grassDim, grassSum, mipLevel, winOriginX, winOriginY, winW, winH };
  }, constants);

  console.log(`[lod-smoke] T0: tick=${tick0} data=${JSON.stringify(t0)}`);

  // 5. Wait for more ticks.
  await page.waitForTimeout(1000);

  // 6. Read T1 — tick + grass density sample.
  const tick1 = await readTick(page);
  const t1GrassSum = await page.evaluate((c) => {
    const w = window as unknown as Record<string, unknown>;
    const buf = w.__smokeSnapshotBuf as SharedArrayBuffer | ArrayBuffer | null;
    const base = w.__smokeSnapshotBase as number | undefined;
    const grassDim = w.__smokeGrassDim as number | undefined;
    if (!buf || base === undefined || !grassDim) return -1;

    const winAxis = Math.min(grassDim, c.BUDGET);
    const grassRegBytes = winAxis * winAxis;
    const slotSz = c.HDR + c.CSOA + grassRegBytes;
    const grassOff = base + 0 * slotSz + c.HDR + c.CSOA;
    const view8 = new Uint8Array(buf, grassOff, grassRegBytes);
    let sum = 0;
    for (let i = 0; i < grassRegBytes; i += 16) sum += view8[i];
    return sum;
  }, constants);

  console.log(
    `[lod-smoke] T1: tick=${tick1} grassSum=${t1GrassSum}`,
  );

  // ── Assertions ───────────────────────────────────────────────────────────

  // (b) Tick advanced.
  const tickDelta = tick1 - tick0;
  expect(
    tickDelta,
    `tick counter must advance >= 5 ticks in ~1 s at 500 TPS (got Δtick=${tickDelta})`,
  ).toBeGreaterThanOrEqual(5);

  // (c) Grass accessible and evolves.
  expect(t0, "snapshot data (T0) must be accessible").not.toBeNull();
  expect(
    t0!.grassSum,
    "T0 grass sample must be >= 0",
  ).toBeGreaterThanOrEqual(0);
  expect(
    t1GrassSum,
    "grass sample must be accessible at T1 (not -1)",
  ).not.toBe(-1);
  const grassChanged = Math.abs(t1GrassSum - t0!.grassSum) > 0;
  expect(
    grassChanged,
    `grass density must change over ${tickDelta} ticks (T0=${t0!.grassSum}, T1=${t1GrassSum})`,
  ).toBe(true);

  // (d) Window metadata at default scale.
  // Viewport = 1280×720 (Desktop Chrome), grassDim = 480 (cell_size=20, world=9600).
  // Default zoom = ~0.06 (fit-world-in-viewport). At this zoom the visible span
  // is ~8000 cells horizontally, which exceeds grassDim=480, so the full field
  // is always visible:
  // - mip_level=0: visible_cell_span_x≫grassDim, ratio<1 → level=0.
  // - win_w = win_h = grassDim (full field, clamped at level bounds).
  // - winOriginX = winOriginY = 0 (camera centered, window covers the whole grid).
  const { mipLevel, winOriginX, winOriginY, winW, winH, grassDim } = t0!;
  console.log(
    `[lod-smoke] window: mip=${mipLevel} origin=(${winOriginX},${winOriginY}) win=(${winW}x${winH}) grassDim=${grassDim}`,
  );
  expect(mipLevel, "default scale: mip_level must be 0").toBe(0);
  expect(winOriginX, "default scale: win_origin_x must be 0").toBe(0);
  expect(winW, "default scale: win_w must equal grassDim (full horizontal field)").toBe(grassDim);
  expect(
    winH,
    "default scale: win_h must equal grassDim (full vertical field — zoom-out shows > whole grass grid)",
  ).toBe(grassDim);
  expect(
    winOriginY,
    "default scale: win_origin_y must be 0 (centered window covering full grid)",
  ).toBe(0);

  // (e) World alive.
  const status1 = await readStatus(page);
  const popMatch = /pop (\d+)/.exec(status1);
  const pop1 = popMatch ? Number(popMatch[1]) : 0;
  expect(
    pop1,
    `population must be > 0 at T1; status="${status1}"`,
  ).toBeGreaterThan(0);

  // Final console error check.
  const fatal = consoleErrors.filter(
    (e) => !/initThreadPool/.test(e) && !/GL Driver Message/.test(e),
  );
  expect(fatal, "no fatal console errors during test").toEqual([]);
});
