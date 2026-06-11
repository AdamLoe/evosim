// Headless grass_step measurement harness — CONTROLLED.
//
// To make grass_step reproducible across builds we remove the two sources of
// variance:
//   1. Population: pin maxPopulation=0 so the population dies out immediately and
//      the sim runs its thin path (grass_step + bitset rebuild only) every tick —
//      zero creature/NN noise, grass spreads unimpeded to full coverage.
//   2. Seed: pin a fixed worldSeed so biome + initial grass clumps are identical.
// Then grass_step is measured over a fixed warmup at max TPS, at the same
// (full) grass coverage in both the baseline and the fix.
//
// Reports per-tick wall-clock µs for tick.grass_step (the metric that was your
// 15%/56%), read from the live Rust profile tree (window.__lastProfilerReport).
//
// Usage: node measure-grass.mjs <label>   (server must be up on :47821)
import { chromium } from "@playwright/test";

const label = process.argv[2] ?? "run";
const WARMUP_MS = 150_000;
const FIXED_SEED = 12345;
const BASE = "http://localhost:47821/";

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
page.on("console", (m) => {
  if (m.type() === "error") console.log("  [page error]", m.text());
});

// First load to let the app write default settings, then pin maxPopulation + seed
// and reload so the FIRST boot of the measured page uses them.
await page.goto(BASE);
await page.waitForTimeout(2000);
await page.evaluate((seed) => {
  const KEY = "evosim.settings.v2";
  const s = JSON.parse(localStorage.getItem(KEY) ?? "{}");
  s.maxPopulation = 0;
  s.founderCount = 0; // zero creatures ever → guaranteed thin path (pure grass)
  s.worldSeed = seed;
  s.showProfiler = true;
  // The loader discards any blob whose vMajor != SCHEMA_MAJOR (2). Stamp it so
  // our injected values survive the reload instead of resetting to defaults.
  s.vMajor = 2;
  s.vMinor = 10;
  localStorage.setItem(KEY, JSON.stringify(s));
}, FIXED_SEED);
await page.goto(BASE); // fresh first-boot with pinned settings

await page
  .waitForFunction(
    () =>
      /tick \d+/.test(
        document.getElementById("perf-status-line")?.textContent ?? "",
      ),
    { timeout: 20_000 },
  )
  .catch(() => console.log("  (boot wait timed out — continuing)"));

// Ensure the profiler panel is visible (its 1 Hz poll publishes the report).
await page.evaluate(() => {
  const box = document.getElementById("perf-box");
  if (box && getComputedStyle(box).display === "none") {
    document.getElementById("perf-btn")?.click();
  }
});

// Max TPS so grass fills to steady state quickly.
await page.evaluate(() => {
  const el = document.getElementById("target-tps-input");
  if (el) {
    el.value = "1000";
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }
});

const readGrassStep = () =>
  page.evaluate(() => {
    const rep = window.__lastProfilerReport;
    if (!rep || !rep.tree) return null;
    const stack = [...rep.tree];
    while (stack.length) {
      const n = stack.pop();
      if (n.name === "tick.grass_step")
        return n.call_count ? Math.round(n.total_us / n.call_count) : null;
      if (n.children) stack.push(...n.children);
    }
    return null;
  });

const t0 = Date.now();
while (Date.now() - t0 < WARMUP_MS) {
  await page.waitForTimeout(5000);
  const status = await page.evaluate(
    () => document.getElementById("perf-status-line")?.textContent ?? "",
  );
  const gs = await readGrassStep();
  console.log(
    `  [${label}] +${Math.round((Date.now() - t0) / 1000)}s grass_step=${gs}µs  ${status}`,
  );
}

const result = await page.evaluate(() => {
  const rep = window.__lastProfilerReport;
  if (!rep || !rep.tree) return { error: "no profiler report" };
  const find = (roots, fullName) => {
    const stack = [...roots];
    while (stack.length) {
      const n = stack.pop();
      if (n.name === fullName) return n;
      if (n.children) stack.push(...n.children);
    }
    return null;
  };
  const perTick = (n) =>
    n && n.call_count ? +(n.total_us / n.call_count).toFixed(1) : null;
  return {
    window_ms: rep.window_ms,
    grass_step_us_per_tick: perTick(find(rep.tree, "tick.grass_step")),
    pyramid_refresh_us_per_tick: perTick(find(rep.tree, "tick.pyramid_refresh")),
    tick_total_us_per_tick: perTick(find(rep.tree, "tick")),
  };
});

const status = await page.evaluate(
  () => document.getElementById("perf-status-line")?.textContent ?? "",
);

console.log(`\n=== RESULT [${label}] ===`);
console.log("status:", status);
console.log(JSON.stringify(result, null, 2));

await browser.close();
