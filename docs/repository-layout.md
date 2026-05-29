# Repository layout

One-line purpose for every non-trivial path. Skim this before grepping.

## Top level

| Path | Purpose |
|---|---|
| `Cargo.toml` | Single Rust crate at repo root. Builds both `cdylib` (wasm) and `rlib` (native tests). `threads` feature flag pulls in `rayon` + `wasm-bindgen-rayon`. |
| `Cargo.lock` | Committed lockfile. |
| `rust-toolchain.toml` | Pins stable `1.95.0` for native + non-threaded builds. The threaded wasm build needs nightly; invoked via `rustup run nightly wasm-pack ...`. |
| `clippy.toml` | Forbids `HashMap`/`HashSet` `iter*` in sim-critical files (determinism guard). |
| `.cargo/config.toml` | Compile + link flags for `wasm32-unknown-unknown`: atomics, bulk-memory, shared memory, `__heap_base` / TLS exports. Native builds unaffected. |
| `README.md` | Brief project intro and a pointer into `docs/`. |
| `src/` | Rust simulation crate. |
| `web/` | TypeScript + Vite shell. |
| `docs/` | This tree. |
| `docs-old/` | Tombstoned. References inside are broken on purpose; the user deletes this tree in a later pass. |
| `.github/workflows/` | CI: `cargo fmt/clippy/test --lib` (default + threads); `pnpm typecheck`; `wasm-pack build --release --features threads`. |

## `src/` — Rust simulation engine

| Path | Purpose |
|---|---|
| `src/lib.rs` | Crate root. Re-exports `wasm_api::*` and `init_thread_pool` (under `feature = "threads"`). Installs the panic hook in `_start`. |
| `src/constants.rs` | Every magic number used by the sim: world size, grass grid dim, NN topology, founder defaults, `MAX_POP_FOR_SIM`. |
| `src/wasm_api.rs` | `WorldHandle` — the sole wasm-bindgen surface. Owns `step_n`, `write_snapshot_to`, `set_slider`, inspector helpers, profile getters. Plus free functions `max_pop_for_sim()` and `rayon_current_num_threads()`. |
| `src/world/mod.rs` | `World` (SoA + grass + RNG + tick orchestration). `step()` runs the numbered tick phases. Owns `handle_births` (which contains the random cull back to `MAX_POP_FOR_SIM`). |
| `src/world/tick.rs` | Per-phase step bodies: `apply_movement_and_repulsion`, `graze`, `eat`, `energy_bookkeeping`, `collect_deaths`, `color_ema_update`. |
| `src/world/nn.rs` | NN forward pass — both sequential and rayon-parallel paths. Builds the 32-input vector, dispatches the chunked `Brain::forward`, decodes actions. |
| `src/world/nn_stats.rs` | Per-worker + per-tick atomic counters bracketing the NN sub-phases. Read post-tick by `world/mod.rs` and pushed into the profiler's top-level `nn` tree. |
| `src/world/proximity.rs` | The 8-sector creature + grass proximity NN inputs, plus the 4-wall proximity helper. Owns the `sector_lut` build. |
| `src/brain.rs` | `Brain` — a 32 → 48 → 24 → 5 pyramid with Leaky ReLU and SIMD forward (`wide::f32x8`). Per-layer He init. |
| `src/creature.rs` | `CreatureSoA` (every per-creature column) and the 3-variant `Action` enum (`Graze`/`Eat`/`Split`). |
| `src/grass.rs` | `GrassGrid` — 960×960 density Vec, logistic in-cell growth + cross-kernel propagation, per-row "any non-empty" bitset, atomic per-tick timers. |
| `src/grid.rs` | `SpatialGrid` — 5u cells, 240×240 = 57_600. Single shared structure for neighbour queries. |
| `src/profiler.rs` | The four-tree profiler (`tick`, `frame`, `nn`, `grass_step`). `Profiler::push_root_named`, `record_under_root`, `ensure_root`. JSON report shape. |
| `src/rng.rs` | `SimRng` — seedable xoshiro wrapper used everywhere a determinism guarantee is wanted. |

## `web/` — TypeScript shell

