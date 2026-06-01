# Simulation core

The Rust world model: SoA, grass field, RNG, NN, the tick step body, and
the bounds invariant.

## What it is

A single-crate Rust simulation compiled to wasm. One `World` owns every
piece of simulation state: a `CreatureSoA` (each per-creature attribute
held as its own `Vec`), a `GrassGrid` (runtime-sized density field), a
`SpatialGrid` (10u cells for neighbour queries), a `SimRng`, the live
`DevSliders`, a `Profiler`, an `NnStats` block, and a small set of
per-tick scratch buffers promoted to long-lived fields. `World::step()`
runs one tick by sequentially executing the numbered phases below.

### Runtime world sizing + wrap (v2.0 Wave 1a)

The world is **runtime-sized**. `world_size` (default **9600u**) and
`wrap_world` (default **true** = toroidal) are construction settings on
`DevSliders`. At construction `WorldDims::from_world_size(world_size,
wrap_world)` computes and caches the per-axis dims used everywhere:

- `grass_dim = round(world_size / GRASS_CELL_SIZE)` — **5u** grass cells →
  **1920** cells/axis at 9600u; `grass_cell_count = grass_dim²` = 3_686_400.
- `hash_dim = ceil(world_size / HASH_CELL)` — **10u** spatial-hash cells →
  **960** cells/axis at 9600u. (Bumped 2.5u→10u: the world grew 64× in area at
  the same creature counts, so coarser cells keep the per-tick `hash_dim²`
  rebuild cheap. Flagged for later profiler measurement.)

`World` carries `dims: WorldDims` and a numeric `world_seed: u32` (separate from
the string/XxHash64 RNG seed; Wave 1b uses it for biome gen). Every
bounds/clamp/wrap/spawn site reads `self.dims` / `World::world_size()` instead of
a compile-time constant.

**Wrap-awareness** (gated on `dims.wrap_world`): position/movement step,
`SpatialGrid` rebuild + neighbor queries (toroidal cell-index fold), creature &
grass proximity sector distance + the starburst/AABB scans (toroidal
minimum-image displacement), split-child placement, and eat reach all wrap across
the seam when on. When off they keep the walled clamp-to-`[ri, world_size-ri]`
behavior and the 4 wall-proximity NN inputs are present. The legacy test ctor
`World::new` pins a walled 1200u world.

## What it owns

- The `World` struct, `CreatureSoA`, `GrassGrid`, `SpatialGrid`, `SimRng`,
  `Brain`, `DevSliders`, `Action`.
- The NN input layout — a composable, settings-derived descriptor
  (`NnInputLayout` in `src/world/nn.rs`) that, given which input groups are
  active, computes each group's slot offset and the total **runtime input
  width**. Width is a multiple of 8 in `[8, MAX_NN_INPUTS = 48]`; the legacy
  layout (all groups active) reproduces the historical width-32 semantic order
  (self/memory, wall, sector creature, sector grass, reserved, current-cell
  density, bias, trailing pad).
- The brain topology (legacy `32 → 48 → 24 → 5`) with per-layer Leaky ReLU and
  SIMD forward (`wide::f32x8`). **Input width is a runtime field of
  `NnTopology`** (alongside the runtime hidden-layer list); it feeds the first
  matmul as `fan_in`.
- The tick step order (see below). Every named phase has a `tick.<name>`
  profile span; `tick.nn` and `tick.grass_step` are leaves.
- The grass mechanic: logistic in-cell growth + cross-kernel propagation,
  grazed bite-by-bite at `GRASS_MAX / grass_bites_per_block` density per
  bite, yielding `grass_energy_per_bite` energy per bite.
- The eat mechanic: per-bite transfer of
  `eat_bite_fraction * prey.energy * (1 - prey.armor)` (armor is 0 today
  — the multiplier is retained for forward compatibility).
- The `MAX_POP_FOR_SIM` invariant. `World::handle_births` includes a
  random cull pass that draws from `self.rng` after every birth phase so
  `pop ≤ MAX_POP_FOR_SIM` at every tick boundary, deterministically.
- The deterministic chunking scheme for the parallel NN pass:
  `chunk_count = clamp(pop / 32, MIN_CHUNKS=4, min(MAX_CHUNKS=16, workers))`.

## What it does NOT own

