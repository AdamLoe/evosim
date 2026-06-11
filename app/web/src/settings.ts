// Persistent dev-panel + pacing settings. Single JSON blob under one
// localStorage key. The Settings shape mirrors the live-tunable sliders in
// the dev panel; defaults must agree with the Rust `*_DEFAULT` constants
// (drift-guarded by a Playwright e2e against `sliders_defaults_json()`).
//
// On load: unknown keys in the stored blob are silently discarded so an
// older session whose schema diverged (e.g. v1.8's `eatCostMultiplier`) can't
// fail typed parsing or carry stale state into a newer session.

// v2.0 Wave 1c: settings schema is now versioned `major.minor`.
//   * MAJOR mismatch  → breaking change (removed/renamed/retyped keys).
//     On load the whole stored blob is discarded and defaults are used.
//   * MINOR mismatch  → additive change (new keys only). On load the stored
//     user values are kept and any keys missing from the blob fall back to
//     their defaults (the normal `{...DEFAULTS, ...stored}` merge already does
//     this). No reset.
// The v2.0 world-shape changes (removed `fullGrassOnInit`, added world_size /
// world_seed / wrap_world / biome penalties) are a MAJOR bump from the v1
// single-`v=1` scheme, so every legacy v1 blob resets cleanly to defaults.
export const SETTINGS_STORAGE_KEY = "evosim.settings.v2";
const SCHEMA_MAJOR = 2;
// v2.0 Wave 2b: added `traitMutationSigmaMultiplier` (a new additive key with a
// default) → MINOR bump 0 → 1. Existing v2 blobs keep their values and pick up
// the new key from DEFAULTS via the `{...DEFAULTS, ...stored}` merge.
// v2.0 Wave 3b: added the species/mating construction keys (`speciesMode`,
// `crossoverMode`, `startingSpeciesCount`, `startingSpeciesMemberCount`,
// `startingSpeciesMemberVariance`) + the live `matingCooldownTicks` — all new
// additive keys with defaults → MINOR bump 1 → 2. Existing v2 blobs keep their
// values and merge the new keys from DEFAULTS.
// v2.0.2 Stream 1d: added six scatter kernel live-tuning sliders — all new
// additive keys with defaults → MINOR bump 2 → 3. Existing v2 blobs keep their
// values and merge the new keys from DEFAULTS.
// v2.0.4 S1: added `lodBias` (render-side LOD nudge knob) — new additive key
// with default 0.0 → MINOR bump 3 → 4. Existing v2 blobs keep their values.
// v2.0.4 S6: added `grassMultisight` construction toggle (same MINOR bump 3 → 4).
// Existing v2 blobs keep their values and pick up the new key from DEFAULTS.
// v2.0.4 S2: added `grassSize` construction knob (cell size, default 5.0) →
// MINOR bump 4 → 5. Existing v2 blobs keep their values.
// v2.0.6 S3: added `grassClumpCount` + `grassClumpSize` construction knobs, and
// fixed `grassDecayPct` from 0.02 → 0.04 to match Rust GRASS_DECAY_PCT_DEFAULT
// → MINOR bump 5 → 6. Existing v2 blobs that stored 0.02 get the new 0.04 on
// next boot via the merge (the stored key is discarded on MINOR mismatch... wait
// actually MINOR keeps stored keys). The decay drift is a bug fix: existing blobs
// carry 0.02 which was wrong; new code deliberately leaves the wrong value in
// place for existing sessions — they can Reset to fix it.
// v2.0.6 S10: added `grassLodStep` (discrete LOD stepper, default 0=Auto) as a
// LIVE setting. New additive key → MINOR bump 6 → 7. Existing blobs keep their
// values and pick up the new key from DEFAULTS on merge.
// Mating/founder + render polish: added `mateReachMultiplier`, `initGrazeBoost`,
// `initSplitBoost` (sim sliders) and `biomeOpacity` / `grassSmoothing` (live
// Display render settings) as additive keys → MINOR bump 7 → 8. All neutral by
// default; existing blobs merge them from DEFAULTS without a reset.
// Grass-detail knobs: added `grassEdgeErosion`, `grassShadeVariation`,
// `grassBladeSize` (live Display render settings) → MINOR bump 8 → 9. Additive,
// merged from DEFAULTS without a reset.
// Grass smoothing rework: `grassSmoothing` repurposed to a bicubic blend, plus
// added `grassSoftness`, `grassTexture`, `grassDensityFloor`, `grassContrast`,
// `grassBrightness` (live Display) → MINOR bump 9 → 10. All neutral by default
// (reproduce the prior look); additive, merged from DEFAULTS without a reset.
const SCHEMA_MINOR = 10;

