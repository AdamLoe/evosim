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

## Runtime world sizing and wrap

`crates/evosim/src/constants.rs` → `WorldDims`, `WorldDims::from_world_size_with_cell_size`
`crates/evosim/src/world/mod.rs` → `World`

The world is **runtime-sized**. `world_size`, `wrap_world`, and
`grass_cell_size` are construction settings on `DevSliders`.
`WorldDims::from_world_size_with_cell_size` computes and caches the
per-axis dims: `grass_dim = round(world_size / grass_cell_size)` and
`hash_dim = ceil(world_size / HASH_CELL)`. Every bounds/clamp/wrap/spawn
site reads `self.dims`.

Construction-only sizing knobs: `grass_cell_size` (controls grass grid
resolution), `grass_clump_count` + `grass_clump_size` (boot grass
seeding), `world_size` + `wrap_world` + `world_seed`. See
`crates/evosim/src/constants.rs` for the default values of each.

**Wrap-awareness** (gated on `dims.wrap_world`): position/movement step,
`SpatialGrid` rebuild + neighbour queries, proximity sector distances +
starburst/AABB scans, split-child placement, and eat reach all wrap across
the seam when on. When off, they clamp to walled bounds and the 4
wall-proximity NN inputs are present.

## Biome and species

These are now separate subsystems with their own docs:

- **Biome** — blob generation, static biome grid, movement/energy/NN/grass-capacity
  effects → [`biome.md`](biome.md)
- **Species and sexual mating** — `SpeciesRegistry`, N-anchor seeding, sexual
  `Mate` + crossover, same-species attack gate → [`species.md`](species.md)

## Slider table (current state)

Current construction and live sliders, their index, and scope. Rationale
for the shape and additions is in `decisions/sim.md`.

| Idx | Name | Scope |
|---|---|---|
| 0–16 | core creature/energy knobs | mostly live |
| 17 | `max_population` | live |
| 18 | `world_size` | construction |
| 19 | `world_seed` | construction |
| 20 | `wrap_world` | construction |
| 21 | `water_movement_penalty` | live |
| 22 | `desert_movement_penalty` | live |
| 23 | `trait_mutation_sigma_multiplier` | live |
| 24 | `species_mode` | construction |
| 25 | `crossover_mode` | construction |
| 26 | `starting_species_count` | construction |
| 27 | `starting_species_member_count` | construction |
| 28 | `starting_species_member_variance` | construction |
| 29 | `mating_cooldown_ticks` | live |
| 30–53 | mutation buckets (`SLIDER_BUCKET_BASE` = 30) | live |
| 54–59 | grass scatter params (decay/spread/ring weights) | live |
| 60 | `lod_bias` | live |
| 61 | `grass_multisight` | construction |
| 62 | `grass_size` / `grass_cell_size` | construction |
| 63 | `grass_lod_step` | live |
| 64 | `mate_reach_multiplier` | live |
| 65 | `init_graze_boost` | construction (founder only) |
| 66 | `init_split_boost` | construction (founder only) |

`SLIDER_COUNT = 67`. Authoritative list: `app/crates/evosim/src/wasm_api/mod.rs` →
`SLIDER_NAMES`. `set_slider(name, value)` is the sole mutation entry
point; bools ride the same path as `0|1`.

`grass_cell_size` (idx 62) and the clump knobs are threaded as explicit
arguments to `newWithFounderCount` because `initial_sliders` is applied
after construction and cannot resize the already-built `WorldDims`.

## What it owns

- The `World` struct, `CreatureSoA`, `GrassGrid`, `SpatialGrid`, `SimRng`,
  `Brain`, `DevSliders`, `Action`.
- The NN input layout — `NnInputLayout` in `crates/evosim/src/world/nn.rs`.
- The brain topology (legacy `32 → 48 → 24 → 5`) with per-layer Leaky ReLU
  and SIMD forward (`wide::f32x8`). Input width is a **runtime field** of
  `NnTopology` — see NN topology section.
- The tick step order (see below). Every named phase has a `tick.<name>`
  profile span.
- The grass mechanic — see Grass section below.
- The `MAX_POP_FOR_SIM` invariant: `World::handle_births` includes a random
  cull pass that draws from `self.rng` after every birth phase so
  `pop ≤ MAX_POP_FOR_SIM` at every tick boundary, deterministically.
- The deterministic chunking scheme for the parallel NN pass:
  `chunk_count = clamp(pop / 32, MIN_CHUNKS=4, min(MAX_CHUNKS=16, workers))`.
