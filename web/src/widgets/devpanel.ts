// Settings tab installer for the right rail.
//
// Two interaction tiers:
//
// 1. **Live-apply** (run / display groups): widget edits push to the sim
//    worker and write `localStorage` immediately. No dirty tracking.
// 2. **Stage-then-apply** (sim / energy / lifecycle / curriculum groups):
//    widget edits update only the in-memory widget value. The Settings tab
//    footer's Apply pushes every dirty staged slider via `set_slider` and
//    persists to `localStorage`; Cancel restores widgets to the last-applied
//    snapshot; Reset restores to `DEFAULTS` and pushes/persists those.
//
// Construction-only knobs (`founder_count`, `energy_max`,
// `initial_grass_seed_count`, `full_grass_on_init`) ride the same staged
// path but only shape the *next* world — Apply/Reset fire a toast when any
// were committed.
//
// `currentSliderState()` snapshots the live widget values for the boot
// payload (`initial_sliders`), so a mid-drag restart carries the dragged
// value (not the last-applied one).

import type { SimBridge } from "../sim-bridge";
import { getTargetTPS, setTpsChangeListener } from "../main";
import { getSettings, setSetting, resetSettings, DEFAULTS, type Settings } from "../settings";
import { setProfilerVisible } from "./perf-panel";
import { showToast } from "../toast";
import { THEMES, applyTheme } from "../themes";

// Construction-only sim slider names. Edits land in DevSliders via
// set_slider so they round-trip through restart, but the active world keeps
// whatever it spawned with until restart.
const CONSTRUCTION_ONLY_SLIDERS = new Set<string>([
  "founder_count",
  "energy_max",
  "grass_initial_seed_count",
  "full_grass_on_init",
]);

const TOAST_CONSTRUCTION =
  "Some changes only take effect on new simulations.";

// ─── Boot-payload accessors ───────────────────────────────────────────────

export function getInitialGrassSeedCount(): number {
  return widgetReaders.get("grass_initial_seed_count")?.() ?? getSettings().initialGrassSeedCount;
}
export function getEnergyMax(): number {
  return widgetReaders.get("energy_max")?.() ?? getSettings().energyMax;
}
export function getFounderCount(): number {
  return widgetReaders.get("founder_count")?.() ?? getSettings().founderCount;
}
export function getFullGrassOnInit(): boolean {
  const v = widgetReaders.get("full_grass_on_init")?.();
  if (v !== undefined) return v !== 0;
  return getSettings().fullGrassOnInit;
}

// ─── Live widget state (drives currentSliderState) ────────────────────────

type SliderReader = () => number;
const widgetReaders = new Map<string, SliderReader>();

function registerWidget(name: string, reader: SliderReader): void {
  widgetReaders.set(name, reader);
}

export function currentSliderState(): Record<string, number> {
  const out: Record<string, number> = {};
  for (const [name, read] of widgetReaders) {
    out[name] = read();
  }
  return out;
}

// ─── Dirty tracking (staged sliders only) ─────────────────────────────────

interface StagedHandle {
  /** Rust slider name. */
  simName: string;
  /** Settings key the value mirrors. */
  settingKey: keyof Settings;
  /** Read the current widget value (numeric; bools encode as 0|1). */
  readWidget: () => number;
  /** Write a value into the widget (updates UI + readout); does NOT push to sim. */
  writeWidget: (value: number) => void;
  /** The last-applied value (initially = persisted setting at install). */
  snapshot: number;
  /** Per-row container so we can toggle the .is-dirty CSS class. */
  rowEl: HTMLDivElement;
}

const stagedHandles: StagedHandle[] = [];
let footerApply: HTMLButtonElement | null = null;
let footerCancel: HTMLButtonElement | null = null;

function settingToNumber(key: keyof Settings): number {
  const v = getSettings()[key] as unknown;
  if (typeof v === "boolean") return v ? 1 : 0;
  return v as number;
}

function isDirty(h: StagedHandle): boolean {
  return Math.abs(h.readWidget() - h.snapshot) > 1e-9;
}

