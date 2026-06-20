# Decisions -- app shell

Current design choices that bind the app shell only: settings-panel behavior,
UI lifecycle, localStorage schema, and the shell-facing persistence wiring.
Cross-subsystem decisions that also bind the shell live in
[`cross-cutting.md`](cross-cutting.md).

---

### Settings panel stages sim controls and live-applies immediate UI controls

- **Decision**: Sim-affecting settings use staged Apply / Cancel / Reset
  behavior in the Settings panel. Immediate UI controls apply live. The
  profiler gate follows the same live rule: selecting the Profiler rail tab
  turns recording and polling on; leaving the tab turns both off.
- **Why**: batch sim edits should commit atomically, while display and panel
  visibility changes should preview immediately.
- **Tradeoffs**: the split adds code compared with a uniform rule, but it
  keeps the running world stable and the UI responsive.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/widgets/devpanel.ts → makeStagedSlider`,
  `makeLiveSlider`, `CONSTRUCTION_ONLY_SLIDERS`; `app/web/src/widgets/perf-panel.ts → setProfilerVisible`.

### Construction-only settings stage into the next WorldConfig

- **Decision**: Construction-only settings persist locally and feed the next
  boot payload, but they do not push live `set_slider` updates. Apply and
  Reset still persist them locally and show the restart-needed toast.
- **Why**: world shape, grass layout, species topology, founder boosts, and
  science/replay flags are consumed before the first tick. Rebuilding the
  world is the correct boundary for those changes.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/widgets/devpanel.ts → CONSTRUCTION_ONLY_SLIDERS`,
  `currentWorldConfig`, `TOAST_CONSTRUCTION`.

### Settings schema is versioned major/minor

- **Decision**: `evosim.settings.v2` stores `vMajor` and `vMinor`. A major
  mismatch, or any legacy blob, is discarded and replaced with defaults. A
  minor mismatch keeps the user's values and fills missing keys from
  `DEFAULTS`.
- **Why**: breaking changes need a deterministic reset, while additive changes
  should not wipe user preferences.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/settings.ts → SETTINGS_STORAGE_KEY`,
  `SCHEMA_MAJOR`, `SCHEMA_MINOR`.

### Runtime world size drives the shell's view geometry

- **Decision**: `worldSize` is a construction setting. The shell sizes its
  snapshot views from the boot-reported `grass_dim` rather than a hardcoded
  constant.
- **Why**: the worker owns the final constructed dimensions, so the shell
  should follow the boot payload instead of duplicating layout math.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/sim/bridge.ts → makeSlotLayout`,
  `app/web/src/main.ts → spawnSimWorker`.
- **Revisit when**: a feature needs the world to resize after boot.

### Settings panel uses a shared navigable surface; the top bar stays compact

- **Decision**: The Settings rail owns the left sub-nav categories. The top
  bar keeps play/pause, Restart, the hamburger rail toggle, and transient
  worker status / Retry controls. Auto-restart, Export / Import, and autosave
  status live in the General tab.
- **Why**: a shared settings surface is easier to scan than separate top-bar
  openers, and the top bar stays focused on run control and failure recovery.
- **Tradeoffs**: save and auto-run controls are deeper in the rail, but the
  rail remains the app configuration surface.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/main.ts → installTopBarButtons`,
  `installWorkerStatusUi`, `installMenuTab`; `app/web/index.html → #rail-tabs`;
  `app/web/src/rail/index.ts → RailTab`.

### Escape toggles Settings before inspector cleanup

- **Decision**: Escape opens Settings when the rail is closed or another tab is
  active, closes the rail when Settings is active, and is ignored while focus is
  inside text inputs or textareas.
- **Why**: Settings is the primary keyboard-accessible rail action, and text
  editing should keep normal Escape semantics.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/main.ts → main`.

### World saves live in IndexedDB; settings remain preferences

- **Decision**: Saved worlds use IndexedDB, while settings stay in
  localStorage.
- **Why**: world artifacts are larger and versioned separately from UI
  preferences.
- **Applies to**: `architecture/app-shell.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `app/web/src/storage/world-saves.ts → putAutosave`,
  `putNamedSave`, `latestWorldSave`; `app/web/src/main.ts → installMenuTab`,
  `loadWorldArtifact`.

### Worker recovery uses top-bar status and Retry

- **Decision**: Worker boot, recovery, and failure state appear as a top-bar
  status chip, with Retry shown only after repeated automatic recovery fails.
- **Why**: failure needs to be visible without opening the rail, but healthy
  state should keep the chrome quiet.
- **Tradeoffs**: the top bar can temporarily contain more than the normal run
  controls during failure handling. The extra controls disappear in healthy
  states.
- **Applies to**: `architecture/app-shell.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `app/web/src/main.ts → installWorkerStatusUi`,
  `recoverWorker`.

### Profiler activates from its rail tab, not a top-bar toggle

- **Decision**: Selecting the top-level Profiler rail tab turns profiler
  recording on. Navigating to any other rail tab turns it off.
- **Why**: the profiler is a major inspection surface, but it should still idle
  its polling when not visible.
- **Applies to**: `architecture/app-shell.md`, `architecture/profiler.md`.
- **Code anchors**: `app/web/src/rail/index.ts → RailTab`;
  `app/web/src/widgets/perf-panel.ts → setProfilerVisible`.

### NN editor lives in the Settings rail

- **Decision**: The NN editor stays in `#settings-nn-pane` and is not a
  top-level rail tab.
- **Why**: NN topology and mutation policy are configuration, not inspection.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/rail/nn-tab.ts → installNnTab`,
  `app/web/index.html → #settings-nn-pane`.

## How to use / See also

- [`../architecture/app-shell.md`](../architecture/app-shell.md) - the
  architecture doc this file constrains.
- [`index.md`](index.md) - decisions index and domain map.
- [`cross-cutting.md`](cross-cutting.md) - decisions that bind the shell and
  other subsystems.
- [`~/agent-docs/v1/rules/authoring-rules.md`](~/agent-docs/v1/rules/authoring-rules.md)
  - doc maintenance rules.