- **Snapshot SAB layout, message protocol** — owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md). Sim
  core exposes `WorldHandle::write_snapshot_to(creatures, grass, stats)`;
  the byte layout it writes is the protocol's contract.
- **Worker spawn / tick loop / pacing** — owned by
  [`worker-runtime.md`](worker-runtime.md). Sim core is single-threaded
  from its own perspective; the rayon pool is initialized by the worker.
- **Rendering, frustum cull, camera** — owned by
  [`render-pipeline.md`](render-pipeline.md). The renderer never calls
  into wasm.
- **Profiler tree shape, no-rollup rule, span names** — owned by
  [`profiler.md`](profiler.md). Sim core opens spans via
  `profile_span!(&self.profile, "tick.X")`; their tree-position is the
  profiler doc's concern.

## How it interacts with neighbours

The `WorldHandle` wasm-bindgen surface is small by design. The sim worker
holds the only `WorldHandle` and calls into it via:

```text
WorldHandle::step_n(n: u32) -> bool                   // one or more ticks
WorldHandle::write_snapshot_to(c, g, s)               // SAB byte dump
WorldHandle::set_slider(name: &str, value: f32)       // sole mutation entry
WorldHandle::sliders_defaults_json() -> String        // canonical DevSliders defaults
WorldHandle::creature_at(wx, wy, tol) -> Option<f64>  // inspector click
WorldHandle::creature_idx_by_id(id: f64) -> Option<u32>
WorldHandle::creature_inspect_json(idx: u32) -> Option<String>
WorldHandle::profile_enable(on: bool)                 // v1.9.1: worker calls once with true at boot
WorldHandle::profile_clear()                          // v1.9.1: zero ring buffers, keep topology
WorldHandle::profile_report_json() -> String
WorldHandle::nn_worker_stats_json() -> String
WorldHandle::tps / jank_count / tick / population / world_ended / world_size
// Plus free functions:
max_pop_for_sim() -> u32
rayon_current_num_threads() -> u32
```

The 22 per-typed `set_*` setters are gone. `set_slider(name, value)` is
the sole mutation entry point. Bools (`wrap_world`) ride the same path as `0|1`.
**v2.0 Wave 1a slider changes:** the four `_reserved_curriculum_*` slots and
`full_grass_on_init` are **dropped** from `SLIDER_NAMES`; the construction
settings `world_size` (18), `world_seed` (19), `wrap_world` (20) are **added**;
`max_population` moves to 17 and `SLIDER_BUCKET_BASE` shifts 23→21
(`SLIDER_COUNT = 45`). `full_grass_on_init` remains a `DevSliders` field carried
through the boot construction call (`newWithFounderCount`), just not a live
slider. Regenerate `web/src/generated/slider-ids.ts` via `cargo run --bin
gen-bindings` (the `bindings_in_sync` test guards it).

## Tick step order

```text
fn step(&mut self) -> bool {
    let _tick = profile_span!(&self.profile, "tick");

    // Thin path: once world_ended (or pop==0) we only run grass_step +
    // bitset rebuild + tick++, then return false. Lets the canvas keep
    // filling in the background while the UI shows the world-end popup.

    // 1. tick.grid.rebuild      — SpatialGrid::rebuild
    // 2. tick.nn                — chunked Brain::forward (LEAF in tick tree)
    // 3. tick.movement          — apply_movement_and_repulsion
    // 4. tick.graze             — multi-cell density consume (sequential)
    // 5. tick.eat_scavenge      — per-bite energy transfer
    // 6. tick.grass_step        — grass propagation + bitset rebuild (LEAF)
    // 7. tick.energy_bookkeeping
    // 8. tick.collect_deaths
    // 9. tick.handle_births     — split + RANDOM CULL back to MAX_POP_FOR_SIM
    //10. tick.color_ema         — per-creature RGB drift toward action argmax
    //11. tick.bookkeeping_tail  — last_action promote, tick++, world-end check
}
```

`tick.nn` and `tick.grass_step` measure the main sim worker's wall-clock
wait for the rayon dispatch only. The breakdown lives in the sibling
top-level `nn` and `grass_step` trees — see
[`profiler.md`](profiler.md).

## NN topology (runtime input width → hidden layers → 5)

