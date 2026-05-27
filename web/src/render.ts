// Camera + projection helpers. Pure math, no draw calls. The actual
// rendering lives in render-gl.ts (WebGL2 instanced renderer).

export interface Camera {
  zoom: number; // 1 = 1 world-unit / pixel × (px-per-size-base)
  cx: number;   // world-space center x
  cy: number;   // world-space center y
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
