# Decisions — app shell

Decisions that bind the app shell only (settings panel, UI lifecycle, localStorage
schema, and the SAB-view binding that the shell owns). Cross-subsystem decisions
that also bind the shell live in [`cross-cutting.md`](cross-cutting.md).

---

### Settings tab is stage-then-apply for sim sliders; live-apply for run/display

- **Decision**: Sim sliders (Energy / Grass / Eat / Lifecycle /
  Curriculum groups) stage edits and reconcile via the Settings tab's
  Apply / Cancel / Reset footer. Run + Display widgets (`autoRun`,
  `showProfiler`, `showPopGraph`, `showGrass`, `grassOpacity`) skip
  staging and apply immediately.
- **Why**: Sim sliders perturb the running world; users want to set up
  a batch of changes (e.g. raise mut rate AND lower energy max) and
  commit them atomically. Display toggles are page-side render flips
  with no sim cost — staging them would block live preview.
- **Tradeoffs**: Two interaction tiers means more code than a uniform
  rule. The carve-out is small and load-bearing for both UX goals.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/widgets/devpanel.ts → makeStagedSlider`,
  `makeLiveSlider`, `CONSTRUCTION_ONLY_SLIDERS`.

### Construction-only sliders are committed via `set_slider` but only shape the next world

- **Decision**: `founder_count`, `energy_max`,
  `grass_initial_seed_count`, and `full_grass_on_init` ride the same
  staged path as live-tunable sliders. Apply persists + pushes them to
  the worker's `DevSliders`, but the *current* world keeps whatever
  values it spawned with — a manual restart is needed to see them.
  The Settings tab fires a toast ("Some changes only take effect on
  new simulations.") whenever any construction-only knob commits.
- **Why**: Mid-run construction changes can't safely re-shape the
  running world (e.g. founder_count is meaningless after pop ≠
  founder_count). Surfacing the constraint via the toast keeps the
  user aware without forcing an auto-restart.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/widgets/devpanel.ts →
  CONSTRUCTION_ONLY_SLIDERS`, `TOAST_CONSTRUCTION`,
  `applyAll`, `resetAll`.

### Settings schema is `major.minor`; major resets, minor merges

- **Decision**: The persisted settings blob (key `evosim.settings.v2`) carries
  a `major.minor` version. On load: a **major** mismatch (or any legacy v1
  blob) is **discarded** and defaults are used; a **minor** mismatch
  (additive-only — new keys shipped) keeps the user's values and merges missing
  keys from `DEFAULTS` (`{...DEFAULTS, ...stored}`). v2.0 bumps the major
  version (it removes / renames sliders and re-lays-out the control SAB); every
  ordinary settings change thereafter bumps minor.
- **Why**: v2.0 renames/removes sliders and changes the control-SAB layout, so
  old localStorage can't be trusted — a version stamp makes the reset
  deterministic. And every future settings change gets a cheap non-destructive
  migration (minor bump) instead of silently carrying stale or missing keys.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/settings.ts → SCHEMA_MAJOR` / `SCHEMA_MINOR`, the
  load merge + unknown-key filter.

### Runtime `world_size` ⇒ computed-dims-equality SAB view binding

- **Decision**: `world_size` (default 9600u) is a **runtime** construction
  setting, not a compile-time constant. Consequently grass-grid dims
  (`grass_dim = round(world_size / GRASS_CELL_SIZE)`), the snapshot grass
  region, and the biome region are all **derived from settings at boot**. The
  app shell sizes every view off the **boot-reported** `grass_dim`: `boot_ready`
  carries the runtime `grass_dim`, and the shell builds `makeSlotLayout` from
  it — never a hardcoded constant. `MAX_POP_FOR_SIM` / `CREATURE_STRIDE`
  stay constant-asserted (the creature region is world-size-independent). The
  cross-language SAB-safety invariant (grass region = biome region = `grass_dim²`
  bytes; a Rust `debug_assert` checks byte-equality) lives in
  `decisions/cross-cutting.md`; this entry records only the app-shell SAB-view
  binding side.
- **Why**: The user explicitly wants worlds editable across a wide range from one
  binary. Sizing both grass and biome views off the same boot-reported `grass_dim`
  keeps the shell's layout object self-consistent without a second constant to drift.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/main.ts → makeSlotLayout`,
  `boot_ready.grass_dim`.