- Body genome (6 `f32 ∈ [0,1]` traits in `CreatureSoA`; `Genome` in
  `crates/evosim/src/creature.rs`).
- The action ring-flash (`FlashTag` + `flash_ticks` countdown;
  priority: Killed > CreatedChild > Attacked > Grazed > Born).

## What it does NOT own

- **Snapshot SAB layout, message protocol** — owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md).
- **Worker spawn / tick loop / pacing** — owned by
  [`worker-runtime.md`](worker-runtime.md).
- **Rendering, frustum cull, camera** — owned by
  [`render-pipeline.md`](render-pipeline.md).
- **Profiler tree shape, no-rollup rule, span names** — owned by
  [`profiler.md`](profiler.md).
- **Biome generation and effects** — owned by [`biome.md`](biome.md).
- **Species registry, seeding, mating** — owned by [`species.md`](species.md).

## How it interacts with neighbours

The `WorldHandle` wasm-bindgen surface (`app/crates/evosim/src/wasm_api/mod.rs`):

```text
WorldHandle::step_n(n: u32) -> bool
WorldHandle::write_snapshot(slot: u32)
WorldHandle::set_slider(name: &str, value: f32)
WorldHandle::sliders_defaults_json() -> String
WorldHandle::creature_at(wx, wy, tol) -> Option<f64>
WorldHandle::creature_idx_by_id(id: f64) -> Option<u32>
WorldHandle::creature_inspect_json(idx: u32) -> Option<String>
WorldHandle::profile_enable(on: bool)
WorldHandle::profile_clear()
WorldHandle::profile_report_json() -> String
WorldHandle::nn_worker_stats_json() -> String
WorldHandle::tps / jank_count / tick / population / world_ended / world_size
// Free functions:
max_pop_for_sim() -> u32
rayon_current_num_threads() -> u32
```

## Tick step order

```text
fn step(&mut self) -> bool {
    let _tick = profile_span!(&self.profile, "tick");

    // Thin path: once world_ended (or pop==0) only grass_step +
    // bitset rebuild + tick++ run, then return false.

    // 1. tick.grid.rebuild      — SpatialGrid::rebuild
    // 2. tick.nn                — chunked Brain::forward (LEAF)
    // 4. tick.movement          — apply_movement_and_repulsion
    // 5. tick.graze             — multi-cell density consume
    // 6. tick.attack            — per-bite energy transfer
    // 7. tick.grass_step        — compute_propagation (Scatter or Blur)
    //                             + rebuild_row_bitset (LEAF)
    // 7b.tick.pyramid_refresh   — GrassGrid::refresh_pyramid (full mip recompute)
    // 8. tick.energy_bookkeeping
    // 9a.(tick.handle_births)   — species_mode only: handle_mating (sexual Mate)
    // 9b.tick.collect_deaths
    //10. tick.handle_births     — single-pool asexual Split
    //                             RANDOM CULL back to MAX_POP_FOR_SIM
    //12. tick.color_ema         — ring-flash decay (span name kept)
    //    tick.bookkeeping_tail  — last_action promote, tick++, world-end check
}
```

`tick.nn` and `tick.grass_step` measure the main sim worker's wall-clock
wait for the rayon dispatch only. See [`profiler.md`](profiler.md).

## NN topology (runtime input width → hidden layers → 5)

`app/crates/evosim/src/brain/mod.rs` → `NnTopology`, `Brain`
`crates/evosim/src/world/nn.rs` → `NnInputLayout`, `NnInputGroup`, `build_nn_input`

Input width is a **runtime field** of `NnTopology`. The active width is
computed by the composable `NnInputLayout` descriptor
(`NnInputLayout::for_settings(wrap_world, species_mode, grass_multisight)`)
then fed to the first matmul as `fan_in`. Width must be a multiple of 8
and `≤ MAX_NN_INPUTS`; see `crates/evosim/src/constants.rs` for the ceiling value.

**Active input groups** (always-on): `SelfMemory` (8), `GrassSectors` (8),
`BiomeDir` (4), `CurrCellPenalty` (1), `CurrGrass` (1), `Bias` (1).
**Conditional groups**: `WallProximity` (4) when `wrap_world = false`;
`CreatureSectors` (8 single-pool / 16 species mode);
`GrassBandsFar` (8 far sectors at `GRASS_FAR_SIGHT_RADIUS`, mip level
`GRASS_FAR_MIP_LEVEL`) when `grass_multisight = true` and it fits within
`MAX_NN_INPUTS`. The walled+species combination exceeds the ceiling; the
layout automatically falls back to single-band for that configuration.