/** v1.12: one row of the 8-row mutation policy table. Mirrors the Rust
 * `Bucket` struct (`src/brain.rs`). `weight` is any non-negative float;
 * `rate` is clamped [0, 1]; `sigma >= 0`. */
export interface MutationBucket {
  weight: number;
  rate: number;
  sigma: number;
}

/** v1.12: hidden-layer-only topology. Inputs=32 and outputs=5 are implicit.
 * `layerSizes.length === activations.length` (one activation per
 * hidden-layer output). Each width must be a multiple of 8 in [8, 256]. */
export interface NnTopology {
  layerSizes: number[];
  activations: ActivationName[];
}

export type ActivationName = "lrelu" | "relu" | "tanh" | "sigmoid" | "linear";

export const MUTATION_BUCKET_COUNT = 8 as const;

export const DEFAULT_MUTATION_BUCKETS: MutationBucket[] = (() => {
  const out: MutationBucket[] = [];
  // Three equal-weight buckets: 1/3 no change, 1/3 small/medium, 1/3 big.
  out.push({ weight: 1.0, rate: 0.0, sigma: 0.0 });
  out.push({ weight: 1.0, rate: 0.05, sigma: 0.05 });
  out.push({ weight: 1.0, rate: 0.30, sigma: 0.20 });
  for (let i = 3; i < MUTATION_BUCKET_COUNT; i++) {
    out.push({ weight: 0, rate: 0, sigma: 0 });
  }
  return out;
})();

export const DEFAULT_NN_TOPOLOGY: NnTopology = {
  layerSizes: [48, 24],
  activations: ["lrelu", "lrelu"],
};

const VALID_ACTIVATIONS: ReadonlySet<string> = new Set([
  "lrelu", "relu", "tanh", "sigmoid", "linear",
]);

