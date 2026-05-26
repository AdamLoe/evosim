// P3b: dev panel overlay with 6 sliders. Toggle with ~ hotkey or ⚙ button.
// Defaults below must match src/constants.rs — keep in sync (v1.3 follow-up: build-time assertion).
//   GRASS_PROPAGATION_RATE_K_DEFAULT = 0.1
//   GRASS_IN_CELL_GROWTH_R_DEFAULT   = 0.05
//   GRASS_INITIAL_SEED_COUNT_DEFAULT = 100
//   EAT_BITE_FRACTION_DEFAULT        = 0.5
//   NN_MUT_SIGMA_DEFAULT resolved at Rust side.
// M4: dropped mouth_tax slider (no per-creature eat_efficiency anchor in v1.3).

import type { WorldHandle } from "../../wasm/evosim";

/** Module-local state: passed to WorldHandle.newWithGrassSeed on world creation. */
let initialGrassSeedCount = 100; // keep in sync with GRASS_INITIAL_SEED_COUNT_DEFAULT

/** Returns the current initial-grass-seed-count for use by world construction. */
export function getInitialGrassSeedCount(): number {
  return initialGrassSeedCount;
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

function makeSlider(spec: SliderSpec): HTMLDivElement {
  const row = document.createElement("div");
  row.className = "devpanel-row";
  const labelEl = document.createElement("label");
  labelEl.textContent = spec.label;
  const input = document.createElement("input");
  input.type = "range";
  input.min = String(spec.min);
  input.max = String(spec.max);
  input.step = String(spec.step);
  input.value = String(spec.default);
  const readout = document.createElement("span");
  readout.className = "devpanel-readout";
  const fmt = spec.formatValue ?? ((v: number) => v.toFixed(3));
  readout.textContent = fmt(spec.default);
  input.addEventListener("input", () => {
    const v = Number(input.value);
    readout.textContent = fmt(v);
    spec.onChange(v);
  });
  row.append(labelEl, input, readout);
  return row;
}

/** Install the dev-panel overlay. Call once after WorldHandle is ready. */
export function installDevPanel(world: WorldHandle): void {
  const box = document.getElementById("devpanel-box") as HTMLDivElement | null;
  if (!box) return;

  const sliders: SliderSpec[] = [
    {
      label: "eat bite frac",
      min: 0,
      max: 1,
      step: 0.01,
      default: 0.5, // EAT_BITE_FRACTION_DEFAULT
      onChange: (v) => world.set_eat_bite_fraction(v),
    },
    {
      label: "grass propag k",
      min: 0,
      max: 0.2,
      step: 0.005,
      default: 0.1, // GRASS_PROPAGATION_RATE_K_DEFAULT
      onChange: (v) => world.set_grass_propagation_rate_k(v),
    },
    {
      label: "grass growth r",
      min: 0,
      max: 0.05,
      step: 0.001,
      default: 0.05, // GRASS_IN_CELL_GROWTH_R_DEFAULT
      onChange: (v) => world.set_grass_in_cell_growth_r(v),
    },
    {
      label: "initial seed N",
      min: 1,
      max: 100,
      step: 1,
      default: 100, // GRASS_INITIAL_SEED_COUNT_DEFAULT
      formatValue: (v) => String(Math.round(v)),
      onChange: (v) => {
        initialGrassSeedCount = Math.round(v);
      },
    },
    {
      label: "mut rate",
      min: 0,
      max: 5,
      step: 0.05,
      default: 1.0, // mutation_rate_multiplier default
      onChange: (v) => world.set_mutation_rate_multiplier(v),
    },
    {
      label: "nn σ",
      min: 0,
      max: 0.2,
      step: 0.005,
      default: 0.02, // NN_MUT_SIGMA_DEFAULT — keep in sync with constants.rs
      onChange: (v) => world.set_nn_mutation_sigma(v),
    },
  ];

  for (const spec of sliders) {
    box.appendChild(makeSlider(spec));
  }

  // ~ hotkey toggle (skip if focus is in an input/textarea).
  window.addEventListener("keydown", (e) => {
    if (e.key === "~" && !(e.target instanceof HTMLInputElement) && !(e.target instanceof HTMLTextAreaElement)) {
      box.style.display = box.style.display === "none" ? "flex" : "none";
    }
  });

  // Top-bar ⚙ button.
  const topBar = document.getElementById("top-bar");
  if (topBar) {
    const btn = document.createElement("button");
    btn.id = "devpanel-toggle";
    btn.textContent = "⚙";
    btn.title = "Dev panel (~ hotkey)";
    btn.addEventListener("click", () => {
      box.style.display = box.style.display === "none" ? "flex" : "none";
    });
    topBar.appendChild(btn);
  }
}
