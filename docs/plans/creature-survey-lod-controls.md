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

# Creature and border survey LOD controls

## Mission

Expose formula-based Display controls for creature dots and repeated-map borders
at survey zoom so the user can tune legibility across one-map and multi-map
views without changing simulation state. Done means creature dots transition
smoothly across zoom bands, survey-dot controls cover the requested wider
ranges, and borders have matching min/scale/opacity controls.

## Scope

In scope:

- Add additive, live Display settings for the creature survey LOD formula.
- Replace the fixed bottom-band point constants with values derived from those
  settings.
- Widen the survey-dot control ranges to `0.1..8px` for minimum size and
  `1..32x` for scale. Update both UI ranges and renderer-side clamps so the
  controls are not silently limited after input.
- Fix the current creature-dot LOD discontinuity where zooming in appears to
  jump to a different rendering style. The implementation may adjust thresholds,
  blend/fade across the point/disc transition, or otherwise make the transition
  visually continuous, but it should preserve the near/mid/bottom intent.
- Add additive, live Display settings for map-border LOD that mirror the survey
  dot shape: border minimum pixel thickness, border scale, and border opacity.
- Apply border controls to every rendered repeat tile border, not only the base
  map outline.
- Make the Settings controls understandable as render-side tuning, not sim
  mutation.
- Preserve existing defaults as closely as possible so current visuals do not
  jump for existing users unless they move the new controls.

Out of scope:

- Adding separate per-repeat-count controls such as explicit 1x, 3x3, 5x5, and
  7x7 dot sizes.
- Changing creature body radius in simulation state or snapshot layout.
- Reworking color, species palette, action flash semantics, or inspector
  highlighting.
- Introducing a minimap or heatmap.
- Changing the repeat camera's zoom floor or tile-fill behavior beyond what is
  needed to verify border LOD on repeated tiles; that is owned by
  `repeat-camera-rendering.md`.

## Context Routes

- `docs/architecture/render-pipeline.md` for creature instance packing,
  radial-shaded survey dots, halo alpha bands, and current LOD thresholds.
- `docs/architecture/app-shell.md` for live Display settings and Settings
  panel wiring.
- `docs/decisions/render.md` for the existing survey-zoom decision.
- Code routes: `app/web/src/render/gl.ts`, `app/web/src/settings.ts`,
  `app/web/src/widgets/devpanel.ts`, and `app/web/tests/e2e/defaults-drift.spec.ts`
  or nearby settings-persistence coverage.

## Open Assumptions

- Formula controls are preferred over explicit 1x / 3x3 / 5x5 / 7x7 controls.
- The controls should be live-apply Display settings because they only affect
  main-thread rendering.
- Survey dot controls should expose `min = 0.1..8px` and `scale = 1..32x`.
- Defaults should reproduce the current approximate behavior: sub-1px bodies
  render as 1px radial-shaded survey dots.
- Border controls should be live Display settings. Defaults should preserve the
  current border appearance closely enough that existing sessions do not look
  unexpectedly different.
- The border control surface should be similar to survey dots, not a separate
  bespoke interaction model.

## Approach

Design the controls around the question, "How visible should creatures and map
boundaries remain as map coverage increases?" rather than around a specific
repeat grid. The implementation can still compute creature size from the
existing raw pixel radius (`radiusWorld * PX_PER_SIZE * zoom`) but should route
the bottom-band size and transition behavior through named settings instead of
fixed local constants.

The implementer should avoid expanding snapshot data or Rust slider lanes.
This is display-only state that belongs in the TS settings layer and is read
by the renderer each frame.

For borders, keep the world-frame draw path render-side. The likely shape is a
setting-derived pixel thickness formula with an opacity uniform, applied inside
the existing repeated-tile frame loop. Review should check both the Settings
range and the GL draw result, because range widening in the UI alone is not
enough when renderer clamps remain narrower.

## Acceptance / Verification

- Settings exposes live controls for creature survey LOD tuning in the Display
  / Render area.
- The creature controls support minimum dot size `0.1..8px` and scale `1..32x`.
- Moving the controls changes survey-zoom creature dot size or shrink behavior
  immediately without restarting the sim.
- Zooming in and out across the point/disc threshold no longer produces an
  obvious pop to a different rendering style.
- Settings exposes live controls for border minimum thickness, border scale, and
  border opacity.
- Border controls affect every visible repeated tile border immediately without
  restarting the sim.
- Defaults preserve current visual behavior closely enough that existing
  sessions do not look unexpectedly different.
- The controls remain useful when the camera is zoomed to one-map fit and
  multi-repeat views.
- Typecheck/build pass, and either Playwright settings persistence coverage or
  a targeted manual smoke confirms persistence and live renderer response.

## Handoff Notes

This work can be implemented after or alongside repeat-camera rendering, but
final verification is stronger once the camera reliably fills the viewport with
repeat tiles. Keep the setting names generic and formula-oriented so the UI does
not need to change if repeat tiling becomes viewport-budget-aware later.

## Migration Notes

When this ships, migrate the new creature and border LOD formulae, setting
defaults, and repeat-border behavior into `architecture/render-pipeline.md`, and
document the Display setting ownership in `architecture/app-shell.md`. If the
formula materially changes the current survey-zoom rationale, update the
existing `decisions/render.md` survey-zoom entry instead of adding a duplicate
decision.

Shipped 2026-06-17:

- Implemented live Display settings for survey-dot min/scale and repeat-border
  min/scale/opacity.
- Migrated current behavior to `architecture/render-pipeline.md` and Display
  setting ownership to `architecture/app-shell.md`.
- Updated `decisions/render.md` so survey/repeat rationale covers formula-based
  dots, coverage-derived repeats, and repeat borders.
