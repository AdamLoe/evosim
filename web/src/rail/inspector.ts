// Inspector panel (E.24): click-targeted creature panel.
//
// v1.6 Wave B: click → async `inspect_at` request via SimBridge; per-frame
// refresh → async `inspect_id` request. Stale-request_id replies are dropped
// by the SimBridge's correlation map. If a reply lands within 1 frame the
// panel paints; otherwise the placeholder "selecting…" stays visible.

import type { SimBridge } from "../sim-bridge";
import type { Camera } from "../render";
import { screenToWorld } from "../render";
import { highlights, HIGHLIGHT_PERMANENT } from "./highlight";
import type { RailState } from "./index";

type IS =
  | { kind: "empty" }
  | { kind: "pending" } // click sent, no reply yet
  | { kind: "selected"; creatureId: number; diedAt?: number };

let state: IS = { kind: "empty" };

// Per-selection request-id tracker: drop replies that arrive after a newer
// `inspect_id` has been sent. Cheap belt-and-braces (SimBridge already
// correlates by id, but a selection switch should also invalidate older
// in-flight refreshes).
let lastInspectIdRequestSeq = 0;

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

// v1.5 S3: TS-side action ring buffer (per selected creature). Reset on
// selection change; entries are the current_action string each refresh.
let actionHistory: string[] = [];
let historyForId: number | null = null;

function pushHistory(creatureId: number, action: string): void {
  if (historyForId !== creatureId) {
    actionHistory = [];
    historyForId = creatureId;
  }
  actionHistory.push(action);
  if (actionHistory.length > 60) actionHistory.shift();
}

function renderInspector(data: CreatureInspectJson): void {
  set("ins-id", `${data.id}`);
  set("ins-pos", `(${data.x.toFixed(1)}, ${data.y.toFixed(1)})`);
  set("ins-action", data.current_action);
  set("ins-age", `${data.age} / ${data.max_age}`);
  set("ins-energy", `${data.energy.toFixed(1)} (${(data.energy_frac * 100).toFixed(0)}%)`);
  set("ins-cooldown", `${data.cooldown_remaining}`);
  // v1.5 S3: EMA swatch (display-floored to 0.15) + raw triplet text.
  const [r, g, b] = data.color_ema;
  const r255 = Math.round(Math.max(r, 0.15) * 255);
  const g255 = Math.round(Math.max(g, 0.15) * 255);
  const b255 = Math.round(Math.max(b, 0.15) * 255);
  const swatch = document.getElementById("ins-ema-swatch");
  if (swatch) swatch.style.backgroundColor = `rgb(${r255}, ${g255}, ${b255})`;
  set("ins-ema-values", `${r.toFixed(2)} / ${g.toFixed(2)} / ${b.toFixed(2)}`);
  const [wn, ws, we, ww] = data.wall_proximity;
  set("ins-walls", `${wn.toFixed(2)} / ${ws.toFixed(2)} / ${we.toFixed(2)} / ${ww.toFixed(2)}`);
  pushHistory(data.id, data.current_action);
  set("ins-history", actionHistory.slice(-30).map(a => a[0]).join(""));
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
  move_speed: number;
  cooldown_remaining: number;
  // v1.5 S3: action-EMA color (raw, unfloored).
  color_ema: [number, number, number];
  // v1.5 S3: N/S/E/W wall proximity in [0, 1].
  wall_proximity: [number, number, number, number];
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
 * by stable id via an async `inspect_id` request through the SimBridge;
 * replies that arrive after a newer request are dropped via the
 * `lastInspectIdRequestSeq` guard. Handles death with a 2 s placeholder
 * then auto-close.
 */
export function refreshInspector(
  simBridge: SimBridge,
  rail: RailState,
): void {
  const box = getInspectorBox();
  // Primary guard: skip all requests when the box is closed.
  if (box && box.style.display === "none") return;
  if (state.kind !== "selected") return;
  const { creatureId } = state;

  const mySeq = ++lastInspectIdRequestSeq;
  void simBridge.requestInspectId(creatureId).then((jsonStr) => {
    // Drop replies superseded by a newer request (selection moved on).
    if (mySeq !== lastInspectIdRequestSeq) return;
    if (state.kind !== "selected" || state.creatureId !== creatureId) return;

    if (jsonStr === null) {
      // Creature died (or reply dropped). 2-second placeholder per DECISIONS E.24.
      if (state.kind === "selected" && !state.diedAt) {
        state.diedAt = performance.now();
        set("ins-action", "Creature died");
      }
      if (state.kind === "selected" && state.diedAt &&
          performance.now() - state.diedAt > 2000) {
        clearSelection(rail);
      }
      return;
    }

    try {
      const data: CreatureInspectJson = JSON.parse(jsonStr);
      renderInspector(data);
      highlights.set(creatureId, HIGHLIGHT_PERMANENT);
    } catch {
      // Malformed reply — leave panel alone.
    }
  });
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
 * zoom level.
 */
export function installCanvasClickHandler(
  canvas: HTMLCanvasElement,
  cam: Camera,
  getView: () => { w: number; h: number },
  simBridge: SimBridge,
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

    const { w, h } = getView();
    const rect = canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const [wx, wy] = screenToWorld(cam, w, h, sx, sy);
    // Tolerance in world units so tap target is at least 6 screen pixels wide.
    const toleranceWorld = 6.0 / cam.zoom;

    // Mark a request in flight; if the reply takes > 1 frame the user sees
    // "selecting…" instead of stale data from a previous selection.
    state = { kind: "pending" };
    set("ins-action", "selecting…");
    const box = getInspectorBox();
    if (box) box.style.display = "block";

    void simBridge.requestInspectAt(wx, wy, toleranceWorld).then((jsonStr) => {
      // If the user clicked again before this reply arrived, the new click's
      // `state = { kind: "pending" }` set above will have overwritten ours.
      // Only honor this reply if state is still pending.
      if (state.kind !== "pending") return;
      if (!jsonStr) {
        clearSelection(rail);
        return;
      }
      try {
        const data: CreatureInspectJson = JSON.parse(jsonStr);
        openInspector(data, rail);
      } catch {
        clearSelection(rail);
      }
    });
  });
}

/** Close the inspector and clear its selection. Used by world-restart so
 * the stale creature id from the previous world doesn't linger. */
export function resetInspectorSelection(rail: RailState): void {
  clearSelection(rail);
}
