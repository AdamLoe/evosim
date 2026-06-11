# Repository layout

Brief purpose for every non-trivial path, ideally one line. Skim this
before grepping.

## Top level

| Path | Purpose |
|---|---|
| `app/Cargo.toml` | Workspace manifest. Defines the `evosim` crate and its `cdylib`/`rlib` targets; `threads` feature flag pulls in `rayon` + `wasm-bindgen-rayon`. |
| `app/Cargo.lock` | Committed lockfile. |
| `app/rust-toolchain.toml` | Pins stable `1.95.0` for native + non-threaded builds. The threaded wasm build needs nightly; invoked via `rustup run nightly wasm-pack ...`. |
| `app/clippy.toml` | Forbids `HashMap`/`HashSet` `iter*` in sim-critical files (determinism guard). |
| `app/.cargo/config.toml` | Compile + link flags for `wasm32-unknown-unknown`: atomics, bulk-memory, shared memory, `__heap_base` / TLS exports. Native builds unaffected. |
| `README.md` | Brief project intro and a pointer into `docs/`. |
| `app/` | Rust workspace root: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml`, `.cargo/config.toml`, `crates/`, `web/`. |
| `app/web/` | TypeScript + Vite shell. |
| `docs/` | This tree. App bindings + ownership data for the global agent-docs kit live in `docs/_meta/` (`manifest.md`, `ownership.json`). |
| `.github/workflows/` | CI: `cargo fmt/clippy/test --lib` (default + threads); `pnpm typecheck`; `wasm-pack build --release --features threads`. |

## `app/crates/evosim/` — Rust simulation engine

| Path | Purpose |
|---|---|
| `app/crates/evosim/src/lib.rs` | Crate root. Re-exports `wasm_api::*` and `init_thread_pool` (under `feature = "threads"`). Installs the panic hook in `_start`. |
| `app/crates/evosim/src/constants.rs` | Every magic number used by the sim: `WORLD_SIZE_DEFAULT`/`WRAP_WORLD_DEFAULT`, `GRASS_CELL_SIZE`/`HASH_CELL`, NN topology + `MAX_NN_INPUTS`, biome enum + penalty knobs, species/mating defaults, `MAX_POP_FOR_SIM`. Owns `WorldDims::from_world_size` (runtime `grass_dim`/`grass_cell_count`/`hash_dim`). |
| `app/crates/evosim/src/wasm_api/mod.rs` | `WorldHandle` — the sole wasm-bindgen surface. Owns `step_n`, `write_snapshot` (writes snapshot bytes into wasm linear memory; TS publishes the slot), `set_slider`, `SLIDER_NAMES`, the genome→color + render-packing helpers (`genome_color_u32`, `pack_render_u32`), the per-slot grass/biome window layout, the polled `species_table_json` report, inspector + profile getters. Plus free functions `max_pop_for_sim()` and `rayon_current_num_threads()`. |
| `app/crates/evosim/src/control_sab.rs` | Canonical control-SAB byte layout (every `CTRL_*` slot + byte-buffer offset). Code-gen source for `app/web/src/generated/control-sab.ts`; the `bindings_in_sync` test guards drift. |
| `app/crates/evosim/src/bin/gen_bindings.rs` | `cargo run --bin gen-bindings` — regenerates the TS mirrors (`control-sab.ts`, `slider-ids.ts`) from the Rust constants. |
| `app/crates/evosim/src/world/mod.rs` | `World` (SoA + grass + biome grid + species registry + RNG + tick orchestration). `step()` runs the numbered tick phases. Owns `handle_births` (asexual split + random cull back to `MAX_POP_FOR_SIM`), `handle_mating` (species sexual mate), the N-anchor species seeding, `movement_penalty_for` / `biome_at`. |
| `app/crates/evosim/src/world/tick.rs` | Per-phase step bodies: `apply_movement_and_repulsion` (biome-penalty modulated), `graze`, `attack` (Eat→Attack rename, same-species gate in species mode), `energy_bookkeeping`, `collect_deaths`, `flash_decay` (ring-flash countdown). |
| `app/crates/evosim/src/world/nn.rs` | NN forward pass — both sequential and rayon-parallel paths. `NnInputLayout`/`NnInputGroup` (composable settings-derived input layout, `for_settings(wrap, species)`), `build_nn_input`, `BiomeSampler`, `ActionGate`/`decode_action`/`is_valid_action` (mode-dependent action[2] gate), chunked `Brain::forward` dispatch. |
| `app/crates/evosim/src/world/biome.rs` | `generate_biome_grid` — SplitMix64 blob generator seeded from `world_seed` (a few large water/desert blobs over Plains), `biome_from_u8`. |
| `app/crates/evosim/src/world/species.rs` | `Species` / `SpeciesRegistry` (dynamic id→color/name registry), the hand-spread 10-hue `SPECIES_PALETTE`. |
| `app/crates/evosim/src/world/nn_stats.rs` | Per-worker + per-tick atomic counters bracketing the NN sub-phases. Read post-tick by `world/mod.rs` and pushed into the profiler's top-level `nn` tree. |
| `app/crates/evosim/src/world/proximity.rs` | The creature + grass proximity NN inputs (8-sector single-pool / 16-sector species), the 4-wall proximity helper, the wrap-aware starburst scan. Owns the `LUT_RADIUS = 4` sector LUT build. |
| `app/crates/evosim/src/brain/mod.rs` | `Brain` — Leaky ReLU pyramid (legacy `32 → 48 → 24 → 5`) with SIMD forward (`wide::f32x8`). `NnTopology` carries a **runtime** `input_width` field; per-layer He init. `child_from_with_sigma` (asexual) + `child_from_crossover_with_sigma` (sexual) + `founder_spread_with_sigma`. Module dir includes a `tests/` subdir. |
| `app/crates/evosim/src/creature.rs` | `CreatureSoA` (every per-creature column, incl. `genome` / `flash_*` / `species_id` / `mating_cooldown`), the 6-trait body `Genome` (+ `crossed` / `canonical_for_biome`), the `FlashTag` enum, and the 3-variant `Action` enum (`Graze`/`Attack`/`Split`). |
| `app/crates/evosim/src/grass/mod.rs` | `GrassGrid` — runtime-sized density Vec (`grass_dim²`, 5u cells), biome-capacity-scaled logistic in-cell growth + scatter propagation with retained blur test path, 32×32 active-tile frontier, per-row "any non-empty" bitset, `GrassPyramid` clipmap window extraction, atomic per-tick timers. Module dir includes a `tests/` subdir. |
| `app/crates/evosim/src/grid.rs` | `SpatialGrid` — 10u cells (`HASH_CELL`), runtime `hash_dim²` (960² at default), wrap-aware queries. Single shared structure for neighbour queries. |
| `app/crates/evosim/src/profiler.rs` | The four-tree profiler (`tick`, `frame`, `nn`, `grass_step`). `Profiler::push_root_named`, `record_under_root`, `ensure_root`. JSON report shape. |
| `app/crates/evosim/src/rng.rs` | `SimRng` — seedable xoshiro wrapper used everywhere a determinism guarantee is wanted; `from_u64` seeds the dedicated species-seeding stream. |
| `app/crates/evosim/src/*_tests.rs`, `app/crates/evosim/src/world/*_tests.rs` | Unit-test modules: `brain_width_tests`, `grass_v201_tests` (active-tile equivalence for the grass rewrite), `wasm_dims_tests`, `wasm_snapshot_tests`; `biome_tests`, `genome_tests`, `grass_biome_tests`, `mating_tests`, `mating_grid_regression_tests`, `seeding_tests`, `wrap_tests`. |

## `app/web/` — TypeScript shell

| Path | Purpose |
|---|---|
| `app/web/index.html` | Single page. Mounts the canvas, the right-rail panels, the dev-panel + settings overlays, and the profiler box. |
| `app/web/package.json` | `pnpm dev` / `pnpm build` / `pnpm typecheck` / `pnpm test:e2e`. Devdeps: `vite`, `typescript`, `@playwright/test`. |
| `app/web/vite.config.ts` | Pins port 47821. Sets COOP/COEP headers on dev + preview. `worker: { format: "es" }` so the rayon worker helper is bundled as an ES module. |
| `app/web/tsconfig.json` | Includes `ES2024.SharedMemory` lib for `Atomics.waitAsync` types. |
| `app/web/playwright.config.ts` | Boots Vite via `webServer`; reuses an existing server on `:47821`. |
| `app/web/public/_headers` | Cloudflare-Pages-style header file mirroring the dev COOP/COEP + caching for `/assets/*` and `/wasm/*`. |
| `app/web/wasm/` | wasm-pack output. Gitignored; regenerated every Rust change. Contains `evosim.js`, `evosim_bg.wasm`, `evosim.d.ts`. |
| `app/web/src/main.ts` | Boot. Spawns the sim worker, awaits `boot_ready`, asserts `max_pop_for_sim` parity, sets up the camera + render loop, wires the dev-panel + settings overlay + restart hotkey. |
| `app/web/src/sim/worker.ts` | The sim worker entry. Inits wasm + rayon, allocates the control SAB (snapshot + biome regions live in wasm memory), runs the synchronous `Atomics.wait` tick loop, reads the control SAB, writes snapshots + the polled species table. |
| `app/web/src/sim/bridge.ts` | Single source of truth for the main↔worker boundary. Discriminated unions for every `SimMessage` / `SimReply`, SAB layout constants, snapshot-stride/lane decode, slot-offset helpers, and the `SimBridge` runtime class (incl. `latestSpeciesTable()`). |
| `app/web/src/render/gl.ts` | WebGL2 renderer. One instanced draw call for all creature bodies + a ring-flash pass + highlight rings; a fullscreen quad for the R8 grass texture and a second R8 biome-id quad drawn under it; a LINE_LOOP for the world frame. Wrap-aware ghost-copy cull + survey-zoom 1px points. JS-side frustum cull + genome/species color decode. |
| `app/web/src/render/scene.ts` | Camera math (no draw calls). `Camera`, `PX_PER_SIZE`, `MIN_ZOOM = 0.04` / `MAX_ZOOM`, `makeCamera`, `clampCamera`, `worldToScreen`. |
| `app/web/src/render/camera.ts` | Pointer-driven pan / zoom controls bound to the canvas. |
| `app/web/src/perf.ts` | TS-side mirror of the profiler. Holds the `frame` tree; `span(name)` opens / closes a sample. The panel concatenates this with the Rust trees. |
| `app/web/src/themes.ts` | The `THEMES` palette map + `applyTheme(id)`; writes CSS custom properties onto `<html>`. |
| `app/web/src/settings.ts` | `localStorage`-backed user prefs (autoRun, targetTPS, grass opacity, world-shape + species construction knobs, NN topology + mutation buckets, theme, etc.) under key `evosim.settings.v2`, with the `major.minor` schema migration. |
| `app/web/src/generated/control-sab.ts`, `app/web/src/generated/slider-ids.ts` | Code-generated TS mirrors of the control-SAB layout + `SLIDER_NAMES` (committed; regenerated by `cargo run --bin gen-bindings`). |
| `app/web/src/widgets/devpanel.ts` | Settings tab installer. Stage-then-apply for sim sliders + live-apply for run/display toggles; the World + Species & mating sections + `refreshSpeciesGating()`. `currentSliderState()` + the construction-only ctor accessors feed the boot payload. |
| `app/web/src/widgets/perf-panel.ts` | Profiler bottom panel (`#perf-box`) + 1 Hz polled tables (`frame` / `tick` / `nn` / `grass_step`), the FPS/TPS chart, the **population chart** (`#chart-pop`, per-species lines in species mode via the polled species table), and the CPU monitor (hosts `worker-stats.ts`). `setProfilerVisible()` is the single source of truth for visibility. |
| `app/web/src/widgets/worker-stats.ts` | NN-worker health table. Installed by `perf-panel.ts` into the CPU-monitor section; polls `nn_worker_stats_json` at ~750 ms. |
| `app/web/src/rail/index.ts` | Three-tab right-rail orchestrator (Settings / NN / Inspector; default Settings, rail starts collapsed). `installRail()` + `pollRail(rail, header, simBridge, ...)` from main's RAF. |
| `app/web/src/rail/inspector.ts` | Inspector tab. Click → `inspect_at` + tab switch + empty-state toggle. Per-frame refresh → `inspect_id`. SAB id-column fast-path. Shows genome traits + (species mode) species name/color/breadcrumb. |
| `app/web/src/rail/nn-tab.ts` | NN tab (`#nn-tab-host`). Stage-then-apply topology editor (Apply respawns the worker) + live-apply 8-bucket mutation-policy table + per-layer perf log. |
| `app/web/src/rail/highlight.ts` | Highlight-ring book-keeping. Inspector selection + transient highlights with TTL. |
| `app/web/src/toast.ts` | Transient-notice helper. Used by the Settings tab to surface "construction-only changes" after Apply / Reset. |
| `app/web/tests/README.md` | How to run the Playwright e2e suite. |
| `app/web/tests/e2e/sim-bridge.spec.ts` | Smoke tests for pause / TPS / slider / profile-toggle / restart — every one runs at `targetTPS = 1000` to catch the futex-pacing regression class. |
| `app/web/tests/e2e/sab-control.spec.ts` | Regression coverage for the v1.10 all-SAB control transport. |
| `app/web/tests/e2e/defaults-drift.spec.ts` | Wave D drift-guard. Asserts Rust `sliders_defaults_json()` agrees with `settings.ts` DEFAULTS for every shared slider. |

## See also

- [`overview.md`](overview.md) — system at a glance.
- [`architecture/simulation-core.md`](architecture/simulation-core.md) — sim
  internals these files implement.
- [`agent-context/maintaining-docs.md`](agent-context/maintaining-docs.md) —
  when to update this table.
