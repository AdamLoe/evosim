# Simulation core

The Rust world model: SoA, grass field, RNG, NN, the tick step body, and
the bounds invariant.

## What it is

A single-crate Rust simulation compiled to wasm. One `World` owns every
piece of simulation state: a `CreatureSoA` (each per-creature attribute
held as its own `Vec`), a `GrassGrid` (960×960 density field), a
`SpatialGrid` (5u cells for neighbour queries), a `SimRng`, the live
`DevSliders`, a `Profiler`, an `NnStats` block, and a small set of
per-tick scratch buffers promoted to long-lived fields. `World::step()`
runs one tick by sequentially executing the numbered phases below.

## What it owns

- The `World` struct, `CreatureSoA`, `GrassGrid`, `SpatialGrid`, `SimRng`,
  `Brain`, `DevSliders`, `Action`.
- The 32-input NN layout (semantic — self/memory, wall, sector creature,
  sector grass, current-cell density, bias slot, padding).
- The 32 → 48 → 24 → 5 brain topology with per-layer Leaky ReLU and SIMD
  forward (`wide::f32x8`).
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
WorldHandle::profile_enable(on: bool)
WorldHandle::profile_report_json() -> String
WorldHandle::nn_worker_stats_json() -> String
WorldHandle::tps / jank_count / tick / population / world_ended / world_size
// Plus free functions:
max_pop_for_sim() -> u32
rayon_current_num_threads() -> u32
```

The 22 per-typed `set_*` setters are gone. `set_slider(name, value)` is
the sole mutation entry point. Bools (only `auto_curriculum` today) ride
the same path as `0|1` via a dedicated arm in `try_set_slider`.

## Tick step order

```text
fn step(&mut self) -> bool {
    let _tick = profile_span!(&self.profile, "tick");

    // 0. Curriculum: cost factor = MIN_FACTOR + (1-MIN_FACTOR) * smoothstep(...)
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

## NN topology (32 → 48 → 24 → 5)

| Slot | Inputs |
|---|---|
| `[0..8)` | self / memory: hunger, age_frac, prev_vx, prev_vy, is_last_graze, is_last_eat, ticks_since_split_norm, cooldown_ready |
| `[8..12)` | wall_proximity N/S/E/W (range `WALL_PROXIMITY_RANGE = 50u`) |
| `[12..20)` | creature_proximity × 8 world-aligned sectors (range `PROXIMITY_RANGE = 20u`) |
| `[20..28)` | grass_density × 8 sectors (range `GRASS_PROXIMITY_RANGE = 8u`) |
| `[28]` | padding (0.0) |
| `[29]` | curr_grass_density (under the body) |
| `[30]` | `1.0` (bias-learning constant) |
| `[31]` | padding (0.0) — SIMD alignment to 32 = 4 × 8 |

Outputs: `out[0] = vx`, `out[1] = vy`, `out[2..5]` = action logits for
`{Graze=0, Eat=1, Split=2}`. Hidden layers use Leaky ReLU (slope 0.01).
Per-layer init range follows He: `r_l1 = 0.433`, `r_l2 = 0.354`,
`r_l3 = 0.500`. Founder weights are pure uniform-random — no
hardwiring. The 32-byte input vector is SIMD-aligned for `wide::f32x8`.

## Code anchors

- `src/world/mod.rs` → `World`, `DevSliders`, `World::step`,
  `World::handle_births`, `World::new_with_sliders`.
- `src/world/tick.rs` → `apply_movement_and_repulsion`, `graze`, `eat`,
  `energy_bookkeeping`, `collect_deaths`, `color_ema_update`.
- `src/world/nn.rs` → `nn_forward_all_chunks`, `build_nn_input`,
  `decode_action`, `chunk_ranges`, `dynamic_chunks`, `PickTimings`.
- `src/world/nn_stats.rs` → `NnStats`.
- `src/world/proximity.rs` → `LUT_DIM`, sector LUT build, wall +
  creature + grass proximity helpers.
- `src/brain.rs` → `Brain`, `Brain::founder`, `Brain::forward`,
  `Brain::child_from`, the `lrelu` helper, `NN_WEIGHT_COUNT == 2808`
  compile-asserts.
- `src/creature.rs` → `CreatureSoA`, `Action` (3 variants), `Action::ALL`.
- `src/grass.rs` → `GrassGrid`, `compute_propagation`, `bilinear_sample`,
  `consume`, `for_each_cell_overlapping_circle`, `rebuild_row_bitset`.
- `src/grid.rs` → `SpatialGrid`, `cell_of`, `rebuild`,
  `for_each_in_radius`.
- `src/constants.rs` → `WORLD_SIZE = 1200`, `GRASS_GRID_DIM = 960`,
  `GRASS_CELL_COUNT = 921_600`, `MAX_POP_FOR_SIM = 32_000`,
  `NN_INPUTS = 32`, `NN_HIDDEN_1 = 48`, `NN_HIDDEN_2 = 24`,
  `NN_OUTPUTS = 5`, `NN_WEIGHT_COUNT = 2808`,
  `MIN_CHUNKS = 4`, `MAX_CHUNKS = 16`, `STARTING_POP_DEFAULT = 8`,
  `CREATURE_SIZE = 1.0`, `START_ENERGY_DEFAULT = 200.0`,
  `MAX_AGE_DEFAULT = 5000`, `SPLIT_THRESHOLD_DEFAULT = 50.0`,
  `SPLIT_GIFT_MAX_DEFAULT = 30.0`, `SPLIT_JITTER_DEFAULT = 50.0`,
  `FULL_GRASS_ON_INIT_DEFAULT = false`.
- `src/wasm_api.rs` → `WorldHandle`, `WorldHandle::set_slider`,
  `WorldHandle::write_snapshot_to`,
  `WorldHandle::sliders_defaults_json`,
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
