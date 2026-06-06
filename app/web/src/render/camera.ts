// Pointer-driven pan + wheel zoom + pinch zoom (touch).

import { clampCamera, type Camera, screenToWorld } from "./scene";

export function attachCameraControls(
  canvas: HTMLCanvasElement,
  cam: Camera,
  getView: () => { w: number; h: number },
  getWorldSize: () => number,
): void {
  let dragging = false;
  let lastX = 0;
  let lastY = 0;

  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    dragging = true;
    lastX = e.clientX;
    lastY = e.clientY;
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener("pointerup", (e) => {
    dragging = false;
    canvas.releasePointerCapture(e.pointerId);
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    lastX = e.clientX;
    lastY = e.clientY;
    cam.cx -= dx / cam.zoom;
    cam.cy -= dy / cam.zoom;
    const { w, h } = getView();
    clampCamera(cam, getWorldSize(), w, h);
  });
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const { w, h } = getView();
      const [wx, wy] = screenToWorld(cam, w, h, e.clientX, e.clientY);
      // 1.2^(1/3) ≈ 1.063 — three scroll ticks equal one old tick.
      const factor = e.deltaY < 0 ? 1.063 : 1 / 1.063;
      cam.zoom *= factor;
      clampCamera(cam, getWorldSize(), w, h);
      // Keep the world point under the cursor stable across the zoom.
      const [wx2, wy2] = screenToWorld(cam, w, h, e.clientX, e.clientY);
      cam.cx += wx - wx2;
      cam.cy += wy - wy2;
      clampCamera(cam, getWorldSize(), w, h);
    },
    { passive: false },
  );

  // Two-finger pinch (touch).
  const touches = new Map<number, { x: number; y: number }>();
  let pinchInitDist = 0;
  let pinchInitZoom = 0;

  canvas.addEventListener("touchstart", (e) => {
    for (const t of Array.from(e.changedTouches)) {
      touches.set(t.identifier, { x: t.clientX, y: t.clientY });
    }
    if (touches.size === 2) {
      const [a, b] = Array.from(touches.values());
      pinchInitDist = Math.hypot(a.x - b.x, a.y - b.y);
      pinchInitZoom = cam.zoom;
    }
  });
  canvas.addEventListener("touchmove", (e) => {
    for (const t of Array.from(e.changedTouches)) {
      touches.set(t.identifier, { x: t.clientX, y: t.clientY });
    }
    if (touches.size === 2 && pinchInitDist > 0) {
      const [a, b] = Array.from(touches.values());
      const d = Math.hypot(a.x - b.x, a.y - b.y);
      cam.zoom = pinchInitZoom * (d / pinchInitDist);
      const { w, h } = getView();
      clampCamera(cam, getWorldSize(), w, h);
    }
  });
  canvas.addEventListener("touchend", (e) => {
    for (const t of Array.from(e.changedTouches)) {
      touches.delete(t.identifier);
    }
    if (touches.size < 2) {
      pinchInitDist = 0;
    }
  });
}
