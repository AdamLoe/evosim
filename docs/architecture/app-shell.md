# App shell

The main-thread UI structure: how the DOM is laid out, which TS module
installs into which container, how the right-rail tabs route, and how the
Settings tab's stage-then-apply machinery works.

## What it is

A two-column shell that wraps the canvas and every non-canvas UI element.

```
┌──────────────────────────────────┬──────────────────────┐
│ top-bar (status + pacing + ⚙)    │ ┌────┬─────┬───────┐ │
│                                  │ │Insp│Monit│Settngs│ │
├──────────────────────────────────┤ ├────┴─────┴───────┤ │
│                                  │ │                  │ │
│ canvas                           │ │ active tab body  │ │
│                                  │ │                  │ │
├──────────────────────────────────┤ │                  │ │
│ profiler (optional, ✕)           │ │                  │ │
└──────────────────────────────────┴──┴──────────────────┘
         left column (flex)              right rail
                                         (fixed 420 px)
```

The DOM tree lives in `web/index.html`. The CSS layout uses a top-level
grid (`#app-shell { grid-template-columns: 1fr var(--rail-w); }`); the
left column uses a row grid for top-bar / canvas / profiler.

## What it owns

- The top-level DOM structure: `#app-shell` → `#left-col` (`#top-bar`,
  `#canvas-wrap` with `#aquarium`, `#perf-box`) and `#right-rail`
  (`#rail-tabs` and the three `.rail-panel` sections).
- The CSS palette tokens (`--bg-app`, `--bg-panel`, `--fg`, `--accent`,
  `--accent-dirty`, `--danger`, …) — single source of truth for color in
  `web/src/styles.css`.
- Right-rail tab routing: tab switching, default tab on boot, the
  switch-on-creature-click rule.
- Which TS module installs into which DOM element. Installers run from
  `main.ts` in the order: profiler panel → dev panel (= Settings tab)
  → Monitor tab → Settings button. Installation order matters because
  the dev panel's "Show profiler" toggle calls into the profiler panel
  installer's exported visibility setter.
- The stage-then-apply pattern in the Settings tab: per-row dirty
  tracking, Apply / Cancel / Reset semantics, the live-vs-staged
  carve-out, the "construction-only" toast trigger.
- Cross-widget source-of-truth coupling: `showProfiler` is the single
  state for both the Settings checkbox and the perf-panel ✕ button;
  flipping either writes the same setting and re-calls
  `setProfilerVisible()`. v1.9.1: visibility-only — the Rust profiler is
  always-on (the worker enables it at boot) and the panel keeps polling
  the bundled report whenever it's visible, regardless of any
  backend "enabled" flag.
- The rail open/closed toggle: `Settings.railOpen` (default `true`)
  drives `#app-shell.rail-collapsed`, which collapses the grid track to
  `0` and hides `#right-rail`. The ⚙ button and the `~` hotkey both
  route through `setRailOpen` in `main.ts`.
- Theming: `web/src/themes.ts` owns the palette map. `applyTheme(id)`
  writes inline custom properties onto `<html>`; the four shipped themes
  (charcoal, slate, light, vivid) each define **every** CSS var listed
  in `styles.css`'s `:root` block. Charcoal is byte-identical to the
  `:root` fallback so default users see no change after v1.9.1.

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
| `#top-bar` | Always-visible status strip + pacing buttons + settings button. | `main.ts` populates with status text, play/pause toggle, target-TPS dropdown, restart, ⚙ Settings. |
| `#canvas-wrap > #aquarium` | The WebGL2 sim view. | `render-gl.ts`. |
| `#perf-box` | Profiler bottom panel (collapsed by default). | `widgets/perf-panel.ts → installProfilerPanel`. Visibility driven by `Settings.showProfiler`. |
| `#perf-close` | ✕ button that hides the profiler. | Flips `showProfiler` to false; perf-panel reacts. |
| `#right-rail` | Persistent right column, 420 px. | `rail/index.ts → installRail`. |
| `#rail-tabs` | Three tab buttons: Inspector / Monitor / Settings. | `rail/index.ts`. |
| `#rail-inspector` | Inspector body or empty-state. | `rail/inspector.ts` reads / writes `#inspector-empty` and the `#ins-*` rows inside `#inspector-body`. |
| `#rail-monitor` | Population graph + worker-stats host. | `rail/monitor.ts → installMonitorTab`; pop-graph paint via `rail/stats.ts → maybeSampleStats`. |
| `#rail-settings` | Dev-panel content + Apply / Cancel / Reset footer. | `widgets/devpanel.ts → installDevPanel`. |
| `#toast-host` | Bottom-center transient notice slot. | `toast.ts → showToast`. |

