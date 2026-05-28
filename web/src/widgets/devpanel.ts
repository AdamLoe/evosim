// Dev-panel overlay: sliders + toggles for live-tuning the sim. Toggle the
// panel itself with the ~ hotkey or the ⚙ top-bar button.

import type { WorldHandle } from "../../wasm/evosim";
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

interface SliderSpec {
  label: string;
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

  row.append(labelEl, slider, numInput, readout);
  return { row, readout };
}

interface ToggleSpec {
  label: string;
  default: boolean;
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
  return row;
}

/** Re-apply all dev-slider values to `world`. Call after restart so the
 * user's tweaks persist across world recreation. `initialGrassSeedCount`
 * is already baked into world construction via `newWithGrassSeed`. */
export function reapplyDevSliders(world: WorldHandle): void {
  const s = getSettings();
  world.set_eat_bite_fraction(s.eatBiteFrac);
  world.set_grass_propagation_rate_k(s.grassPropagK);
  world.set_grass_in_cell_growth_r(s.grassGrowthR);
  world.set_mutation_rate_multiplier(s.mutRate);
  world.set_nn_mutation_sigma(s.nnSigma);
  world.set_upkeep_multiplier(s.upkeepMultiplier);
  world.set_move_cost_multiplier(s.moveCostMultiplier);
  world.set_eat_cost_multiplier(s.eatCostMultiplier);
  world.set_energy_max(s.energyMax);
  world.set_grass_energy_per_bite(s.grassEnergyPerBite);
  world.set_grass_bites_per_block(s.grassBitesPerBlock);
  world.set_max_age(s.maxAge);
  world.set_split_threshold(s.splitThreshold);
  world.set_split_gift(s.splitGift);
  world.set_founder_count(s.founderCount);
  world.set_curriculum_min_pop(s.curriculumMinPop);
  world.set_curriculum_max_pop(s.curriculumMaxPop);
  world.set_curriculum_min_factor(s.curriculumMinFactor);
  world.set_auto_curriculum(s.autoCurriculum);
}

// Default per-tick upkeep at multiplier=1: must match the Rust sum
// UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + UPKEEP_MOUTH_DEFAULT = 0.17.
// Used to display the per-second drain in the dev panel.
const UPKEEP_PER_TICK_DEFAULT = 0.17;

