import { test, expect, type Page } from "@playwright/test";
import {
  DEFAULTS,
  SETTINGS_KEYS,
  SETTINGS_STORAGE_KEY,
  SETTINGS_USER_KEYS,
  type Settings,
  type WorldConfig,
} from "../../src/settings";

const RUST_SLIDER_TO_SETTING: Record<string, keyof Settings> = {
  mutation_rate_multiplier: "mutRate",
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
  max_population: "maxPopulation",
  grass_initial_seed_count: "initialGrassSeedCount",
  world_size: "worldSize",
  world_seed: "worldSeed",
  wrap_world: "wrapWorld",
  species_mode: "speciesMode",
  crossover_mode: "crossoverMode",
  starting_species_count: "startingSpeciesCount",
  starting_species_member_count: "startingSpeciesMemberCount",
  starting_species_member_variance: "startingSpeciesMemberVariance",
  mating_cooldown_ticks: "matingCooldownTicks",
  grass_decay_pct: "grassDecayPct",
  grass_decay_amount: "grassDecayAmount",
  grass_spread_pct: "grassSpreadPct",
  grass_spread_amount: "grassSpreadAmount",
  grass_spread_ring1_pct: "grassSpreadRing1Pct",
  grass_spread_ring2_pct: "grassSpreadRing2Pct",
  lod_bias: "lodBias",
  grass_multisight: "grassMultisight",
  grass_size: "grassSize",
  grass_lod_step: "grassLodStep",
  grass_clump_count: "grassClumpCount",
  grass_clump_size: "grassClumpSize",
  mate_reach_multiplier: "mateReachMultiplier",
  init_graze_boost: "initGrazeBoost",
  init_split_boost: "initSplitBoost",
  crowding_strength: "crowdingStrength",
  crowding_radius: "crowdingRadius",
  starvation_threshold: "starvationThreshold",
  starvation_drain_rate: "starvationDrainRate",
};

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function variant(seed: 1 | 2): Settings {
  const s = clone(DEFAULTS);
  Object.assign(s, {
    targetTPS: seed === 1 ? 333 : 444,
    appFPS: seed === 1 ? 15 : 120,
    autoRun: seed === 1,
    showProfiler: seed === 1,
    showGrass: seed !== 1,
    grassOpacity: seed === 1 ? 0.35 : 0.65,
    biomeOpacity: seed === 1 ? 0.45 : 0.75,
    grassSmoothing: seed === 1 ? 0.2 : 0.8,
    grassSoftness: seed === 1 ? 0.15 : 0.35,
    grassTexture: seed === 1 ? 0.4 : 0.6,
    grassEdgeErosion: seed === 1 ? 0.25 : 0.7,
    grassShadeVariation: seed === 1 ? 0.3 : 0.6,
    grassBladeSize: seed === 1 ? 6.5 : 9.5,
    visualRepeats: seed === 1,
    maxZoomOutMaps: seed === 1 ? 12 : 24,
    creatureSurveyDotMinPx: seed === 1 ? 1.7 : 2.8,
    creatureSurveyDotScale: seed === 1 ? 2.5 : 5.5,
    repeatBorderMinPx: seed === 1 ? 1.4 : 2.6,
    repeatBorderScale: seed === 1 ? 3.5 : 7.5,
    repeatBorderOpacity: seed === 1 ? 0.35 : 0.65,
    grassDensityFloor: seed === 1 ? 0.08 : 0.12,
    grassContrast: seed === 1 ? 1.35 : 1.8,
    grassBrightness: seed === 1 ? 1.1 : 1.35,
    profilerWindowMs: seed === 1 ? 5000 : 30000,
    railOpen: seed === 1,
    theme: seed === 1 ? "slate" : "light",
    railW: seed === 1 ? 511 : 577,
    profilerH: seed === 1 ? 277 : 333,
    upkeepMultiplier: seed === 1 ? 0.77 : 1.23,
    moveCostMultiplier: seed === 1 ? 0.66 : 1.44,
    energyMax: seed === 1 ? 155 : 211,
    eatBiteFrac: seed === 1 ? 0.25 : 0.75,
    grassPropagK: seed === 1 ? 0.012 : 0.034,
    grassGrowthR: seed === 1 ? 0.021 : 0.043,
    grassEnergyPerBite: seed === 1 ? 12.5 : 22.5,
    grassBitesPerBlock: seed === 1 ? 3 : 5,
    digestionCooldown: seed === 1 ? 17 : 29,
    repulsionMax: seed === 1 ? 1.7 : 2.9,
    maxPopulation: seed === 1 ? 1234 : 2345,
    initialGrassSeedCount: seed === 1 ? 123 : 456,
    mutRate: seed === 1 ? 0.55 : 1.55,
    maxAge: seed === 1 ? 3456 : 4567,
    splitThreshold: seed === 1 ? 80 : 120,
    splitGift: seed === 1 ? 18 : 28,
    splitJitter: seed === 1 ? 7 : 13,
    founderCount: seed === 1 ? 44 : 88,
    worldSize: seed === 1 ? 4800 : 7200,
    worldSeed: seed === 1 ? 123456 : 654321,
    wrapWorld: seed !== 1,
    scienceMode: seed === 1,
    speciesMode: seed === 1,
    crossoverMode: seed === 1 ? 0 : 1,
    startingSpeciesCount: seed === 1 ? 7 : 11,
    startingSpeciesMemberCount: seed === 1 ? 12 : 18,
    startingSpeciesMemberVariance: seed === 1 ? 1.5 : 2.5,
    matingCooldownTicks: seed === 1 ? 77 : 99,
    grassDecayPct: seed === 1 ? 0.11 : 0.22,
    grassDecayAmount: seed === 1 ? 0.017 : 0.027,
    grassSpreadPct: seed === 1 ? 0.33 : 0.44,
    grassSpreadAmount: seed === 1 ? 0.037 : 0.047,
    grassSpreadRing1Pct: seed === 1 ? 0.51 : 0.61,
    grassSpreadRing2Pct: seed === 1 ? 0.21 : 0.31,
    lodBias: seed === 1 ? 1.25 : 2.25,
    grassMultisight: seed !== 1,
    creatureSectorRange: seed === 1 ? 33 : 44,
    grassSectorRange: seed === 1 ? 55 : 66,
    grassFarSectorRange: seed === 1 ? 210 : 260,
    noGrassZones: seed === 1,
    grassSize: seed === 1 ? 10 : 15,
    grassClumpCount: seed === 1 ? 24 : 48,
    grassClumpSize: seed === 1 ? 9 : 14,
    grassLodStep: seed === 1 ? 2 : 4,
    mateReachMultiplier: seed === 1 ? 3 : 4,
    initGrazeBoost: seed === 1 ? 1.7 : 2.7,
    initSplitBoost: seed === 1 ? 1.8 : 2.8,
    crowdingStrength: seed === 1 ? 0.015 : 0.025,
    crowdingRadius: seed === 1 ? 15 : 18,
    starvationThreshold: seed === 1 ? 12 : 18,
    starvationDrainRate: seed === 1 ? 0.25 : 0.35,
  } satisfies Partial<Settings>);
  s.mutationBuckets = Array.from({ length: DEFAULTS.mutationBuckets.length }, (_, i) => ({
    weight: seed + i,
    rate: Math.min(1, 0.01 * (seed + i)),
    sigma: 0.02 * (seed + i),
  }));
  s.nnTopology =
    seed === 1
      ? { layerSizes: [32, 16], activations: ["relu", "tanh"] }
      : { layerSizes: [64, 32, 16], activations: ["lrelu", "relu", "linear"] };
  return s;
}

