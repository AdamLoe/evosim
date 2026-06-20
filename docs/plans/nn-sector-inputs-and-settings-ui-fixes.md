---
status:        shipped
owner:         Codex
last_updated:  2026-06-17
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/app-shell.md
  - architecture/simulation-core.md
  - architecture/testing.md
  - decisions/app-shell.md
  - decisions/sim.md
---

# NN sector inputs and Settings UI fixes

## Mission

Fix the Settings/right-rail regressions and make the sector-search NN inputs
testable, configurable, and trustworthy. Done means opening or closing Settings
while paused never blacks the canvas, Escape toggles Settings, the General tab
no longer exposes Save / Resume / Fork, the app badge is a small sharp
bottom-left rectangle, and `CreatureSectors`, `GrassSectors`, and
`GrassBandsFar` each have real implementation tests plus user-facing
configuration controls.

## Scope

In scope:

- Fix the paused-canvas black frame when the right rail opens, closes, or
  changes canvas dimensions. Current code only repaints paused frames when the
  camera changes; a rail-driven canvas resize must also repaint from the latest
  snapshot so WebGL resize does not leave a cleared buffer visible.
- Make Escape open Settings when Settings is not currently the open active rail
  tab, and close the rail when Settings is already open. Ignore Escape while
  focus is inside text inputs or textareas.
- Preserve intentional inspector behavior where practical, but Settings
  open/close is the requested Escape behavior and should be the primary action.
- Remove the named `Save`, `Resume`, and `Fork` controls from the General tab.
  Keep unrelated run controls and status surfaces unless implementation
  discovers they are coupled to those removed buttons.
- Move `evosim v<version>` from the current top-left overlay into a small sharp
  rectangular badge at the bottom-left of the simulation viewport.
- Add configuration options for the three sector-search NN inputs:
  `CreatureSectors`, near `GrassSectors`, and far `GrassBandsFar`.
- Fix the real NN sensing issue found during implementation, not just inspector
  labels. At planning time, one concrete suspect is that far-grass sampling uses
  the fallback `GRASS_CELL_SIZE` constant instead of the runtime
  `grass.dims.grass_cell_size`; near-grass LUT/radius assumptions must also be
  audited before adding configurable search ranges.
- Add real tests that construct or control a world, place creatures and grass in
  known directions/ranges, run the same APIs used by ticks or inspector, and
  assert the actual NN input values for each sector-search group.
- Update generated TS mirrors and settings persistence/default-drift tests if
  the Rust `WorldConfig`, slider list, or generated constants change.

Out of scope:

- Reworking NN topology beyond the sector-search configuration needed here.
- Adding new direct biome or zone-type NN inputs.
- Reworking save artifact UX or storage behavior except for removing the named
  General-tab controls from the UI. If `WorldConfig` gains fields, schema
  compatibility, serde/default handling, and import/persistence tests are in
  scope because saved worlds embed that config.
- Large visual redesign of the rail, top bar, Settings categories, or renderer.
- Normalizing, reverting, or cleaning up the existing dirty worktree. The
  implementation starts from a heavily modified baseline across `app/`, `docs/`,
  and `docs/plans/`; treat those changes as user/in-progress work.

## Context Routes

- `docs/architecture/app-shell.md` for right-rail tab routing, Settings
  stage-then-apply, General tab ownership, theme/badge DOM, and boot-payload
  settings.
- `docs/architecture/simulation-core.md` for `WorldConfig`, construction-only
  settings, `NnInputLayout`, `build_nn_input`, inspector NN JSON, grass sensing,
  and saved-world state.
- `docs/architecture/testing.md` and `docs/agent-context/testing-how-to.md` for
  Rust unit tests, generated-binding drift, and Playwright conventions.
- `docs/decisions/app-shell.md` and `docs/decisions/sim.md` if implementation
  records durable rationale for Escape behavior or construction-vs-live NN
  sector configuration.
- Code routes: `app/web/src/main.ts`, `app/web/src/rail/index.ts`,
  `app/web/src/styles.css`, `app/web/src/settings.ts`,
  `app/web/src/widgets/devpanel.ts`, `app/web/tests/e2e/`, 
  `app/crates/evosim/src/world/nn.rs`,
  `app/crates/evosim/src/world/proximity.rs`,
  `app/crates/evosim/src/world/mod.rs`,
  `app/crates/evosim/src/constants.rs`,
  `app/crates/evosim/src/wasm_api/mod.rs`,
  `app/crates/evosim/src/bin/gen_bindings.rs`, and generated TS under
  `app/web/src/generated/`.

