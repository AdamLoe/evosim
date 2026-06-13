# App shell

The main-thread UI structure: how the DOM is laid out, which TS module
installs into which container, how the right-rail tabs route, and how the
Settings panel's stage-then-apply machinery works.

## What it is

A two-column shell that wraps the canvas and every non-canvas UI element.

```
┌──────────────────────────────────┬────────────────────────┐
│ evosim vX.Y.Z     Pause Restart …│ ┌────────┬───────────┐ │
│                                  │ │Settings│ Inspector │ │
├──────────────────────────────────┤ ├────────┴───────────┤ │
│                                  │ │ ┌──sub-nav──┐ pane │ │
│ canvas                           │ │ │ Energy    │      │ │
│                                  │ │ │ Grass     │      │ │
│                                  │ │ │ Lifecycle │      │ │
│                                  │ │ │ World     │      │ │
│                                  │ │ │ Equilib.  │      │ │
│                                  │ │ │ Display   │      │ │
│                                  │ │ │ Profiler  │      │ │
│                                  │ │ │ NN        │      │ │
└──────────────────────────────────┴──┴───────────┴────────┘
         left column (flex)               right rail
                                          (fixed 420 px)
```

The DOM tree lives in `app/web/index.html`. The CSS layout uses a top-level
grid (`#app-shell { grid-template-columns: 1fr var(--rail-w); }`); the
left column hosts the canvas plus absolute overlays for the top-bar controls
and app/version badge.

## What it owns

- The top-level DOM structure: `#app-shell` → `#left-col` (`#top-bar`,
  `#canvas-wrap` with `#aquarium`, `#perf-box` as empty placeholder) and
  `#right-rail` (`#rail-tabs` with Settings and Inspector tabs, and the
  two `.rail-panel` sections).
- The CSS palette tokens (`--bg-app`, `--bg-panel`, `--fg`, `--accent`,
  `--accent-dirty`, `--danger`, …) — single source of truth for color in
  `app/web/src/styles.css`.
- Right-rail tab routing: tab switching, default tab on boot, the
  switch-on-creature-click rule. Only two tabs exist: Settings and Inspector.
  NN editor and Profiler are settings categories, not rail tabs.
- Which TS module installs into which DOM element. `main.ts` installs the
  dev panel first so `currentSliderState()` is ready for worker boot, then
  creates the worker status UI and rail before spawning the worker. After
  boot it wires canvas click handling, the profiler panel, NN tab, and
  top-bar buttons.
- The stage-then-apply pattern in the Settings panel: per-row dirty
  tracking, Apply / Cancel / Reset semantics, the live-vs-staged
  carve-out, the "construction-only" toast trigger.
- Settings row anatomy: each devpanel row is a compact wrapping row made from
  a header segment (effect dot, setting name, optional tooltip), an optional
  slider segment, and a meta segment (input, value readout where present, and
  row-local RESET). Wide rows can stay on one line; narrower slider rows wrap
  as header / slider+meta, then header / slider / meta. Effect colors are
  green for current-run updates, yellow for restart-needed construction
  settings, and red is reserved for page-refresh-required settings.
- The Settings panel left sub-nav: eight category buttons (`data-cat`
  attributes) activate the corresponding `#settings-<cat>-pane`. Sub-nav
  wiring lives in `devpanel.ts → installDevPanel`. Category row containers
  are mounted by `categoryBox(id)` helpers.
- Cross-widget source-of-truth coupling: `showProfiler` is the single
  source of truth for profiler recording state. Selecting the Profiler
  sub-nav category calls `setSetting("showProfiler", true)` +
  `setProfilerVisible(true)` (starts recording + poll); navigating to any
  other category calls both with `false` (idles). The Rust profiler backend
  is always-on (the worker enables it at boot); only the panel
  visibility + poll are gated by `showProfiler`.
- The rail open/closed toggle: `Settings.railOpen` (default `false`,
  so fresh users start with the rail collapsed) drives
  `#app-shell.rail-collapsed`, which collapses the grid track to `0`
  and hides `#right-rail`. The ⚙ button and the `~` hotkey both route
  through `setRailOpen` in `main.ts`.
- Worker recovery status: the top bar normally shows the run and persistence
  controls, but `#worker-status` appears while the sim worker is booting,
  recovering, stalled/crashed, or failed. `#worker-retry-btn` appears only
  after repeated automatic recovery fails.
