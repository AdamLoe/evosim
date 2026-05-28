// Inspector panel (E.24): click-targeted creature panel.
//
// v1.6 Wave B: click → async `inspect_at` request via SimBridge; per-frame
// refresh → async `inspect_id` request. Stale-request_id replies are dropped
// by the SimBridge's correlation map. If a reply lands within 1 frame the
// panel paints; otherwise the placeholder "selecting…" stays visible.
//
// v1.6 Wave D: SAB id-column fast-path. The Wave C snapshot SAB carries each
// creature's `[x, y, body_r, r, g, b, id_lo, id_hi]` SoA inline (stride 8).
// We mirror it into a per-frame `latestSoA` cache so:
//   1. Canvas-click hit-tests resolve the nearest creature locally in JS and
//      paint placeholder body fields (id, x, y, color) within the same frame
//      — then async-request the full JSON for cooldown/action/NN stats.
//   2. Per-frame refresh paints the SoA fields directly when the selected id
//      still appears in the SoA, and only sends `inspect_id` if the panel is
//      open AND > 250 ms have elapsed since the last successful reply.
// Cuts inspector traffic ~4× at 60 RAF (was 1 inspect_id every frame; now
// every ~4 frames) and pulls click→panel latency from 2 frames to sub-frame.

import {
  CREATURE_STRIDE,
  type SimBridge,
} from "../sim-bridge";
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

// ─── Wave D SAB id-column fast-path ────────────────────────────────────────
//
// Cache of the most recent SoA snapshot. `pollRail` refreshes this each RAF
// via `updateLatestSoA` before calling `refreshInspector`. Click handlers
// consult it on `pointerup`. Held as the Float32Array view directly so the
// inspector doesn't pay the cost of a per-frame copy — the view aliases the
// SAB and is valid for the current frame only.

interface SoASnapshot {
  creatures: Float32Array;
  /** u32-typed view over the SAME bytes as `creatures`; index = base + 6/7 for id lo/hi. */
  creaturesU32: Uint32Array;
  pop: number;
}

let latestSoA: SoASnapshot | null = null;

/**
 * Time (ms) of the most recent successful `inspect_id` reply, used to throttle
 * per-frame async refreshes. The Rust-side JSON adds NN stats / cooldown /
 * action that the SoA can't paint locally, so we still need it — just not at
 * 60 Hz when the SoA already gives us a usable refresh. 250 ms ⇒ ~4× traffic
 * reduction at 60 RAF.
 */
let lastInspectReplyMs = 0;
const INSPECT_ID_REFRESH_INTERVAL_MS = 250;

/**
 * Wave D: refresh the per-frame SoA cache. Main calls this each RAF, before
 * `refreshInspector`, with the same Float32Array view its renderer is about
 * to consume (so we never read a slot mid-flip).
 */
export function updateLatestSoA(creatures: Float32Array, pop: number): void {
  if (pop <= 0 || creatures.length === 0) {
    latestSoA = null;
    return;
  }
  // Build a Uint32Array view aliasing the same bytes so the id u32 halves
  // can be read without ceremony. The Float32Array's underlying buffer is
  // the SAB; the SoA stride 32 B is u32-aligned at every base.
  const u32 = new Uint32Array(
    creatures.buffer,
    creatures.byteOffset,
    creatures.length,
  );
  latestSoA = { creatures, creaturesU32: u32, pop };
}

/** Decode the (id_lo, id_hi) → f64 id at SoA index `i`. */
function idAt(soa: SoASnapshot, i: number): number {
  const base = i * CREATURE_STRIDE;
  const lo = soa.creaturesU32[base + 6];
  const hi = soa.creaturesU32[base + 7];
  // f64 mantissa is exact up to 2^53 — safe for any v1-session id. Same
  // round-trip Rust uses (DECISIONS E.21).
  return hi * 0x100000000 + lo;
}

/** Find the SoA index for `id`, or -1 if it's not in the current snapshot. */
function findIdIndex(soa: SoASnapshot, id: number): number {
  for (let i = 0; i < soa.pop; i++) {
    if (idAt(soa, i) === id) return i;
  }
  return -1;
}

/**
 * Find the nearest creature to (`wx`, `wy`) in world space within
 * `body_r + toleranceWorld`, identical to the Rust-side `creature_at` logic
 * but in JS over the SAB SoA. Returns the SoA index or -1.
 */
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

/**
 * Paint the inspector body using only SoA fields. Used for the click
 * placeholder and the per-frame fast refresh. The "rich" fields (action,
 * cooldown, age, energy, NN stats, wall_proximity) keep whatever the last
 * `inspect_id` reply wrote; if there has never been one, they stay empty or
 * carry whatever `set("ins-…", "…")` initialized them with.
 */