export interface Settings {
  // v2.0 Wave 1c: schema version stamp split into major/minor (see header).
  vMajor: number;
  vMinor: number;
  targetTPS: number;
  autoRun: boolean;
  // Display toggles (live-apply, page-side only)
  showProfiler: boolean;
  showGrass: boolean;
  grassOpacity: number;
  // Biome/terrain (world) layer opacity, 0..1. 1.0 = fully opaque (default);
  // lower fades the colored ground toward the canvas background. Grass keeps its
  // own `grassOpacity` on top. Live Display setting (renderer reads each frame).
  biomeOpacity: number;
  // Grass smoothing, 0..1. 0 = crisp cells (original look, default); 1 = full
  // bicubic (Catmull-Rom) upscale of the density intensities. (Key name kept as
  // `grassSmoothing` for blob compatibility.) Live Display.
  grassSmoothing: number;
  // Extra blur on top of bicubic, 0..1 (radius scales with it). 0 = none.
  grassSoftness: number;
  // Procedural texture overlay master, 0..1. 0 = off (pure smoothed intensities).
  grassTexture: number;
  // Grass edge erosion strength, 0..1 (ragged patch borders; needs grassTexture>0).
  grassEdgeErosion: number;
  // Grass blade-shade variation strength, 0..1 (needs grassTexture>0).
  grassShadeVariation: number;
  // Grass blade size in world-units (fine detail noise wavelength; patch = 6x).
  grassBladeSize: number;
  // Intensity ramp: density floor (0..0.5). Below this reads empty; rest rescales.
  grassDensityFloor: number;
  // Intensity ramp: contrast/gamma on density. 1 = linear (original).
  grassContrast: number;
  // Intensity ramp: peak green brightness multiplier. 1 = original.
  grassBrightness: number;
  // Profiler rolling-window length in ms. Drives both the Rust trees (via
  // CTRL_PROFILE_WINDOW_MS) and the TS `frame` tree (via setProfilerWindowMs).
  profilerWindowMs: number;
  // v1.9.1: right-rail open/closed; toggled by the ⚙ button + `~` hotkey.
  // Forced back open by a creature click (inspector flow).
  railOpen: boolean;
  // v1.9.1: active theme id; valid ids live in web/src/themes.ts. Unknown
  // values fall back to the default theme on apply.
  theme: string;
  // v1.13 Wave 3: persisted UI layout sizes. `railW` is the right-rail width
  // in px (clamped [280, 720] by the drag handle); `profilerH` is the
  // bottom-panel height in px (clamped [160, 60% of innerHeight] at drag
  // time). Both mirror the CSS vars `--rail-w` / `--profiler-h`.
  railW: number;
  profilerH: number;
  // Energy economy
  upkeepMultiplier: number;
  moveCostMultiplier: number;
  energyMax: number;
  // Sim sliders
  eatBiteFrac: number;
  grassPropagK: number;
  grassGrowthR: number;
  grassEnergyPerBite: number;
  grassBitesPerBlock: number;
  digestionCooldown: number;
  repulsionMax: number;
  maxPopulation: number;
  initialGrassSeedCount: number;
  // v2.0 Wave 1b: live-tunable biome movement-penalty base severities [0, 1].
  // Plains = 0 (implicit); Water/Desert read these. Apply to the running world.
  waterMovementPenalty: number;
  desertMovementPenalty: number;
  // v2.0 Wave 2b: live-tunable multiplier on the per-birth genome-trait
  // mutation sigma (× the active mutation bucket's sigma). Apply to the running
  // world. Must match Rust TRAIT_MUTATION_SIGMA_MULTIPLIER_DEFAULT.
  traitMutationSigmaMultiplier: number;
  mutRate: number;
  // v1.12: 8 mutation buckets × {weight, rate, sigma}. Replaces the legacy
  // single-knob nnSigma. Bucket 0 carries the legacy `(1.0, 0.02, 0.02)`.
  mutationBuckets: MutationBucket[];
  // v1.12: NN topology (hidden layers only; inputs=32, outputs=5 implicit).
  // layerSizes.length === activations.length. Both must validate at load.
  nnTopology: NnTopology;
  // Lifecycle
  maxAge: number;
  splitThreshold: number;
  splitGift: number;
  splitJitter: number;
  founderCount: number;
  // v2.0 Wave 1a: construction-only world shape (restart-required). `worldSize`
  // is the world extent (default 9600). `wrapWorld` selects torus vs walled.
  // `worldSeed` seeds the biome generator (0 ⇒ Rust picks + reports a random
  // one on first boot; the status strip then reuses it across restarts).
  worldSize: number;
  worldSeed: number;
  wrapWorld: boolean;
  // v2.0 Wave 3b: species + sexual-mating construction settings. All four are
  // CONSTRUCTION-ONLY (restart-required, toast on apply) and ride the boot call
  // (`newWithFounderCount`'s 5 trailing args), not the live slider SAB.
  //   * `speciesMode` — hard mode switch (off ⇒ single-pool Split; on ⇒ seeded
  //     species + sexual Mate). Default off.
  //   * `crossoverMode` — per-trait/per-weight crossover policy (mating mode).
  //     Carried as the Rust f32 slider encoding: 0 = average, 1 = fifty_fifty.
  //     Default fifty_fifty (1).
  //   * `startingSpeciesCount` / `startingSpeciesMemberCount` — species and
  //     founders-per-species (mating mode). Defaults 10 / 10.
  //   * `startingSpeciesMemberVariance` — multiplies the per-founder bucket draw
  //     spread (mating mode). Default 3.0.
  speciesMode: boolean;
  crossoverMode: number;
  startingSpeciesCount: number;
  startingSpeciesMemberCount: number;
  startingSpeciesMemberVariance: number;
  // v2.0 Wave 3b: live-tunable initiator-only post-mating cooldown (mating
  // mode). Apply to the running world. Must match Rust
  // MATING_COOLDOWN_TICKS_DEFAULT.
  matingCooldownTicks: number;
  // v2.0.2 Stream 1d: six scatter kernel live-tuning sliders. All live (apply
  // to the running world). Defaults must match the Rust GRASS_*_DEFAULT consts.
  grassDecayPct: number;
  grassDecayAmount: number;
  grassSpreadPct: number;
  grassSpreadAmount: number;
  grassSpreadRing1Pct: number;
  grassSpreadRing2Pct: number;
  // v2.0.4 S1: render-side LOD nudge knob. Subtracts from the computed mip
  // level in write_snapshot — positive values pick a finer pyramid level.
  // Default 0.0 (no bias). Biasing finer than the budget can hold crops edges.
  lodBias: number;
  // v2.0.4 S6: construction-only multi-band grass NN sight. ON = near+far band
  // (16 grass NN inputs); OFF = near band only (8 inputs, legacy). Default ON.
  // Must match Rust GRASS_MULTISIGHT_DEFAULT (true).
  grassMultisight: boolean;
  // v2.0.4 S2: construction-only grass cell size in world-units. Larger cells
  // → fewer cells → less grass_step work (cell count ∝ 1/size²). Restart-
  // required (resizes grass_dim, capacity[], biome grid, snapshot slot).
  // Default 5.0 (= Rust GRASS_CELL_SIZE_DEFAULT). Valid range: 5–20u.
  // Must match Rust GRASS_CELL_SIZE_DEFAULT (5.0).
  grassSize: number;
  // v2.0.6 S3: construction-only seeded grass clumps. Both knobs ride the
  // explicit boot param (same rule as grassSize). clumpCount=0 falls back to
  // the old uniform-scatter behaviour.
  // Default grassClumpCount=40, grassClumpSize=8 (Rust GRASS_CLUMP_*_DEFAULT).
  grassClumpCount: number;
  grassClumpSize: number;
  // v2.0.6 S10: discrete grass LOD stepper. LIVE setting (no restart required).
  // 0 = Auto (existing formula + lodBias), 1 = Full (~grass_dim), 2 = Half,
  // 3 = Quarter, 4 = Eighth, 5 = Sixteenth. When non-zero, forces the absolute
  // mip level and overrides lodBias. Must match Rust default (0 = Auto).
  grassLodStep: number;
  // Live multiplier on the mating contact radius (species mode). 1.0 =
  // touching-bodies default; higher lets creatures mate from further apart.
  // Must match Rust MATE_REACH_MULTIPLIER_DEFAULT (1.0).
  mateReachMultiplier: number;
  // Construction-only founder action-output boosts (applied once to founder
  // brains at construction; not per birth). >1.0 biases brand-new creatures
  // toward grazing / splitting / mating. Both default 1.0 (neutral) — match
  // Rust INIT_GRAZE_BOOST_DEFAULT / INIT_SPLIT_BOOST_DEFAULT.
  initGrazeBoost: number;
  initSplitBoost: number;
}