- World persistence controls: top-bar Save, Resume, Fork, Export, and Import
  actions request a saved-world artifact from the worker, store named/autosave
  records in IndexedDB, or boot a replacement worker from an imported/exported
  artifact. `#save-status` reports save/autosave/import/export errors and the
  last successful action.
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
| `#left-col` | Top-bar + canvas + app badge + empty `#perf-box` placeholder. | CSS-only plus `main.ts → installAppBadge`. |
| `#app-badge` | Top-left `evosim v<version>` badge. Version is imported from `app/web/package.json`. | `main.ts → installAppBadge`. |
| `#top-bar` | Always-visible primary controls: play/pause, Restart (always rerolls seed), auto-restart, saved-world Save/Resume/Fork/Export/Import, save status, ⚙ settings rail toggle; conditional `#worker-status` and `#worker-retry-btn` appear during recovery/failure. | `main.ts → installTopBarButtons`, `installPersistenceUi`, `installWorkerStatusUi`. NN opener, Inspector opener, and perf-toggle opener were removed; all three surfaces now live inside the Settings panel or as rail tabs. |
| `#canvas-wrap > #aquarium` | The WebGL2 sim view. | `render/gl.ts`. |
| `#perf-box` | Empty hidden placeholder (kept in DOM for resize-handle compat). The profiler content was relocated to `#settings-profiler-pane`. | DOM-only; `display:none`. |
| `#right-rail` | Persistent right column, 420 px. | `rail/index.ts → installRail`. |
| `#rail-tabs` | Two tab buttons: Settings / Inspector (DOM order; Settings is default active). The NN tab was removed; NN editor is a Settings sub-nav category. | `rail/index.ts`. |
| `#rail-settings` | Settings panel: left sub-nav + right category pane area + Apply/Cancel/Reset footer. | `widgets/devpanel.ts → installDevPanel`. |
| `#rail-settings` sub-nav | Eight `.settings-cat-btn` buttons (`data-cat`: energy, grass, lifecycle, world, equilibrium, display, profiler, nn). Wired in `installDevPanel`; active button + active pane kept in sync. | `widgets/devpanel.ts` sub-nav wiring. |
| `#settings-<cat>-pane` | One `.settings-pane` per category (e.g. `#settings-energy-pane`). Only the active one is visible. Each `devpanel-<cat>` div inside is the mount for `categoryBox`. | `widgets/devpanel.ts → categoryBox`. |
| `#devpanel-equilibrium` | Mount for the 6 equilibrium sliders (P3) inside `#settings-equilibrium-pane`. | `widgets/devpanel.ts`. |
| `#settings-profiler-pane` | Padded profiler panel — status line, FPS/TPS chart, pop chart, telemetry CSV/JSON export actions, CPU monitor, profile trees. Activated by selecting the Profiler sub-nav category. | `widgets/perf-panel.ts → installProfilerPanel`; CSS owns padding/box sizing. |
| `#settings-nn-pane` | NN topology + mutation-bucket editors (`#nn-tab-host`). | `rail/nn-tab.ts → installNnTab`. |
| `#rail-inspector` | Inspector body or empty-state. | `rail/inspector.ts` reads/writes `#inspector-empty` and `#ins-*` rows inside `#inspector-body`. Includes `#ins-nn-block` — the per-creature NN I/O block (inputs grouped by compass, outputs: vx/vy, 3 logit bars, highlighted chosen action). Shows a `#ins-species-block` (hidden by default) when inspect JSON carries species fields. |
| `#toast-host` | Bottom-center transient notice slot. | `toast.ts → showToast`. |

## Tab routing rules

There are two rail tabs: Settings and Inspector. The NN editor is a category
inside the Settings panel; the Profiler is also a Settings category.

- **Default tab on boot:** Settings (`activeTab = "settings"` inside
  `installRail`). The rail defaults to closed (see `railOpen`), so a
  fresh user sees the canvas full-bleed and must open the rail (⚙ / `~`)
  before any tab is visible.
- **`⚙` top-bar button or `~` hotkey** → toggle the rail open/closed.
  Routes through `setRailOpen` in `main.ts` so the persisted setting +
  the `.rail-collapsed` class on `#app-shell` stay in sync.
- **Click a rail tab while the rail is open and that tab is already
  active** → collapses the rail. `installRail` receives a `setRailOpen`
  callback from `main.ts`; the tab click handler compares the incoming
  tab name against `activeTab` and the `Settings.railOpen` flag.
- **Click a rail tab while the rail is closed, or on a different tab** →
  opens the rail and switches to that tab.
- **Click a creature on canvas** → force the rail open via
  `setRailOpen(true)` AND `rail.switchTab("inspector")` AND populate
  the inspector body (including the `#ins-nn-block` NN I/O rows).
  Applies in both the SoA fast-path and the `inspect_at` fallback.
  NN I/O requires the sim to be paused; `requestNnInspectId` issues
  `CTRL_INSPECT_REQ_KIND=2` and the worker serves it in the PAUSED
  branch of `simLoop` before parking.
- **Deselect (click empty world)** → Inspector tab stays active and
  shows the empty-state hint; the user switches away manually.
- The `~` hotkey is ignored when focus is inside an `<input>` /
  `<textarea>` so typing a tilde in the dev panel doesn't fire the
  toggle.