| Path | Purpose |
|---|---|
| `web/index.html` | Single page. Mounts the canvas, the right-rail panels, the dev-panel + settings overlays, and the profiler box. |
| `web/package.json` | `pnpm dev` / `pnpm build` / `pnpm typecheck` / `pnpm test:e2e`. Devdeps: `vite`, `typescript`, `@playwright/test`. |
| `web/vite.config.ts` | Pins port 47821. Sets COOP/COEP headers on dev + preview. `worker: { format: "es" }` so the rayon worker helper is bundled as an ES module. |
| `web/tsconfig.json` | Includes `ES2024.SharedMemory` lib for `Atomics.waitAsync` types. |
| `web/playwright.config.ts` | Boots Vite via `webServer`; reuses an existing server on `:47821`. |
| `web/public/_headers` | Cloudflare-Pages-style header file mirroring the dev COOP/COEP + caching for `/assets/*` and `/wasm/*`. |
| `web/wasm/` | wasm-pack output. Gitignored; regenerated every Rust change. Contains `evosim.js`, `evosim_bg.wasm`, `evosim.d.ts`. |
| `web/src/main.ts` | Boot. Spawns the sim worker, awaits `boot_ready`, asserts `max_pop_for_sim` parity, sets up the camera + render loop, wires the dev-panel + settings overlay + restart hotkey. |
| `web/src/sim-worker.ts` | The sim worker entry. Inits wasm + rayon, allocates the two SABs, runs the async `Atomics.waitAsync` tick loop, dispatches messages, writes snapshots. |
| `web/src/sim-bridge.ts` | Single source of truth for the main↔worker boundary. Discriminated unions for every `SimMessage` / `SimReply`, SAB layout constants, slot-offset helpers, and the `SimBridge` runtime class. |
| `web/src/render-gl.ts` | WebGL2 renderer. One instanced draw call for all creature bodies + highlight rings; a fullscreen quad for the R8 grass texture; a LINE_LOOP for the world frame. JS-side frustum cull. |
| `web/src/render.ts` | Camera math (no draw calls). `Camera`, `PX_PER_SIZE`, `makeCamera`, `worldToScreen`. |
| `web/src/camera.ts` | Pointer-driven pan / zoom controls bound to the canvas. |
| `web/src/perf.ts` | TS-side mirror of the profiler. Holds the `frame` tree; `span(name)` opens / closes a sample. The panel concatenates this with the Rust trees. |
| `web/src/settings.ts` | `localStorage`-backed user prefs (autoRun, targetTPS, grass opacity, etc.). |
| `web/src/themes.ts` | Theme palette map (charcoal / slate / light / vivid). `applyTheme(id)` writes CSS-var tokens onto `<html>`. v1.9.1. |
| `web/src/widgets/devpanel.ts` | Settings tab installer. Stage-then-apply for sim sliders + live-apply for run/display toggles. `currentSliderState()` + the construction-only ctor accessors feed the boot payload. |
| `web/src/widgets/perf-panel.ts` | Profiler bottom panel + 1 Hz polled tables (`frame` / `tick` / `nn` / `grass_step`). `setProfilerVisible()` is the single source of truth for visibility — Settings checkbox and the panel's ✕ both call it. |
| `web/src/widgets/worker-stats.ts` | NN-worker health panel. Installs into the Monitor tab; polls `nn_worker_stats_json` at ~750 ms. |
| `web/src/rail/index.ts` | Three-tab right-rail orchestrator (Inspector / Monitor / Settings). `installRail()` + `pollRail(rail, header, simBridge, ...)` from main's RAF. |
| `web/src/rail/inspector.ts` | Inspector tab. Click → `inspect_at` + tab switch + empty-state toggle. Per-frame refresh → `inspect_id`. SAB id-column fast-path. |
| `web/src/rail/monitor.ts` | Monitor tab installer. Wires `worker-stats.ts` into `#worker-stats-host`; pop graph paints itself via the rail poller. |
| `web/src/rail/stats.ts` | Population time-series sampler. Reads from `SnapshotHeader.tick / pop`; guard is "≥ 10 ticks since last sample". Paints to `#chart-pop` inside the Monitor tab. |
| `web/src/rail/highlight.ts` | Highlight-ring book-keeping. Inspector selection + transient highlights with TTL. |
| `web/src/toast.ts` | Transient-notice helper. Used by the Settings tab to surface "construction-only changes" after Apply / Reset. |
| `web/tests/README.md` | How to run the Playwright e2e suite. |
| `web/tests/e2e/sim-bridge.spec.ts` | Smoke tests for pause / TPS / slider / profile-toggle / restart — every one runs at `targetTPS = 1000` to catch the `Atomics.waitAsync(0)` regression class. |
| `web/tests/e2e/defaults-drift.spec.ts` | Wave D drift-guard. Asserts Rust `sliders_defaults_json()` agrees with `settings.ts` DEFAULTS for every shared slider. |

## See also

- [`overview.md`](overview.md) — system at a glance.
- [`architecture/simulation-core.md`](architecture/simulation-core.md) — sim
  internals these files implement.
- [`agent-context/maintaining-docs.md`](agent-context/maintaining-docs.md) —
  when to update this table.
