# App shell

The main-thread UI structure: how the DOM is laid out, which TS module
installs into which container, how the right-rail tabs route, and how the
Settings tab's stage-then-apply machinery works.

## What it is

A two-column shell that wraps the canvas and every non-canvas UI element.

```
┌──────────────────────────────────┬──────────────────────┐
│ top-bar (status + pacing + ⚙)    │ ┌────┬────┬────────┐ │
│                                  │ │Setn│ NN │ Inspct │ │
├──────────────────────────────────┤ ├────┴────┴────────┤ │
│                                  │ │                  │ │
│ canvas                           │ │ active tab body  │ │
│                                  │ │                  │ │
├──────────────────────────────────┤ │                  │ │
│ perf-box (pop graph + profiler,  │ │                  │ │
│   optional, ✕)                   │ │                  │ │
└──────────────────────────────────┴──┴──────────────────┘
         left column (flex)              right rail
                                         (fixed 420 px)
```

The DOM tree lives in `app/web/index.html`. The CSS layout uses a top-level
grid (`#app-shell { grid-template-columns: 1fr var(--rail-w); }`); the
left column uses a row grid for top-bar / canvas / profiler.

## What it owns

- The top-level DOM structure: `#app-shell` → `#left-col` (`#top-bar`,
  `#canvas-wrap` with `#aquarium`, `#perf-box`) and `#right-rail`
  (`#rail-tabs` and the three `.rail-panel` sections).
- The CSS palette tokens (`--bg-app`, `--bg-panel`, `--fg`, `--accent`,
  `--accent-dirty`, `--danger`, …) — single source of truth for color in
  `app/web/src/styles.css`.
- Right-rail tab routing: tab switching, default tab on boot, the
  switch-on-creature-click rule.
- Which TS module installs into which DOM element. Installers run from
  `main.ts` in the order: dev panel (= Settings tab) → profiler panel
  → NN tab → Settings button. Installation order matters because
  the dev panel's "Show profiler" toggle calls into the profiler panel
  installer's exported visibility setter.
- The stage-then-apply pattern in the Settings tab: per-row dirty
  tracking, Apply / Cancel / Reset semantics, the live-vs-staged
  carve-out, the "construction-only" toast trigger.
- Cross-widget source-of-truth coupling: `showProfiler` is the single
  state for both the Settings checkbox and the perf-panel ✕ button;
  flipping either writes the same setting and re-calls
  `setProfilerVisible()`. Visibility-only — the Rust profiler is
  always-on (the worker enables it at boot) and the panel keeps polling
  the bundled report whenever it's visible.
- The rail open/closed toggle: `Settings.railOpen` (default `false`,
  so fresh users start with the rail collapsed) drives
  `#app-shell.rail-collapsed`, which collapses the grid track to `0`
  and hides `#right-rail`. The ⚙ button and the `~` hotkey both route
  through `setRailOpen` in `main.ts`.
- Theming: `app/web/src/themes.ts` owns the palette map. `applyTheme(id)`
  writes inline custom properties onto `<html>`; shipped themes each define
  every CSS var listed in `styles.css`'s `:root` block.

## What it does NOT own

- **Canvas painting, GL programs, camera math** — owned by
  [`render-pipeline.md`](render-pipeline.md).
- **Snapshot read on each RAF** — owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md). The
  shell hands `pollRail(rail, snapshot, simBridge, creatures, pop)`
  the live snapshot view; the rail consumers (stats sampler, inspector
  refresher) own how they interpret it.
- **Worker lifecycle, pacing, restart** — owned by
  [`worker-runtime.md`](worker-runtime.md).
- **Sim sliders + their Rust defaults** — owned by
  [`simulation-core.md`](simulation-core.md). The shell maps each
  Rust slider name to a Settings-tab widget and to a `Settings` key;
  the values come from Rust.

## DOM map