**Input width is a runtime field of `NnTopology`** (`src/brain.rs`), mirroring
how the hidden-layer list is runtime-sized. The active width is computed by the
composable `NnInputLayout` descriptor (`src/world/nn.rs`) from the set of active
input groups, then fed to the first matmul as `fan_in`. The width must be a
multiple of 8 (SIMD chunks) and `≤ MAX_NN_INPUTS = 48`; these are **runtime**
checks in `NnTopology::with_input_width`, which replaced the old compile-time
`NN_INPUTS == 32` hard assert. `build_nn_input` writes into a
`MAX_NN_INPUTS`-sized buffer; only the active `width()` lanes are populated.

The **active layout** is driven by construction settings
(`NnInputLayout::for_settings(wrap_world)`, v2.0 Wave 1a): the
`ReservedPredator` group is **dropped**, and `WallProximity` (4 slots) is present
**only when `wrap_world == false`**. Both default layouts pad to the SIMD-aligned
32, so the default topology input width (`NN_INPUTS = 32`) is unchanged.

Default (wrap **on**) — real width 26 → padded 32:

| Slot | Group / inputs |
|---|---|
| `[0..8)` | `SelfMemory`: hunger, age_frac, prev_vx, prev_vy, is_last_graze, is_last_eat, ticks_since_split_norm, cooldown_ready |
| `[8..16)` | `CreatureSectors` × 8 world-aligned sectors (range `PROXIMITY_RANGE = 20u`) |
| `[16..24)` | `GrassSectors` × 8 sectors (range `GRASS_PROXIMITY_RANGE = 20u`) |
| `[24]` | `CurrGrass`: curr_grass_density (under the body) |
| `[25]` | `Bias` — `1.0` (bias-learning constant) |
| `[26..32)` | trailing pad (0.0) — SIMD alignment to 32 |

Walled (wrap **off**) — real width 30 → padded 32 — inserts
`WallProximity` N/S/E/W (range `WALL_PROXIMITY_RANGE = 50u`) at `[8..12)`,
shifting `CreatureSectors`→`[12..20)`, `GrassSectors`→`[20..28)`,
`CurrGrass`→`[28]`, `Bias`→`[29]`, pad `[30..32)`.

`GRASS_PROXIMITY_RANGE` was raised **8u → 20u** so each of the 8 grass sectors
spans `ceil(20/5) = 4` grass cells (≥3) at the 5u cell; the sector LUT radius
`LUT_RADIUS` is recomputed to **4** (was 16 at the old 1.25u cell). The
`NnInputLayout::legacy()` width-32 anchor (all 7 groups, ReservedPredator at
`[28]`) survives as a **test-only** calibration baseline. Group offsets + total
width are **recomputed by the descriptor** when the active group set changes — no
hand-maintained slot constants in the build path. The legacy `weight_count` is
unchanged: `32*48 + 48*24 + 24*5 = 2808`, and old brains are discarded (no
save/load).

