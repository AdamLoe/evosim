// Aquarium renderer. Pure draw helpers — owns no sim state. Called once per
// requestAnimationFrame from main.ts with a fresh creature/carrion snapshot.

import type { WorldHandle } from "../wasm/evosim";
import type { GenomeJson } from "./eulogy"; // used by renderCreaturePortrait below

/// Ring colors per v6 §B: eye=white, move=yellow, scav=brown, mouth=red, armor=silver.
/// Shared between drawCreatures (SoA path) and renderCreaturePortrait (F.28).
export const RING_COLORS = {
  eye: "rgba(255,255,255,0.85)",
  move: "rgba(240,220, 80,0.85)",
  scav: "rgba(140, 90, 40,0.85)",
  mouth: "rgba(255, 80, 80,0.85)",
  armor: "rgba(200,200,210,0.85)",
} as const;

export interface Camera {
  zoom: number; // 1 = 1 world-unit / pixel × (px-per-size-base)
  cx: number; // world-space center x
  cy: number; // world-space center y
}

export const PX_PER_SIZE = 2.5; // v6 §B: render radius = size × 2.5 px at base zoom

export function makeCamera(worldSize: number): Camera {
  // v6 §C: initial zoom 4× (creature radius ≈ 10 px on screen).
  return { zoom: 4, cx: worldSize / 2, cy: worldSize / 2 };
}

export function clampCamera(cam: Camera, worldSize: number, viewW: number, viewH: number): void {
  cam.zoom = Math.max(0.5, Math.min(16, cam.zoom));
  // Pan limit: world edge cannot leave the viewport center (v6 §N).
  cam.cx = Math.max(0, Math.min(worldSize, cam.cx));
  cam.cy = Math.max(0, Math.min(worldSize, cam.cy));
  // viewW/viewH are kept for future tighter clamping if we restrict scrolling
  // past the world edges by more than half the viewport. v6 §N's wording
  // ("edge cannot leave viewport center") matches what we have.
  void viewW;
  void viewH;
}

export function worldToScreen(cam: Camera, viewW: number, viewH: number, x: number, y: number): [number, number] {
  const sx = (x - cam.cx) * cam.zoom + viewW / 2;
  const sy = (y - cam.cy) * cam.zoom + viewH / 2;
  return [sx, sy];
}

export function screenToWorld(cam: Camera, viewW: number, viewH: number, sx: number, sy: number): [number, number] {
  const x = (sx - viewW / 2) / cam.zoom + cam.cx;
  const y = (sy - viewH / 2) / cam.zoom + cam.cy;
  return [x, y];
}

export function drawCarrion(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  viewW: number,
  viewH: number,
  carrion: Float32Array,
): void {
  ctx.fillStyle = "rgb(102, 102, 102)"; // v6 §B fixed gray
  for (let k = 0; k < carrion.length; k += 3) {
    const x = carrion[k];
    const y = carrion[k + 1];
    const pool = carrion[k + 2];
    const [sx, sy] = worldToScreen(cam, viewW, viewH, x, y);
    const r = 2 + 4 * pool;
    ctx.beginPath();
    ctx.arc(sx, sy, r * cam.zoom * 0.5 + 1, 0, Math.PI * 2);
    ctx.fill();
  }
}

export function drawCreatures(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  viewW: number,
  viewH: number,
  data: Float32Array,
  ids: Float64Array,
  stride: number,
  highlightMap: Map<number, number>,
  nowMs: number,
): void {
  // Build a transient set of SoA indices to highlight (E.22).
  const highlightIdx = new Set<number>();
  if (highlightMap.size > 0) {
    for (let k = 0; k < ids.length; k++) {
      const cid = ids[k];
      const exp = highlightMap.get(cid);
      if (exp !== undefined && nowMs < exp) highlightIdx.add(k);
    }
  }

  for (let i = 0; i < data.length; i += stride) {
    const x = data[i];
    const y = data[i + 1];
    const radiusWorld = data[i + 2];
    const r = data[i + 3];
    const g = data[i + 4];
    const b = data[i + 5];
    // Feature ring flags (v6 §B, Milestone C.11): eye→move→scav→mouth→armor
    // S21: offsets shifted from 8-12 → 6-10 (dropped energy_frac + age_frac).
    const flagEye   = data[i + 6] > 0.5;
    const flagMove  = data[i + 7] > 0.5;
    const flagScav  = data[i + 8] > 0.5;
    const flagMouth = data[i + 9] > 0.5;
    const flagArmor = data[i + 10] > 0.5;

    const idx = i / stride;
    const [sx, sy] = worldToScreen(cam, viewW, viewH, x, y);
    const radiusPx = Math.max(1, radiusWorld * PX_PER_SIZE * cam.zoom);
    const cr = Math.round(255 * r);
    const cg = Math.round(255 * g);
    const cb = Math.round(255 * b);
    ctx.fillStyle = `rgb(${cr},${cg},${cb})`;
    ctx.beginPath();
    ctx.arc(sx, sy, radiusPx, 0, Math.PI * 2);
    ctx.fill();

    // Active-trait rings (v6 §B) — drawn inward from the body edge.
    // Ring order (innermost→outermost): eye(white), move(yellow),
    // scav(brown), mouth(red), armor(silver). v6 §B fixes eye=innermost;
    // rest ordered by DECISIONS (see DECISIONS.md). Colors from RING_COLORS.
    let ringR = radiusPx - 1;
    const RING_STEP = 1.5;
    const drawRing = (color: string): void => {
      if (ringR < 1) return;
      ctx.strokeStyle = color;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(sx, sy, ringR, 0, Math.PI * 2);
      ctx.stroke();
      ringR -= RING_STEP;
    };
    if (flagEye)   drawRing(RING_COLORS.eye);   // white  — innermost
    if (flagMove)  drawRing(RING_COLORS.move);  // yellow
    if (flagScav)  drawRing(RING_COLORS.scav);  // brown
    if (flagMouth) drawRing(RING_COLORS.mouth); // red
    if (flagArmor) drawRing(RING_COLORS.armor); // silver — outermost

    // E.22: highlight ring — golden rgb(255,200,50), 2px, fades over 1.5s (v6 §B).
    if (highlightIdx.has(idx)) {
      const cid = ids[idx];
      const exp = highlightMap.get(cid)!;
      const remainingMs = exp - nowMs;
      const alpha =
        exp >= Number.MAX_SAFE_INTEGER - 1
          ? 1 // permanent (inspector selection)
          : Math.min(1, remainingMs / 1500);
      ctx.strokeStyle = `rgba(255, 200, 50, ${alpha.toFixed(3)})`;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(sx, sy, radiusPx + 3, 0, Math.PI * 2);
      ctx.stroke();
    }
  }
}