function renderInspectorSoA(soa: SoASnapshot, idx: number, id: number): void {
  const base = idx * CREATURE_STRIDE;
  const x = soa.creatures[base];
  const y = soa.creatures[base + 1];
  const r = soa.creatures[base + 3];
  const g = soa.creatures[base + 4];
  const b = soa.creatures[base + 5];
  set("ins-id", `${id}`);
  set("ins-pos", `(${x.toFixed(1)}, ${y.toFixed(1)})`);
  // Display-floor for the swatch matches `renderInspector`'s 0.15 floor.
  const r255 = Math.round(Math.max(r, 0.15) * 255);
  const g255 = Math.round(Math.max(g, 0.15) * 255);
  const b255 = Math.round(Math.max(b, 0.15) * 255);
  const swatch = document.getElementById("ins-ema-swatch");
  if (swatch) swatch.style.backgroundColor = `rgb(${r255}, ${g255}, ${b255})`;
  set("ins-ema-values", `${r.toFixed(2)} / ${g.toFixed(2)} / ${b.toFixed(2)}`);
}

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
 * Refresh the inspector panel each frame.
 *
 * Wave D fast-path: if the selected id is still in `latestSoA`, repaint the
 * SoA fields (id, pos, color) immediately — sub-frame latency, no async hop.
 * The full `inspect_id` JSON (action, cooldown, age, energy, NN stats,
 * wall_proximity) is still needed for the rich rows, but we only request it
 * if the panel is open AND > 250 ms since the last successful reply. That
 * cuts inspector traffic ~4× vs Wave C's per-frame request.
 *
 * Replies that arrive after a newer request are dropped via the
 * `lastInspectIdRequestSeq` guard. Handles death with a 2 s placeholder then
 * auto-close.
 */
export function refreshInspector(
  simBridge: SimBridge,
  rail: RailState,
): void {
  const box = getInspectorBox();
  // Primary guard: skip all work when the box is closed.
  if (box && box.style.display === "none") return;
  if (state.kind !== "selected") return;
  const { creatureId } = state;

  // SoA fast-path: paint the live position + color now if the creature is
  // still in the snapshot. Costs an O(pop) scan but at v1 scale (pop ≤ 8000)
  // this is well under 100 µs per frame and runs only while the panel is open.
  if (latestSoA) {
    const idx = findIdIndex(latestSoA, creatureId);
    if (idx !== -1) {
      renderInspectorSoA(latestSoA, idx, creatureId);
    }
  }

  // Throttled async refresh for the rich rows. Skip entirely if we've had a
  // reply recently — the SoA fast-path keeps position/color current in the
  // meantime, and the rich rows mutate slowly compared to position.
  const now = performance.now();
  if (now - lastInspectReplyMs < INSPECT_ID_REFRESH_INTERVAL_MS) return;

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
      lastInspectReplyMs = performance.now();
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

    const box = getInspectorBox();
    if (box) box.style.display = "block";

    // Wave D fast-path: try resolving the click locally against the SAB
    // SoA. If we hit, paint placeholder body fields (id/x/y/color) within
    // this frame, mark a permanent highlight, and async-request `inspect_id`
    // for the rich rows. If we miss, fall back to the Wave B `inspect_at`
    // request so the worker's authoritative spatial-grid lookup gets a shot
    // (covers the case where main's SoA cache is stale-by-one-frame).
    if (latestSoA) {
      const idx = findNearestInSoA(latestSoA, wx, wy, toleranceWorld);
      if (idx !== -1) {
        const id = idAt(latestSoA, idx);
        // Switching selection: drop the previous highlight ring.
        if (state.kind === "selected" && state.creatureId !== id) {
          highlights.delete(state.creatureId);
        }
        state = { kind: "selected", creatureId: id };
        highlights.set(id, HIGHLIGHT_PERMANENT);
        renderInspectorSoA(latestSoA, idx, id);
        // Rich-rows placeholder while we wait for the JSON reply.
        set("ins-action", "…");
        // Reset throttle so refreshInspector requests fresh JSON right away.
        lastInspectReplyMs = 0;
        // Legacy rail tab toggle (no-op while rail is hidden).
        if (!isRailHidden()) {
          const tab = ensureInspectorTab(rail);
          tab.classList.remove("hidden");
          rail.switchTab("inspector");
        }
        // Kick off the full JSON request for cooldown/action/NN stats. The
        // reply lands via `refreshInspector`'s pipeline (shared
        // `lastInspectIdRequestSeq` + `lastInspectReplyMs`) so a click + a
        // concurrent refresh can't race each other.
        const mySeq = ++lastInspectIdRequestSeq;
        void simBridge.requestInspectId(id).then((jsonStr) => {
          if (mySeq !== lastInspectIdRequestSeq) return;
          if (state.kind !== "selected" || state.creatureId !== id) return;
          if (jsonStr === null) return; // creature died between click + reply.
          try {
            const data: CreatureInspectJson = JSON.parse(jsonStr);
            renderInspector(data);
            lastInspectReplyMs = performance.now();
          } catch {
            // Malformed — leave SoA paint in place.
          }
        });
        return;
      }
    }

    // SoA miss (or SoA not yet populated): fall back to Wave B path.
    state = { kind: "pending" };
    set("ins-action", "selecting…");
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
        lastInspectReplyMs = performance.now();
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
