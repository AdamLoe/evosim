// Aquarium renderer. Pure draw helpers — owns no sim state. Called once per
// requestAnimationFrame from main.ts with a fresh creature/sun/carrion snapshot.

import type { WorldHandle } from "../wasm/evosim";

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

export function drawSunMap(
  ctx: CanvasRenderingContext2D,
  cam: Camera,
  viewW: number,
  viewH: number,
  worldSize: number,
  sunDim: number,
  sunFracs: Float32Array,
  sunCaps: Float32Array,
): void {
  const cell = worldSize / sunDim;
  for (let j = 0; j < sunDim; j++) {
    for (let i = 0; i < sunDim; i++) {
      const k = j * sunDim + i;
      const cap = sunCaps[k];
      const frac = sunFracs[k];
      // Cell color: warm yellow scaled by capacity (background potential),
      // brightness modulated by current fraction.
      // capacity ranges roughly 1..7 → normalize to [0,1] for hue intensity.
      const capN = Math.min(1, Math.max(0, (cap - 1) / 6));
      const brightness = 0.04 + 0.18 * capN * frac; // dim by default; cells light up when full
      const [sx, sy] = worldToScreen(cam, viewW, viewH, i * cell, j * cell);
      const w = cell * cam.zoom + 1;
      const r = Math.round(255 * brightness);
      const g = Math.round(220 * brightness);
      const b = Math.round(120 * brightness * 0.6);
      ctx.fillStyle = `rgb(${r},${g},${b})`;
      ctx.fillRect(sx, sy, w, w);
    }
  }
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
  stride: number,
): void {
  for (let i = 0; i < data.length; i += stride) {
    const x = data[i];
    const y = data[i + 1];
    const radiusWorld = data[i + 2];
    const r = data[i + 3];
    const g = data[i + 4];
    const b = data[i + 5];
    // const energyFrac = data[i + 6]; // reserved for future visual cue
    // const ageFrac = data[i + 7];
    // Feature ring flags (v6 §B, Milestone C.11): eye→move→scav→mouth→armor
    const flagEye   = data[i + 8] > 0.5;
    const flagMove  = data[i + 9] > 0.5;
    const flagScav  = data[i + 10] > 0.5;
    const flagMouth = data[i + 11] > 0.5;
    const flagArmor = data[i + 12] > 0.5;

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
    // rest ordered by DECISIONS (see DECISIONS.md).
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
    if (flagEye)   drawRing("rgba(255,255,255,0.85)"); // white  — innermost
    if (flagMove)  drawRing("rgba(240,220, 80,0.85)"); // yellow
    if (flagScav)  drawRing("rgba(140, 90, 40,0.85)"); // brown
    if (flagMouth) drawRing("rgba(255, 80, 80,0.85)"); // red
    if (flagArmor) drawRing("rgba(200,200,210,0.85)"); // silver — outermost
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
): void {
  ctx.fillStyle = "#04070b";
  ctx.fillRect(0, 0, viewW, viewH);
  drawSunMap(ctx, cam, viewW, viewH, world.world_size, world.sun_dim, world.sun_buffer(), world.sun_capacity_buffer());
  drawAquariumFrame(ctx, cam, viewW, viewH, world.world_size);
  drawCarrion(ctx, cam, viewW, viewH, world.carrion_buffer());
  drawCreatures(ctx, cam, viewW, viewH, world.creatures_buffer(), stride);
}