## Open Assumptions

- "Configuration options" means controls for the sector-search behavior itself:
  at minimum search radius/range for `CreatureSectors`, near `GrassSectors`, and
  far `GrassBandsFar`. Keep the sector count at 8 and preserve current defaults
  unless the fix proves a different surface is necessary.
- These sector-search controls should be next-world construction settings by
  default. If an implementer makes any of them live, they must prove all cached
  data (`sector_lut`, layout/topology compatibility, grass pyramid sampling, and
  tests) updates safely without rebuilding the world.
- `GrassBandsFar` enable/disable can continue to be represented by the existing
  `grass_multisight` control; this plan adds configuration for the far search,
  not a second duplicate enable toggle.
- Removing Save / Resume / Fork means removing those three General-tab buttons.
  Export / Import are not named by the request and may remain if they still fit
  the simplified General tab.
- Existing autosave internals may remain as long as the General tab no longer
  exposes the removed controls.

## Approach

Split the work into three implementation streams that can mostly run in
parallel, with one final integration pass.

**Stream 1: Settings rail and paused canvas**

Owns `app/web/src/main.ts`, `app/web/src/rail/index.ts`,
`app/web/src/styles.css`, and targeted e2e coverage. Add a repaint trigger for
canvas size changes while paused, likely by tracking the last painted
`viewW/viewH` or a resize generation in the paused `seq === lastPaintedSeq`
branch. The repaint should reuse the latest snapshot slot just like the existing
paused camera-move repaint.

Wire Escape through the rail state so it opens Settings when closed or on a
different tab and closes the rail when already on open Settings. Keep the same
input/textarea guard used by other hotkeys.

Update the General tab mount to remove Save / Resume / Fork controls and update
tests/selectors that assumed those buttons exist. Reposition and restyle the app
badge as a small sharp bottom-left rectangle without changing `APP_VERSION`
source.

**Stream 2: NN sector search configuration and real fixes**

Owns Rust world/config code, generated TS, Settings, and persistence defaults.
Audit the sector-search path before adding controls:

- `CreatureSectors`: `build_nn_input` ->
  `compute_creature_proximity_sectors` / species variant, spatial grid rebuild,
  wrap minimum-image handling, and any proposed radius source.
- `GrassSectors`: `compute_grass_density_sectors`, `sector_lut`,
  `LUT_RADIUS`, `GRASS_PROXIMITY_RANGE`, runtime grass cell size, row-bitset
  gating, and wrap/clamp behavior.
- `GrassBandsFar`: `compute_grass_far_band_sectors`, pyramid refresh
  requirements, runtime grass cell size vs fallback constants, far radius, mip
  level selection, and wrap/clamp behavior.

Thread the chosen configuration through the existing construction path:
`WorldConfig` -> `DevSliders` / world construction -> `World` fields or
proximity helpers -> generated TS -> `Settings` -> `currentWorldConfig()` ->
Settings UI. Because `WorldConfig` is embedded in saved worlds, add serde
defaults or schema migration for older artifacts, update generated TS defaults,
and cover import/persistence/default-drift behavior in tests.

Defaults should reproduce current behavior: `CreatureSectors` range `20u`, near
`GrassSectors` range `20u`, and `GrassBandsFar` range `160u`. Define far-grass
sampling in terms of the runtime `grass_cell_size` from `WorldConfig`, not the
fallback `GRASS_CELL_SIZE` constant. At the current generated default
`grass.cell_size = 20.0`, far-band tests must prove the default formula still
lights the expected sectors; add at least one non-default cell-size test so the
bug cannot regress behind fallback constants.

If configurable near-grass range can exceed today's default, replace fixed
`LUT_RADIUS` assumptions with a range/cell-size-aware LUT shape or another
bounded implementation. Do not leave debug-only assertions as the only guard.

**Stream 3: NN verification**

Owns Rust unit tests under or near `world/nn.rs`, `world/proximity.rs`, and
existing grass sight tests. Prefer unit-level access to `build_nn_input` and
`NnInputLayout` so the tests confirm the exact inputs consumed by the NN
forward pass. Inspector JSON tests may be added as a secondary check, but they
must not be the only proof.

Add targeted tests that:

- Place one neighbor creature in each compass direction and assert
  `CreatureSectors` lights the expected slot, with a clear value falloff when
  distance changes and no value when outside configured range.