| Element | Purpose | Installer / consumer |
|---|---|---|
| `#app-shell` | Two-column grid. Carries `.rail-collapsed` when the rail is hidden. | CSS-only; class flipped by `main.ts → applyRailOpen`. |
| `#left-col` | Top-bar + canvas + profiler. | CSS-only. |
| `#top-bar` | Always-visible pacing + action buttons. | `main.ts → installTopBarButtons` populates: play/pause, Restart (always rerolls seed), auto-restart, perf toggle, settings/NN/inspector rail openers. |
| `#canvas-wrap > #aquarium` | The WebGL2 sim view. | `render/gl.ts`. |
| `#perf-box` | Profiler bottom panel (collapsed by default): status line, FPS/TPS chart, population chart (`#chart-pop`), TPS/max-pop selectors, CPU monitor, profile trees. | `widgets/perf-panel.ts → installProfilerPanel`. Visibility driven by `Settings.showProfiler`. In species mode the pop chart draws one line per live species (colored by each species' `color_u32`), fed by the polled `species_table_json` report. Each report tick pushes one sparse `id → count` sample into a 500-deep ring; the draw loop unions all ids seen in-window, breaking polylines across ticks where a species is absent. `restart()` calls `setPanelBridge(newBridge)` + `resetPanelSamples()`. |
| `#perf-close` | ✕ button that hides the profiler. | Flips `showProfiler` to false; perf-panel reacts. |
| `#right-rail` | Persistent right column, 420 px. | `rail/index.ts → installRail`. |
| `#rail-tabs` | Three tab buttons: Settings / NN / Inspector (DOM order; Settings is default active). | `rail/index.ts`. |
| `#rail-settings` | Dev-panel content (`#devpanel-box`) + Apply / Cancel / Reset footer. | `widgets/devpanel.ts → installDevPanel`. |
| `#rail-nn` | NN topology + mutation-bucket editors + per-layer perf log (`#nn-tab-host`). | `rail/nn-tab.ts → installNnTab`. |
| `#rail-inspector` | Inspector body or empty-state. | `rail/inspector.ts` reads/writes `#inspector-empty` and `#ins-*` rows inside `#inspector-body`. Shows a packed-color swatch, genome-modulated `movement_penalty`, and 6 genome traits (`#ins-trait-*` bars from `creature_inspect_json`'s `genome` object). A `#ins-species-block` (hidden by default) shows the species name, numeric `species_id`, color swatch, and `#ins-species-history` breadcrumb. The block appears only when the inspect JSON carries species fields (species mode). |
| `#toast-host` | Bottom-center transient notice slot. | `toast.ts → showToast`. |

## Tab routing rules

- **Default tab on boot:** Settings (`activeTab = "settings"` inside
  `installTabs`). The rail defaults to closed (see `railOpen`), so a
  fresh user sees the canvas full-bleed and must open the rail (⚙ / `~`)
  before any tab is visible.
- **`⚙` Settings button or `~` hotkey** → toggle the rail open/closed.
  Routes through `setRailOpen` in `main.ts` so the persisted setting +
  the `.rail-collapsed` class on `#app-shell` stay in sync.
- **Click a rail tab while the rail is open and that tab is already
  active** → collapses the rail. `installRail` receives a `setRailOpen`
  callback from `main.ts`; the tab click handler compares the incoming
  tab name against `activeTab` and the `Settings.railOpen` flag.
- **The top-bar Settings / NN / Inspector opener buttons toggle the same
  way** (`main.ts → toggleRailTab`): if the rail is already open on that
  tab, a second click calls `setRailOpen(false)`; otherwise it opens
  the rail and switches to that tab.
- **Click a rail tab while the rail is closed, or on a different tab** →
  opens the rail and switches to that tab.
- **Click a creature on canvas** → force the rail open via
  `setRailOpen(true)` AND `rail.switchTab("inspector")` AND populate
  the inspector body. Applies in both the SoA fast-path and the
  `inspect_at` fallback.
- **Deselect (click empty world)** → Inspector tab stays active and
  shows the empty-state hint; the user switches away manually.
- The `~` hotkey is ignored when focus is inside an `<input>` /
  `<textarea>` so typing a tilde in the dev panel doesn't fire the
  toggle.

## Settings tab — stage-then-apply

Two interaction tiers live inside the same tab:

- **Live-apply** (Run + Display groups). Edits hit `setSetting(...)`
  and the apply callback immediately. No dirty tracking. Sliders in
  this tier: `autoRun`, `showProfiler`, `showGrass`, `grassOpacity`,
  the grass render knobs (all pure-render, no `simName`): `grassSmoothing`
  (bicubic blend), `grassSoftness` (extra blur), the intensity-ramp trio
  `grassDensityFloor` / `grassContrast` / `grassBrightness`, and the optional
  procedural-texture overlay `grassTexture` + `grassEdgeErosion` /
  `grassShadeVariation` / `grassBladeSize`; `biomeOpacity` ("World opacity",
  fades the biome/terrain layer), `theme` (Display-group dropdown wired
  through `applyTheme`). `grassSmoothing` + `biomeOpacity` are pure render
  settings (no `simName`); the renderer reads `getSettings()` each frame.
- **Stage-then-apply** (every other group: Energy, Grass, Eat,
  Lifecycle). Edits update only the in-memory widget value. The
  Settings tab's footer reconciles staged changes.

Per staged widget, the dev panel keeps `{simName, settingKey,
readWidget, writeWidget, snapshot, rowEl}`. A row is **dirty** iff
`readWidget() !== snapshot`. Dirty rows get a `.is-dirty` class which
the CSS renders as a left-border accent.

Footer buttons:

- **Apply** — for every dirty staged row: persist via
  `setSetting(settingKey, value)`, then — only if the slider is NOT
  in `CONSTRUCTION_ONLY_SLIDERS` — push via
  `simBridge.debouncedSetSlider(simName, value)`. Update the snapshot.
  Fires the construction-only toast if any dirty row's `simName` was in
  the construction-only set. Construction-only settings do not push a
  live slider update; they stage for the next boot payload only, via
  `widgetReaders`.
- **Cancel** — for every dirty staged row: `writeWidget(snapshot)`.
  Never touches the worker or `localStorage`.
- **Reset** — call `resetSettings()` (writes `DEFAULTS` to
  `localStorage`), then for every staged row write the default into
  the widget; push via `set_slider` only for non-construction-only
  sliders. Live-apply widgets sync through their registered
  `liveSyncers`. Fires the construction-only toast if any reset value
  differed from the previous snapshot.

Apply and Cancel are disabled when nothing is dirty. Reset is always
enabled. Closing the panel (switching to another tab) preserves dirty
edits — re-opening Settings shows them still dirty.

**Construction-only set** — these knobs persist via `setSetting` and
shape the next world only. Apply does NOT push a live `set_slider` call
for them. They reach the next boot via `widgetReaders` /
`currentSliderState()`: `founder_count`, `energy_max`,
`grass_initial_seed_count`, the world-shape knobs `world_size`,
`world_seed`, `wrap_world`, the species-construction knobs
`species_mode`, `crossover_mode`, `starting_species_count`,
`starting_species_member_count`, `starting_species_member_variance`,
`grass_size`, `grass_multisight`, `grass_clump_count`, and
`grass_clump_size`. Labelled `(next world)` via the CSS rule
`.devpanel-row label.next-world::after`. Apply fires the
construction-only toast.

`currentSliderState()` also injects the live `max_population` value from
persisted settings because its control lives in the perf panel rather than
the staged Settings widgets. This makes the cap effective at boot and gives
the worker a nonzero value for its slider lane.

**Unexposed sliders:** `grass_in_cell_growth_r` and
`grass_propagation_rate_k` are not exposed in the Settings UI. They are
wired to the Blur propagation path; the live sim runs Scatter and ignores
these values. The Rust fields, slider registry entries, and `settings.ts`
keys are kept intact.

**Species & mating section + gating.** A `Species & mating` section hosts
the `species_mode` construction toggle, the `crossover_mode` construction
dropdown (`makeStagedDropdown`; options `fifty_fifty=1` / `average=0`, the
Rust f32 slider encoding), the three construction-only species-seeding
sliders, and the live `mating_cooldown_ticks` slider (NOT in the
construction-only set — it applies to the running world). The five
construction-only species knobs ride `newWithFounderCount`'s trailing args,
not the live SAB; `mating_cooldown_ticks` flows through
`currentSliderState()` like any live slider.

`refreshSpeciesGating()` shows/hides rows based on the staged
`species_mode` widget value (the next-world choice, NOT the running sim's
mode). It is wired to the `species_mode` checkbox's `change` event and
re-invoked from `cancelAll`/`resetAll` and once at install for the
persisted initial state.

**Toast text** lives in one place
(`app/web/src/widgets/devpanel.ts → TOAST_CONSTRUCTION`) and fires through
`toast.ts → showToast`.

## Theming

`app/web/src/themes.ts → Theme`, `THEMES`, `DEFAULT_THEME_ID`,
`applyTheme(id)`, `REQUIRED_TOKENS`. `applyTheme` writes each of the
`REQUIRED_TOKENS` onto `document.documentElement` via `style.setProperty`.

Invariant: every theme must set every CSS var that appears in the `:root`
block of `styles.css` — otherwise a switch from a heavier theme leaves a
stale value painted. Layout constants (`--rail-w`, `--tab-h`,
`--topbar-h`, `--profiler-h`) live on `:root` only and are not
theme-owned.

Three renderer tokens (`--grass-tint`, `--creature-ring`, `--creature-halo`)
are consumed GL-side by `render/gl.ts` as shader uniforms (parsed via
`getComputedStyle` + `parseRgba` / `parseRgbVec3`; parsed value is cached
against the source string). `--grass-tint` is a comma-separated `r, g, b`
triple in [0, 1] fed as a vec3 uniform. `--creature-ring` and
`--creature-halo` are standard `rgba()` colors fed as vec4 uniforms; the
halo's alpha sets the per-theme max halo intensity.

`main.ts → main()` calls `applyTheme(getSettings().theme)` before any UI
installer runs. The Settings tab's Display group hosts a live-apply
dropdown (`makeThemeRow` in `devpanel.ts`) that persists the choice and
re-calls `applyTheme` on change.

## Boot-payload accessors

The dev panel exposes typed reader functions that `main.ts → spawnSimWorker`
consumes to build the boot payload so a mid-drag restart carries dragged
values. See `app/crates/evosim/src/wasm_api/mod.rs → WorldHandle::newWithFounderCount`
for the full argument order and types (the boot payload). The key
invariant: construction-only args (`world_size`, `grass_cell_size`,
`grass_clump_count`, `grass_clump_size`, and the species args) must ride
this explicit path because `initial_sliders` is applied after construction
and cannot resize the already-built `WorldDims` or re-seed boot grass.

`currentSliderState()` snapshots the in-memory widget value for every
registered staged widget; the worker applies it via `set_slider` after
construction (this is the path the live `mating_cooldown_ticks` slider
takes).

## Runtime-dims SAB view binding

The world is runtime-sized, so the snapshot grass region and the biome
window are no longer fixed-size constants. After `boot_ready`,
`spawnSimWorker`:

- builds `slotLayout = makeSlotLayout(ready.grass_dim)` — the single
  source of truth for the per-slot byte geometry. The grass region is
  u8, `min(grass_dim, 4096)²` bytes (the clipmap budget axis,
  `GRASS_LOD_BUDGET_AXIS = 4096`).
- pre-seeds the camera SAB lanes to world-center (`ready.world_size / 2`)
  and zoom `1.0` so the first snapshot the worker writes uses a sensible
  window rather than the SAB-default `cx=cy=0, zoom=0`.
- stores `latestWrapWorld` / `latestWorldSeed` from the reply.

The biome window is read from the snapshot slot each frame
(`biomeWinOffset(layout, slot)`) rather than from a separately bound
static buffer. Getting the slot geometry wrong silently over/under-runs
the SAB slot — hence one layout object, rebuilt per boot.

## Camera SAB lanes

The shell writes the camera lanes each RAF; the worker reads them — see
[`architecture/shared-memory-and-protocol.md`](shared-memory-and-protocol.md)
for the lane layout.

## Settings schema migration

`app/web/src/settings.ts` versions the blob as `major.minor` under key
`evosim.settings.v2`:

- **`SCHEMA_MAJOR` mismatch** (or a missing `vMajor`) → stored blob is
  discarded and defaults are used.
- **`SCHEMA_MINOR` mismatch** (additive-only) → user values are kept; the
  `{...DEFAULTS, ...stored}` merge fills any keys the older blob lacked.
  No reset. Additive keys added at later minor versions are picked up from
  `DEFAULTS` without a reset.

On persist the live copy always restamps `vMajor`/`vMinor` to the current
values. See [`decisions/app-shell.md`](../decisions/app-shell.md) for
rationale on the major/minor split.

## Code anchors

- `app/web/index.html` → DOM skeleton, all element IDs in the table above.
- `app/web/src/styles.css` → palette tokens, grid layout, dirty-row accent, toast styling.
- `app/web/src/main.ts` → `main`, `spawnSimWorker`, `installPacingControls`, `installSettingsButton`, `installRestartButton`, frame loop, camera-lane pre-seed + RAF writes, `makeSlotLayout` binding.
- `app/web/src/rail/index.ts` → `installRail`, `pollRail`, `RailState`, `switchTab`.
- `app/web/src/rail/inspector.ts` → click→tab switch, empty-state toggle, SoA fast-path, `inspect_id` throttle.
- `app/web/src/rail/nn-tab.ts` → `installNnTab`.
- `app/web/src/widgets/perf-panel.ts` → `installProfilerPanel`, `setProfilerVisible`, pop-graph sampler + species paint.
- `app/web/src/widgets/devpanel.ts` → Settings tab installer, staged/live tier helpers, dirty tracking, Apply/Cancel/Reset wiring, construction-only toast.
- `app/web/src/widgets/worker-stats.ts` → polled NN thread health table.
- `app/web/src/toast.ts` → `showToast(message, durationMs)`.
- `app/web/src/settings.ts` → `Settings` interface, `DEFAULTS`, `getSettings` / `setSetting` / `resetSettings`, major/minor schema migration.
- `app/web/src/sim/bridge.ts` → `CTRL_CAMERA_*` constants, `readWindowMetadata`, `WindowMetadata`, `SlotLayout`, `makeSlotLayout`, `biomeWinOffset`.
- `app/web/src/themes.ts` → `Theme`, `THEMES`, `DEFAULT_THEME_ID`, `applyTheme`, `REQUIRED_TOKENS`.
- `app/crates/evosim/src/wasm_api/mod.rs` → `WorldHandle::newWithFounderCount` (boot payload arg order + types).

## Update when

- A new tab is added to the rail (update DOM map + routing rules).
- A new live-apply widget is added (update the live-vs-staged carve-out list).
- A new construction-only slider is added (update the construction-only set + boot-payload accessors).
- A widget moves between live and staged tiers.
- `Settings` interface gains or loses a key (and the corresponding Rust `*_DEFAULT` — also update the Wave D drift-guard fixture in `tests/e2e/defaults-drift.spec.ts`).

## See also

- [`simulation-core.md`](simulation-core.md) — slider names + their Rust defaults.
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) — boot payload shape, boot_ready reply, camera lane layout.
- [`worker-runtime.md`](worker-runtime.md) — boot handshake.
- [`render-pipeline.md`](render-pipeline.md) — canvas painting, GL programs, camera math.
- [`profiler.md`](profiler.md) — the panel `#perf-box` renders into.
- [`../decisions/app-shell.md`](../decisions/app-shell.md) — stage-then-apply rationale, construction-only sliders, settings schema major.minor, world_size SAB binding.
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md) — Rust-canonical defaults.
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
