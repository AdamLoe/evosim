// Settings tab installer for the right rail.
//
// Two interaction tiers:
//
// 1. **Live-apply** (run / display groups): widget edits push to the sim
//    worker and write `localStorage` immediately. No dirty tracking.
// 2. **Stage-then-apply** (sim / energy / lifecycle groups):
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
import { showToast } from "../toast";
import { THEMES, applyTheme } from "../themes";

// Construction-only sim slider names. Edits land in DevSliders via
// set_slider so they round-trip through restart, but the active world keeps
// whatever it spawned with until restart.
// v2.0 Wave 1c: dropped `full_grass_on_init`; added the world-shape knobs
// (`world_size`, `world_seed`, `wrap_world`).
// v2.0 Wave 3b: the species/mating construction settings join the set
// (`species_mode`, `crossover_mode`, `starting_species_count`,
// `starting_species_member_count`, `starting_species_member_variance`). Note
// `mating_cooldown_ticks` is LIVE — it is deliberately NOT listed here.
const CONSTRUCTION_ONLY_SLIDERS = new Set<string>([
  "founder_count",
  "energy_max",
  "grass_initial_seed_count",
  "world_size",
  "world_seed",
  "wrap_world",
  "species_mode",
  "crossover_mode",
  "starting_species_count",
  "starting_species_member_count",
  "starting_species_member_variance",
  // v2.0.4 S6: multi-band grass sight (changes NN layout → restart required).
  "grass_multisight",
]);

const TOAST_CONSTRUCTION =
  "Saved. Click Restart to see these changes in a new world.";

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
// v2.0 Wave 1c: `full_grass_on_init` was removed from the Settings UI + the
// persisted blob. The Rust constructor still takes the boolean arg, so boot
// passes the Rust default (false) — there is no UI knob for it anymore.
export function getFullGrassOnInit(): boolean {
  return false;
}
// v2.0 Wave 1a: construction-only world-shape accessors for the boot payload.
export function getWorldSize(): number {
  return widgetReaders.get("world_size")?.() ?? getSettings().worldSize;
}
export function getWrapWorld(): boolean {
  const v = widgetReaders.get("wrap_world")?.();
  if (v !== undefined) return v !== 0;
  return getSettings().wrapWorld;
}
export function getWorldSeed(): number {
  const v = widgetReaders.get("world_seed")?.();
  return Math.max(0, Math.round(v ?? getSettings().worldSeed)) >>> 0;
}
// v2.0 Wave 3b: species + sexual-mating construction accessors for the boot
// payload. These ride `newWithFounderCount`'s 5 trailing args (not the live
// slider SAB). Each falls back to the persisted setting if its widget is not
// installed (e.g. when species_mode is staged off, the species rows are hidden
// but the readers stay registered, so this mostly reads the live widget value).
export function getSpeciesMode(): boolean {
  const v = widgetReaders.get("species_mode")?.();
  if (v !== undefined) return v !== 0;
  return getSettings().speciesMode;
}
export function getCrossoverMode(): number {
  // Rust f32 encoding: 0 = average, non-zero = fifty_fifty.
  const v = widgetReaders.get("crossover_mode")?.();
  return v !== undefined ? v : getSettings().crossoverMode;
}
export function getStartingSpeciesCount(): number {
  return widgetReaders.get("starting_species_count")?.() ?? getSettings().startingSpeciesCount;
}
export function getStartingSpeciesMemberCount(): number {
  return (
    widgetReaders.get("starting_species_member_count")?.() ??
    getSettings().startingSpeciesMemberCount
  );
}
export function getStartingSpeciesMemberVariance(): number {
  return (
    widgetReaders.get("starting_species_member_variance")?.() ??
    getSettings().startingSpeciesMemberVariance
  );
}