function refreshDirtyState(): void {
  let anyDirty = false;
  for (const h of stagedHandles) {
    const dirty = isDirty(h);
    h.rowEl.classList.toggle("is-dirty", dirty);
    if (dirty) anyDirty = true;
  }
  if (footerApply) footerApply.disabled = !anyDirty;
  if (footerCancel) footerCancel.disabled = !anyDirty;
}

function applyAll(simBridge: SimBridge): void {
  let anyConstructionOnly = false;
  for (const h of stagedHandles) {
    if (!isDirty(h)) continue;
    const v = h.readWidget();
    // Persist via the keyed setting.
    const currentValue = getSettings()[h.settingKey];
    if (typeof currentValue === "boolean") {
      setSetting(h.settingKey as keyof Settings, (v !== 0) as never);
    } else {
      setSetting(h.settingKey as keyof Settings, v as never);
    }
    simBridge.debouncedSetSlider(h.simName, v);
    h.snapshot = v;
    if (CONSTRUCTION_ONLY_SLIDERS.has(h.simName)) anyConstructionOnly = true;
  }
  if (anyConstructionOnly) showToast(TOAST_CONSTRUCTION);
  refreshDirtyState();
}

function cancelAll(): void {
  for (const h of stagedHandles) {
    if (!isDirty(h)) continue;
    h.writeWidget(h.snapshot);
  }
  refreshDirtyState();
}

function resetAll(simBridge: SimBridge): void {
  let anyConstructionOnly = false;
  resetSettings();
  for (const h of stagedHandles) {
    const defaultVal = (DEFAULTS as unknown as Record<string, number | boolean>)[h.settingKey];
    const v = typeof defaultVal === "boolean" ? (defaultVal ? 1 : 0) : (defaultVal as number);
    h.writeWidget(v);
    simBridge.debouncedSetSlider(h.simName, v);
    if (h.snapshot !== v && CONSTRUCTION_ONLY_SLIDERS.has(h.simName)) {
      anyConstructionOnly = true;
    }
    h.snapshot = v;
  }
  // Live-apply widgets (run + display) also reset via the resetSettings() call
  // above; their UI sync happens through liveSyncers registered below.
  for (const sync of liveSyncers) sync();
  if (anyConstructionOnly) showToast(TOAST_CONSTRUCTION);
  refreshDirtyState();
}

const liveSyncers: Array<() => void> = [];

// ─── Slider / toggle builders ─────────────────────────────────────────────

interface SliderSpec {
  label: string;
  /** Rust slider name + a Settings key that mirrors it. Both required for
   *  staged sliders; live sliders pass `simName: null` and only persist. */
  simName: string | null;
  settingKey: keyof Settings;
  min: number;
  max: number;
  step: number;
  formatValue?: (v: number) => string;
  /** Mark this row visually as "(next world)" — construction-only knobs. */
  nextWorld?: boolean;
}

interface SliderHooks {
  /** Re-fired on every input event (for read-only readouts that depend on
   *  this slider, e.g. the upkeep /s readout). */
  onInput?: (v: number) => void;
  /** Called on every input event AND on programmatic writeWidget so live
   *  sliders can apply side-effects (e.g. opacity setting). */
  onApply?: (v: number) => void;
}

function makeStagedSlider(
  spec: SliderSpec,
  hooks: SliderHooks = {},
): HTMLDivElement {
  const initial = settingToNumber(spec.settingKey);
  const row = document.createElement("div");
  row.className = "devpanel-row";

  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;
  if (spec.nextWorld) labelEl.classList.add("next-world");

  const slider = document.createElement("input");
  slider.type = "range";
  slider.min = String(spec.min);
  slider.max = String(spec.max);
  slider.step = String(spec.step);
  slider.value = String(initial);

  const numInput = document.createElement("input");
  numInput.type = "number";
  numInput.step = String(spec.step);
  numInput.value = String(initial);
  numInput.className = "devpanel-numinput";

  const readout = document.createElement("span");
  readout.className = "devpanel-readout";
  const fmt = spec.formatValue ?? ((v: number) => v.toFixed(3));
  readout.textContent = fmt(initial);

  const writeWidget = (v: number): void => {
    readout.textContent = fmt(v);
    numInput.value = String(v);
    slider.value = String(Math.max(spec.min, Math.min(spec.max, v)));
    hooks.onApply?.(v);
  };

  const onChange = (v: number, source: "slider" | "num"): void => {
    readout.textContent = fmt(v);
    if (source === "slider") {
      numInput.value = String(v);
    } else {
      slider.value = String(Math.max(spec.min, Math.min(spec.max, v)));
    }
    hooks.onInput?.(v);
    refreshDirtyState();
  };

  slider.addEventListener("input", () => onChange(Number(slider.value), "slider"));
  numInput.addEventListener("input", () => {
    const v = Number(numInput.value);
    if (Number.isFinite(v)) onChange(v, "num");
  });

  if (spec.simName) {
    registerWidget(spec.simName, () => Number(numInput.value));
    stagedHandles.push({
      simName: spec.simName,
      settingKey: spec.settingKey,
      readWidget: () => Number(numInput.value),
      writeWidget,
      snapshot: initial,
      rowEl: row,
    });
  }

  row.append(labelEl, slider, numInput, readout);
  return row;
}

