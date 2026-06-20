import { test, expect } from "@playwright/test";
import {
  DEFAULT_MAX_ZOOM_OUT_MAPS,
  INITIAL_WORLD_FIT,
  clampCamera,
  initialCameraZoom,
  makeCamera,
  minCameraZoom,
  visibleRepeatTiles,
  wrapWorldCoord,
  type Camera,
} from "../../src/render/scene";

test("initial camera fits one full map with padding", () => {
  const zoom = initialCameraZoom(1000, 1200, 800);
  expect(zoom).toBeCloseTo((800 * INITIAL_WORLD_FIT) / 1000, 6);

  const cam = makeCamera(1000, 1200, 800);
  expect(cam).toEqual({ cx: 500, cy: 500, zoom });
});

test("max zoom-out maps controls the camera zoom floor", () => {
  expect(DEFAULT_MAX_ZOOM_OUT_MAPS).toBe(8);
  expect(minCameraZoom(1000, 1200, 800, true, 4)).toBeCloseTo(0.2, 6);
  expect(minCameraZoom(1000, 1200, 800, true, 16)).toBeCloseTo(0.05, 6);

  const cam: Camera = { cx: 500, cy: 500, zoom: 0.01 };
  clampCamera(cam, 1000, 1200, 800, true, 16);
  expect(cam.zoom).toBeCloseTo(0.05, 6);
});

test("bounded camera clamps center while repeated camera can pan into adjacent tiles", () => {
  const bounded: Camera = { cx: 1400, cy: -300, zoom: 0.01 };
  clampCamera(bounded, 1000, 1200, 800, false);
  expect(bounded.cx).toBe(1000);
  expect(bounded.cy).toBe(0);

  const repeated: Camera = { cx: 1400, cy: -300, zoom: 0.01 };
  clampCamera(repeated, 1000, 1200, 800, true);
  expect(repeated.cx).toBe(1400);
  expect(repeated.cy).toBe(-300);
});

test("visible repeat tiles are frustum-derived without a fixed seven-by-seven cap", () => {
  const oneMapFit = { cx: 500, cy: 500, zoom: 0.8 } satisfies Camera;
  expect(visibleRepeatTiles(oneMapFit, 1000, 1000, 1000, true)).toHaveLength(9);
  expect(visibleRepeatTiles(oneMapFit, 1000, 1000, 1000, false)).toEqual([{ x: 0, y: 0 }]);

  const veryWide = { cx: 500, cy: 500, zoom: 0.04 } satisfies Camera;
  const tiles = visibleRepeatTiles(veryWide, 1000, 1000, 1000, true);
  expect(tiles).toHaveLength(625);
  expect(Math.min(...tiles.map((t) => t.x))).toBe(-12000);
  expect(Math.max(...tiles.map((t) => t.x))).toBe(12000);
});

test("worker camera folding maps repeated view centers back into the base world", () => {
  expect(wrapWorldCoord(1250, 1000)).toBe(250);
  expect(wrapWorldCoord(-250, 1000)).toBe(750);
});