// v2.0.4 S6: multi-band grass sight accessor for the boot payload.
export function getGrassMultisight(): boolean {
  const v = widgetReaders.get("grass_multisight")?.();
  if (v !== undefined) return v !== 0;
  return getSettings().grassMultisight;
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
  // v1.12: fan persisted mutation buckets into bucket_k_<field> sliders so
  // boot/respawn re-applies the most-recently-saved policy. NN tab widgets
  // override these via registerWidget when installed (live-edit path).
  const buckets = getSettings().mutationBuckets;
  for (let k = 0; k < buckets.length; k++) {
    const b = buckets[k];
    const wn = `bucket_${k}_weight`;
    const rn = `bucket_${k}_rate`;
    const sn = `bucket_${k}_sigma`;
    if (!(wn in out)) out[wn] = b.weight;
    if (!(rn in out)) out[rn] = b.rate;
    if (!(sn in out)) out[sn] = b.sigma;
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
// v2.0 Wave 3b: re-runs species-mode row gating. Set in installDevPanel; called
// after Cancel/Reset (which rewrite the species_mode widget without firing its
// `change` event) so hidden/shown rows track the restored staged value.
let speciesGatingSync: () => void = () => {};

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

function applyAll(getBridge: () => SimBridge): void {
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
    getBridge().debouncedSetSlider(h.simName, v);
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
  speciesGatingSync();
  refreshDirtyState();
}

function resetAll(getBridge: () => SimBridge): void {
  let anyConstructionOnly = false;
  resetSettings();
  for (const h of stagedHandles) {
    const defaultVal = (DEFAULTS as unknown as Record<string, number | boolean>)[h.settingKey];
    const v = typeof defaultVal === "boolean" ? (defaultVal ? 1 : 0) : (defaultVal as number);
    h.writeWidget(v);
    getBridge().debouncedSetSlider(h.simName, v);
    if (h.snapshot !== v && CONSTRUCTION_ONLY_SLIDERS.has(h.simName)) {
      anyConstructionOnly = true;
    }
    h.snapshot = v;
  }
  // Live-apply widgets (run + display) also reset via the resetSettings() call
  // above; their UI sync happens through liveSyncers registered below.
  for (const sync of liveSyncers) sync();
  speciesGatingSync();
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

// v2.0 Wave 3b: a staged enum dropdown. The widget value is the numeric option
// the Rust slider expects (e.g. crossover_mode: 0 = average, 1 = fifty_fifty).
// Participates in dirty tracking exactly like a staged slider/toggle.
interface DropdownSpec {
  label: string;
  simName: string | null;
  settingKey: keyof Settings;
  /** Ordered {numeric value → display label} options. */
  options: ReadonlyArray<{ value: number; label: string }>;
  nextWorld?: boolean;
}

function makeStagedDropdown(spec: DropdownSpec): HTMLDivElement {
  const initial = settingToNumber(spec.settingKey);
  const row = document.createElement("div");
  row.className = "devpanel-row devpanel-row-wide";

  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;
  if (spec.nextWorld) labelEl.classList.add("next-world");

  const select = document.createElement("select");
  for (const o of spec.options) {
    const opt = document.createElement("option");
    opt.value = String(o.value);
    opt.textContent = o.label;
    select.appendChild(opt);
  }
  select.value = String(initial);

  const writeWidget = (v: number): void => {
    select.value = String(v);
  };

  select.addEventListener("change", () => refreshDirtyState());

  if (spec.simName) {
    registerWidget(spec.simName, () => Number(select.value));
    stagedHandles.push({
      simName: spec.simName,
      settingKey: spec.settingKey,
      readWidget: () => Number(select.value),
      writeWidget,
      snapshot: initial,
      rowEl: row,
    });
  }

  row.append(labelEl, select);
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

export function installDevPanel(getBridge: () => SimBridge): void {
  const box = document.getElementById("devpanel-box") as HTMLDivElement | null;
  if (!box) return;

  // ── Display (live) ──
  // (autoRun moved to top bar's auto-restart icon button in v1.13 Wave 1.)
  // Profiler visibility lives on the floating monitor-icon button at the
  // bottom-right of the canvas, not in Settings. See widgets/perf-panel.ts.
  const displaySec = section("Display");
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
  // v2.0 Wave 1c: the "Full grass on new world" toggle was removed (the
  // setting no longer persists; boot passes the Rust default false).
  grassSec.appendChild(makeStagedSlider({
    label: "Initial seeded cells",
    simName: "grass_initial_seed_count",
    settingKey: "initialGrassSeedCount",
    min: 0, max: 8000, step: 1,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  }));
  // v2.0.2 Stream 1d: scatter kernel live-tuning sliders. All staged (live).
  grassSec.appendChild(makeStagedSlider({
    label: "Scatter decay prob",
    simName: "grass_decay_pct",
    settingKey: "grassDecayPct",
    min: 0, max: 1, step: 0.005,
    formatValue: (v) => v.toFixed(3),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Scatter decay amt",
    simName: "grass_decay_amount",
    settingKey: "grassDecayAmount",
    min: 0, max: 0.5, step: 0.001,
    formatValue: (v) => v.toFixed(3),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Scatter spread prob",
    simName: "grass_spread_pct",
    settingKey: "grassSpreadPct",
    min: 0, max: 1, step: 0.01,
    formatValue: (v) => v.toFixed(2),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Scatter spread amt",
    simName: "grass_spread_amount",
    settingKey: "grassSpreadAmount",
    min: 0, max: 0.5, step: 0.005,
    formatValue: (v) => v.toFixed(3),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Spread ring-1 weight",
    simName: "grass_spread_ring1_pct",
    settingKey: "grassSpreadRing1Pct",
    min: 0, max: 1, step: 0.01,
    formatValue: (v) => v.toFixed(2),
  }));
  grassSec.appendChild(makeStagedSlider({
    label: "Spread ring-2 weight",
    simName: "grass_spread_ring2_pct",
    settingKey: "grassSpreadRing2Pct",
    min: 0, max: 1, step: 0.01,
    formatValue: (v) => v.toFixed(2),
  }));
  // v2.0.4 S6: multi-band grass NN sight (construction-only — changes NN layout).
  // ON  = near band (20u) + far band (~160u via LOD pyramid level 3).
  // OFF = near band only (legacy single-band, 8 NN inputs).
  // Falls back to single-band automatically when walled+species mode fills the
  // MAX_NN_INPUTS budget (no runtime error; the toggle just becomes a no-op in
  // that combination — see Rust nn.rs `for_settings`).
  grassSec.appendChild(makeStagedToggle({
    label: "Multi-band grass sight",
    simName: "grass_multisight",
    settingKey: "grassMultisight",
    nextWorld: true,
  }));
  box.appendChild(grassSec);

  // ── World (v2.0 Wave 1a/1b) ──
  // Construction-only world shape (restart-required, toast on apply) + the
  // two live biome movement-penalty sliders.
  const worldSec = section("World");
  worldSec.appendChild(makeStagedSlider({
    label: "World size",
    simName: "world_size",
    settingKey: "worldSize",
    min: 1200, max: 19200, step: 100,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  }));
  worldSec.appendChild(makeStagedSlider({
    label: "World seed",
    simName: "world_seed",
    settingKey: "worldSeed",
    min: 0, max: 4294967295, step: 1,
    formatValue: (v) => (v === 0 ? "0 (random)" : String(Math.round(v))),
    nextWorld: true,
  }));
  worldSec.appendChild(makeStagedToggle({
    label: "Wrap world (torus)",
    simName: "wrap_world",
    settingKey: "wrapWorld",
    nextWorld: true,
  }));
  // Live biome penalties (apply to the running world).
  worldSec.appendChild(makeStagedSlider({
    label: "Water move penalty",
    simName: "water_movement_penalty",
    settingKey: "waterMovementPenalty",
    min: 0, max: 1, step: 0.01,
    formatValue: (v) => v.toFixed(2),
  }));
  worldSec.appendChild(makeStagedSlider({
    label: "Desert move penalty",
    simName: "desert_movement_penalty",
    settingKey: "desertMovementPenalty",
    min: 0, max: 1, step: 0.01,
    formatValue: (v) => v.toFixed(2),
  }));
  box.appendChild(worldSec);

  // ── Species & mating (v2.0 Wave 3b) ──
  // `species_mode` is a construction toggle that gates the rest of the section:
  // when staged ON the species-seeding sliders + crossover dropdown + the live
  // mating-cooldown slider are shown (and the Lifecycle "Founder count" row is
  // hidden); when staged OFF those rows are hidden and Founder count returns.
  // Gating keys off the STAGED widget value (the next-world choice), not the
  // running sim's mode — per the mission's UI-gating note.
  const speciesSec = section("Species & mating");

  const speciesModeRow = makeStagedToggle({
    label: "Species mode",
    simName: "species_mode",
    settingKey: "speciesMode",
    nextWorld: true,
  });
  speciesSec.appendChild(speciesModeRow);

  // crossover_mode: construction enum {0 = average, 1 = fifty_fifty}.
  const crossoverRow = makeStagedDropdown({
    label: "Crossover mode",
    simName: "crossover_mode",
    settingKey: "crossoverMode",
    options: [
      { value: 1, label: "fifty_fifty" },
      { value: 0, label: "average" },
    ],
    nextWorld: true,
  });
  speciesSec.appendChild(crossoverRow);

  const speciesCountRow = makeStagedSlider({
    label: "Species count",
    simName: "starting_species_count",
    settingKey: "startingSpeciesCount",
    min: 1, max: 64, step: 1,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  });
  speciesSec.appendChild(speciesCountRow);

  const memberCountRow = makeStagedSlider({
    label: "Members / species",
    simName: "starting_species_member_count",
    settingKey: "startingSpeciesMemberCount",
    min: 1, max: 200, step: 1,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  });
  speciesSec.appendChild(memberCountRow);

  const memberVarianceRow = makeStagedSlider({
    label: "Member variance",
    simName: "starting_species_member_variance",
    settingKey: "startingSpeciesMemberVariance",
    min: 0, max: 10, step: 0.1,
    formatValue: (v) => v.toFixed(1),
    nextWorld: true,
  });
  speciesSec.appendChild(memberVarianceRow);

  // mating_cooldown_ticks is LIVE (applies to the running world) — staged like
  // the other sim sliders but NOT in CONSTRUCTION_ONLY_SLIDERS, so it does not
  // fire the restart toast. Still gated visible only when species mode is on.
  const matingCooldownRow = makeStagedSlider({
    label: "Mating cooldown",
    simName: "mating_cooldown_ticks",
    settingKey: "matingCooldownTicks",
    min: 0, max: 2000, step: 1,
    formatValue: (v) => `${Math.round(v)} ticks`,
  });
  speciesSec.appendChild(matingCooldownRow);
  box.appendChild(speciesSec);

  // ── Attack ──
  // v2.0 Wave 5: the action was renamed Eat→Attack; label text only. The
  // slider's `simName` ("eat_bite_fraction") is the Rust wire name — DO NOT
  // rename it or the slider stops applying.
  const eatSec = section("Attack");
  eatSec.appendChild(makeStagedSlider({
    label: "Attack bite fraction",
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
  // v2.0 Wave 3b: the Founder count row is gated by the staged `species_mode`
  // widget — hidden when species mode is staged ON (the species-seeding sliders
  // take its place). Captured here so `refreshSpeciesGating()` can toggle it.
  const founderCountRow = makeStagedSlider({
    label: "Founder count",
    simName: "founder_count",
    settingKey: "founderCount",
    min: 1, max: 32, step: 1,
    formatValue: (v) => String(Math.round(v)),
    nextWorld: true,
  });
  lifeSec.appendChild(founderCountRow);
  lifeSec.appendChild(makeStagedSlider({
    label: "Mutation rate ×",
    simName: "mutation_rate_multiplier",
    settingKey: "mutRate",
    min: 0, max: 5, step: 0.05,
    formatValue: (v) => v.toFixed(2),
  }));
  // v2.0 Wave 2b: genome trait-mutation sigma multiplier (live-tunable). Scales
  // the per-birth Gaussian nudge applied to each of the 6 body-genome traits.
  lifeSec.appendChild(makeStagedSlider({
    label: "Trait mutation σ ×",
    simName: "trait_mutation_sigma_multiplier",
    settingKey: "traitMutationSigmaMultiplier",
    min: 0, max: 2, step: 0.05,
    formatValue: (v) => v.toFixed(2),
  }));
  // v1.12: legacy NN σ slider replaced by the 8-bucket mutation policy editor
  // in the NN rail tab. See docs/plans/v1.12-mission.md.
  lifeSec.appendChild(makeStagedSlider({
    label: "Repulsion max",
    simName: "repulsion_max",
    settingKey: "repulsionMax",
    min: 0, max: 10, step: 0.1,
    formatValue: (v) => v.toFixed(1),
  }));
  // v1.13 Wave 2: the "Max population" selector moved out of Settings and
  // into the bottom perf panel's selectors row. The persisted setting + the
  // `max_population` slider live there now (live-apply, no staging).
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

  // ── Species-mode gating (v2.0 Wave 3b) ──
  // Show/hide rows based on the STAGED species_mode widget value. ON ⇒ show the
  // species-seeding sliders + crossover + mating-cooldown, hide Founder count.
  // OFF ⇒ the reverse. Keys off the live checkbox state, not the running sim.
  const speciesGatedRows = [
    crossoverRow,
    speciesCountRow,
    memberCountRow,
    memberVarianceRow,
    matingCooldownRow,
  ];
  function refreshSpeciesGating(): void {
    const on = (widgetReaders.get("species_mode")?.() ?? 0) !== 0;
    for (const r of speciesGatedRows) r.style.display = on ? "" : "none";
    founderCountRow.style.display = on ? "none" : "";
  }
  // Re-gate whenever the species_mode checkbox flips (in addition to the
  // dirty-state refresh makeStagedToggle already wires).
  const speciesModeCheckbox = speciesModeRow.querySelector(
    "input[type=checkbox]",
  ) as HTMLInputElement | null;
  speciesModeCheckbox?.addEventListener("change", refreshSpeciesGating);
  // Expose for Cancel/Reset (which rewrite the checkbox without a change event)
  // and apply the initial gating from the persisted species_mode value.
  speciesGatingSync = refreshSpeciesGating;
  refreshSpeciesGating();

  // ── Footer wiring ──
  footerApply = document.getElementById("settings-apply") as HTMLButtonElement | null;
  footerCancel = document.getElementById("settings-cancel") as HTMLButtonElement | null;
  const footerReset = document.getElementById("settings-reset") as HTMLButtonElement | null;
  if (footerApply) footerApply.addEventListener("click", () => applyAll(getBridge));
  if (footerCancel) footerCancel.addEventListener("click", () => cancelAll());
  if (footerReset) footerReset.addEventListener("click", () => resetAll(getBridge));
  refreshDirtyState();
}
