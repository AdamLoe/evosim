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
import { getSettings } from "../settings";
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
  // v2.0 Wave 2b: id halves moved to lanes 4/5 (were 6/7).
  const lo = soa.creaturesU32[base + 4];
  const hi = soa.creaturesU32[base + 5];
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

// v1.13: the Inspector tab is always visible in the rail strip. The panel
// body (#inspector-body) and the empty hint (#inspector-empty) swap based
// on whether a creature is currently selected. `emitVisibility(true)`
// shows the body; `emitVisibility(false)` shows the "waiting" hint.
function emitVisibility(selected: boolean): void {
  const empty = document.getElementById("inspector-empty");
  const body = document.getElementById("inspector-body");
  if (empty) empty.style.display = selected ? "none" : "";
  if (body) body.style.display = selected ? "" : "none";
}

function renderInspectorSoA(soa: SoASnapshot, idx: number, id: number): void {
  const base = idx * CREATURE_STRIDE;
  const x = soa.creatures[base];
  const y = soa.creatures[base + 1];
  set("ins-id", `${id}`);
  set("ins-pos", `(${x.toFixed(1)}, ${y.toFixed(1)})`);
  // v2.0 Wave 2b: display color is a single packed RGBA8 u32 at lane 3
  // (LE: R = bits 0..8, G = 8..16, B = 16..24).
  const packed = soa.creaturesU32[base + 3];
  const r255 = packed & 0xff;
  const g255 = (packed >>> 8) & 0xff;
  const b255 = (packed >>> 16) & 0xff;
  const swatch = document.getElementById("ins-color-swatch");
  if (swatch) swatch.style.backgroundColor = `rgb(${r255}, ${g255}, ${b255})`;
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
  // v2.0 Wave 2b: genome-modulated biome movement penalty [0,1].
  set("ins-move-penalty", data.movement_penalty.toFixed(2));
  // v2.0 Wave 2b: render the 6 genome traits as labeled bars (raw [0,1]).
  const genome = data.genome;
  if (genome) {
    for (const key of GENOME_TRAIT_KEYS) {
      const v = genome[key];
      const fill = document.getElementById(`ins-trait-${key}`);
      if (fill) fill.style.width = `${Math.round(Math.max(0, Math.min(1, v)) * 100)}%`;
      set(`ins-trait-${key}-v`, v.toFixed(2));
    }
  }
  // v2.0 Wave 3b: species panel — shown only when the JSON carries species
  // data (i.e. species mode; single-pool inspect omits these fields). The
  // history breadcrumb is Wave 5 — render the (empty) list area, no logic.
  const speciesBlock = document.getElementById("ins-species-block");
  if (data.species_id !== undefined && data.species_color !== undefined) {
    if (speciesBlock) speciesBlock.style.display = "";
    // v2.0 Wave 5: lead with the human species name (e.g. "Species-I"); keep
    // the numeric id as a dim trailing tag for disambiguation.
    set("ins-species-name", data.species_name ?? `Species-#${data.species_id}`);
    set("ins-species-id", `#${data.species_id}`);
    // species_color is an RGBA8-packed u32 (LE: R bits 0..8, G 8..16, B 16..24)
    // — the same lane/decode the renderer uses for the body color.
    const packed = data.species_color >>> 0;
    const r255 = packed & 0xff;
    const g255 = (packed >>> 8) & 0xff;
    const b255 = (packed >>> 16) & 0xff;
    const swatch = document.getElementById("ins-species-swatch");
    if (swatch) swatch.style.backgroundColor = `rgb(${r255}, ${g255}, ${b255})`;
    // v2.0 Wave 5: species-history breadcrumb. Always empty in v2.0 (no splits
    // yet) — render an em-dash placeholder; the area is plumbed for v2.1 to
    // show the ancestral lineage as "Species-A → Species-D → …".
    const history = data.species_history ?? [];
    set("ins-species-history", history.length === 0 ? "—" : history.join(" → "));
  } else if (speciesBlock) {
    speciesBlock.style.display = "none";
  }
  const [wn, ws, we, ww] = data.wall_proximity;
  set("ins-walls", `${wn.toFixed(2)} / ${ws.toFixed(2)} / ${we.toFixed(2)} / ${ww.toFixed(2)}`);
  pushHistory(data.id, data.current_action);
  set("ins-history", actionHistory.slice(-30).map(a => a[0]).join(""));
  set("ins-nn-count", `${data.nn_weight_count}`);
}

// v2.0 Wave 2a: the evolving 6-trait body genome (raw [0,1]). Field names
// mirror src/wasm_api.rs creature_inspect_json's "genome" object.
export interface CreatureGenome {
  body_size: number;
  max_speed: number;
  metabolism: number;
  diet: number;
  water_affinity: number;
  heat_tolerance: number;
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
  // v2.0 Wave 2a: dropped `color_ema`; added the 6 genome traits + the
  // creature's current genome-modulated biome movement penalty [0,1].
  genome: CreatureGenome;
  movement_penalty: number;
  wall_proximity: [number, number, number, number];
  nn_weight_count: number;
  // v2.0 Wave 3b: species fields — present only in species mode (the Rust
  // creature_inspect_json omits them in single-pool mode). `species_color` is
  // the RGBA8-packed u32 the renderer paints into color_u32 (r | g<<8 | b<<16 |
  // a<<24). `species_name` is the human label (e.g. "Species-I"). `species_history`
  // is the breadcrumb — always empty in v2.0 (no splits yet), plumbed for v2.1.
  species_id?: number;
  species_color?: number;
  species_name?: string;
  species_history?: number[];
}

// Trait display order + labels (label only used in HTML; ids derive from key).
const GENOME_TRAIT_KEYS: ReadonlyArray<keyof CreatureGenome> = [
  "body_size",
  "max_speed",
  "metabolism",
  "diet",
  "water_affinity",
  "heat_tolerance",
];

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

    // Creature inspection is opt-in: ignore canvas clicks unless the
    // Inspector tab is the active, visible tab. Avoids accidental
    // highlights / sim-side inspect requests when the user just wanted
    // to look around.
    if (rail.activeTab !== "inspector") return;
    if (!getSettings().railOpen) return;

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