/** Install the dev-panel overlay. Call once after WorldHandle is ready. */
export function installDevPanel(getWorld: () => WorldHandle): void {
  const box = document.getElementById("devpanel-box") as HTMLDivElement | null;
  if (!box) return;

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
  installWorkerStatsPanel(getWorld, box);

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
    min: 0,
    max: 2,
    step: 0.01,
    default: persisted.upkeepMultiplier,
    formatValue: formatUpkeep,
    onChange: (v) => {
      setSetting("upkeepMultiplier", v);
      getWorld().set_upkeep_multiplier(v);
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
    min: 0,
    max: 2,
    step: 0.01,
    default: persisted.moveCostMultiplier,
    formatValue: (v) => `${v.toFixed(2)}×`,
    onChange: (v) => {
      setSetting("moveCostMultiplier", v);
      getWorld().set_move_cost_multiplier(v);
    },
  }).row);

  // Eat-attempt cost multiplier — applied to COST_EAT_ATTEMPT per attempted bite.
  box.appendChild(makeSlider({
    label: "eat cost",
    min: 0,
    max: 2,
    step: 0.01,
    default: persisted.eatCostMultiplier,
    formatValue: (v) => `${v.toFixed(2)}×`,
    onChange: (v) => {
      setSetting("eatCostMultiplier", v);
      getWorld().set_eat_cost_multiplier(v);
    },
  }).row);

  box.appendChild(makeSlider({
    label: "energy max",
    min: 50,
    max: 400,
    step: 1,
    default: persisted.energyMax,
    formatValue: (v) => String(Math.round(v)),
    onChange: (v) => {
      setSetting("energyMax", v);
      getWorld().set_energy_max(v);
    },
  }).row);

  // ── Existing sim sliders ──────────────────────────────────────────
  box.appendChild(header("sim"));

  const sliders: SliderSpec[] = [
    {
      label: "eat bite frac",
      min: 0,
      max: 1,
      step: 0.01,
      default: persisted.eatBiteFrac,
      onChange: (v) => {
        setSetting("eatBiteFrac", v);
        getWorld().set_eat_bite_fraction(v);
      },
    },
    {
      label: "grass propag k",
      min: 0,
      max: 0.005,
      step: 0.0001,
      default: persisted.grassPropagK,
      formatValue: (v) => v.toFixed(4),
      onChange: (v) => {
        setSetting("grassPropagK", v);
        getWorld().set_grass_propagation_rate_k(v);
      },
    },
    {
      label: "grass growth r",
      min: 0,
      max: 0.05,
      step: 0.001,
      default: persisted.grassGrowthR,
      onChange: (v) => {
        setSetting("grassGrowthR", v);
        getWorld().set_grass_in_cell_growth_r(v);
      },
    },
    {
      label: "energy per bite",
      min: 1,
      max: 100,
      step: 1,
      default: persisted.grassEnergyPerBite,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("grassEnergyPerBite", rounded);
        getWorld().set_grass_energy_per_bite(rounded);
      },
    },
    {
      label: "bites per block",
      min: 1,
      max: 10,
      step: 1,
      default: persisted.grassBitesPerBlock,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("grassBitesPerBlock", rounded);
        getWorld().set_grass_bites_per_block(rounded);
      },
    },
    {
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
      min: 0,
      max: 5,
      step: 0.05,
      default: persisted.mutRate,
      onChange: (v) => {
        setSetting("mutRate", v);
        getWorld().set_mutation_rate_multiplier(v);
      },
    },
    {
      label: "nn σ",
      min: 0,
      max: 0.2,
      step: 0.001,
      default: persisted.nnSigma,
      onChange: (v) => {
        setSetting("nnSigma", v);
        getWorld().set_nn_mutation_sigma(v);
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
      min: 500,
      max: 20000,
      step: 100,
      default: persisted.maxAge,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("maxAge", rounded);
        getWorld().set_max_age(rounded);
      },
    },
    {
      label: "split threshold",
      min: 10,
      max: 200,
      step: 1,
      default: persisted.splitThreshold,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("splitThreshold", rounded);
        getWorld().set_split_threshold(rounded);
      },
    },
    {
      label: "split gift",
      min: 1,
      max: 100,
      step: 1,
      default: persisted.splitGift,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("splitGift", rounded);
        getWorld().set_split_gift(rounded);
      },
    },
    {
      label: "founder count",
      min: 1,
      max: 32,
      step: 1,
      default: persisted.founderCount,
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        const rounded = Math.round(v);
        setSetting("founderCount", rounded);
        // Stored for next world construction; also push to the active world.
        getWorld().set_founder_count(rounded);
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
    default: persisted.autoCurriculum,
    onChange: (v) => {
      setSetting("autoCurriculum", v);
      getWorld().set_auto_curriculum(v);
    },
  }));

  box.appendChild(makeSlider({
    label: "min pop",
    min: 0,
    max: 5000,
    step: 50,
    default: persisted.curriculumMinPop,
    formatValue: (v) => String(Math.round(v)),
    onChange: (v) => {
      const rounded = Math.round(v);
      setSetting("curriculumMinPop", rounded);
      getWorld().set_curriculum_min_pop(rounded);
    },
  }).row);

  box.appendChild(makeSlider({
    label: "max pop",
    min: 0,
    max: 5000,
    step: 50,
    default: persisted.curriculumMaxPop,
    formatValue: (v) => String(Math.round(v)),
    onChange: (v) => {
      const rounded = Math.round(v);
      setSetting("curriculumMaxPop", rounded);
      getWorld().set_curriculum_max_pop(rounded);
    },
  }).row);

  box.appendChild(makeSlider({
    label: "min factor",
    min: 0,
    max: 1,
    step: 0.05,
    default: persisted.curriculumMinFactor,
    formatValue: (v) => v.toFixed(2),
    onChange: (v) => {
      setSetting("curriculumMinFactor", v);
      getWorld().set_curriculum_min_factor(v);
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