## Tab routing rules

- **Default tab on boot:** Settings (`activeTab = "settings"` inside
  `installTabs`).
- **`⚙` Settings button or `~` hotkey** → toggle the rail
  open/closed (v1.9.1; previously: switch to Settings tab). Routes
  through `setRailOpen` in `main.ts` so the persisted setting + the
  `.rail-collapsed` class on `#app-shell` stay in sync.
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
  this tier: `autoRun`, `showProfiler`, `showPopGraph`, `showGrass`,
  `grassOpacity`, `theme` (Display-group dropdown wired through
  `applyTheme`).
- **Stage-then-apply** (every other group: Energy, Grass, Eat,
  Lifecycle, Curriculum). Edits update only the in-memory widget
  value. The Settings tab's footer reconciles staged changes.

Per staged widget, the dev panel keeps `{simName, settingKey,
readWidget, writeWidget, snapshot, rowEl}`. A row is **dirty** iff
`readWidget() !== snapshot`. Dirty rows get a `.is-dirty` class which
the CSS renders as a left-border accent.

Footer buttons:

- **Apply** — for every dirty staged row: persist via
  `setSetting(settingKey, value)`, push via
  `simBridge.debouncedSetSlider(simName, value)`, update the
  snapshot. Fires the construction-only toast if any dirty row's
  `simName` was in the construction-only set.
- **Cancel** — for every dirty staged row: `writeWidget(snapshot)`.
  Never touches the worker or `localStorage`.
- **Reset** — call `resetSettings()` (writes `DEFAULTS` to
  `localStorage`), then for every staged row write the default into
  the widget and push it via `set_slider`. Live-apply widgets sync
  through their registered `liveSyncers`. Fires the
  construction-only toast if any reset value differed from the
  previous snapshot.

Apply and Cancel are disabled when nothing is dirty. Reset is always
enabled. Closing the panel (switching to another tab) preserves dirty
edits — re-opening Settings shows them still dirty.

**Construction-only set** — these knobs ride
`set_slider` and persist, but only shape the *next* world:
`founder_count`, `energy_max`, `grass_initial_seed_count`,
`full_grass_on_init`. Labelled `(next world)` via the CSS rule
`.devpanel-row label.next-world::after`.

**Toast text** lives in one place
(`widgets/devpanel.ts → TOAST_CONSTRUCTION`) and fires through
`toast.ts → showToast`.

## Theming

`web/src/themes.ts` exports a `Theme` interface (`id`, `name`, `tokens:
Record<string, string>`), the `THEMES` map (currently four entries:
`charcoal`, `slate`, `light`, `vivid`), `DEFAULT_THEME_ID = "charcoal"`,
and `applyTheme(id)`. `applyTheme` looks the theme up in the map (falling
back to the default if `id` is unknown) and writes each of the
`REQUIRED_TOKENS` onto `document.documentElement` via
`style.setProperty`.

Invariant: every theme must set **every** CSS var that appears in the
`:root` block of `styles.css` — otherwise a switch from a heavier theme
leaves a stale value painted. The current required set is the palette
tokens (`--bg-app`, `--bg-panel`, `--bg-panel-alt`, `--bg-canvas`,
`--fg`, `--fg-muted`, `--fg-faint`, `--border`, `--border-strong`,
`--accent`, `--accent-2`, `--accent-dirty`, `--danger`, `--success`,
`--warning`, `--info`, `--chart-line`, `--chart-grid`) plus three
renderer tokens added in v1.9.2 (`--grass-tint`, `--creature-ring`,
`--creature-halo`). Layout constants (`--rail-w`, `--tab-h`,
`--topbar-h`, `--profiler-h`) live on `:root` only — they are not
theme-owned.

