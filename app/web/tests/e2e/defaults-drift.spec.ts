import { test, expect } from "@playwright/test";
import {
  DEFAULT_LIVE_SLIDER_VALUES,
  DEFAULT_WORLD_CONFIG,
  WORLD_CONFIG_PRESETS,
  WORLD_CONFIG_SCHEMA_VERSION,
  type WorldConfig,
} from "../../src/generated/world-config";
import { DEFAULTS } from "../../src/settings";

function sliderDefault(name: string): number {
  return DEFAULT_LIVE_SLIDER_VALUES[name] ?? Number.NaN;
}

function completeWorldConfigShape(config: WorldConfig): unknown {
  return {
    schema_version: config.schema_version,
    master_seed: typeof config.master_seed,
    science: Object.keys(config.science).sort(),
    world: Object.keys(config.world).sort(),
    grass: Object.keys(config.grass).sort(),
    population: Object.keys(config.population).sort(),
    species: Object.keys(config.species).sort(),
    founders: Object.keys(config.founders).sort(),
    nn_sensing: Object.keys(config.nn_sensing).sort(),
    nn_topology: Object.keys(config.nn_topology).sort(),
  };
}

test("generated live slider defaults match Rust runtime defaults", async ({ page }) => {
  await page.addInitScript(() => {
    try { localStorage.clear(); } catch { /* ignore */ }
  });

  await page.goto("/");

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
  for (const [name, expected] of Object.entries(DEFAULT_LIVE_SLIDER_VALUES)) {
    if (!(name in rust)) {
      mismatches.push(`${name}: missing from Rust JSON`);
      continue;
    }
    if (Math.abs(rust[name] - expected) > 1e-6) {
      mismatches.push(`${name}: Rust=${rust[name]}, generated=${expected}`);
    }
  }

  expect(mismatches, "Rust live slider defaults drifted from generated TS").toEqual([]);
});

test("settings defaults are derived from generated world config and live defaults", () => {
  expect(DEFAULTS.worldSize).toBe(DEFAULT_WORLD_CONFIG.world.size);
  expect(DEFAULTS.worldSeed).toBe(DEFAULT_WORLD_CONFIG.master_seed);
  expect(DEFAULTS.wrapWorld).toBe(DEFAULT_WORLD_CONFIG.world.wrap);
  expect(DEFAULTS.scienceMode).toBe(DEFAULT_WORLD_CONFIG.science.deterministic);
  expect(DEFAULTS.initialGrassSeedCount).toBe(DEFAULT_WORLD_CONFIG.grass.initial_seed_count);
  expect(DEFAULTS.grassSize).toBe(DEFAULT_WORLD_CONFIG.grass.cell_size);
  expect(DEFAULTS.grassClumpCount).toBe(DEFAULT_WORLD_CONFIG.grass.clump_count);
  expect(DEFAULTS.grassClumpSize).toBe(DEFAULT_WORLD_CONFIG.grass.clump_size);
  expect(DEFAULTS.grassMultisight).toBe(DEFAULT_WORLD_CONFIG.grass.multisight);
  expect(DEFAULTS.creatureSectorRange).toBe(
    DEFAULT_WORLD_CONFIG.nn_sensing.creature_sector_range,
  );
  expect(DEFAULTS.grassSectorRange).toBe(DEFAULT_WORLD_CONFIG.nn_sensing.grass_sector_range);
  expect(DEFAULTS.grassFarSectorRange).toBe(
    DEFAULT_WORLD_CONFIG.nn_sensing.grass_far_sector_range,
  );
  expect(DEFAULTS.noGrassZones).toBe(DEFAULT_WORLD_CONFIG.grass.no_grass_zones);
  expect(DEFAULTS.visualRepeats).toBe(true);
  expect(DEFAULTS.maxZoomOutMaps).toBe(8);
  expect(DEFAULTS.creatureSurveyDotMinPx).toBe(1.0);
  expect(DEFAULTS.creatureSurveyDotScale).toBe(1.0);
  expect(DEFAULTS.repeatBorderMinPx).toBe(1.0);
  expect(DEFAULTS.repeatBorderScale).toBe(0.0);
  expect(DEFAULTS.repeatBorderOpacity).toBe(0.25);
  expect(DEFAULTS.founderCount).toBe(DEFAULT_WORLD_CONFIG.population.founder_count);
  expect(DEFAULTS.energyMax).toBe(DEFAULT_WORLD_CONFIG.population.energy_max);
  expect(DEFAULTS.speciesMode).toBe(DEFAULT_WORLD_CONFIG.species.enabled);
  expect(DEFAULTS.crossoverMode).toBe(
    DEFAULT_WORLD_CONFIG.species.crossover_mode === "average" ? 0 : 1,
  );
  expect(DEFAULTS.startingSpeciesCount).toBe(
    DEFAULT_WORLD_CONFIG.species.starting_species_count,
  );
  expect(DEFAULTS.startingSpeciesMemberCount).toBe(
    DEFAULT_WORLD_CONFIG.species.starting_species_member_count,
  );
  expect(DEFAULTS.startingSpeciesMemberVariance).toBe(
    DEFAULT_WORLD_CONFIG.species.starting_species_member_variance,
  );
  expect(DEFAULTS.initGrazeBoost).toBe(DEFAULT_WORLD_CONFIG.founders.init_graze_boost);
  expect(DEFAULTS.initSplitBoost).toBe(DEFAULT_WORLD_CONFIG.founders.init_split_boost);
  expect(DEFAULTS.mutRate).toBe(sliderDefault("mutation_rate_multiplier"));
  expect(DEFAULTS.maxPopulation).toBe(sliderDefault("max_population"));
  expect(DEFAULTS.matingCooldownTicks).toBe(sliderDefault("mating_cooldown_ticks"));
  expect(DEFAULTS.crowdingStrength).toBe(sliderDefault("crowding_strength"));
});

test("generated presets are complete WorldConfig payloads", () => {
  expect(DEFAULT_WORLD_CONFIG.schema_version).toBe(WORLD_CONFIG_SCHEMA_VERSION);
  const expectedShape = completeWorldConfigShape(DEFAULT_WORLD_CONFIG);
  expect(WORLD_CONFIG_PRESETS.length).toBeGreaterThan(0);
  for (const preset of WORLD_CONFIG_PRESETS) {
    expect(preset.config.schema_version).toBe(WORLD_CONFIG_SCHEMA_VERSION);
    expect(completeWorldConfigShape(preset.config)).toEqual(expectedShape);
  }
});
