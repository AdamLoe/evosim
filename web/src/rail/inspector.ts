// Inspector tab: click-targeted creature panel inside the right rail.
//
// SoA fast-path: when the selected id is still in the live snapshot, repaint
// the body fields locally (id/pos/color). The rich rows (action, cooldown,
// age, energy, NN stats, wall_proximity) come from a throttled `inspect_id`
// async refresh — 250 ms cap, ~4× traffic reduction at 60 RAF.

import {
  CREATURE_STRIDE,
  type SimBridge,
} from "../sim-bridge";
import type { Camera } from "../render";
import { screenToWorld } from "../render";
import { setRailOpen } from "../main";
import { highlights, HIGHLIGHT_PERMANENT } from "./highlight";
import type { RailState } from "./index";

type IS =
  | { kind: "empty" }
  | { kind: "pending" }
  | { kind: "selected"; creatureId: number; diedAt?: number };

let state: IS = { kind: "empty" };
let lastInspectIdRequestSeq = 0;
let lastInspectReplyMs = 0;
const INSPECT_ID_REFRESH_INTERVAL_MS = 250;

interface SoASnapshot {
  creatures: Float32Array;
  creaturesU32: Uint32Array;
  pop: number;
}

let latestSoA: SoASnapshot | null = null;

export function updateLatestSoA(creatures: Float32Array, pop: number): void {
  if (pop <= 0 || creatures.length === 0) {
    latestSoA = null;
    return;
  }
  const u32 = new Uint32Array(
    creatures.buffer,
    creatures.byteOffset,
    creatures.length,
  );
  latestSoA = { creatures, creaturesU32: u32, pop };
}

function idAt(soa: SoASnapshot, i: number): number {
  const base = i * CREATURE_STRIDE;
  const lo = soa.creaturesU32[base + 6];
  const hi = soa.creaturesU32[base + 7];
  return hi * 0x100000000 + lo;
}

function findIdIndex(soa: SoASnapshot, id: number): number {
  for (let i = 0; i < soa.pop; i++) {
    if (idAt(soa, i) === id) return i;
  }
  return -1;
}

function findNearestInSoA(
  soa: SoASnapshot,
  wx: number,
  wy: number,
  toleranceWorld: number,
): number {
  let best = -1;
  let bestD2 = Number.POSITIVE_INFINITY;
  for (let i = 0; i < soa.pop; i++) {
    const base = i * CREATURE_STRIDE;
    const x = soa.creatures[base];
    const y = soa.creatures[base + 1];
    const bodyR = soa.creatures[base + 2];
    const r = bodyR + toleranceWorld;
    const dx = wx - x;
    const dy = wy - y;
    const d2 = dx * dx + dy * dy;
    if (d2 <= r * r && d2 < bestD2) {
      best = i;
      bestD2 = d2;
    }
  }
  return best;
}

function set(id: string, text: string): void {
  const el = document.getElementById(id);
  if (el) el.textContent = text;
}

// v1.13 Wave 4: the rail orchestrator subscribes to selection-state changes
// so it can show/hide the Inspector tab button + panel. Fires synchronously
// from openInspector / clearSelection — never per-RAF.
type VisibilityListener = (selected: boolean) => void;
const visibilityListeners = new Set<VisibilityListener>();

export function subscribeInspectorVisibility(cb: VisibilityListener): void {
  visibilityListeners.add(cb);
}

function emitVisibility(selected: boolean): void {
  for (const cb of visibilityListeners) cb(selected);
}

function renderInspectorSoA(soa: SoASnapshot, idx: number, id: number): void {
  const base = idx * CREATURE_STRIDE;
  const x = soa.creatures[base];
  const y = soa.creatures[base + 1];
  const r = soa.creatures[base + 3];
  const g = soa.creatures[base + 4];
  const b = soa.creatures[base + 5];
  set("ins-id", `${id}`);
  set("ins-pos", `(${x.toFixed(1)}, ${y.toFixed(1)})`);
  const r255 = Math.round(Math.max(r, 0.15) * 255);
  const g255 = Math.round(Math.max(g, 0.15) * 255);
  const b255 = Math.round(Math.max(b, 0.15) * 255);
  const swatch = document.getElementById("ins-ema-swatch");
  if (swatch) swatch.style.backgroundColor = `rgb(${r255}, ${g255}, ${b255})`;
  set("ins-ema-values", `${r.toFixed(2)} / ${g.toFixed(2)} / ${b.toFixed(2)}`);
}

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
  color_ema: [number, number, number];
  wall_proximity: [number, number, number, number];
  nn_weight_count: number;
}

