// Persistent dev-panel + pacing settings. Single JSON blob under one
// localStorage key. The Settings shape mirrors the live-tunable sliders in
// the dev panel; defaults must agree with the Rust `*_DEFAULT` constants
// (drift-guarded by a Playwright e2e against `sliders_defaults_json()`).
//
// On load: unknown keys in the stored blob are silently discarded so an
// older session whose schema diverged (e.g. v1.8's `eatCostMultiplier`) can't
// fail typed parsing or carry stale state into a newer session.

const STORAGE_KEY = "evosim.settings.v1";
const SCHEMA_VERSION = 1;

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
  v: number;
  targetTPS: number;
  autoRun: boolean;
  // Display toggles (live-apply, page-side only)
  showProfiler: boolean;
  showGrass: boolean;
  grassOpacity: number;
  // Profiler rolling-window length in ms. Drives both the Rust trees (via
  // CTRL_PROFILE_WINDOW_MS) and the TS `frame` tree (via setProfilerWindowMs).
  profilerWindowMs: number;
  // v1.9.1: right-rail open/closed; toggled by the ⚙ button + `~` hotkey.
  // Forced back open by a creature click (inspector flow).
  railOpen: boolean;
  // v1.9.1: active theme id; valid ids live in web/src/themes.ts. Unknown
  // values fall back to the default theme on apply.
  theme: string;
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
  fullGrassOnInit: boolean;
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
}

export const DEFAULTS: Settings = {
  v: SCHEMA_VERSION,
  targetTPS: 60,
  autoRun: false,
  showProfiler: true,
  showGrass: true,
  grassOpacity: 1.0,
  profilerWindowMs: 10_000,
  railOpen: false,
  theme: "midnight",
  upkeepMultiplier: 1.0,
  moveCostMultiplier: 1.0,
  energyMax: 100,
  eatBiteFrac: 0.5,
  grassPropagK: 0.001,
  grassGrowthR: 0.05,
  grassEnergyPerBite: 10,
  grassBitesPerBlock: 2,
  digestionCooldown: 50,
  repulsionMax: 0.1,
  maxPopulation: 8_000,
  initialGrassSeedCount: 1000,
  fullGrassOnInit: false,
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
};

const KNOWN_KEYS = new Set<string>(Object.keys(DEFAULTS));

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
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    return pickKnown(JSON.parse(raw));
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

// Live, in-memory copy. Persisted on every write.
const current: Settings = {
  ...DEFAULTS,
  ...storedAtLoad,
  v: SCHEMA_VERSION,
};

function persist(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Best-effort: ignore quota / disabled storage.
  }
}

export function getSettings(): Readonly<Settings> {
  return current;
}

export function setSetting<K extends keyof Settings>(key: K, value: Settings[K]): void {
  current[key] = value;
  persist();
}

/** Reset every setting to its canonical default and persist. Used by the
 * Settings tab's Reset button. */
export function resetSettings(): void {
  Object.assign(current, DEFAULTS);
  persist();
}
