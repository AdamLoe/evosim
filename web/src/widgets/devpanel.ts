// Dev-panel overlay: sliders + toggles for live-tuning the sim. Toggle the
// panel itself with the ~ hotkey or the ⚙ top-bar button.
//
// v1.6 Wave B: sliders no longer call wasm directly. Each `onChange` posts a
// `set_slider` message via the `SimBridge`. Worker applies via
// `WorldHandle::set_slider(name, value)` — the sole entry point post-cutover.
// Bools (auto_curriculum) ride the same path as 0|1.

import type { SimBridge } from "../sim-bridge";
import { getTargetTPS, setTpsChangeListener, setSettingsOpen, isSettingsOpen } from "../main";
import { getSettings, setSetting, resetSettings } from "../settings";
import { installWorkerStatsPanel } from "./worker-stats";

/** Returns the current initial-grass-seed-count for use by world construction. */
export function getInitialGrassSeedCount(): number {
  return getSettings().initialGrassSeedCount;
}

/** Returns the current energy-max slider value so it can be plumbed into
 * world construction (so the founder spawns under the user's chosen cap). */
export function getEnergyMax(): number {
  return getSettings().energyMax;
}

/** Returns the current founder count for the next world construction. */
export function getFounderCount(): number {
  return getSettings().founderCount;
}

// ─── Live widget state (drives currentSliderState) ────────────────────────
//
// Each entry maps a Rust slider name to a function that reads the current
// IN-MEMORY widget value (slider.value / numInput.value / checkbox.checked).
// `currentSliderState()` snapshots these for boot / restart payloads — this
// is intentionally NOT `getSettings()` because a mid-drag restart should
// carry the dragged value, not the last-persisted one. (gotcha 4)
type SliderReader = () => number;
const widgetReaders = new Map<string, SliderReader>();

function registerWidget(name: string, reader: SliderReader): void {
  widgetReaders.set(name, reader);
}

/**
 * Snapshot every slider's current widget value, keyed by Rust slider name.
 * Sent via `boot.initial_sliders` so the worker applies the user's tweaks
 * before its very first tick.
 */
export function currentSliderState(): Record<string, number> {
  const out: Record<string, number> = {};
  for (const [name, read] of widgetReaders) {
    out[name] = read();
  }
  return out;
}

interface SliderSpec {
  /** Display label in the panel. */
  label: string;
  /** Rust slider name (must match a `try_set_slider` arm). Omit for
   *  display-only sliders (e.g. grass opacity) that don't reach Rust. */
  simName?: string;
  min: number;
  max: number;
  step: number;
  default: number;
  onChange: (v: number) => void;
  formatValue?: (v: number) => string;
}

function makeSlider(spec: SliderSpec): { row: HTMLDivElement; readout: HTMLSpanElement } {
  const row = document.createElement("div");
  row.className = "devpanel-row";
  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;

  const slider = document.createElement("input");
  slider.type = "range";
  slider.min = String(spec.min);
  slider.max = String(spec.max);
  slider.step = String(spec.step);
  slider.value = String(spec.default);

  // Paired numeric input — no min/max so the user can type values outside the
  // slider's range. The slider visually pins to its endpoint when that happens.
  const numInput = document.createElement("input");
  numInput.type = "number";
  numInput.step = String(spec.step);
  numInput.value = String(spec.default);
  numInput.className = "devpanel-numinput";

  const readout = document.createElement("span");
  readout.className = "devpanel-readout";
  const fmt = spec.formatValue ?? ((v: number) => v.toFixed(3));
  readout.textContent = fmt(spec.default);

  const apply = (v: number, source: "slider" | "num") => {
    readout.textContent = fmt(v);
    if (source === "slider") {
      numInput.value = String(v);
    } else {
      // Slider can't render out-of-range values; pin it to the nearest endpoint
      // while letting the underlying value pass through to onChange unmodified.
      slider.value = String(Math.max(spec.min, Math.min(spec.max, v)));
    }
    spec.onChange(v);
  };

  slider.addEventListener("input", () => apply(Number(slider.value), "slider"));
  numInput.addEventListener("input", () => {
    const v = Number(numInput.value);
    if (Number.isFinite(v)) apply(v, "num");
  });

  if (spec.simName) {
    registerWidget(spec.simName, () => Number(numInput.value));
  }

  row.append(labelEl, slider, numInput, readout);
  return { row, readout };
}

interface ToggleSpec {
  label: string;
  default: boolean;
  /** Rust slider name (bool sliders ride `set_slider` as 0|1). Omit for
   *  display-only toggles like "show grass". */
  simName?: string;
  onChange: (v: boolean) => void;
}