**4-way width table (`grass_multisight=true`):**

| `wrap_world` | `species_mode` | real | pad to |
|---|---|---|---|
| on | off | 39 | **40** |
| off | off | 43 | **48** |
| on | on | 47 | **48** |
| off | on | 51 → fallback | — |

With `grass_multisight=false` subtract 8 from each real width.

Default (wrap on, species off, `grass_multisight=true`): real 39 → padded **40**.

Outputs: `out[0]=vx`, `out[1]=vy`, `out[2..5]` = action logits for
`{Graze=0, Attack=1, Split=2}`. In species mode `action[2]` decodes to
`Mate` (same logit slot, mode-dependent validity gate — see
`species.md`). Hidden layers use Leaky ReLU (slope 0.01). Per-layer
founder init: He-uniform `r = sqrt(6 / fan_in)` at runtime.

Biome inputs (`BiomeDir` + `CurrCellPenalty`) are **genome-modulated** per
creature — high-affinity creatures read a lower penalty matching what they
actually pay. See [`biome.md`](biome.md).

`GrassBandsFar`: 8 directional sectors at far radius, sampled at mip
level 3 from `GrassPyramid::sample_clamped` — O(1) per tap regardless of
cells under the creature.

## Grass mechanic

`app/crates/evosim/src/grass/mod.rs` → `GrassGrid`

The density field is `Vec<AtomicU8>` (`GrassGrid::density`), stored on the
snapshot quantization scale — a raw u8 IS the renderable density byte.
Accessors `dget`/`dset` encode/decode via `encode_density`/`decode_density`.

`GrassGrid::compute_propagation` dispatches on a `GrassPropagation` selector:

**`Scatter`** (the live path): stochastic u8 scatter. Per tick, over the
active-tile set (rayon-parallel), each worker freezes its tile into a stack
scratch buffer, then for each non-zero source cell rolls a **decay** event
and a **spread** event. Spread amount is **density-weighted**: denser source
cells push harder, producing tighter self-reinforcing clumps. The spread
target is picked from a precomputed round disc (`DiscTable`,
`GRASS_SPREAD_RADIUS` cells), with isotropic angular distribution via
radial ring weights. Cross-tile writes are lossy relaxed RMW (accepted
noise). The per-cell hash RNG is `grass_hash_fused_4` — stateless
SplitMix64-finalizer keyed on `(world_seed, cell, tick)`, never touches
`SimRng`. No spontaneous spawn; dead cells are never spread sources.

**`Blur`** (retained for tests): original deterministic separable-Gaussian
path. Grid-direct test constructors default to `Blur`.

Both paths share the **32×32 active-tile frontier**: only frontier tiles
and their 8-neighbour fringe are processed; equilibrium tiles are skipped.
The snapshot grass write is dirty-tile-incremental via
`quantize_dirty_tiles_into`.

**`GrassPyramid`**: u8 box-filter mip pyramid (clipmap). L0 aliases the
live density field; L1+ are owned, mean-downsampled. Refreshed each tick
at step 7b under `tick.pyramid_refresh`. Serves render LOD + snapshot
windowing.

**Boot seeding**: `grass_clump_count` (default 40) filled discs of radius
`grass_clump_size` (default 8 cells), disc centres derived deterministically
from `world_seed` via `grass_hash_u64`. Fallback to uniform-scatter when
`grass_clump_count = 0`.

**Graze**: per-bite transfer using u8 quantization (`GRASS_BITES_PER_BLOCK`).
Six live scatter-params sliders (indices 54–59); three construction-scoped
grass knobs (60–62); two construction-only clump knobs.

**Determinism**: single-threaded runs are deterministic. Under
`--features threads` the scatter kernel's lossy cross-tile relaxed RMW
makes grass density **intentionally non-reproducible run-to-run** — an
accepted perf-vs-reproducibility trade-off. Determinism tests assert exact
equality only under `#[cfg(not(feature="threads"))]`.

## Body genome

`crates/evosim/src/creature.rs` → `Genome`, `CreatureSoA` (`genome` column)

Each creature carries 6 `f32 ∈ [0,1]` traits: `body_size`, `max_speed`,
`metabolism`, `diet`, `water_affinity`, `heat_tolerance`. Each is rescaled
at its use site; a median genome (all 0.5) approximates current constants.
`water_affinity` and `heat_tolerance` modulate biome penalties — see
[`biome.md`](biome.md).

