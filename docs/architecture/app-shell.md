# App shell

The main-thread UI shell: how the DOM is laid out, which TS module mounts
into which container, how the rail tabs route, and how the Settings panel
splits staged sim controls from live-apply UI controls.

## What it owns

- The top-level DOM structure in `app/web/index.html`: `#app-shell`,
  `#left-col`, `#top-bar`, `#canvas-wrap`, `#right-rail`, `#rail-tabs`,
  the Settings sub-nav, the inspector body, the General tab body, and the
  Profiler tab body.
- Rail tab routing in `app/web/src/rail/index.ts → RailTab` and
  `installRail`.
- Which TS module mounts into which part of the shell. `main.ts` installs
  the dev panel before worker spawn so `currentSliderState()` is ready for
  boot, then wires worker status, the rail, canvas click handling, the
  profiler panel, the NN tab, the General tab, and the top-bar controls.
- The stage-then-apply pattern in the Settings panel: per-row dirty tracking,
  Apply / Cancel / Reset semantics, the live-vs-staged split, and the
  restart-needed toast for construction-only settings.
- The Settings panel category wiring in `app/web/index.html` and
  `widgets/devpanel.ts → installDevPanel`, including the left sub-nav and
  the matching `#settings-<cat>-pane` containers.
- Cross-widget profiler gating: `showProfiler` is the source of truth for
  profiler recording state. Selecting the top-level Profiler tab enables
  recording and polling; leaving the tab disables both.
- The General tab: `main.ts → installMenuTab` mounts auto-restart, world
  Export / Import, and the autosave status line. Named Save / Resume / Fork
  controls are not exposed in the General tab.
- The app badge: `main.ts → installAppBadge` mounts `evosim v<APP_VERSION>` as
  a small sharp rectangle in the bottom-left corner of `#canvas-wrap`.
- Theme application: `app/web/src/themes.ts → applyTheme` writes the active
  palette tokens onto `<html>`.

## What it does not own

- Canvas painting, GL programs, and camera math - owned by
  [`render-pipeline.md`](render-pipeline.md).
- Snapshot reads on each RAF - owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md).
- Worker lifecycle, pacing, and restart - owned by
  [`worker-runtime.md`](worker-runtime.md).
- Sim slider defaults and live-tunable simulation settings - owned by
  [`simulation-core.md`](simulation-core.md) and `app/web/src/settings.ts`.

## DOM map

| Element | Purpose | Installer / consumer |
|---|---|---|
| `#app-shell` | Shell grid that collapses when the rail is hidden. | CSS in `app/web/src/styles.css`; class flipped by `main.ts → setRailOpen`. |
| `#left-col` | Top bar, canvas, app badge, and the hidden profiler placeholder. | CSS plus `main.ts → installAppBadge`. |
| `#top-bar` | Play/pause, Restart, the rail toggle, and conditional worker status / Retry controls. | `main.ts → installTopBarButtons`, `installWorkerStatusUi`. |
| `#canvas-wrap > #aquarium` | The WebGL2 sim view. | `render/gl.ts`. |
| `#perf-box` | Hidden placeholder kept for resize-handle compatibility. | `widgets/perf-panel.ts`. |
| `#right-rail` | Persistent right column. | `rail/index.ts → installRail`. |
| `#rail-tabs` | General, Inspector, Settings, and Profiler rail tabs. | `rail/index.ts`. |
| `#rail-settings` | Settings panel with the left sub-nav and category panes. | `widgets/devpanel.ts → installDevPanel`. |
| `#settings-<cat>-pane` | Category pane mounted by `categoryBox(id)`. | `widgets/devpanel.ts → categoryBox`. |
| `#rail-profiler` → `#settings-profiler-pane` | Profiler panel: status line, charts, telemetry export, CPU monitor, profile trees. | `widgets/perf-panel.ts → installProfilerPanel`. |
| `#settings-nn-pane` | NN topology and mutation-bucket editors. | `rail/nn-tab.ts → installNnTab`. |
| `#rail-inspector` | Creature inspector body and empty state. | `rail/inspector.ts`. |
| `#rail-general` → `#menu-inner` | General tab body for auto-restart, Export / Import, and autosave status. | `main.ts → installMenuTab`. |
| `#toast-host` | Transient notice slot. | `toast.ts → showToast`. |

