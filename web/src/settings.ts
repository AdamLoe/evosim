// Persistent dev-panel + pacing settings. Single JSON blob under one
// localStorage key, schema-versioned so we can evolve the shape later.
// Missing fields fall back to DEFAULTS, so adding a new setting won't
// break existing users' saved blobs.

const STORAGE_KEY = "evosim.settings.v1";
const SCHEMA_VERSION = 1;

export interface Settings {
  v: number;
  targetTPS: number;
  autoRun: boolean;
  // Display toggles
  showProfiler: boolean;
  showPopGraph: boolean;
  showGrass: boolean;
  // Energy economy
  upkeepMultiplier: number;
  moveCostMultiplier: number;
  eatCostMultiplier: number;
  energyMax: number;
  // Sim sliders
  eatBiteFrac: number;
  grassPropagK: number;
  grassGrowthR: number;
  grassEnergyPerBite: number;
  grassBitesPerBlock: number;
  initialGrassSeedCount: number;
  mutRate: number;
  nnSigma: number;
}

const DEFAULTS: Settings = {
  v: SCHEMA_VERSION,
  targetTPS: 60,
  autoRun: false,
  showProfiler: false,
  showPopGraph: false,
  showGrass: true,
  upkeepMultiplier: 1.0,
  moveCostMultiplier: 1.0,
  eatCostMultiplier: 1.0,
  energyMax: 100,
  eatBiteFrac: 0.5,
  grassPropagK: 0.001,
  grassGrowthR: 0.05,
  grassEnergyPerBite: 10,
  grassBitesPerBlock: 2,
  initialGrassSeedCount: 100,
  mutRate: 1.0,
  nnSigma: 0.02,
};

function readFromStorage(): Partial<Settings> | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    return parsed as Partial<Settings>;
  } catch {
    // localStorage may be unavailable (private mode, quota, disabled);
    // fall back to defaults silently.
    return null;
  }
}

// Live, in-memory copy. Persisted on every write.
const current: Settings = (() => {
  const stored = readFromStorage();
  if (!stored) return { ...DEFAULTS };
  // Merge defaults with stored values so newly-added keys get a default.
  return { ...DEFAULTS, ...stored, v: SCHEMA_VERSION };
})();

function persist(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Best-effort: ignore quota / disabled storage. The in-memory copy
    // still works for this session.
  }
}

export function getSettings(): Readonly<Settings> {
  return current;
}

export function setSetting<K extends keyof Settings>(key: K, value: Settings[K]): void {
  current[key] = value;
  persist();
}

/** Wipe stored settings and revert to defaults. Useful for a debug button. */
export function resetSettings(): void {
  Object.assign(current, DEFAULTS);
  persist();
}