The three renderer tokens are consumed GL-side by `render-gl.ts` as
shader uniforms (parsed via `getComputedStyle` + small `parseRgba` /
`parseRgbVec3` helpers; the parsed value is cached against the source
string so a theme switch re-parses and a steady state doesn't).
`--grass-tint` is a comma-separated `r, g, b` triple in [0, 1] fed as
a vec3 uniform (chosen so it composes cleanly with the texture's R8
density without an extra colorspace pass). `--creature-ring` and
`--creature-halo` are standard `rgba()` colors fed as vec4 uniforms;
the halo's `rgb` channels are unused (the halo paints body color) but
its alpha sets the per-theme max halo intensity.

`main.ts → main()` calls `applyTheme(getSettings().theme)` before any UI
installer runs, so the first paint uses the persisted theme rather than
flashing the `:root` fallback (charcoal). The Settings tab's Display
group hosts a live-apply dropdown (`makeThemeRow` in `devpanel.ts`) that
persists the choice and re-calls `applyTheme` on change.

## Boot-payload accessors

The dev panel exposes typed reader functions that `main.ts → spawnSimWorker`
consumes to build the boot payload (so a mid-drag restart carries the
dragged values, not the last-persisted ones):

```text
getInitialGrassSeedCount() → number
getEnergyMax()             → number
getFounderCount()          → number
getFullGrassOnInit()       → boolean
currentSliderState()       → Record<string, number>
```

`currentSliderState()` snapshots the in-memory widget value for every
registered staged widget; the worker applies it via `set_slider` after
construction.

## Code anchors

- `web/index.html` → DOM skeleton, all element IDs in the table above.
- `web/src/styles.css` → palette tokens, grid layout, dirty-row accent,
  toast styling.
- `web/src/main.ts` → `main`, `spawnSimWorker`, `installPacingControls`,
  `installSettingsButton`, `installRestartButton`, frame loop.
- `web/src/rail/index.ts` → `installRail`, `pollRail`, `RailState`,
  `switchTab` (the implementation that flips `.is-active` classes).
- `web/src/rail/inspector.ts` → click→tab switch, empty-state toggle,
  SoA fast-path, `inspect_id` throttle.
- `web/src/rail/monitor.ts` → `installMonitorTab` (wires the worker-
  stats panel into `#worker-stats-host`).
- `web/src/rail/stats.ts` → pop-graph sampler + paint to `#chart-pop`.
- `web/src/widgets/devpanel.ts` → Settings tab installer, staged /
  live tier helpers, dirty tracking, Apply / Cancel / Reset wiring,
  construction-only toast.
- `web/src/widgets/perf-panel.ts` → `installProfilerPanel`,
  `setProfilerVisible` (single source of truth for show/hide).
- `web/src/widgets/worker-stats.ts` → polled NN thread health panel
  rendered inside the Monitor tab.
- `web/src/toast.ts` → `showToast(message, durationMs)`.
- `web/src/settings.ts` → `Settings` interface, `DEFAULTS`,
  `getSettings` / `setSetting` / `resetSettings`, the unknown-key
  filter on load.
- `web/src/themes.ts` → `Theme` interface, `THEMES` map,
  `DEFAULT_THEME_ID`, `applyTheme(id)`, `REQUIRED_TOKENS` invariant.

## Update when

- A new tab is added to the rail (update DOM map + routing rules).
- A new live-apply widget is added (update the live-vs-staged
  carve-out list).
- A new construction-only slider is added (update the
  construction-only set + boot-payload accessors).
- A widget moves between live and staged tiers.
- `Settings` interface gains or loses a key (and the corresponding
  Rust `*_DEFAULT` — also update the Wave D drift-guard fixture in
  `tests/e2e/defaults-drift.spec.ts`).

## See also

- [`simulation-core.md`](simulation-core.md) — slider names + their
  Rust defaults.
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) —
  `boot` payload shape (includes `full_grass_on_init`), `boot_ready`
  reply shape (includes `sliders_defaults_json`).
- [`worker-runtime.md`](worker-runtime.md) — boot handshake.
- [`profiler.md`](profiler.md) — the panel `#perf-box` renders into.
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md) —
  stage-then-apply rationale, Rust-canonical defaults.
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