Outputs: `out[0] = vx`, `out[1] = vy`, `out[2..5]` = action logits for
`{Graze=0, Eat=1, Split=2}`. Hidden layers use Leaky ReLU (slope 0.01).
Per-layer founder init follows He-uniform `r = sqrt(6 / fan_in)`, computed at
runtime per matmul (the first matmul's `fan_in` is the runtime input width).
Founder weights are pure uniform-random — no hardwiring.

## Code anchors

- `src/world/mod.rs` → `World`, `DevSliders`, `World::step`,
  `World::handle_births`, `World::new_with_sliders`.
- `src/world/tick.rs` → `apply_movement_and_repulsion`, `graze`, `eat`,
  `energy_bookkeeping`, `collect_deaths`, `color_ema_update`.
- `src/world/nn.rs` → `nn_forward_all_chunks`, `build_nn_input`,
  `NnInputLayout` / `NnInputGroup` (composable input-layout descriptor),
  `decode_action`, `chunk_ranges`, `dynamic_chunks`, `PickTimings`.
- `src/world/nn_stats.rs` → `NnStats`.
- `src/world/proximity.rs` → `LUT_RADIUS = 4` / `LUT_DIM`, sector LUT build,
  wall + creature + grass proximity helpers (all wrap-aware via the grid/grass
  `dims`).
- `src/brain.rs` → `Brain`, `Brain::founder`, `Brain::forward`,
  `Brain::child_from`, `NnTopology` (runtime `input_width` field +
  `with_input_width`), the `lrelu` helper, the `NN_OUTPUTS == 5` compile-assert
  (the old `NN_INPUTS == 32` assert is gone — input width is now a runtime
  check). Width tests: `src/brain_width_tests.rs`.
- `src/creature.rs` → `CreatureSoA`, `Action` (3 variants), `Action::ALL`.
- `src/grass.rs` → `GrassGrid`, `compute_propagation`, `bilinear_sample`,
  `consume`, `for_each_cell_overlapping_circle`, `rebuild_row_bitset`.
- `src/grid.rs` → `SpatialGrid` (carries `dims: WorldDims`), `cell_of`
  (instance method: clamp walled / `rem_euclid` toroidal), `rebuild`,
  `for_each_in_radius` (wrap-aware cell fold).
- `src/constants.rs` → `WorldDims` (runtime `world_size` / `grass_dim` /
  `grass_cell_count` / `hash_dim`; replaces the old compile-time `WORLD_SIZE` /
  `GRASS_GRID_DIM` / `GRASS_CELL_COUNT` / `HASH_DIM` constants),
  `WORLD_SIZE_DEFAULT = 9600`, `WRAP_WORLD_DEFAULT = true`,
  `GRASS_CELL_SIZE = 5.0`, `HASH_CELL = 10.0`,
  `GRASS_PROXIMITY_RANGE = 20.0`, `Biome` enum `{Plains=0, Water=1, Desert=2}`
  (u8; Wave 1b populates it),
  `MAX_POP_FOR_SIM = 32_000`,
  `MAX_POPULATION_DEFAULT = 8_000` (TS lowers to 2000 on first boot when
  `navigator.hardwareConcurrency < 8`),
  `NN_INPUTS = 32` (legacy/default input width), `MAX_NN_INPUTS = 48`
  (runtime input-width ceiling), `NN_OUTPUTS = 5`,
  `MIN_CHUNKS = 4`, `MAX_CHUNKS = 16`, `STARTING_POP_DEFAULT = 32`,
  `CREATURE_SIZE = 1.0`, `START_ENERGY_DEFAULT = 200.0`,
  `MAX_AGE_DEFAULT = 5000`, `SPLIT_THRESHOLD_DEFAULT = 99.0`,
  `SPLIT_GIFT_MAX_DEFAULT = 30.0`, `SPLIT_JITTER_DEFAULT = 1.0`,
  `REPULSION_MAX = 0.1`, `GRASS_INITIAL_SEED_COUNT_DEFAULT = 8000`,
  `FULL_GRASS_ON_INIT_DEFAULT = false`.
  (`LUT_RADIUS = 4` lives in `src/world/proximity.rs`.)
- `src/wasm_api.rs` → `WorldHandle`, `WorldHandle::set_slider`,
  `WorldHandle::write_snapshot_to`,
  `WorldHandle::sliders_defaults_json`,
  `WorldHandle::profile_enable`,
  `WorldHandle::profile_clear` (v1.9.1),
  `WorldHandle::profile_report_json`,
  free functions `max_pop_for_sim`,
  `rayon_current_num_threads`.

## Update when

- A new tick phase is added, removed, or reordered (update the step order
  diagram).
- The NN topology or input layout changes (update the topology table +
  the constants anchors).
- `MAX_POP_FOR_SIM` changes, or the cull policy moves (update the
  invariant note + cross-check `decisions/cross-cutting.md`).
- The `WorldHandle` wasm surface gains or loses a method.
- The `DevSliders` shape changes (update the slider list in the
  worker-runtime + decisions/sim docs).
- A new chunking constant lands (`MIN_CHUNKS`, `MAX_CHUNKS`, or the
  dynamic formula).
- The world-sizing model changes (`world_size` / `wrap_world` / `world_seed`
  defaults, `GRASS_CELL_SIZE` / `HASH_CELL`, or the `WorldDims` derivation) — or
  any wrap-awareness gap is found between sim math and the renderer mirror.

## Why is it shaped this way

See [`decisions/sim.md`](../decisions/sim.md) — in particular the
SoA-not-AoS choice, the structural "splitters get prioritized via
ordering" cull design, the deterministic chunking scheme, and the
single-`set_slider`-entry-point rule.

## See also

- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) — what
  `write_snapshot_to` actually writes.
- [`worker-runtime.md`](worker-runtime.md) — who calls `step_n`.
- [`profiler.md`](profiler.md) — where the `tick.*` spans live in the
  larger tree.
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