interface ToggleSpec {
  label: string;
  simName: string | null;
  settingKey: keyof Settings;
  nextWorld?: boolean;
}

function makeStagedToggle(spec: ToggleSpec): HTMLDivElement {
  const initial = settingToNumber(spec.settingKey) !== 0;
  const row = document.createElement("div");
  row.className = "devpanel-row";

  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;
  if (spec.nextWorld) labelEl.classList.add("next-world");

  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = initial;

  const spacer1 = document.createElement("span");
  const spacer2 = document.createElement("span");

  const writeWidget = (v: number): void => {
    input.checked = v !== 0;
  };

  input.addEventListener("change", () => refreshDirtyState());

  if (spec.simName) {
    registerWidget(spec.simName, () => (input.checked ? 1 : 0));
    stagedHandles.push({
      simName: spec.simName,
      settingKey: spec.settingKey,
      readWidget: () => (input.checked ? 1 : 0),
      writeWidget,
      snapshot: initial ? 1 : 0,
      rowEl: row,
    });
  }

  row.append(labelEl, spacer1, spacer2, input);
  return row;
}

// Live-apply sliders / toggles. These don't participate in dirty tracking;
// edits hit `setSetting` and `onApply` immediately.
function makeLiveSlider(
  spec: SliderSpec,
  hooks: SliderHooks = {},
): HTMLDivElement {
  const initial = settingToNumber(spec.settingKey);
  const row = document.createElement("div");
  row.className = "devpanel-row";

  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;

  const slider = document.createElement("input");
  slider.type = "range";
  slider.min = String(spec.min);
  slider.max = String(spec.max);
  slider.step = String(spec.step);
  slider.value = String(initial);

  const numInput = document.createElement("input");
  numInput.type = "number";
  numInput.step = String(spec.step);
  numInput.value = String(initial);
  numInput.className = "devpanel-numinput";

  const readout = document.createElement("span");
  readout.className = "devpanel-readout";
  const fmt = spec.formatValue ?? ((v: number) => v.toFixed(2));
  readout.textContent = fmt(initial);

  const apply = (v: number, source: "slider" | "num") => {
    readout.textContent = fmt(v);
    if (source === "slider") numInput.value = String(v);
    else slider.value = String(Math.max(spec.min, Math.min(spec.max, v)));
    setSetting(spec.settingKey as keyof Settings, v as never);
    hooks.onApply?.(v);
    hooks.onInput?.(v);
  };

  slider.addEventListener("input", () => apply(Number(slider.value), "slider"));
  numInput.addEventListener("input", () => {
    const v = Number(numInput.value);
    if (Number.isFinite(v)) apply(v, "num");
  });

  // Allow Reset to re-sync the widget if defaults change.
  liveSyncers.push(() => {
    const v = settingToNumber(spec.settingKey);
    slider.value = String(v);
    numInput.value = String(v);
    readout.textContent = fmt(v);
    hooks.onApply?.(v);
  });

  row.append(labelEl, slider, numInput, readout);
  return row;
}

/**
 * v1.9.1: Display-group theme dropdown. Reads options from `THEMES`, persists
 * to `settings.theme`, and live-applies via `applyTheme` so the palette
 * switches the moment the user changes the selection. No Rust side — themes
 * are pure CSS-var swaps.
 */