export const DEFAULTS: Settings = {
  vMajor: SCHEMA_MAJOR,
  vMinor: SCHEMA_MINOR,
  targetTPS: 180,
  autoRun: false,
  showProfiler: false,
  showGrass: true,
  grassOpacity: 1.0,
  biomeOpacity: 1.0,
  grassSmoothing: 0.0,
  grassSoftness: 0.0,
  grassTexture: 0.0,
  grassEdgeErosion: 0.5,
  grassShadeVariation: 0.5,
  grassBladeSize: 4.0,
  grassDensityFloor: 0.0,
  grassContrast: 1.0,
  grassBrightness: 1.0,
  profilerWindowMs: 10_000,
  railOpen: false,
  theme: "midnight",
  railW: 420,
  profilerH: 240,
  upkeepMultiplier: 1.0,
  moveCostMultiplier: 1.0,
  energyMax: 100,
  eatBiteFrac: 0.5,
  grassPropagK: 0.003,
  grassGrowthR: 0.01,
  grassEnergyPerBite: 10,
  grassBitesPerBlock: 2,
  digestionCooldown: 0,
  repulsionMax: 0.1,
  maxPopulation: 8_000,
  initialGrassSeedCount: 8000,
  // v2.0 Wave 1b: must match Rust WATER/DESERT_MOVEMENT_PENALTY_DEFAULT.
  waterMovementPenalty: 0.8,
  desertMovementPenalty: 0.4,
  // v2.0 Wave 2b: must match Rust TRAIT_MUTATION_SIGMA_MULTIPLIER_DEFAULT.
  traitMutationSigmaMultiplier: 0.3,
  mutRate: 1.0,
  mutationBuckets: DEFAULT_MUTATION_BUCKETS.map((b) => ({ ...b })),
  nnTopology: {
    layerSizes: DEFAULT_NN_TOPOLOGY.layerSizes.slice(),
    activations: DEFAULT_NN_TOPOLOGY.activations.slice(),
  },
  maxAge: 5000,
  splitThreshold: 99,
  splitGift: 30,
  splitJitter: 1,
  founderCount: 32,
  // v2.0 Wave 1a: construction-only world shape. Must match Rust
  // WORLD_SIZE_DEFAULT / WRAP_WORLD_DEFAULT. `worldSeed` 0 ⇒ "let Rust pick".
  worldSize: 9600,
  worldSeed: 0,
  wrapWorld: true,
  // v2.0 Wave 3b: species + sexual-mating construction settings. Must match the
  // Rust DevSliders defaults: SPECIES_MODE_DEFAULT (false), CROSSOVER_MODE_DEFAULT
  // (FiftyFifty ⇒ slider 1), STARTING_SPECIES_COUNT_DEFAULT (10),
  // STARTING_SPECIES_MEMBER_COUNT_DEFAULT (10),
  // STARTING_SPECIES_MEMBER_VARIANCE_DEFAULT (3.0).
  speciesMode: false,
  crossoverMode: 1,
  startingSpeciesCount: 10,
  startingSpeciesMemberCount: 10,
  startingSpeciesMemberVariance: 3.0,
  // v2.0 Wave 3b: live mating cooldown. Must match Rust
  // MATING_COOLDOWN_TICKS_DEFAULT (200).
  matingCooldownTicks: 200,
  // v2.0.2 Stream 1d: scatter kernel defaults. Must match Rust constants:
  // GRASS_DECAY_PCT_DEFAULT (0.04), GRASS_DECAY_AMOUNT_DEFAULT (0.008),
  // GRASS_SPREAD_PCT_DEFAULT (0.55), GRASS_SPREAD_AMOUNT_DEFAULT (0.05),
  // GRASS_SPREAD_RING1_PCT_DEFAULT (0.70), GRASS_SPREAD_RING2_PCT_DEFAULT (0.22).
  // v2.0.6 S3: grassDecayPct corrected from 0.02 → 0.04 to match Rust
  // GRASS_DECAY_PCT_DEFAULT (raised in v2.0.2 S4 tuning, TS side lagged).
  grassDecayPct: 0.04,
  grassDecayAmount: 0.008,
  grassSpreadPct: 0.55,
  grassSpreadAmount: 0.05,
  grassSpreadRing1Pct: 0.70,
  grassSpreadRing2Pct: 0.22,
  // v2.0.4 S1: LOD bias. Default 0.0 (no bias = computed level is used as-is).
  lodBias: 0.0,
  // v2.0.4 S6: multi-band grass sight. Must match Rust GRASS_MULTISIGHT_DEFAULT (true).
  grassMultisight: true,
  // v2.0.4 S2: grass cell size. Must match Rust GRASS_CELL_SIZE_DEFAULT (5.0).
  grassSize: 5.0,
  // v2.0.6 S3: seeded grass clumps. Must match Rust GRASS_CLUMP_COUNT_DEFAULT (40)
  // and GRASS_CLUMP_SIZE_DEFAULT (8). clumpCount=0 → old uniform-scatter fallback.
  grassClumpCount: 40,
  grassClumpSize: 8,
  // v2.0.6 S10: discrete LOD stepper. Default 0 (Auto = use formula + lodBias).
  grassLodStep: 0,
  // Mating reach + founder action-output boosts. All default 1.0 (neutral) so
  // behaviour is unchanged until dialed. Must match the Rust *_DEFAULT consts.
  mateReachMultiplier: 1.0,
  initGrazeBoost: 1.0,
  initSplitBoost: 1.0,
};