## Tab routing rules

- General is the default visible rail tab and the top-bar hamburger opens it.
- Clicking the active tab while the rail is open closes the rail.
- Clicking another tab, or clicking while the rail is closed, opens the rail
  and switches to that tab.
- Selecting the Profiler tab enables profiler recording and polling; leaving
  the tab idles both.
- Clicking a creature opens the rail to Inspector and populates the inspector
  body.
- Clicking empty space leaves Inspector open with its empty-state hint.
- The `~` hotkey is ignored while focus is inside an input or textarea.
- Escape opens Settings when the rail is closed or another tab is active, and
  closes the rail when Settings is already open. Escape is ignored while focus
  is inside an input or textarea.

## Settings panel

The Settings panel separates staged sim controls from live-apply UI controls.

- Staged sim controls use dirty tracking plus footer Apply / Cancel / Reset.
  `widgets/devpanel.ts → makeStagedSlider`, `makeStagedToggle`,
  `makeStagedDropdown`, `applyAll`, `cancelAll`, and `resetAll` own the
  behavior.
- Live-apply controls update immediately and do not participate in dirty
  tracking. That includes run controls and render-side UI settings such as
  `autoRun`, `showProfiler`, `visualRepeats`, `maxZoomOutMaps`, the creature
  survey-dot controls, repeat-border controls, and the display/theme controls.
  These settings are render-side only and are read by the camera/renderer each
  frame; they do not mutate simulation state.
- Construction-only settings stage into the next `WorldConfig` boot payload.
  Apply and Reset persist them locally and show the restart-needed toast.
  `CONSTRUCTION_ONLY_SLIDERS` and `currentWorldConfig()` are the owning
  symbols. The no-grass-zone toggle is in this group: it changes world
  construction capacity/zone bytes and therefore requires a restarted world.
- `currentSliderState()` includes staged widget values plus the live controls
  that sit outside the staged Settings widgets, such as the perf-panel
  `max_population` selector, the legacy grass wave sliders, and the mutation
  buckets until the NN tab registers its own readers.
- Paused worlds repaint every RAF frame from the latest snapshot while
  `seq === lastPaintedSeq` and a slot layout is ready. This keeps the canvas
  filled across pause, rail toggles, and camera pan/zoom — avoiding blank-canvas
  artifacts that a movement-gated repaint could not cover.

The Settings sub-nav is wired from the category buttons in
`app/web/index.html`; the corresponding pane visibility is managed by
`installDevPanel`.

## Theming

`themes.ts` owns the palette map. `applyTheme(id)` writes each required CSS
token onto `document.documentElement`. The theme must provide every CSS var
that the shell expects in `styles.css`.

## Boot-payload accessors

`widgets/devpanel.ts` exposes the typed reader functions that `main.ts →
spawnSimWorker` consumes when building the boot payload. Construction-only
settings ride the boot payload because `initial_sliders` is applied after the
world is already constructed; those settings cannot resize the world, rebuild
the NN input layout, or re-seed boot grass after the fact.

`app/web/src/generated/world-config.ts` is generated and supplies the
defaults used by the settings layer. Display defaults that do not ride
`WorldConfig` live in `settings.ts`; generated construction defaults include
the grass/no-grass-zone enable flag.

## See also

- [`../decisions/app-shell.md`](../decisions/app-shell.md)
- [`../architecture/index.md`](../architecture/index.md)
- [`../ownership.md`](../ownership.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- Global authoring rules: `~/agent-docs/v1/rules/authoring-rules.md`
