---
status:        shipped
owner:         unassigned
last_updated:  2026-06-17
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/render-pipeline.md
  - architecture/app-shell.md
  - decisions/render.md
---

# Repeat camera rendering

## Mission

Make visual repeat rendering fill the viewport reliably at the existing zoom
limits. The user does not want additional zoom-out range; they want the renderer
to repeat the map enough that the current allowed view never stops at a visible
7x7 tile patch with empty background around it.

## Scope

In scope:

- Preserve the current zoom-out limits and initial camera feel unless a small
  correction is required to keep repeat tiling coherent.
- Keep the render/display setting that controls repeated visual tiling
  separately from the existing sim-side `wrapWorld` physics setting.
- Ensure repeat-tile offsets are derived from the camera frustum each paint and
  include enough tiles to cover the viewport at the current minimum zoom.
- Remove or replace the behavior that truncates rendering to a visible 7x7 patch
  when the screen still needs more tiles. A performance guard is acceptable only
  if it cannot produce empty canvas at any supported zoom/viewport.
- Draw map borders for every rendered repeat, not only the center map. Border
  styling controls are owned by `creature-survey-lod-controls.md`.
- Keep terrain, biome, grass, creature bodies, trails, flash rings, halos, and
  inspector highlights aligned under the same repeat-tile offset model.

Out of scope:

- Changing the simulation topology, creature positions, save format, or
  `wrapWorld` physics behavior.
- Adding a minimap, heatmap, or non-camera survey UI.
- Adding more zoom-out range. The current minimum zoom is acceptable; the issue
  is incomplete repeat coverage at that zoom.
- Reworking grass or biome snapshot window selection beyond what is necessary
  for correct repeated visual rendering.
- Adding border LOD or opacity controls; those belong to
  `creature-survey-lod-controls.md`.

## Context Routes

- `docs/architecture/render-pipeline.md` for current camera helpers,
  wrap-aware ghost rendering, world frame drawing, and render ownership.
- `docs/architecture/app-shell.md` for Settings panel ownership and the
  live-vs-staged setting split.
- `docs/decisions/render.md` for the current survey-zoom rationale and the
  existing `MIN_ZOOM` / point-LOD decision.
- Code routes: `app/web/src/render/scene.ts`,
  `app/web/src/render/camera.ts`, `app/web/src/render/gl.ts`,
  `app/web/src/settings.ts`, `app/web/src/widgets/devpanel.ts`, and
  `app/web/src/main.ts`.

## Open Assumptions

- Visual repeat rendering is a Display/render option and can be enabled
  independently of the sim-side `wrapWorld` construction setting.
- When visual repeat rendering is disabled, the existing bounded single-map
  behavior remains available.
- The existing zoom floor should remain approximately as-is.
- Repeat rendering should never expose the canvas background around a finite
  tile patch at a supported zoom and viewport size.
- If the implementation keeps a tile-count guard, the guard must be computed
  from the supported zoom floor and viewport dimensions rather than from the old
  fixed 7x7 assumption.

## Approach

Treat repeat tiles as a render-space concern derived from the camera frustum.
The camera may store absolute world coordinates; rendering converts those
coordinates into a base tile plus local world coordinates and a list of visible
tile offsets. Every draw path that currently special-cases the center world or a
small seam-offset set should consume that shared visible-tile list.

The implementation should preserve the current frustum-culling and instanced
draw structure. The important boundary is to make the repeat-offset derivation
coverage-driven: it should cover the visible frustum at the existing zoom floor
and should not silently crop to 7x7 when the frustum asks for more.

## Acceptance / Verification

- On a fresh runtime-sized world, the initial canvas keeps the existing
  one-map-fit feel.
- With visual repeat rendering enabled, panning can move the camera into
  neighboring repeats and keep rendering the correct repeated terrain,
  creatures, borders, trails, and highlights.
- At the current minimum zoom on desktop and mobile-sized viewports, repeat
  rendering fills the whole canvas with repeated map tiles; no finite 7x7 patch
  is visible against empty background.
- If a tile-count guard remains, it is high enough or adaptive enough to satisfy
  the supported viewport/zoom combinations and is documented in
  `architecture/render-pipeline.md`.
- The map border appears around every visible repeat tile.
- With visual repeat rendering disabled, the app still offers a bounded
  single-map camera mode.
- Existing Playwright smoke coverage still passes, and a targeted render smoke
  or manual screenshot verifies the one-map fit, multi-repeat pan, and current
  min-zoom fill cases.

## Handoff Notes

This work is a dependency for final border-LOD verification because the border
controls must apply to every rendered repeat tile. Keep the repeat-setting
storage additive so it does not disturb existing saved settings. Review should
focus on whether all draw paths share one tile-offset model, whether coverage is
complete at the existing zoom floor, and whether the worker camera lanes still
receive meaningful folded camera coordinates for snapshot window selection.

## Migration Notes

When this ships, migrate current-state facts into `architecture/render-pipeline.md`
and the setting/UI facts into `architecture/app-shell.md`. Update the
`decisions/render.md` repeat-camera entry so it no longer describes a fixed 7x7
visible cap as the intended behavior; the durable decision should be "fill the
supported viewport at the existing zoom floor while keeping visual repeats
display-only."

Shipped 2026-06-17:

- Removed the fixed 7x7 repeat crop from tile derivation and preserved the
  existing zoom floor.
- Shared the frustum-derived repeat tile list across terrain, creatures,
  trails, highlights, and repeat borders.
- Migrated repeat-camera behavior to `architecture/render-pipeline.md` and
  `decisions/render.md`; UI ownership lives in `architecture/app-shell.md`.