export const SETTINGS_KEYS = Object.freeze(Object.keys(DEFAULTS)) as readonly (keyof Settings)[];
export const SETTINGS_USER_KEYS = Object.freeze(
  SETTINGS_KEYS.filter((k) => k !== "vMajor" && k !== "vMinor"),
) as readonly (keyof Settings)[];

const KNOWN_KEYS = new Set<string>(SETTINGS_KEYS);

function cloneBuckets(src: readonly MutationBucket[]): MutationBucket[] {
  return src.map((b) => ({ ...b }));
}

function cloneTopology(src: NnTopology): NnTopology {
  return {
    layerSizes: src.layerSizes.slice(),
    activations: src.activations.slice(),
  };
}

function cloneSettingsDefaults(): Settings {
  return {
    ...DEFAULTS,
    mutationBuckets: cloneBuckets(DEFAULTS.mutationBuckets),
    nnTopology: cloneTopology(DEFAULTS.nnTopology),
  };
}

function cloneSettingValue<K extends keyof Settings>(key: K, value: Settings[K]): Settings[K] {
  if (key === "mutationBuckets") {
    return cloneBuckets(value as MutationBucket[]) as Settings[K];
  }
  if (key === "nnTopology") {
    return cloneTopology(value as NnTopology) as Settings[K];
  }
  return value;
}