- **Revisit when**: a feature needs the world to resize *after* boot (today
  nothing resizes after boot — restart rebuilds the world).

### Settings panel = one navigable surface with left sub-nav; 4-control top bar

- **Decision**: The settings panel uses a single navigable surface with a
  vertical left sub-nav of categories (Energy, Grass, Lifecycle, World,
  Equilibrium, Display, Profiler, NN). The top bar is trimmed to exactly
  four controls: play/pause, Restart, Auto-restart, ⚙ rail toggle.
  Removed from the top bar: NN opener, Inspector opener, perf-toggle opener.
  Inspector is still accessible via creature click (auto-opens the rail to
  the inspector tab). NN editor and Profiler are settings categories.
- **Why**: Fewer top-bar openers reduces discoverable surface area — one ⚙
  opens one panel, and all configuration lives in one navigable place. The
  old three-tab rail plus three top-bar openers duplicated surface area and
  made the panel feel like four separate tools. A single navigable surface
  makes the settings a coherent place to configure and inspect the sim.
- **Tradeoffs**: Inspector is no longer one click from the top bar; creature
  click is the only entry point. Considered a worthy tradeoff since inspector
  is creature-contextual — it has no meaningful empty state.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/main.ts → installTopBarButtons`,
  `app/web/index.html → #rail-tabs`, `app/web/src/rail/index.ts → RailTab`.

### Profiler activated by category-select, not by a top-bar toggle

- **Decision**: Selecting the Profiler sub-nav category in the settings panel
  calls `setSetting("showProfiler", true)` + `setProfilerVisible(true)`.
  Navigating to any other category calls both with `false`. `showProfiler`
  remains the single source of truth for recording state.
- **Why**: The old top-bar perf-toggle was the only path that set
  `showProfiler`. With the profiler moved into Settings, the profiler
  became un-enableable (always showed "Enable to record" placeholder). Coupling
  category-select → `showProfiler` restores the enable path without adding a
  separate checkbox inside the profiler pane. The Rust backend stays always-on;
  only the TS poll is gated.
- **Applies to**: `architecture/app-shell.md`, `architecture/profiler.md`.
- **Code anchors**: `app/web/src/widgets/devpanel.ts → installDevPanel` (sub-nav
  wiring block), `app/web/src/widgets/perf-panel.ts → setProfilerVisible`.

### NN editor folded into Settings "NN" category

- **Decision**: The NN editor (topology + mutation-bucket editors) is a sub-nav
  category inside the settings panel (`#settings-nn-pane → #nn-tab-host`),
  not a top-level rail tab.
- **Why**: The NN editor is configuration, not inspection. Folding it into the
  settings panel makes it consistent with every other configurable surface and
  removes the three-tab rail that made the UI feel multi-paned.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/rail/nn-tab.ts → installNnTab`,
  `app/web/index.html → #settings-nn-pane`.

### Stage-then-apply semantics preserved across Settings restructure

- **Decision**: The stage-then-apply / live-vs-staged carve-out and
  Apply / Cancel / Reset semantics are unchanged by the settings panel
  overhaul. Only the navigation layout changed (left sub-nav categories
  replaced the single flat list).
- **Why**: The two-tier interaction model is correct — staging sim changes
  and applying display changes live are independently valuable properties that
  do not depend on how categories are navigated. Preserving the semantics
  avoids re-litigating a settled decision.
- **Applies to**: `architecture/app-shell.md`.
- **Code anchors**: `app/web/src/widgets/devpanel.ts → makeStagedSlider`,
  `makeLiveSlider`, `applyAll`, `cancelAll`, `resetAll`.

## How to use / See also

- [`../architecture/app-shell.md`](../architecture/app-shell.md) — the architecture doc this file constrains.
- [`index.md`](index.md) — decisions index and domain map.
- [`cross-cutting.md`](cross-cutting.md) — decisions that bind the shell AND other subsystems
  (e.g. the full computed-dims-equality SAB safety model, `MAX_POP_FOR_SIM` duplication,
  slider defaults drift guard).
- [`~/agent-docs/v1/rules/authoring-rules.md`](~/agent-docs/v1/rules/authoring-rules.md) — doc maintenance rules.