function pickSettings(value: Settings, keys: readonly (keyof Settings)[]): Partial<Settings> {
  const out: Partial<Settings> = {};
  for (const key of keys) {
    out[key] = clone(value[key]) as never;
  }
  return out;
}

async function readSettings(page: Page): Promise<Settings> {
  return await page.evaluate(() => {
    const hook = (window as unknown as {
      __evosimSettingsForTests?: { getSettings: () => Settings };
    }).__evosimSettingsForTests;
    if (!hook) throw new Error("missing settings test hook");
    return hook.getSettings();
  });
}

test("settings storage loads and saves every Settings key", async ({ page }) => {
  const initial = variant(1);
  await page.addInitScript(({ key, value }) => {
    if (localStorage.getItem("__evosim_settings_seeded") === "1") return;
    localStorage.clear();
    localStorage.setItem(key, JSON.stringify(value));
    localStorage.setItem("__evosim_settings_seeded", "1");
  }, { key: SETTINGS_STORAGE_KEY, value: initial });

  await page.goto("/");
  expect(pickSettings(await readSettings(page), SETTINGS_KEYS)).toEqual(
    pickSettings(initial, SETTINGS_KEYS),
  );

  const next = variant(2);
  const storedRaw = await page.evaluate(({ values, userKeys }) => {
    const hook = (window as unknown as {
      __evosimSettingsForTests?: {
        setSetting: (key: keyof Settings, value: Settings[keyof Settings]) => void;
        storageKey: string;
      };
    }).__evosimSettingsForTests;
    if (!hook) throw new Error("missing settings test hook");
    for (const key of userKeys) {
      hook.setSetting(key, values[key]);
    }
    return localStorage.getItem(hook.storageKey);
  }, { values: next, userKeys: SETTINGS_USER_KEYS });
  expect(storedRaw).not.toBeNull();
  const stored = JSON.parse(storedRaw!) as Settings;
  expect(pickSettings(stored, SETTINGS_KEYS)).toEqual(pickSettings(next, SETTINGS_KEYS));

  await page.reload();
  expect(pickSettings(await readSettings(page), SETTINGS_KEYS)).toEqual(
    pickSettings(next, SETTINGS_KEYS),
  );
});

