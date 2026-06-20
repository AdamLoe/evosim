// Camera + projection helpers. Pure math, no draw calls. The actual
// rendering lives in render-gl.ts (WebGL2 instanced renderer).

export interface Camera {
  zoom: number; // 1 = 1 world-unit / pixel × (px-per-size-base)
  cx: number;   // world-space center x
  cy: number;   // world-space center y
}

export const PX_PER_SIZE = 2.5; // v6 §B: render radius = size × 2.5 px at base zoom

export const MAX_ZOOM = 16;
export const INITIAL_WORLD_FIT = 0.8;
export const DEFAULT_MAX_ZOOM_OUT_MAPS = 8;
export const MIN_MAX_ZOOM_OUT_MAPS = 2;
export const MAX_MAX_ZOOM_OUT_MAPS = 64;

export interface RepeatTile {
  x: number;
  y: number;
}

function safeWorldSize(worldSize: number): number {
  return Number.isFinite(worldSize) && worldSize > 0 ? worldSize : 1;
}

function safeViewportMin(viewW: number, viewH: number): number {
  const w = Number.isFinite(viewW) && viewW > 0 ? viewW : 1;
  const h = Number.isFinite(viewH) && viewH > 0 ? viewH : 1;
  return Math.min(w, h);
}

export function clampMaxZoomOutMaps(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_MAX_ZOOM_OUT_MAPS;
  return Math.max(MIN_MAX_ZOOM_OUT_MAPS, Math.min(MAX_MAX_ZOOM_OUT_MAPS, value));
}

export function initialCameraZoom(
  worldSize: number,
  viewW: number,
  viewH: number,
  maxZoomOutMaps = DEFAULT_MAX_ZOOM_OUT_MAPS,
): number {
  return Math.max(
    minCameraZoom(worldSize, viewW, viewH, true, maxZoomOutMaps),
    safeViewportMin(viewW, viewH) * INITIAL_WORLD_FIT / safeWorldSize(worldSize),
  );
}

export function minCameraZoom(
  worldSize: number,
  viewW: number,
  viewH: number,
  visualRepeats: boolean,
  maxZoomOutMaps = DEFAULT_MAX_ZOOM_OUT_MAPS,
): number {
  void visualRepeats;
  return safeViewportMin(viewW, viewH) / (safeWorldSize(worldSize) * clampMaxZoomOutMaps(maxZoomOutMaps));
}

export function makeCamera(
  worldSize: number,
  viewW = 0,
  viewH = 0,
  maxZoomOutMaps = DEFAULT_MAX_ZOOM_OUT_MAPS,
): Camera {
  return {
    zoom: initialCameraZoom(worldSize, viewW, viewH, maxZoomOutMaps),
    cx: worldSize / 2,
    cy: worldSize / 2,
  };
}

export function clampCamera(
  cam: Camera,
  worldSize: number,
  viewW: number,
  viewH: number,
  visualRepeats = false,
  maxZoomOutMaps = DEFAULT_MAX_ZOOM_OUT_MAPS,
): void {
  cam.zoom = Math.max(
    minCameraZoom(worldSize, viewW, viewH, visualRepeats, maxZoomOutMaps),
    Math.min(MAX_ZOOM, cam.zoom),
  );
  if (!visualRepeats) {
    cam.cx = Math.max(0, Math.min(worldSize, cam.cx));
    cam.cy = Math.max(0, Math.min(worldSize, cam.cy));
  }
  if (!Number.isFinite(cam.cx)) cam.cx = worldSize / 2;
  if (!Number.isFinite(cam.cy)) cam.cy = worldSize / 2;
}

function tileOffsetsForAxis(min: number, max: number, worldSize: number, center: number): number[] {
  const eps = worldSize * 1e-7;
  let first = Math.floor(min / worldSize);
  let last = Math.floor((max - eps) / worldSize);
  if (!Number.isFinite(first) || !Number.isFinite(last)) return [0];
  if (last < first) last = first;
  void center;
  const out: number[] = [];
  for (let tile = first; tile <= last; tile++) out.push(tile * worldSize);
  return out;
}

export function visibleRepeatTiles(
  cam: Camera,
  worldSize: number,
  viewW: number,
  viewH: number,
  visualRepeats: boolean,
): RepeatTile[] {
  if (!visualRepeats || worldSize <= 0 || cam.zoom <= 0) return [{ x: 0, y: 0 }];
  const halfW = (viewW / cam.zoom) * 0.5;
  const halfH = (viewH / cam.zoom) * 0.5;
  const xOffsets = tileOffsetsForAxis(cam.cx - halfW, cam.cx + halfW, worldSize, cam.cx);
  const yOffsets = tileOffsetsForAxis(cam.cy - halfH, cam.cy + halfH, worldSize, cam.cy);
  const out: RepeatTile[] = [];
  for (const x of xOffsets) {
    for (const y of yOffsets) out.push({ x, y });
  }
  return out;
}

export function wrapWorldCoord(value: number, worldSize: number): number {
  if (worldSize <= 0) return value;
  return ((value % worldSize) + worldSize) % worldSize;
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