- Cover species mode if implementation touches the 16-slot same/other layout.
- Place grass in each compass direction and assert near `GrassSectors` lights
  the expected slot while the current cell remains represented only by
  `CurrGrass`.
- Place far grass patches, refresh the pyramid as needed, and assert
  `GrassBandsFar` lights the expected slot using runtime grass cell size and the
  configured far range. Include default `grass_cell_size = 20.0` and at least
  one non-default grass cell size.
- Confirm wrap and walled behavior at seams/edges for all three groups when
  practical.
- Confirm changing the new sector-search config changes the resulting NN input
  values or active sensing distance, without changing unrelated defaults.

**Integration and docs pass**

After the streams merge, regenerate generated TS if Rust bindings changed, run
the relevant gates, then migrate durable facts into the owning docs before the
plan is marked shipped. Review should focus on the dirty baseline: do not
discard unrelated existing source/doc changes, and stage/commit only the files
intentionally touched by the implementation.

## Acceptance / Verification

- Opening the Settings tab while paused, closing it while paused, and toggling
  it with Escape never leaves the canvas black or blank. A Playwright smoke
  should pause the world, wait for a nonblank canvas, open/close Settings, and
  assert canvas pixels remain nonblank after the rail resize.
- Escape opens Settings from a closed rail or another tab, closes the rail when
  Settings is already open, and does nothing while typing in inputs/textareas.
- The General tab no longer exposes `Save`, `Resume`, or `Fork` controls.
- The app badge reads `evosim v<APP_VERSION>` and is positioned as a small
  sharp rectangle in the bottom-left of `#canvas-wrap`.
- Settings exposes configuration controls for creature-sector search,
  near-grass-sector search, and far-grass-band search with restart semantics if
  implemented as construction settings.
- Rust tests confirm `CreatureSectors`, `GrassSectors`, and `GrassBandsFar`
  each affect the actual NN input buffer for controlled world fixtures.
- Tests cover at least one configuration change per sector-search group and
  prove defaults preserve current sensing behavior.
- If Rust config or generated mirrors change,
  `cd app && cargo run --bin gen-bindings` has been applied and the
  generated-binding drift test passes.
- Targeted gates pass at minimum:
  `cargo test --lib`, `cargo test --lib --features threads`,
  `cargo fmt --all --check`, `cd app/web && pnpm typecheck`, and targeted
  Playwright specs for Settings/rail behavior. Run broader manifest gates when
  practical before shipping.

## Discipline Rules

- Planning created this file only; implementation must treat all pre-existing
  dirty worktree changes as user/in-progress work.
- Before editing an owned file, capture `git status --short` and
  `git diff -- <file>` for that file. Avoid broad formatting or cleanup, and
  compare the final diff against the captured baseline before staging so
  unrelated dirty work is preserved.
- Do not fix the NN issue only in inspector formatting. The NN forward input
  path and tests are the source of truth.
- Keep UI labels concise and avoid explanatory in-app text. Use Settings
  control labels/tooltips consistent with existing `devpanel.ts` patterns.
- Preserve current defaults unless a tested bug fix requires changing them.
- Escape prioritizes Settings rail open/close and should preserve the current
  inspector selection unless existing rail APIs make that impossible.

## Migration Notes

Before setting `status: shipped`, migrate:

- Current right-rail Escape behavior, General tab contents, paused resize repaint
  behavior, and badge placement to `architecture/app-shell.md`.
- New NN sector-search configuration fields, defaults, construction/live
  semantics, and fixed sensing behavior to `architecture/simulation-core.md`.
- New or changed Rust and Playwright coverage to `architecture/testing.md` and
  `agent-context/testing-how-to.md` if commands or coverage categories change.
- Any durable rationale for construction-only sector controls or hotkey behavior
  to `decisions/sim.md` or `decisions/app-shell.md`.

Shipped migration:

- `architecture/app-shell.md` records Escape Settings behavior, General tab
  contents, paused resize repaint, and badge placement.
- `architecture/simulation-core.md` records `WorldConfig.nn_sensing` defaults,
  construction semantics, and runtime-cell far grass sensing.
- `architecture/testing.md` and `agent-context/testing-how-to.md` record the new
  Rust NN input coverage and `settings-rail.spec.ts` smoke.
- `decisions/app-shell.md` and `decisions/sim.md` record the Escape and
  construction-only sensing-range rationale.

## See Also

- `docs/plans/index.md`
- `~/agent-docs/v1/plan-lifecycle.md`
- `docs/architecture/app-shell.md`
- `docs/architecture/simulation-core.md`
- `docs/architecture/testing.md`