function makeThemeRow(): HTMLDivElement {
  const row = document.createElement("div");
  row.className = "devpanel-row devpanel-row-wide";

  const labelEl = document.createElement("label");
  labelEl.textContent = "Theme";

  const select = document.createElement("select");
  for (const t of Object.values(THEMES)) {
    const opt = document.createElement("option");
    opt.value = t.id;
    opt.textContent = t.name;
    select.appendChild(opt);
  }
  select.value = getSettings().theme;

  select.addEventListener("change", () => {
    setSetting("theme", select.value);
    applyTheme(select.value);
  });

  liveSyncers.push(() => {
    select.value = getSettings().theme;
    applyTheme(select.value);
  });

  row.append(labelEl, select);
  return row;
}

function makeLiveToggle(
  spec: ToggleSpec,
  onApply: (v: boolean) => void,
): HTMLDivElement {
  const initial = settingToNumber(spec.settingKey) !== 0;
  const row = document.createElement("div");
  row.className = "devpanel-row";

  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;

  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = initial;
  const spacer1 = document.createElement("span");
  const spacer2 = document.createElement("span");

  input.addEventListener("change", () => {
    setSetting(spec.settingKey as keyof Settings, input.checked as never);
    onApply(input.checked);
  });

  // Apply initial state immediately so the UI matches the persisted setting.
  onApply(initial);

  liveSyncers.push(() => {
    const v = settingToNumber(spec.settingKey) !== 0;
    input.checked = v;
    onApply(v);
  });

  row.append(labelEl, spacer1, spacer2, input);
  return row;
}

// ─── Section helpers ──────────────────────────────────────────────────────

function section(title: string): HTMLDivElement {
  const wrap = document.createElement("div");
  wrap.className = "devpanel-section";
  const hdr = document.createElement("div");
  hdr.className = "devpanel-section-header";
  hdr.textContent = title;
  wrap.appendChild(hdr);
  return wrap;
}

// Default per-tick upkeep at multiplier=1 — match the Rust sum
// UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + UPKEEP_MOUTH_DEFAULT.
const UPKEEP_PER_TICK_DEFAULT = 0.17;

// ─── Install ──────────────────────────────────────────────────────────────