export function drawAquariumFrame(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  viewW: number,
  viewH: number,
  worldSize: number,
): void {
  // Subtle border around the world's extent.
  const [tlx, tly] = worldToScreen(cam, viewW, viewH, 0, 0);
  const [brx, bry] = worldToScreen(cam, viewW, viewH, worldSize, worldSize);
  ctx.strokeStyle = "rgba(180, 200, 230, 0.25)";
  ctx.lineWidth = 1;
  ctx.strokeRect(tlx, tly, brx - tlx, bry - tly);
}

export function renderWorld(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  viewW: number,
  viewH: number,
  world: WorldHandle,
  stride: number,
  ids: Float64Array,
  highlightMap: Map<number, number>,
  nowMs: number,
): void {
  // S21 mandatory guard: stride mismatch silently corrupts the renderer.
  // Rust wasm_api.rs::creature_stride() and web/src/render.ts must agree.
  if (stride !== 11) {
    throw new Error(
      `creature_stride mismatch: expected 11, got ${stride} ` +
      `(Rust wasm_api.rs::creature_stride and web/src/render.ts must agree)`,
    );
  }
  ctx.fillStyle = "#04070b";
  ctx.fillRect(0, 0, viewW, viewH);
  // TODO P1g: grass density tint will be inserted here.
  drawAquariumFrame(ctx, cam, viewW, viewH, world.world_size);
  drawCarrion(ctx, cam, viewW, viewH, world.carrion_buffer());
  drawCreatures(ctx, cam, viewW, viewH, world.creatures_buffer(), ids, stride, highlightMap, nowMs);
}

/// F.28 — single-creature portrait renderer. Renders a creature from a GenomeJson
/// into a fresh offscreen canvas (canvasSize × canvasSize px), centered.
/// Used by the eulogy card image grid. Ring colors match RING_COLORS.
export function renderCreaturePortrait(genome: GenomeJson, canvasSize = 96): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = canvasSize;
  c.height = canvasSize;
  const ctx = c.getContext("2d")!;

  // Dark background to match aquarium.
  ctx.fillStyle = "rgba(8, 12, 20, 1)";
  ctx.fillRect(0, 0, canvasSize, canvasSize);

  const cx = canvasSize / 2;
  const cy = canvasSize / 2;

  // Pick a fixed portrait zoom: largest size we expect (~10) should fit with
  // 8px margin. radius_px = size × portraitScale. Want 10 × scale = canvasSize/2 - 8.
  const portraitScale = (canvasSize / 2 - 8) / 10;
  const radiusPx = Math.max(6, genome.size * portraitScale);

  // Body fill (pigment).
  const r = Math.round(genome.pigment_r * 255);
  const g = Math.round(genome.pigment_g * 255);
  const b = Math.round(genome.pigment_b * 255);
  ctx.fillStyle = `rgb(${r},${g},${b})`;
  ctx.beginPath();
  ctx.arc(cx, cy, radiusPx, 0, Math.PI * 2);
  ctx.fill();

  // Active-trait rings (same order as drawCreatures; RING_COLORS shared).
  let ringR = radiusPx - 1;
  const drawPortraitRing = (color: string): void => {
    if (ringR < 1) return;
    ctx.strokeStyle = color;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.arc(cx, cy, ringR, 0, Math.PI * 2);
    ctx.stroke();
    ringR -= 1.5;
  };
  if (genome.eye_count > 0)            drawPortraitRing(RING_COLORS.eye);
  if (genome.move_speed > 0)           drawPortraitRing(RING_COLORS.move);
  if (genome.scavenge_efficiency > 0)  drawPortraitRing(RING_COLORS.scav);
  if (genome.eat_efficiency > 0)       drawPortraitRing(RING_COLORS.mouth);
  if (genome.armor > 0)                drawPortraitRing(RING_COLORS.armor);

  return c;
}
