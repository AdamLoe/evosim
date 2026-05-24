// Inspector panel (E.21/E.24): click-targeted creature panel.
// Opens on canvas click via creature_at; refreshes per frame while selected.

import type { WorldHandle } from "../../wasm/evosim";
import type { Camera } from "../render";
import { screenToWorld } from "../render";
import { highlights, HIGHLIGHT_PERMANENT } from "./highlight";
import type { RailState } from "./index";

type IS = { kind: "empty" } | { kind: "selected"; creatureId: number; diedAt?: number };

let state: IS = { kind: "empty" };

// The Inspector tab button (injected dynamically into #rail-tabs).
let inspectorTab: HTMLButtonElement | null = null;

function ensureInspectorTab(rail: RailState): HTMLButtonElement {
  if (inspectorTab) return inspectorTab;
  const btn = document.createElement("button");
  btn.dataset.tab = "inspector";
  btn.className = "rail-tab hidden";
  btn.textContent = "Inspector";
  document.getElementById("rail-tabs")!.appendChild(btn);
  btn.addEventListener("click", () => rail.switchTab("inspector"));
  inspectorTab = btn;
  return btn;
}

function set(id: string, text: string): void {
  const el = document.getElementById(id);
  if (el) el.textContent = text;
}

function renderInspector(data: CreatureInspectJson): void {
  set("ins-species", `${data.species_name} (#${data.species_id})`);
  set("ins-parent", data.parent_species_name ?? "—");
  set("ins-action", data.current_action);
  set("ins-age", `${data.age} / ${data.max_age}`);
  set("ins-energy", `${data.energy.toFixed(1)} (${(data.energy_frac * 100).toFixed(0)}%)`);
  set("ins-size", data.size.toFixed(2));
  set("ins-photo", data.photosynth_efficiency.toFixed(2));
  set("ins-eat", data.eat_efficiency.toFixed(2));
  set("ins-scav", data.scavenge_efficiency.toFixed(2));
  set("ins-move", data.move_speed.toFixed(2));
  set("ins-eyes", `${data.eye_count}`);
  set("ins-vision", data.vision_range.toFixed(1));
  set("ins-armor", data.armor.toFixed(2));
  set("ins-bite", data.bite_reach.toFixed(2));
  const [r, g, b] = data.pigment;
  set(
    "ins-pigment",
    `(${(r * 255).toFixed(0)}, ${(g * 255).toFixed(0)}, ${(b * 255).toFixed(0)})`,
  );
}

export interface CreatureInspectJson {
  index: number;
  id: number;
  species_id: number;
  species_name: string;
  parent_species_id: number;
  parent_species_name: string | null;
  x: number;
  y: number;
  age: number;
  max_age: number;
  energy: number;
  energy_frac: number;
  size: number;
  current_action: string;
  photosynth_efficiency: number;
  eat_efficiency: number;
  scavenge_efficiency: number;
  move_speed: number;
  vision_range: number;
  eye_count: number;
  armor: number;
  bite_reach: number;
  pigment: [number, number, number];
}

function openInspector(data: CreatureInspectJson, rail: RailState): void {
  // Remove the previous creature's permanent highlight before switching
  // selection, so only one ring is ever visible at a time (Bug 2 fix).
  if (state.kind === "selected" && state.creatureId !== data.id) {
    highlights.delete(state.creatureId);
  }
  state = { kind: "selected", creatureId: data.id };
  highlights.set(data.id, HIGHLIGHT_PERMANENT);
  renderInspector(data);
  const tab = ensureInspectorTab(rail);
  tab.classList.remove("hidden");
  rail.switchTab("inspector");
}

function clearSelection(rail: RailState): void {
  if (state.kind === "selected") {
    highlights.delete(state.creatureId);
  }
  state = { kind: "empty" };
  if (inspectorTab) {
    inspectorTab.classList.add("hidden");
  }
  if ((rail as { activeTab?: string }).activeTab === "inspector") {
    rail.switchTab("events");
  }
}

/**
 * Refresh the inspector panel each frame. Resolves the selected creature
 * by id via the ids buffer; handles death with 2s placeholder then auto-close.
 */
export function refreshInspector(
  world: WorldHandle,
  idsBuffer: Float64Array,
  rail: RailState,
): void {
  if (state.kind !== "selected") return;
  const { creatureId } = state;

  // Build id → index map.
  let foundIdx = -1;
  for (let k = 0; k < idsBuffer.length; k++) {
    if (idsBuffer[k] === creatureId) {
      foundIdx = k;
      break;
    }
  }

  if (foundIdx < 0) {
    // Creature died.
    if (!state.diedAt) {
      state.diedAt = performance.now();
      set("ins-species", "Creature died");
      set("ins-action", "—");
    }
    if (performance.now() - state.diedAt > 2000) {
      clearSelection(rail);
    }
    return;
  }

  const jsonStr = world.creature_inspect_json(foundIdx);
  if (!jsonStr) {
    clearSelection(rail);
    return;
  }
  const data: CreatureInspectJson = JSON.parse(jsonStr);
  renderInspector(data);
  // Keep permanent highlight.
  highlights.set(creatureId, HIGHLIGHT_PERMANENT);
}

/**
 * Install canvas click handler.
 *
 * Tap detection thresholds (chosen to be forgiving at high sim speeds):
 *   - movement: < 10 px  (was 4 — too strict; cursor naturally drifts while reading)
 *   - elapsed:  < 500 ms (was 250 — too strict; deliberate targeting takes longer)
 *
 * Hit-test uses a tolerance_world radius of 6 / cam.zoom so that the minimum
 * tap target is always at least 6 screen pixels regardless of creature size or
 * zoom level (fixes the 4 screen-px hit target for size-1 creatures at zoom 4).
 */
export function installCanvasClickHandler(
  canvas: HTMLCanvasElement,
  cam: Camera,
  getView: () => { w: number; h: number },
  world: WorldHandle,
  rail: RailState,
): void {
  let downX = 0;
  let downY = 0;
  let downTime = 0;

  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    downX = e.clientX;
    downY = e.clientY;
    downTime = performance.now();
  });

  canvas.addEventListener("pointerup", (e) => {
    if (e.button !== 0) return;
    const dx = e.clientX - downX;
    const dy = e.clientY - downY;
    const elapsed = performance.now() - downTime;
    const dist = Math.sqrt(dx * dx + dy * dy);
    // Tap detection: < 10 px movement, < 500 ms elapsed.
    if (dist >= 10 || elapsed >= 500) return;

    const { w, h } = getView();
    const rect = canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const [wx, wy] = screenToWorld(cam, w, h, sx, sy);
    // Tolerance in world units so tap target is at least 6 screen pixels wide.
    const toleranceWorld = 6.0 / cam.zoom;
    const idx = world.creature_at(wx, wy, toleranceWorld);

    if (idx === undefined || idx === null) {
      clearSelection(rail);
    } else {
      const jsonStr = world.creature_inspect_json(idx);
      if (jsonStr) {
        const data: CreatureInspectJson = JSON.parse(jsonStr);
        openInspector(data, rail);
      }
    }
  });
}