export function installDevPanel(simBridge: SimBridge): void {
  const box = document.getElementById("devpanel-box") as HTMLDivElement | null;
  if (!box) return;

  // ── Run (live) ──
  const runSec = section("Run");
  runSec.appendChild(makeLiveToggle(
    { label: "Auto run on world end", simName: null, settingKey: "autoRun" },
    () => { /* read on demand */ },
  ));
  box.appendChild(runSec);

  // ── Display (live) ──
  const displaySec = section("Display");
  displaySec.appendChild(makeLiveToggle(
    { label: "Show profiler", simName: null, settingKey: "showProfiler" },
    (v) => setProfilerVisible(v),
  ));
  displaySec.appendChild(makeLiveToggle(
    { label: "Show population graph", simName: null, settingKey: "showPopGraph" },
    () => { /* graph lives in Monitor tab — toggle is informational for now */ },
  ));
  displaySec.appendChild(makeLiveToggle(
    { label: "Show grass", simName: null, settingKey: "showGrass" },
    () => { /* renderer reads getSettings() each frame */ },
  ));
  displaySec.appendChild(makeLiveSlider({
    label: "Grass opacity",
    simName: null,
    settingKey: "grassOpacity",
    min: 0, max: 1, step: 0.05,
    formatValue: (v) => v.toFixed(2),
  }));
  displaySec.appendChild(makeThemeRow());
  box.appendChild(displaySec);

  // ── Energy ──
  const energySec = section("Energy");

  // Upkeep with /s readout that depends on TPS too.
  const initialUpkeep = getSettings().upkeepMultiplier;
  const upkeepRow = makeStagedSlider({
    label: "Basic upkeep",
    simName: "upkeep_multiplier",
    settingKey: "upkeepMultiplier",
    min: 0, max: 2, step: 0.01,
    formatValue: (v) => `${(v * UPKEEP_PER_TICK_DEFAULT * getTargetTPS()).toFixed(1)} /s`,
  });
  energySec.appendChild(upkeepRow);
  setTpsChangeListener(() => {
    const readout = upkeepRow.querySelector(".devpanel-readout") as HTMLSpanElement | null;
    if (readout) {
      const v = Number((upkeepRow.querySelector("input[type=number]") as HTMLInputElement).value);
      readout.textContent = `${(v * UPKEEP_PER_TICK_DEFAULT * getTargetTPS()).toFixed(1)} /s`;
    }
  });
  void initialUpkeep;

  energySec.appendChild(makeStagedSlider({
    label: "Move cost",
    simName: "move_cost_multiplier",
    settingKey: "moveCostMultiplier",
    min: 0, max: 2, step: 0.01,
    formatValue: (v) => `${v.toFixed(2)}×`,
  }));
  energySec.appendChild(makeStagedSlider({
    label: "Energy max",
    simName: "energy_max",
    settingKey: "energyMax",
    min: 50, max: 400, step: 1,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  }, {
    onInput: () => updateSplitThresholdCap(),
  }));
  box.appendChild(energySec);

  // ── Grass ──
  const grassSec = section("Grass");
  grassSec.appendChild(makeStagedSlider({
    label: "Energy per bite",
    simName: "grass_energy_per_bite",
    settingKey: "grassEnergyPerBite",
    min: 0.1, max: 100, step: 0.1,
    formatValue: (v) => v.toFixed(1),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Bites per block",
    simName: "grass_bites_per_block",
    settingKey: "grassBitesPerBlock",
    min: 1, max: 10, step: 1,
    formatValue: (v) => String(Math.round(v)),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Growth rate r",
    simName: "grass_in_cell_growth_r",
    settingKey: "grassGrowthR",
    min: 0, max: 0.05, step: 0.001,
    formatValue: (v) => v.toFixed(3),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Propagation k",
    simName: "grass_propagation_rate_k",
    settingKey: "grassPropagK",
    min: 0, max: 0.005, step: 0.0001,
    formatValue: (v) => v.toFixed(4),
  }));
  grassSec.appendChild(makeStagedToggle({
    label: "Full grass on new world",
    simName: "full_grass_on_init",
    settingKey: "fullGrassOnInit",
    nextWorld: true,
  }));
  const initialSeedRow = makeStagedSlider({
    label: "Initial seeded cells",
    simName: "grass_initial_seed_count",
    settingKey: "initialGrassSeedCount",
    min: 0, max: 8000, step: 1,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  });
  grassSec.appendChild(initialSeedRow);
  // Disable initialSeedRow when full_grass_on_init is true (and update live).
  function syncFullGrassDisable(): void {
    const v = widgetReaders.get("full_grass_on_init")?.();
    initialSeedRow.classList.toggle("is-disabled", v === 1);
  }
  syncFullGrassDisable();
  // Re-run sync whenever any widget changes — cheap.
  box.addEventListener("input", syncFullGrassDisable);
  box.addEventListener("change", syncFullGrassDisable);
  box.appendChild(grassSec);

  // ── Eat ──
  const eatSec = section("Eat");
  eatSec.appendChild(makeStagedSlider({
    label: "Bite fraction",
    simName: "eat_bite_fraction",
    settingKey: "eatBiteFrac",
    min: 0, max: 1, step: 0.01,
    formatValue: (v) => v.toFixed(2),
  }));
  eatSec.appendChild(makeStagedSlider({
    label: "Digestion cooldown",
    simName: "digestion_cooldown",
    settingKey: "digestionCooldown",
    min: 0, max: 500, step: 1,
    formatValue: (v) => `${Math.round(v)} ticks`,
  }));
  box.appendChild(eatSec);

  // ── Lifecycle ──
  const lifeSec = section("Lifecycle");
  lifeSec.appendChild(makeStagedSlider({
    label: "Max age",
    simName: "max_age",
    settingKey: "maxAge",
    min: 500, max: 20000, step: 100,
    formatValue: (v) => String(Math.round(v)),
  }));
  const splitThresholdRow = makeStagedSlider({
    label: "Split threshold",
    simName: "split_threshold",
    settingKey: "splitThreshold",
    min: 10, max: 200, step: 1,
    formatValue: (v) => String(Math.round(v)),
  });
  lifeSec.appendChild(splitThresholdRow);
  lifeSec.appendChild(makeStagedSlider({
    label: "Split gift",
    simName: "split_gift",
    settingKey: "splitGift",
    min: 1, max: 100, step: 1,
    formatValue: (v) => String(Math.round(v)),
  }));
  lifeSec.appendChild(makeStagedSlider({
    label: "Split distance",
    simName: "split_jitter",
    settingKey: "splitJitter",
    min: 0, max: 200, step: 1,
    formatValue: (v) => String(Math.round(v)),
  }));
  lifeSec.appendChild(makeStagedSlider({
    label: "Founder count",
    simName: "founder_count",
    settingKey: "founderCount",
    min: 1, max: 32, step: 1,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  }));
  lifeSec.appendChild(makeStagedSlider({
    label: "Mutation rate ×",
    simName: "mutation_rate_multiplier",
    settingKey: "mutRate",
    min: 0, max: 5, step: 0.05,
    formatValue: (v) => v.toFixed(2),
  }));
  lifeSec.appendChild(makeStagedSlider({
    label: "NN σ",
    simName: "nn_mutation_sigma",
    settingKey: "nnSigma",
    min: 0, max: 0.2, step: 0.001,
    formatValue: (v) => v.toFixed(3),
  }));
  lifeSec.appendChild(makeStagedSlider({
    label: "Repulsion max",
    simName: "repulsion_max",
    settingKey: "repulsionMax",
    min: 0, max: 10, step: 0.1,
    formatValue: (v) => v.toFixed(1),
  }));
  box.appendChild(lifeSec);

  // Dynamic split-threshold cap = energy_max - 1.
  function updateSplitThresholdCap(): void {
    const energyMaxInput = energySec.querySelectorAll("input[type=number]")[2] as
      | HTMLInputElement
      | undefined;
    if (!energyMaxInput) return;
    const cap = Math.max(1, Math.round(Number(energyMaxInput.value)) - 1);
    const slider = splitThresholdRow.querySelector("input[type=range]") as HTMLInputElement;
    const num = splitThresholdRow.querySelector("input[type=number]") as HTMLInputElement;
    const readout = splitThresholdRow.querySelector(".devpanel-readout") as HTMLSpanElement;
    slider.max = String(cap);
    if (Number(num.value) > cap) {
      num.value = String(cap);
      slider.value = String(cap);
      readout.textContent = String(cap);
      refreshDirtyState();
    }
  }
  updateSplitThresholdCap();

  // ── Curriculum ──
  const currSec = section("Curriculum");
  currSec.appendChild(makeStagedToggle({
    label: "Auto curriculum",
    simName: "auto_curriculum",
    settingKey: "autoCurriculum",
  }));
  currSec.appendChild(makeStagedSlider({
    label: "Min pop",
    simName: "curriculum_min_pop",
    settingKey: "curriculumMinPop",
    min: 0, max: 5000, step: 50,
    formatValue: (v) => String(Math.round(v)),
  }));
  currSec.appendChild(makeStagedSlider({
    label: "Max pop",
    simName: "curriculum_max_pop",
    settingKey: "curriculumMaxPop",
    min: 0, max: 5000, step: 50,
    formatValue: (v) => String(Math.round(v)),
  }));
  currSec.appendChild(makeStagedSlider({
    label: "Min factor",
    simName: "curriculum_min_factor",
    settingKey: "curriculumMinFactor",
    min: 0, max: 1, step: 0.05,
    formatValue: (v) => v.toFixed(2),
  }));
  box.appendChild(currSec);

  // ── Footer wiring ──
  footerApply = document.getElementById("settings-apply") as HTMLButtonElement | null;
  footerCancel = document.getElementById("settings-cancel") as HTMLButtonElement | null;
  const footerReset = document.getElementById("settings-reset") as HTMLButtonElement | null;
  if (footerApply) footerApply.addEventListener("click", () => applyAll(simBridge));
  if (footerCancel) footerCancel.addEventListener("click", () => cancelAll());
  if (footerReset) footerReset.addEventListener("click", () => resetAll(simBridge));
  refreshDirtyState();
}