test("boot slider state includes every persisted Rust slider setting", async ({ page }) => {
  const initial = variant(1);
  await page.addInitScript(({ key, value }) => {
    if (localStorage.getItem("__evosim_settings_seeded") === "1") return;
    localStorage.clear();
    localStorage.setItem(key, JSON.stringify(value));
    localStorage.setItem("__evosim_settings_seeded", "1");
  }, { key: SETTINGS_STORAGE_KEY, value: initial });

  await page.goto("/");

  const state = await page.evaluate(() => {
    const hook = (window as unknown as {
      __evosimDevPanelForTests?: { currentSliderState: () => Record<string, number> };
    }).__evosimDevPanelForTests;
    if (!hook) throw new Error("missing devpanel test hook");
    return hook.currentSliderState();
  });

  const mismatches: string[] = [];
  for (const [rustName, settingsKey] of Object.entries(RUST_SLIDER_TO_SETTING)) {
    if (!(rustName in state)) {
      mismatches.push(`${rustName}: missing from currentSliderState()`);
      continue;
    }
    const expected = initial[settingsKey];
    const expectedNumber = typeof expected === "boolean" ? (expected ? 1 : 0) : Number(expected);
    if (Math.abs(state[rustName] - expectedNumber) > 1e-6) {
      mismatches.push(`${rustName}: state=${state[rustName]}, expected=${expectedNumber}`);
    }
  }
  for (let k = 0; k < initial.mutationBuckets.length; k++) {
    const bucket = initial.mutationBuckets[k];
    for (const field of ["weight", "rate", "sigma"] as const) {
      const rustName = `bucket_${k}_${field}`;
      if (!(rustName in state)) {
        mismatches.push(`${rustName}: missing from currentSliderState()`);
        continue;
      }
      if (Math.abs(state[rustName] - bucket[field]) > 1e-6) {
        mismatches.push(`${rustName}: state=${state[rustName]}, expected=${bucket[field]}`);
      }
    }
  }

  expect(mismatches, "persisted sim settings must seed every Rust slider lane").toEqual([]);
});

test("boot world config includes every persisted construction setting", async ({ page }) => {
  const initial = variant(1);
  await page.addInitScript(({ key, value }) => {
    if (localStorage.getItem("__evosim_settings_seeded") === "1") return;
    localStorage.clear();
    localStorage.setItem(key, JSON.stringify(value));
    localStorage.setItem("__evosim_settings_seeded", "1");
  }, { key: SETTINGS_STORAGE_KEY, value: initial });

  await page.goto("/");

  const config = await page.evaluate(() => {
    const hook = (window as unknown as {
      __evosimDevPanelForTests?: {
        currentWorldConfig: (masterSeed: number) => WorldConfig;
      };
    }).__evosimDevPanelForTests;
    if (!hook) throw new Error("missing devpanel test hook");
    return hook.currentWorldConfig(987654);
  });

  expect(config).toMatchObject({
    master_seed: 987654,
    world: {
      size: initial.worldSize,
      wrap: initial.wrapWorld,
    },
    science: {
      deterministic: initial.scienceMode,
    },
    grass: {
      initial_seed_count: initial.initialGrassSeedCount,
      full_on_init: false,
      cell_size: initial.grassSize,
      clump_count: initial.grassClumpCount,
      clump_size: initial.grassClumpSize,
      multisight: initial.grassMultisight,
      no_grass_zones: initial.noGrassZones,
    },
    population: {
      founder_count: initial.founderCount,
      energy_max: initial.energyMax,
    },
    species: {
      enabled: initial.speciesMode,
      crossover_mode: initial.crossoverMode === 0 ? "average" : "fifty_fifty",
      starting_species_count: initial.startingSpeciesCount,
      starting_species_member_count: initial.startingSpeciesMemberCount,
      starting_species_member_variance: initial.startingSpeciesMemberVariance,
    },
    founders: {
      init_graze_boost: initial.initGrazeBoost,
      init_split_boost: initial.initSplitBoost,
    },
    nn_sensing: {
      creature_sector_range: initial.creatureSectorRange,
      grass_sector_range: initial.grassSectorRange,
      grass_far_sector_range: initial.grassFarSectorRange,
    },
    nn_topology: {
      hidden_sizes: initial.nnTopology.layerSizes,
      activations: initial.nnTopology.activations,
    },
  });
});