## Settings panel — stage-then-apply

The stage-then-apply / live-vs-staged carve-out and apply/cancel/reset
semantics are **unchanged** by the Settings restructure — only
navigation layout changed. Two interaction tiers live inside the same panel:

- **Live-apply** (Run + Display groups). Edits hit `setSetting(...)`
  and the apply callback immediately. No dirty tracking. Sliders in
  this tier: `autoRun`, `showProfiler`, `showGrass`, `grassOpacity`,
  the grass render knobs (all pure-render, no `simName`): `grassSmoothing`
  (bicubic blend), `grassSoftness` (extra blur), the intensity-ramp trio
  `grassDensityFloor` / `grassContrast` / `grassBrightness`, and the optional
  procedural-texture overlay `grassTexture` + `grassEdgeErosion` /
  `grassShadeVariation` / `grassBladeSize`; `biomeOpacity` ("World opacity",
  fades the biome/terrain layer), `appFPS` (App FPS dropdown: 15/30/60/120,
  default 60), and `theme` (Display-group dropdown wired through
  `applyTheme`). `grassSmoothing`, `biomeOpacity`, and `appFPS` are pure
  render settings (no `simName`); the renderer reads `getSettings()` each
  frame.
- **Stage-then-apply** (every other group: Energy, Grass, Eat,
  Lifecycle, Equilibrium). Edits update only the in-memory widget value.
  The Settings panel footer reconciles staged changes.

Per staged widget, the dev panel keeps `{simName, settingKey,
readWidget, writeWidget, snapshot, rowEl}`. A row is **dirty** iff
`readWidget() !== snapshot`. Dirty rows get a `.is-dirty` class which
the CSS renders as a left-border accent. The row helper also attaches the
effect dot (`.devpanel-effect-instant`, `.devpanel-effect-restart`, or
`.devpanel-effect-refresh`) into the header segment and the row-local RESET
button into the meta segment.

Per-row RESET:

- **Staged rows** — write that row's `DEFAULTS` value into the widget only,
  then refresh dirty state and dependent row gating. This preserves the
  staged contract: Apply must still persist/push, and Cancel can still return
  to the last-applied snapshot.
- **Live rows** — write that row's default through the live apply path, so
  the setting and side effect update immediately.

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
`currentWorldConfig()`: `founder_count`, `energy_max`,
`grass_initial_seed_count`, the world-shape knobs `world_size`,
`world_seed`, `wrap_world`, the species-construction knobs
`species_mode`, `crossover_mode`, `starting_species_count`,
`starting_species_member_count`, `starting_species_member_variance`,
`grass_size`, `grass_multisight`, `grass_clump_count`,
`grass_clump_size`, `init_graze_boost`, and `init_split_boost`.
Labelled `(next world)` via the CSS rule
`.devpanel-row label.next-world::after`. Apply fires the
construction-only toast.

`currentSliderState()` also injects live values from persisted settings when
their controls live outside the staged Settings widgets: `max_population` from
the Profiler category pane, the hidden legacy Blur grass knobs, the six
equilibrium sliders, and the NN mutation buckets unless the NN tab has
registered widget readers. This makes every SAB lane nonzero/canonical at boot.

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
construction-only species knobs ride the boot `WorldConfig`, not the live SAB;
`mating_cooldown_ticks` flows through
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
consumes to build `WorldConfig` so a mid-drag restart carries dragged
values. See `app/crates/evosim/src/wasm_api/mod.rs → WorldConfig` and
`WorldHandle::newWithConfigJson` for the Rust-owned schema. The key
invariant: construction-only args (`world_size`, `grass_cell_size`,
`grass_multisight`, `grass_clump_count`, `grass_clump_size`, founder boosts,
NN topology, and the species args) must ride `WorldConfig` because
`initial_sliders` is applied after construction and cannot resize the
already-built `WorldDims`, rebuild the NN input layout, or re-seed boot grass.

`app/web/src/generated/world-config.ts` is produced by
`cargo run --bin gen-bindings` and supplies `DEFAULT_WORLD_CONFIG`,
`WORLD_CONFIG_PRESETS`, and `DEFAULT_LIVE_SLIDER_VALUES`. Settings defaults use
those generated values rather than hand-maintained copies.

`currentSliderState()` snapshots the in-memory widget value for every
registered staged widget; the worker applies it via `set_slider` after
construction (this is the path the live `mating_cooldown_ticks` slider
takes). It also injects persisted sim settings whose controls live outside
the staged Settings widgets: `max_population` from the perf panel, the hidden
legacy Blur grass knobs `grass_propagation_rate_k` /
`grass_in_cell_growth_r`, the equilibrium live sliders, and the mutation bucket
table from the NN tab unless that tab has registered live widget readers.

## World persistence UI