function pickKnown(raw: unknown): Partial<Settings> {
  if (typeof raw !== "object" || raw === null) return {};
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (!KNOWN_KEYS.has(k)) continue;
    if (k === "mutationBuckets") {
      const sanitised = sanitiseBuckets(v);
      if (sanitised) out[k] = sanitised;
      continue;
    }
    if (k === "nnTopology") {
      const sanitised = sanitiseTopology(v);
      if (sanitised) out[k] = sanitised;
      continue;
    }
    out[k] = v;
  }
  return out as Partial<Settings>;
}

function sanitiseBuckets(raw: unknown): MutationBucket[] | null {
  if (!Array.isArray(raw) || raw.length !== MUTATION_BUCKET_COUNT) return null;
  const out: MutationBucket[] = [];
  for (const entry of raw) {
    if (typeof entry !== "object" || entry === null) return null;
    const e = entry as Record<string, unknown>;
    const w = Number(e.weight); const r = Number(e.rate); const s = Number(e.sigma);
    if (!isFinite(w) || !isFinite(r) || !isFinite(s)) return null;
    out.push({
      weight: Math.max(0, w),
      rate: Math.min(1, Math.max(0, r)),
      sigma: Math.max(0, s),
    });
  }
  return out;
}

function sanitiseTopology(raw: unknown): NnTopology | null {
  if (typeof raw !== "object" || raw === null) return null;
  const t = raw as Record<string, unknown>;
  const sizes = t.layerSizes;
  const acts = t.activations;
  if (!Array.isArray(sizes) || !Array.isArray(acts)) return null;
  if (sizes.length === 0 || sizes.length > 8) return null;
  if (sizes.length !== acts.length) return null;
  const layerSizes: number[] = [];
  for (const s of sizes) {
    const n = Math.round(Number(s));
    if (!isFinite(n) || n < 8 || n > 256 || n % 8 !== 0) return null;
    layerSizes.push(n);
  }
  const activations: ActivationName[] = [];
  for (const a of acts) {
    if (typeof a !== "string" || !VALID_ACTIVATIONS.has(a)) return null;
    activations.push(a as ActivationName);
  }
  return { layerSizes, activations };
}

function readFromStorage(): Partial<Settings> {
  try {
    const raw = localStorage.getItem(SETTINGS_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    // v2.0 Wave 1c major/minor migration:
    //   * A MAJOR mismatch (or a missing `vMajor`, i.e. a legacy v1 blob)
    //     resets to defaults — drop the whole stored blob.
    //   * A MINOR mismatch keeps the user's values; the `{...DEFAULTS,
    //     ...stored}` merge below fills any keys the older blob lacked.
    const storedMajor =
      typeof parsed === "object" && parsed !== null
        ? Number((parsed as Record<string, unknown>).vMajor)
        : NaN;
    if (storedMajor !== SCHEMA_MAJOR) {
      // Major mismatch / unknown schema → clean reset to defaults.
      return {};
    }
    return pickKnown(parsed);
  } catch {
    return {};
  }
}

const storedAtLoad = readFromStorage();

/** True iff `key` was present in localStorage at module load time. Lets boot
 * code apply environment-derived defaults (e.g. lowering `maxPopulation` on
 * low-core devices) without clobbering an explicit user choice. */
export function hasStoredSetting<K extends keyof Settings>(key: K): boolean {
  return Object.prototype.hasOwnProperty.call(storedAtLoad, key);
}

// Live, in-memory copy. Persisted on every write. The version stamp is always
// rewritten to the current major.minor so a future load sees a coherent
// schema marker (and a minor-only bump merges cleanly on the NEXT load).
const current: Settings = {
  ...cloneSettingsDefaults(),
  ...storedAtLoad,
  vMajor: SCHEMA_MAJOR,
  vMinor: SCHEMA_MINOR,
};

function persist(): void {
  try {
    localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Best-effort: ignore quota / disabled storage.
  }
}

export function getSettings(): Readonly<Settings> {
  return current;
}

export function setSetting<K extends keyof Settings>(key: K, value: Settings[K]): void {
  current[key] = cloneSettingValue(key, value);
  persist();
}

/** Reset every setting to its canonical default and persist. Used by the
 * Settings tab's Reset button. */
export function resetSettings(): void {
  Object.assign(current, cloneSettingsDefaults());
  persist();
}

if (import.meta.env?.DEV && typeof window !== "undefined") {
  (window as unknown as {
    __evosimSettingsForTests?: {
      getSettings: typeof getSettings;
      setSetting: typeof setSetting;
      storageKey: string;
    };
  }).__evosimSettingsForTests = {
    getSettings,
    setSetting,
    storageKey: SETTINGS_STORAGE_KEY,
  };
}