function makeToggle(spec: ToggleSpec): HTMLDivElement {
  const row = document.createElement("div");
  row.className = "devpanel-row";
  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = spec.default;
  // The 4-cell grid expects [label, range, num, readout]; toggles fill the
  // middle two with empty spans so the checkbox lands in the readout column.
  const spacer1 = document.createElement("span");
  const spacer2 = document.createElement("span");
  input.addEventListener("change", () => spec.onChange(input.checked));
  row.append(labelEl, spacer1, spacer2, input);
  // Apply the default state immediately so the UI matches the toggle's
  // initial value without waiting for the user's first click.
  spec.onChange(spec.default);

  if (spec.simName) {
    registerWidget(spec.simName, () => (input.checked ? 1 : 0));
  }
  return row;
}

// Default per-tick upkeep at multiplier=1: must match the Rust sum
// UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + UPKEEP_MOUTH_DEFAULT = 0.17.
// Used to display the per-second drain in the dev panel.
const UPKEEP_PER_TICK_DEFAULT = 0.17;

/** Install the dev-panel overlay. Call once after the SimBridge is ready. */
export function installDevPanel(simBridge: SimBridge): void {
  const box = document.getElementById("devpanel-box") as HTMLDivElement | null;
  if (!box) return;

  /**
   * Send a `set_slider` for a Rust-named slider via Wave D's per-name
   * trailing-edge debouncer (16 ms). High-frequency pointermove events on a
   * slider track coalesce to one postMessage per RAF tick; "last value wins"
   * so the released value lands within `SLIDER_DEBOUNCE_MS` of the input.
   * Boot's `initial_sliders` map (read via `currentSliderState()`) bypasses
   * this entirely.
   */
  const send = (name: string, value: number): void => {
    simBridge.debouncedSetSlider(name, value);
  };

  // Section header helper — keeps the panel scannable when more rows show up.
  function header(text: string): HTMLDivElement {
    const h = document.createElement("div");
    h.textContent = text;
    h.style.marginTop = "6px";
    h.style.opacity = "0.65";
    h.style.fontSize = "11px";
    h.style.textTransform = "uppercase";
    h.style.letterSpacing = "0.05em";
    return h;
  }

  const persisted = getSettings();

  // ── Thread health (live poller; cheap by default) ─────────────────
  installWorkerStatsPanel(simBridge, box);

  // ── Sim run controls ──────────────────────────────────────────────
  box.appendChild(header("run"));

  box.appendChild(makeToggle({
    label: "auto run",
    default: persisted.autoRun,
    onChange: (v) => setSetting("autoRun", v),
  }));

  // ── Display toggles ───────────────────────────────────────────────
  box.appendChild(header("display"));

  box.appendChild(makeToggle({
    label: "show profiler",
    default: persisted.showProfiler,
    onChange: (v) => {
      setSetting("showProfiler", v);
      const el = document.getElementById("perf-box");
      if (el) el.style.display = v ? "" : "none";
    },
  }));

  box.appendChild(makeToggle({
    label: "show pop graph",
    default: persisted.showPopGraph,
    onChange: (v) => {
      setSetting("showPopGraph", v);
      const el = document.getElementById("stats-box");
      if (el) el.style.display = v ? "" : "none";
    },
  }));

  // The grass shader can be skipped entirely (no draw call) — useful when
  // perf is tight and you only want to watch creatures.
  box.appendChild(makeToggle({
    label: "show grass",
    default: persisted.showGrass,
    onChange: (v) => setSetting("showGrass", v),
  }));

  // Render-time alpha multiplier for the grass overlay. 0 = invisible (but the
  // upload + draw still happens — toggle the row above to skip entirely);
  // 1 = full density-coloured opacity. The renderer reads this every frame.
  box.appendChild(makeSlider({
    label: "grass opacity",
    min: 0,
    max: 1,
    step: 0.05,
    default: persisted.grassOpacity,
    formatValue: (v) => v.toFixed(2),
    onChange: (v) => setSetting("grassOpacity", v),
  }).row);

  // ── Energy economy ────────────────────────────────────────────────
  box.appendChild(header("energy"));

  // Basic upkeep multiplier 0 → 2 (default 1 = current). Readout in energy /sec
  // at the current target TPS; updates when either slider or TPS changes.
  let upkeepReadout: HTMLSpanElement | null = null;
  const formatUpkeep = (mult: number): string => {
    const perSec = mult * UPKEEP_PER_TICK_DEFAULT * getTargetTPS();
    return `${perSec.toFixed(1)} /s`;
  };
  const upkeepRow = makeSlider({
    label: "basic upkeep",
    simName: "upkeep_multiplier",
    min: 0,
    max: 2,
    step: 0.01,
    default: persisted.upkeepMultiplier,
    formatValue: formatUpkeep,
    onChange: (v) => {
      setSetting("upkeepMultiplier", v);
      send("upkeep_multiplier", v);
    },
  });
  upkeepReadout = upkeepRow.readout;
  box.appendChild(upkeepRow.row);

  // TPS changes update the upkeep /s readout (without firing the sim setter).
  setTpsChangeListener(() => {
    if (upkeepReadout) {
      upkeepReadout.textContent = formatUpkeep(getSettings().upkeepMultiplier);
    }
  });

  // Move cost multiplier — applied to `dist * COST_MOVE_PER_DIST` each tick.
  // Formatted as a plain Nx multiplier since the per-second value depends on
  // how much the creature actually moves (not a clean wall-clock rate).
  box.appendChild(makeSlider({
    label: "move cost",
    simName: "move_cost_multiplier",
    min: 0,
    max: 2,
    step: 0.01,
    default: persisted.moveCostMultiplier,
    formatValue: (v) => `${v.toFixed(2)}×`,
    onChange: (v) => {
      setSetting("moveCostMultiplier", v);
      send("move_cost_multiplier", v);
    },
  }).row);

  // Eat-attempt cost multiplier — applied to COST_EAT_ATTEMPT per attempted bite.
  box.appendChild(makeSlider({
    label: "eat cost",
    simName: "eat_cost_multiplier",
    min: 0,
    max: 2,
    step: 0.01,
    default: persisted.eatCostMultiplier,
    formatValue: (v) => `${v.toFixed(2)}×`,
    onChange: (v) => {
      setSetting("eatCostMultiplier", v);
      send("eat_cost_multiplier", v);
    },
  }).row);

  box.appendChild(makeSlider({
    label: "energy max",
    simName: "energy_max",
    min: 50,
    max: 400,
    step: 1,
    default: persisted.energyMax,
    formatValue: (v) => String(Math.round(v)),
    onChange: (v) => {
      setSetting("energyMax", v);
      send("energy_max", v);
    },
  }).row);

  // ── Existing sim sliders ──────────────────────────────────────────
  box.appendChild(header("sim"));

  const sliders: SliderSpec[] = [
    {
      label: "eat bite frac",
      simName: "eat_bite_fraction",
      min: 0,
      max: 1,
      step: 0.01,
      default: persisted.eatBiteFrac,
      onChange: (v) => {
        setSetting("eatBiteFrac", v);
        send("eat_bite_fraction", v);
      },
    },
    {
      label: "grass propag k",
      simName: "grass_propagation_rate_k",
      min: 0,
      max: 0.005,
      step: 0.0001,
      default: persisted.grassPropagK,
      formatValue: (v) => v.toFixed(4),
      onChange: (v) => {
        setSetting("grassPropagK", v);
        send("grass_propagation_rate_k", v);
      },
    },
    {
      label: "grass growth r",
      simName: "grass_in_cell_growth_r",
      min: 0,
      max: 0.05,
      step: 0.001,
      default: persisted.grassGrowthR,
      onChange: (v) => {
        setSetting("grassGrowthR", v);
        send("grass_in_cell_growth_r", v);
      },
    },
    {
      label: "energy per bite",
      simName: "grass_energy_per_bite",
      min: 1,
      max: 100,
      step: 1,
      default: persisted.grassEnergyPerBite,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("grassEnergyPerBite", rounded);
        send("grass_energy_per_bite", rounded);
      },
    },
    {
      label: "bites per block",
      simName: "grass_bites_per_block",
      min: 1,
      max: 10,
      step: 1,
      default: persisted.grassBitesPerBlock,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("grassBitesPerBlock", rounded);
        send("grass_bites_per_block", rounded);
      },
    },
    {
      label: "digestion cooldown",
      simName: "digestion_cooldown",
      min: 0,
      max: 500,
      step: 1,
      default: persisted.digestionCooldown,
      formatValue: (v) => `${Math.round(v)} ticks`,
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("digestionCooldown", rounded);
        send("digestion_cooldown", rounded);
      },
    },
    {
      label: "repulsion max",
      simName: "repulsion_max",
      min: 0,
      max: 10,
      step: 0.1,
      default: persisted.repulsionMax,
      formatValue: (v) => v.toFixed(1),
      onChange: (v) => {
        setSetting("repulsionMax", v);
        send("repulsion_max", v);
      },
    },
    {
      // Display-only: baked into world construction, no live slider in Rust.
      label: "initial seed N",
      min: 0,
      max: 8000,
      step: 1,
      default: persisted.initialGrassSeedCount,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        setSetting("initialGrassSeedCount", Math.round(v));
      },
    },
    {
      label: "mut rate",
      simName: "mutation_rate_multiplier",
      min: 0,
      max: 5,
      step: 0.05,
      default: persisted.mutRate,
      onChange: (v) => {
        setSetting("mutRate", v);
        send("mutation_rate_multiplier", v);
      },
    },
    {
      label: "nn σ",
      simName: "nn_mutation_sigma",
      min: 0,
      max: 0.2,
      step: 0.001,
      default: persisted.nnSigma,
      onChange: (v) => {
        setSetting("nnSigma", v);
        send("nn_mutation_sigma", v);
      },
    },
  ];
  for (const spec of sliders) {
    box.appendChild(makeSlider(spec).row);
  }

  // ── Lifecycle (v1.5) ──────────────────────────────────────────────
  box.appendChild(header("lifecycle"));

  const lifecycleSliders: SliderSpec[] = [
    {
      label: "max age",
      simName: "max_age",
      min: 500,
      max: 20000,
      step: 100,
      default: persisted.maxAge,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("maxAge", rounded);
        send("max_age", rounded);
      },
    },
    {
      label: "split threshold",
      simName: "split_threshold",
      min: 10,
      max: 200,
      step: 1,
      default: persisted.splitThreshold,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("splitThreshold", rounded);
        send("split_threshold", rounded);
      },
    },
    {
      label: "split gift",
      simName: "split_gift",
      min: 1,
      max: 100,
      step: 1,
      default: persisted.splitGift,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("splitGift", rounded);
        send("split_gift", rounded);
      },
    },
    {
      label: "founder count",
      simName: "founder_count",
      min: 1,
      max: 32,
      step: 1,
      default: persisted.founderCount,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("founderCount", rounded);
        // Stored for next world construction; also push to the active world.
        send("founder_count", rounded);
      },
    },
  ];
  for (const spec of lifecycleSliders) {
    box.appendChild(makeSlider(spec).row);
  }

  // ── Curriculum ────────────────────────────────────────────────────
  box.appendChild(header("curriculum"));

  box.appendChild(makeToggle({
    label: "auto curriculum",
    simName: "auto_curriculum",
    default: persisted.autoCurriculum,
    onChange: (v) => {
      setSetting("autoCurriculum", v);
      send("auto_curriculum", v ? 1 : 0);
    },
  }));

  box.appendChild(makeSlider({
    label: "min pop",
    simName: "curriculum_min_pop",
    min: 0,
    max: 5000,
    step: 50,
    default: persisted.curriculumMinPop,
    formatValue: (v) => String(Math.round(v)),
    onChange: (v) => {
      const rounded = Math.round(v);
      setSetting("curriculumMinPop", rounded);
      send("curriculum_min_pop", rounded);
    },
  }).row);

  box.appendChild(makeSlider({
    label: "max pop",
    simName: "curriculum_max_pop",
    min: 0,
    max: 5000,
    step: 50,
    default: persisted.curriculumMaxPop,
    formatValue: (v) => String(Math.round(v)),
    onChange: (v) => {
      const rounded = Math.round(v);
      setSetting("curriculumMaxPop", rounded);
      send("curriculum_max_pop", rounded);
    },
  }).row);

  box.appendChild(makeSlider({
    label: "min factor",
    simName: "curriculum_min_factor",
    min: 0,
    max: 1,
    step: 0.05,
    default: persisted.curriculumMinFactor,
    formatValue: (v) => v.toFixed(2),
    onChange: (v) => {
      setSetting("curriculumMinFactor", v);
      send("curriculum_min_factor", v);
    },
  }).row);

  // ── Reset ─────────────────────────────────────────────────────────
  const resetWrap = document.createElement("div");
  resetWrap.style.marginTop = "8px";
  resetWrap.style.display = "flex";
  resetWrap.style.justifyContent = "flex-end";
  const resetBtn = document.createElement("button");
  resetBtn.textContent = "reset to defaults";
  resetBtn.style.background = "rgba(255,255,255,0.08)";
  resetBtn.style.color = "var(--fg)";
  resetBtn.style.border = "1px solid rgba(255,255,255,0.15)";
  resetBtn.style.padding = "3px 10px";
  resetBtn.style.borderRadius = "3px";
  resetBtn.style.cursor = "pointer";
  resetBtn.style.font = "inherit";
  resetBtn.addEventListener("click", () => {
    resetSettings();
    // Easiest way to re-sync every slider, toggle, and input to the new
    // defaults and start the world fresh under them: reload the page.
    window.location.reload();
  });
  resetWrap.appendChild(resetBtn);
  box.appendChild(resetWrap);

  // ~ hotkey toggle (skip if focus is in an input/textarea). Routes through
  // setSettingsOpen so the body class flips and the canvas resizes too.
  window.addEventListener("keydown", (e) => {
    if (
      e.key === "~" &&
      !(e.target instanceof HTMLInputElement) &&
      !(e.target instanceof HTMLTextAreaElement)
    ) {
      setSettingsOpen(!isSettingsOpen());
    }
  });
}