The app shell owns the browser storage and user-facing artifact actions; Rust
owns the artifact format and validation. `app/web/src/storage/world-saves.ts`
wraps IndexedDB database `evosim.world-saves`, store `saves`, with autosave,
named, and imported records. Autosave is coarse (`30 s` minimum cadence and no
same-tick rewrite), requests the same worker artifact as manual Save, and
writes only the latest autosave record. Named Save creates a timestamped record.

Resume and Fork load the latest saved record into a fresh worker lifetime.
Resume preserves the saved run identity and records resumed lineage metadata;
Fork creates a new run id with the saved run as parent. Export downloads the
current artifact JSON; Import validates the file, stores it as an imported
record, and resumes it. All actions surface success/failure through
`#save-status` and toasts; corrupt or unsupported artifacts fail before the
old worker is terminated.

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
- stores `latestWrapWorld` and the resolved `master_seed` from the reply.

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

The historical `worldSeed` settings key is retained for localStorage
compatibility, but it now stores the `WorldConfig.master_seed`. Existing
numeric values are preserved on minor-version migration; the exact old biome
layout is not promised because the new constructor derives the internal
`world_seed` from the master seed.

## Code anchors

- `app/web/index.html` → DOM skeleton, all element IDs in the table above.
- `app/web/src/styles.css` → palette tokens, grid layout, sub-nav styles (`.settings-cat-btn`, `.settings-pane`), dirty-row accent, toast styling.
- `app/web/src/main.ts` → `main`, `spawnSimWorker`, `installTopBarButtons`, `installWorkerStatusUi`, `checkWorkerWatchdog`, frame loop, camera-lane pre-seed + RAF writes, `makeSlotLayout` binding.
- `app/web/src/rail/index.ts` → `installRail`, `pollRail`, `RailState`, `switchTab`. `RailTab` = `"inspector" | "settings"` only.
- `app/web/src/rail/inspector.ts` → click→tab switch, empty-state toggle, SoA fast-path, `inspect_id` throttle, `#ins-nn-block` NN I/O block.
- `app/web/src/rail/nn-tab.ts` → `installNnTab` (mounts into `#settings-nn-pane → #nn-tab-host`).
- `app/web/src/widgets/perf-panel.ts` → `installProfilerPanel` (mounts into `#settings-profiler-pane`), `setProfilerVisible`, pop-graph sampler + species paint.
- `app/web/src/widgets/devpanel.ts` → Settings panel installer, `categoryBox`, sub-nav wiring (category-select → `showProfiler` coupling), staged/live tier helpers, dirty tracking, Apply/Cancel/Reset wiring, construction-only toast.
- `app/web/src/widgets/worker-stats.ts` → polled NN thread health table.
- `app/web/src/toast.ts` → `showToast(message, durationMs)`.
- `app/web/src/settings.ts` → `Settings` interface, generated-default-backed `DEFAULTS`, `getSettings` / `setSetting` / `resetSettings`, major/minor schema migration.
- `app/web/src/sim/bridge.ts` → `CTRL_CAMERA_*` constants, `CTRL_INSPECT_REQ_KIND`, `requestNnInspectId`, `readWindowMetadata`, `WindowMetadata`, `SlotLayout`, `makeSlotLayout`, `biomeWinOffset`.
- `app/web/src/themes.ts` → `Theme`, `THEMES`, `DEFAULT_THEME_ID`, `applyTheme`, `REQUIRED_TOKENS`.
- `app/crates/evosim/src/wasm_api/mod.rs` → `WorldConfig`, `WorldHandle::newWithConfigJson`.

## Update when

- A new tab is added to the rail (update DOM map + routing rules).
- A new settings sub-nav category is added (update the DOM map, sub-nav wiring in `devpanel.ts`, and the ASCII diagram above).
- A new live-apply widget is added (update the live-vs-staged carve-out list).
- A new construction-only slider is added (update the construction-only set + `WorldConfig` builder).
- A widget moves between live and staged tiers.
- `Settings` interface gains or loses a key (and the corresponding generated Rust default source).

## See also

- [`simulation-core.md`](simulation-core.md) — slider names + their Rust defaults.
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) — boot payload shape, boot_ready reply, camera lane layout, `CTRL_INSPECT_REQ_KIND` protocol.
- [`worker-runtime.md`](worker-runtime.md) — boot handshake.
- [`render-pipeline.md`](render-pipeline.md) — canvas painting, GL programs, camera math.
- [`profiler.md`](profiler.md) — the panel that mounts into `#settings-profiler-pane`.
- [`../decisions/app-shell.md`](../decisions/app-shell.md) — settings IA rationale, stage-then-apply rationale, construction-only sliders, settings schema major.minor, world_size SAB binding.
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md) — Rust-canonical defaults.
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- [Agent-docs authoring rules](~/agent-docs/v1/rules/authoring-rules.md)
