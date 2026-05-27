// Inspector panel (E.24): click-targeted creature panel.
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

function getInspectorBox(): HTMLElement | null {
  return document.getElementById("inspector-box");
}
function getInspectorClose(): HTMLButtonElement | null {
  return document.getElementById("inspector-close") as HTMLButtonElement | null;
}
function isRailHidden(): boolean {
  const rail = document.getElementById("right-rail");
  return !rail || rail.style.display === "none" ||
         getComputedStyle(rail).display === "none";
}

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
  set("ins-id", `${data.id}`);
  set("ins-pos", `(${data.x.toFixed(1)}, ${data.y.toFixed(1)})`);
  set("ins-action", data.current_action);
  set("ins-age", `${data.age} / ${data.max_age}`);
  set("ins-energy", `${data.energy.toFixed(1)} (${(data.energy_frac * 100).toFixed(0)}%)`);
  set("ins-size", data.size.toFixed(2));
  set("ins-photo", data.graze_efficiency.toFixed(2));
  set("ins-eat", data.eat_efficiency.toFixed(2));
  set("ins-move", data.move_speed.toFixed(2));
  set("ins-eyes", `${data.eye_count}`);
  set("ins-vision", data.vision_range.toFixed(1));
  set("ins-armor", data.armor.toFixed(2));
  set("ins-bite", data.bite_reach.toFixed(2));
  // M5: NN-weight hash color replaces placeholder pigment.
  set("ins-color", data.color_rgb);
  set("ins-hash", data.weight_hash);
  set("ins-nn-rate", data.nn_mutation_rate.toFixed(4));
  set("ins-nn-count", `${data.nn_weight_count}`);
}

export interface CreatureInspectJson {
  index: number;
  id: number;
  x: number;
  y: number;
  age: number;
  max_age: number;
  energy: number;
  energy_frac: number;
  size: number;
  current_action: string;
  graze_efficiency: number;
  eat_efficiency: number;
  move_speed: number;
  vision_range: number;
  eye_count: number;
  armor: number;
  bite_reach: number;
  // M5: NN-weight hash color replaces placeholder pigment.
  color_rgb: string;
  weight_hash: string;
  nn_mutation_rate: number;
  nn_weight_count: number;
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
  const box = getInspectorBox();
  if (box) box.style.display = "block";
  // Legacy rail tab injection — guarded no-op while rail is hidden (D2).
  if (!isRailHidden()) {
    const tab = ensureInspectorTab(rail);
    tab.classList.remove("hidden");
    rail.switchTab("inspector");
  }
}

function clearSelection(rail: RailState): void {
  if (state.kind === "selected") {
    highlights.delete(state.creatureId);
  }
  state = { kind: "empty" };
  const box = getInspectorBox();
  if (box) box.style.display = "none";
  // Legacy rail tab cleanup — guarded no-op while rail is hidden (D2).
  if (!isRailHidden()) {
    if (inspectorTab) inspectorTab.classList.add("hidden");
    if ((rail as { activeTab?: string }).activeTab === "inspector") {
      rail.switchTab("stats");
    }
  }
}

/**
 * Refresh the inspector panel each frame. Resolves the selected creature
 * by stable id via creature_idx_by_id; handles death with 2s placeholder
 * then auto-close. S18: no longer scans the ids buffer per frame.
 */
export function refreshInspector(
  world: WorldHandle,
  rail: RailState,
): void {
  const box = getInspectorBox();
  // Primary guard: skip all wasm calls when the box is closed (D3).
  // The display check is the explicit contract for "closed = no wasm calls"
  // and protects against future state-vs-DOM drift.
  if (box && box.style.display === "none") return;
  // Defensive guard: steady-state fallback; in practice clearSelection()
  // already sets state.kind = "empty" before the display-none check can
  // be reached, so both guards agree — this is belt-and-braces only.
  if (state.kind !== "selected") return;
  const { creatureId } = state;

  // S18: stable-id lookup. Returns Option<u32> via wasm-bindgen
  // (`number | undefined`). None means the creature died.
  const foundIdx = world.creature_idx_by_id(creatureId);

  if (foundIdx === undefined) {
    // Creature died. 2-second placeholder per DECISIONS E.24.
    if (!state.diedAt) {
      state.diedAt = performance.now();
      set("ins-action", "Creature died");
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
  getWorld: () => WorldHandle,
  rail: RailState,
): void {
  const closeBtn = getInspectorClose();
  if (closeBtn) {
    closeBtn.addEventListener("click", () => clearSelection(rail));
  }

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

    const world = getWorld();
    const { w, h } = getView();
    const rect = canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const [wx, wy] = screenToWorld(cam, w, h, sx, sy);
    // Tolerance in world units so tap target is at least 6 screen pixels wide.
    const toleranceWorld = 6.0 / cam.zoom;
    // S18: creature_at returns a stable id (Option<f64>, surfaced as
    // `number | undefined`). We immediately resolve it to an idx for the
    // first inspect call; per-frame refresh uses creature_idx_by_id.
    const id = world.creature_at(wx, wy, toleranceWorld);

    if (id === undefined || id === null) {
      clearSelection(rail);
    } else {
      // resolve id → idx for creature_inspect_json (same call stack; no step()
      // between creature_at and inspect, so idx is guaranteed valid here).
      const idx = world.creature_idx_by_id(id);
      if (idx !== undefined) {
        const jsonStr = world.creature_inspect_json(idx);
        if (jsonStr) {
          const data: CreatureInspectJson = JSON.parse(jsonStr);
          openInspector(data, rail);
        }
      }
    }
  });
}

/** Close the inspector and clear its selection. Used by world-restart so
 * the stale creature id from the previous world doesn't linger. */
export function resetInspectorSelection(rail: RailState): void {
  clearSelection(rail);
}
