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

export interface Settings {
  v: number;
  targetTPS: number;
  autoRun: boolean;
  // Display toggles (live-apply, page-side only)
  showProfiler: boolean;
  showPopGraph: boolean;
  showGrass: boolean;
  grassOpacity: number;
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
  initialGrassSeedCount: number;
  fullGrassOnInit: boolean;
  mutRate: number;
  nnSigma: number;
  // Lifecycle
  maxAge: number;
  splitThreshold: number;
  splitGift: number;
  splitJitter: number;
  founderCount: number;
  // Curriculum
  curriculumMinPop: number;
  curriculumMaxPop: number;
  curriculumMinFactor: number;
  autoCurriculum: boolean;
}

export const DEFAULTS: Settings = {
  v: SCHEMA_VERSION,
  targetTPS: 60,
  autoRun: false,
  showProfiler: true,
  showPopGraph: true,
  showGrass: true,
  grassOpacity: 1.0,
  railOpen: true,
  theme: "charcoal",
  upkeepMultiplier: 1.0,
  moveCostMultiplier: 1.0,
  energyMax: 100,
  eatBiteFrac: 0.5,
  grassPropagK: 0.001,
  grassGrowthR: 0.05,
  grassEnergyPerBite: 10,
  grassBitesPerBlock: 2,
  digestionCooldown: 50,
  repulsionMax: 5.0,
  initialGrassSeedCount: 100,
  fullGrassOnInit: false,
  mutRate: 1.0,
  nnSigma: 0.02,
  maxAge: 5000,
  splitThreshold: 50,
  splitGift: 30,
  splitJitter: 50,
  founderCount: 8,
  curriculumMinPop: 1000,
  curriculumMaxPop: 2000,
  curriculumMinFactor: 0.0,
  autoCurriculum: true,
};

const KNOWN_KEYS = new Set<string>(Object.keys(DEFAULTS));

function pickKnown(raw: unknown): Partial<Settings> {
  if (typeof raw !== "object" || raw === null) return {};
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (KNOWN_KEYS.has(k)) out[k] = v;
  }
  return out as Partial<Settings>;
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

// Live, in-memory copy. Persisted on every write.
const current: Settings = {
  ...DEFAULTS,
  ...readFromStorage(),
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
