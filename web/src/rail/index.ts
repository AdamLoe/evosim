// Rail orchestrator: boot + pollRail. Called from main.ts each RAF frame.
// E.21: Events panel + species list.
// E.22: Toast routing + highlight management.
// E.23: Stats sampling.
// E.24: Inspector refresh.

import type { WorldHandle } from "../../wasm/evosim";
import {
  renderSpeciesList,
  appendEventRow,
  type SpeciesRow,
} from "./events";
import { maybeSampleStats } from "./stats";
import { refreshInspector } from "./inspector";
import { pushToast, tickToasts } from "./toast";
import { addHighlight, pruneHighlights, highlights } from "./highlight";

// ---- Event shape (matches EventKind #[serde(tag = "type")]) ----

export type EvKind =
  | { type: "Speciation"; new_species_id: number; parent_species_id: number; new_species_name: string; creature_id: number }
  | { type: "Extinction"; species_id: number; species_name: string }
  | { type: "PopulationMilestone"; population: number }
  | { type: "FirstToMove"; creature_id: number }
  | { type: "FirstToEat"; creature_id: number }
  | { type: "WorldEnded"; ticks_lived: number; peak_population: number; peak_species: number };

export interface EvEvent {
  tick: number;
  kind: EvKind;
}

// ---- Rail state (opaque to main.ts) ----

export interface RailState {
  switchTab(name: string): void;
  activeTab: string;
}

// ---- Tab switching ----

function installTabs(): RailState {
  let activeTab = "events";

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
let lastSeenTotalCount = 0;
let lastSpeciesPoll = -Infinity;

// Species name cache for toast parent-name lookups.
const speciesNameCache: Map<number, string> = new Map();

function handleNewEvent(ev: EvEvent): void {
  const k = ev.kind;
  switch (k.type) {
    case "Speciation": {
      const parentName = speciesNameCache.get(k.parent_species_id) ?? "(unknown)";
      pushToast(`New species: ${k.new_species_name} from ${parentName}`);
      addHighlight(k.creature_id);
      break;
    }
    case "Extinction":
      pushToast(`Extinct: ${k.species_name}`);
      break;
    case "FirstToMove":
      pushToast("First mover");
      addHighlight(k.creature_id);
      break;
    case "FirstToEat":
      pushToast("First bite");
      addHighlight(k.creature_id);
      break;
    case "PopulationMilestone":
    case "WorldEnded":
      // Log-only; no toast (v6 §B policy).
      break;
  }
}

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

  // 2. Diff events using events_total_count (PIN C: monotone, survives ring overflow).
  const totalCount = world.events_total_count;
  const newCount = totalCount - lastSeenTotalCount;
  if (newCount > 0) {
    let recent: EvEvent[] = [];
    try {
      recent = JSON.parse(world.recent_events_json()) as EvEvent[];
    } catch (e) {
      console.warn("rail: recent_events_json parse failed", e);
    }
    // Take at most the last `newCount` events from the ring.
    const start = Math.max(0, recent.length - newCount);
    for (let i = start; i < recent.length; i++) {
      const ev = recent[i];
      handleNewEvent(ev);
      appendEventRow(ev, speciesNameCache);
    }
    lastSeenTotalCount = totalCount;
  }

  // 3. Stats sample (E.23).
  maybeSampleStats(world);

  // 4. Inspector refresh (E.24).
  refreshInspector(world, idsBuffer, rail);

  // 5. Toast tick + highlight prune.
  tickToasts(now);
  pruneHighlights(now);
}

export { highlights, addHighlight };
