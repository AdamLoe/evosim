// Wave D drift-guard. Asserts that the Rust-side `sliders_defaults_json()`
// values (the canonical `DevSliders::default()` map) agree with the
// `web/src/settings.ts` DEFAULTS for every shared slider name.
//
// The Rust side is the source of truth; settings.ts mirrors it for
// localStorage. This test fails fast if either side moves so a divergence
// can't silently ship a fresh world that behaves differently from a
// reloaded one.

import { test, expect } from "@playwright/test";
import { DEFAULTS } from "../../src/settings";

// Map Rust slider name → settings.ts key for the shared sliders. Keys
// missing from this table are tolerated (e.g. `auto_curriculum` rides
// `set_slider` as 0|1; settings stores a real boolean; both encode 0/1 here
// for comparison).
const RUST_TO_SETTINGS: Record<string, keyof typeof DEFAULTS> = {
  mutation_rate_multiplier: "mutRate",
  nn_mutation_sigma: "nnSigma",
  eat_bite_fraction: "eatBiteFrac",
  grass_propagation_rate_k: "grassPropagK",
  grass_in_cell_growth_r: "grassGrowthR",
  upkeep_multiplier: "upkeepMultiplier",
  move_cost_multiplier: "moveCostMultiplier",
  energy_max: "energyMax",
  grass_energy_per_bite: "grassEnergyPerBite",
  grass_bites_per_block: "grassBitesPerBlock",
  digestion_cooldown: "digestionCooldown",
  repulsion_max: "repulsionMax",
  max_age: "maxAge",
  split_threshold: "splitThreshold",
  split_gift: "splitGift",
  split_jitter: "splitJitter",
  founder_count: "founderCount",
  grass_initial_seed_count: "initialGrassSeedCount",
  curriculum_min_pop: "curriculumMinPop",
  curriculum_max_pop: "curriculumMaxPop",
  curriculum_min_factor: "curriculumMinFactor",
  auto_curriculum: "autoCurriculum",
  full_grass_on_init: "fullGrassOnInit",
};

function settingsValueAsNumber(key: keyof typeof DEFAULTS): number {
  const v = DEFAULTS[key];
  if (typeof v === "boolean") return v ? 1 : 0;
  return v as number;
}

test("Rust DevSliders defaults match settings.ts DEFAULTS", async ({ page }) => {
  // Clear localStorage so the worker boots under canonical defaults; the
  // stashed JSON is always sourced from Rust so this is belt-and-braces.
  await page.addInitScript(() => {
    try { localStorage.clear(); } catch { /* ignore */ }
  });

  await page.goto("/");

  // Wait for boot — the stash lands inside spawnSimWorker after boot_ready.
  await expect
    .poll(
      async () =>
        await page.evaluate(
          () =>
            (window as unknown as { __rustSlidersDefaults?: string })
              .__rustSlidersDefaults ?? null,
        ),
      { timeout: 15_000 },
    )
    .not.toBeNull();

  const rustJson = await page.evaluate(
    () =>
      (window as unknown as { __rustSlidersDefaults?: string })
        .__rustSlidersDefaults!,
  );
  const rust = JSON.parse(rustJson) as Record<string, number>;

  const mismatches: string[] = [];
  for (const [rustName, settingsKey] of Object.entries(RUST_TO_SETTINGS)) {
    if (!(rustName in rust)) {
      mismatches.push(`${rustName}: missing from Rust JSON`);
      continue;
    }
    const r = rust[rustName];
    const t = settingsValueAsNumber(settingsKey);
    if (Math.abs(r - t) > 1e-6) {
      mismatches.push(
        `${rustName}: Rust=${r}, settings.ts.${String(settingsKey)}=${t}`,
      );
    }
  }

  expect(mismatches, "Rust ↔ settings.ts defaults drift").toEqual([]);
});
