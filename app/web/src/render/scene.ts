// Camera + projection helpers. Pure math, no draw calls. The actual
// rendering lives in render-gl.ts (WebGL2 instanced renderer).

export interface Camera {
  zoom: number; // 1 = 1 world-unit / pixel × (px-per-size-base)
  cx: number;   // world-space center x
  cy: number;   // world-space center y
}

export const PX_PER_SIZE = 2.5; // v6 §B: render radius = size × 2.5 px at base zoom

// v2.0 Wave 1c: minimum zoom (px per world-unit). The world grew 1200 → 9600,
// so the old 0.25 floor could survey only a ~6000px-wide window. 0.04 lets a
// ~384px viewport frame the entire 9600 default world from a single view
// (9600 × 0.04 = 384), and a normal ~1500px viewport surveys ~37 500u.
export const MIN_ZOOM = 0.04;
export const MAX_ZOOM = 16;

export function makeCamera(worldSize: number): Camera {
  // v2.0 Wave 1c: default zoom = 1.0 px/world-unit so a typical ~1500px
  // viewport opens showing ~1500 world-units of the (much larger) world,
  // centered. The camera tracks the runtime world size via clampCamera.
  return { zoom: 1.0, cx: worldSize / 2, cy: worldSize / 2 };
}

export function clampCamera(cam: Camera, worldSize: number, viewW: number, viewH: number): void {
  cam.zoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, cam.zoom));
  // Pan limit: world edge cannot leave the viewport center (v6 §N).
  cam.cx = Math.max(0, Math.min(worldSize, cam.cx));
  cam.cy = Math.max(0, Math.min(worldSize, cam.cy));
  // viewW/viewH are kept for future tighter clamping if we restrict scrolling
  // past the world edges by more than half the viewport.
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