Mutation on split: same per-birth bucket as the brain; each trait takes one
Gaussian nudge then is clamped to `[0,1]`. `trait_mutation_sigma_multiplier`
(live slider) scales the genome sigma.

## Action ring-flash (FlashTag)

`crates/evosim/src/creature.rs` → `FlashTag`, `CreatureSoA` (`flash_tag`, `flash_ticks` columns)
`crates/evosim/src/world/tick.rs` → `flash_decay`

Each creature carries a transient `FlashTag` + `flash_ticks` countdown (5
ticks). Priority: **Killed (red) > CreatedChild (blue) > Attacked (yellow)
> Grazed (green)**; **Born (teal)** set at spawn. Countdown decrements in
`flash_decay` (the `tick.color_ema` span). Packed into snapshot for the
renderer — see [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md).

## Code anchors

- `crates/evosim/src/world/mod.rs` → `World`, `DevSliders`, `World::step`, `World::handle_births`
- `crates/evosim/src/world/tick.rs` → `apply_movement_and_repulsion`, `graze`, `attack`, `energy_bookkeeping`, `collect_deaths`, `flash_decay`
- `crates/evosim/src/world/nn.rs` → `nn_forward_all_chunks`, `build_nn_input`, `NnInputLayout`, `NnInputGroup`, `BiomeSampler`, `ActionGate`, `decode_action`, `is_valid_action`, `chunk_ranges`, `dynamic_chunks`
- `crates/evosim/src/world/nn_stats.rs` → `NnStats`
- `crates/evosim/src/world/proximity.rs` → `LUT_RADIUS`, sector LUT build, creature + grass proximity helpers, `compute_creature_proximity_sectors_species`, `compute_grass_far_band_sectors`
- `app/crates/evosim/src/brain/mod.rs` → `Brain`, `Brain::forward`, `Brain::child_from`, `NnTopology`, `lrelu`
- `crates/evosim/src/creature.rs` → `CreatureSoA`, `Genome`, `FlashTag`, `Action`
- `app/crates/evosim/src/grass/mod.rs` → `GrassGrid`, `GrassPropagation`, `ScatterParams`, `DiscTable`, `GrassPyramid`, `compute_propagation`, `consume`, `quantize_dirty_tiles_into`
- `crates/evosim/src/rng.rs` → `SimRng`, `grass_hash_u64`, `grass_hash_fused_4`
- `crates/evosim/src/grid.rs` → `SpatialGrid`, `cell_of`, `rebuild`, `for_each_in_radius`
- `crates/evosim/src/constants.rs` → `WorldDims`, `MAX_POP_FOR_SIM`, `GRASS_CELL_SIZE`, `MAX_NN_INPUTS`, `MIN_CHUNKS`, `MAX_CHUNKS`, `GRASS_SPREAD_RADIUS`, `GRASS_BITES_PER_BLOCK`, `GRASS_PYRAMID_MAX_LEVELS`, [etc — authoritative list in file]
- `app/crates/evosim/src/wasm_api/mod.rs` → `WorldHandle`, `WorldHandle::set_slider`, `WorldHandle::write_snapshot`, `BiomePyramid`, `max_pop_for_sim`

## Update when

- A new tick phase is added, removed, or reordered (update the step order diagram).
- The NN topology or input layout changes (update the topology table + code anchors).
- `MAX_POP_FOR_SIM` changes, or the cull policy moves.
- The `WorldHandle` wasm surface gains or loses a method.
- The `DevSliders` shape changes (update the slider table above).
- A new chunking constant lands (`MIN_CHUNKS`, `MAX_CHUNKS`, or the dynamic formula).
- The world-sizing model changes (`world_size`, `wrap_world`, `world_seed`, `grass_cell_size`, `HASH_CELL`, or the `WorldDims` derivation).
- The NN input group set changes (`GrassBandsFar` radii/mip level, `MAX_NN_INPUTS`, or the `grass_multisight` fallback logic).

## Why is it shaped this way

See [`decisions/sim.md`](../decisions/sim.md) — SoA-not-AoS choice, the
structural cull design, deterministic chunking, and the
single-`set_slider`-entry-point rule.

## See also

- [`species.md`](species.md) — species registry, N-anchor seeding, sexual mating
- [`biome.md`](biome.md) — biome generation, movement + NN + grass effects
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md)
- [`worker-runtime.md`](worker-runtime.md)
- [`profiler.md`](profiler.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- Global authoring rules: `~/.claude/agent-docs/v1/rules/authoring-rules.md`
