// Rail orchestrator: boot + pollRail. Called from main.ts each RAF frame.
// E.21: Species list (Events panel hidden for v1.1 revisit).
// E.22: Highlight management (toasts suppressed for v1.1 revisit).
// E.23: Stats sampling.
// E.24: Inspector refresh.

import type { WorldHandle } from "../../wasm/evosim";
import { maybeSampleStats } from "./stats";
import { refreshInspector } from "./inspector";
import { tickToasts } from "./toast";
import { addHighlight, pruneHighlights, highlights } from "./highlight";

// ---- Species list (inlined from former events.ts, S14) ----

export interface SpeciesRow {
  id: number;
  name: string;
  population: number;
  parent_id: number | null;
}

let speciesContainer: HTMLDivElement | null = null;

function renderSpeciesList(rows: SpeciesRow[]): void {
  if (!speciesContainer) {
    const panel = document.getElementById("rail-events");
    if (!panel) return;
    const wrapper = document.createElement("div");
    wrapper.className = "events-species-list";
    const label = document.createElement("div");
    label.style.cssText = "opacity:0.4;margin-bottom:4px;font-size:10px;text-transform:uppercase;letter-spacing:0.05em";
    label.textContent = "Live species";
    wrapper.appendChild(label);
    speciesContainer = document.createElement("div");
    wrapper.appendChild(speciesContainer);
    panel.insertBefore(wrapper, panel.firstChild);
  }
  const sorted = [...rows].sort((a, b) => b.population - a.population);
  speciesContainer.innerHTML = "";
  for (const sp of sorted) {
    const div = document.createElement("div");
    div.className = "species-row";
    div.dataset.speciesId = String(sp.id);
    div.textContent = `${sp.name} (${sp.population})`;
    div.title = sp.parent_id !== null ? `Parent: #${sp.parent_id}` : "Founder";
    speciesContainer.appendChild(div);
  }
}

// ---- Rail state (opaque to main.ts) ----

export interface RailState {
  switchTab(name: string): void;
  activeTab: string;
}

// ---- Tab switching ----

function installTabs(): RailState {
  let activeTab = "stats"; // Events tab hidden; Stats is default for v1.1 revisit

  function switchTab(name: string): void {
    activeTab = name;
    document.querySelectorAll(".rail-tab").forEach((btn) => {
      const b = btn as HTMLButtonElement;
      b.classList.toggle("is-active", b.dataset.tab === name);
    });
    document.querySelectorAll(".rail-panel").forEach((panel) => {
      const p = panel as HTMLElement;
      const panelName = p.id.replace("rail-", "");
      p.classList.toggle("is-active", panelName === name);
    });
  }

  document.querySelectorAll(".rail-tab").forEach((btn) => {
    const b = btn as HTMLButtonElement;
    b.addEventListener("click", () => switchTab(b.dataset.tab!));
  });

  return {
    get activeTab() { return activeTab; },
    switchTab,
  };
}

// ---- Event polling state ----
// Event polling removed for v1.1 revisit (events_enabled=false on backend).
let lastSpeciesPoll = -Infinity;

// Species name cache for highlight lookups.
const speciesNameCache: Map<number, string> = new Map();

// ---- installRail ----

export function installRail(world: WorldHandle): RailState {
  const rail = installTabs();
  // Pre-populate species cache with founder.
  try {
    const json: SpeciesRow[] = JSON.parse(world.species_list_json());
    for (const sp of json) speciesNameCache.set(sp.id, sp.name);
    renderSpeciesList(json);
  } catch (e) {
    console.warn("rail: initial species_list_json failed", e);
  }
  return rail;
}

// ---- pollRail (called each RAF frame) ----

export function pollRail(
  rail: RailState,
  world: WorldHandle,
  idsBuffer: Float64Array,
): void {
  const now = performance.now();

  // 1. Refresh species list cache (~1 Hz).
  if (now - lastSpeciesPoll > 1000) {
    try {
      const json: SpeciesRow[] = JSON.parse(world.species_list_json());
      speciesNameCache.clear();
      for (const sp of json) speciesNameCache.set(sp.id, sp.name);
      renderSpeciesList(json);
    } catch (e) {
      console.warn("rail: species_list_json parse failed", e);
    }
    lastSpeciesPoll = now;
  }

  // 2. Event diff removed for v1.1 revisit (events_enabled=false on backend).

  // 3. Stats sample (E.23).
  maybeSampleStats(world);

  // 4. Inspector refresh (E.24).
  refreshInspector(world, idsBuffer, rail);

  // 5. Toast tick + highlight prune.
  tickToasts(now);
  pruneHighlights(now);
}

export { highlights, addHighlight };
