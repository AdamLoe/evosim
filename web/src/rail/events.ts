// Events panel renderer (E.21).
// Two sub-sections: active species list (top) + scrolling event log (bottom).

import type { EvEvent } from "./index";

const MAX_EVENT_ROWS = 200;

// ---- Species list ----

export interface SpeciesRow {
  id: number;
  name: string;
  population: number;
  parent_id: number | null;
}

let speciesContainer: HTMLDivElement | null = null;

function ensureSpeciesContainer(): HTMLDivElement {
  if (speciesContainer) return speciesContainer;
  const panel = document.getElementById("rail-events")!;
  const wrapper = document.createElement("div");
  wrapper.className = "events-species-list";
  const label = document.createElement("div");
  label.style.cssText = "opacity:0.4;margin-bottom:4px;font-size:10px;text-transform:uppercase;letter-spacing:0.05em";
  label.textContent = "Live species";
  wrapper.appendChild(label);
  speciesContainer = document.createElement("div");
  wrapper.appendChild(speciesContainer);
  panel.insertBefore(wrapper, panel.firstChild);
  return speciesContainer;
}

export function renderSpeciesList(rows: SpeciesRow[]): void {
  const container = ensureSpeciesContainer();
  // Sort by population descending.
  const sorted = [...rows].sort((a, b) => b.population - a.population);
  container.innerHTML = "";
  for (const sp of sorted) {
    const div = document.createElement("div");
    div.className = "species-row";
    div.dataset.speciesId = String(sp.id);
    div.textContent = `${sp.name} (${sp.population})`;
    div.title = sp.parent_id !== null ? `Parent: #${sp.parent_id}` : "Founder";
    container.appendChild(div);
  }
}

// ---- Event log ----

let eventLogContainer: HTMLDivElement | null = null;

function ensureEventLog(): HTMLDivElement {
  if (eventLogContainer) return eventLogContainer;
  const panel = document.getElementById("rail-events")!;
  const log = document.createElement("div");
  log.id = "event-log";
  log.style.cssText = "flex:1;overflow-y:auto;border-top:1px solid rgba(255,255,255,0.06);margin-top:6px;padding-top:4px";
  eventLogContainer = log;
  panel.appendChild(log);
  return log;
}

export function formatEvent(ev: EvEvent, speciesNameCache: Map<number, string>): string {
  const k = ev.kind;
  switch (k.type) {
    case "Speciation": {
      const parentName = speciesNameCache.get(k.parent_species_id) ?? "(unknown)";
      return `Speciation: ${k.new_species_name} from ${parentName}`;
    }
    case "Extinction":
      return `Extinction: ${k.species_name}`;
    case "PopulationMilestone":
      return `Pop milestone: ${k.population}`;
    case "FirstToMove":
      return "First mover";
    case "FirstToEat":
      return "First bite";
    case "WorldEnded":
      return `World ended (tick ${ev.tick}, peak pop ${k.peak_population})`;
    default:
      return JSON.stringify(k);
  }
}

export function appendEventRow(ev: EvEvent, speciesNameCache: Map<number, string>): void {
  const log = ensureEventLog();
  const row = document.createElement("div");
  row.className = "event-row";
  row.innerHTML = `<span class="tick">t=${ev.tick}</span>${formatEvent(ev, speciesNameCache)}`;
  // Prepend so newest is at top.
  log.insertBefore(row, log.firstChild);
  // Cap visible rows.
  while (log.children.length > MAX_EVENT_ROWS) {
    log.removeChild(log.lastChild!);
  }
}