function openInspector(data: CreatureInspectJson, rail: RailState): void {
  if (state.kind === "selected" && state.creatureId !== data.id) {
    highlights.delete(state.creatureId);
  }
  state = { kind: "selected", creatureId: data.id };
  highlights.set(data.id, HIGHLIGHT_PERMANENT);
  renderInspector(data);
  rail.switchTab("inspector");
  emitVisibility(true);
}

function clearSelection(_rail: RailState): void {
  if (state.kind === "selected") {
    highlights.delete(state.creatureId);
  }
  state = { kind: "empty" };
  // v1.13 Wave 4: notify the rail orchestrator so it hides the Inspector
  // tab button + panel (and falls back to NN if Inspector was active).
  emitVisibility(false);
}

export function refreshInspector(
  simBridge: SimBridge,
  rail: RailState,
): void {
  if (state.kind !== "selected") return;
  const { creatureId } = state;

  if (latestSoA) {
    const idx = findIdIndex(latestSoA, creatureId);
    if (idx !== -1) {
      renderInspectorSoA(latestSoA, idx, creatureId);
    }
  }

  const now = performance.now();
  if (now - lastInspectReplyMs < INSPECT_ID_REFRESH_INTERVAL_MS) return;

  const mySeq = ++lastInspectIdRequestSeq;
  void simBridge.requestInspectId(creatureId).then((jsonStr) => {
    if (mySeq !== lastInspectIdRequestSeq) return;
    if (state.kind !== "selected" || state.creatureId !== creatureId) return;

    if (jsonStr === null) {
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
      lastInspectReplyMs = performance.now();
    } catch {
      /* Malformed — leave panel alone. */
    }
  });
}

export function installCanvasClickHandler(
  canvas: HTMLCanvasElement,
  cam: Camera,
  getView: () => { w: number; h: number },
  simBridge: SimBridge,
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
    if (dist >= 10 || elapsed >= 500) return;

    const { w, h } = getView();
    const rect = canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const [wx, wy] = screenToWorld(cam, w, h, sx, sy);
    const toleranceWorld = 6.0 / cam.zoom;

    if (latestSoA) {
      const idx = findNearestInSoA(latestSoA, wx, wy, toleranceWorld);
      if (idx !== -1) {
        const id = idAt(latestSoA, idx);
        if (state.kind === "selected" && state.creatureId !== id) {
          highlights.delete(state.creatureId);
        }
        state = { kind: "selected", creatureId: id };
        highlights.set(id, HIGHLIGHT_PERMANENT);
        renderInspectorSoA(latestSoA, idx, id);
        set("ins-action", "…");
        lastInspectReplyMs = 0;
        // v1.9.1: force the rail open so the Inspector tab is actually
        // visible — a creature click on the canvas while the rail is
        // collapsed would otherwise silently switch to a hidden tab.
        setRailOpen(true);
        // v1.13 Wave 4: emit visibility before switchTab so the rail
        // orchestrator un-hides the tab button first; switchTab can then
        // mark it active without flashing a hidden tab.
        emitVisibility(true);
        rail.switchTab("inspector");
        const mySeq = ++lastInspectIdRequestSeq;
        void simBridge.requestInspectId(id).then((jsonStr) => {
          if (mySeq !== lastInspectIdRequestSeq) return;
          if (state.kind !== "selected" || state.creatureId !== id) return;
          if (jsonStr === null) return;
          try {
            const data: CreatureInspectJson = JSON.parse(jsonStr);
            renderInspector(data);
            lastInspectReplyMs = performance.now();
          } catch {
            /* Malformed — leave SoA paint in place. */
          }
        });
        return;
      }
    }

    // SoA miss: fall back to authoritative grid-backed lookup.
    state = { kind: "pending" };
    set("ins-action", "selecting…");
    // v1.9.1: same rail-open guarantee on the fallback path.
    setRailOpen(true);
    // v1.13 Wave 4: same un-hide-then-switch order as the SoA-hit path.
    emitVisibility(true);
    rail.switchTab("inspector");
    void simBridge.requestInspectAt(wx, wy, toleranceWorld).then((jsonStr) => {
      if (state.kind !== "pending") return;
      if (!jsonStr) {
        clearSelection(rail);
        return;
      }
      try {
        const data: CreatureInspectJson = JSON.parse(jsonStr);
        openInspector(data, rail);
        lastInspectReplyMs = performance.now();
      } catch {
        clearSelection(rail);
      }
    });
  });
}

export function resetInspectorSelection(rail: RailState): void {
  clearSelection(rail);
}
