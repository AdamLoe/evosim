# Simulation core

The Rust world model: SoA, grass field, RNG, NN, the tick step body, and
the bounds invariant.

## What it is

A single-crate Rust simulation compiled to wasm. One `World` owns every
piece of simulation state: a `CreatureSoA` (each per-creature attribute
held as its own `Vec`), a `GrassGrid` (runtime-sized density field), a
`SpatialGrid` (20u cells for neighbour queries), a `SimRng`, the live
`DevSliders`, a `Profiler`, an `NnStats` block, and a small set of
per-tick scratch buffers promoted to long-lived fields. `World::step()`
runs one tick by sequentially executing the numbered phases below.

## Runtime world sizing and wrap

`crates/evosim/src/constants.rs` → `WorldDims`, `WorldDims::from_world_size_with_cell_size`
`crates/evosim/src/world/mod.rs` → `World`

The world is **runtime-sized**. `world_size`, `wrap_world`, and
`grass_cell_size` are construction fields in the Rust-owned `WorldConfig`
payload that the worker passes to wasm at boot.
`WorldDims::from_world_size_with_cell_size` computes and caches the
per-axis dims: `grass_dim = round(world_size / grass_cell_size)` and
`hash_dim = ceil(world_size / HASH_CELL)`. Every bounds/clamp/wrap/spawn
site reads `self.dims`.

Construction-only sizing knobs: `grass_cell_size` (controls grass grid
resolution), `grass_clump_count` + `grass_clump_size` (boot grass
seeding), `world_size` + `wrap_world`, and `master_seed`. See
`crates/evosim/src/constants.rs` for the default values and schema version.

## WorldConfig, defaults, and seeds

`app/crates/evosim/src/wasm_api/mod.rs` → `WorldConfig`,
`WorldHandle::new_with_config_json`
`app/crates/evosim/src/bin/gen_bindings.rs` → `render_world_config`

World construction is versioned by `WORLD_CONFIG_SCHEMA_VERSION` and enters
wasm as one JSON `WorldConfig` object. The config owns dimensions/wrap,
master seed, grass boot settings, initial population/energy cap,
species/mating construction settings, founder action-output boosts, NN
topology, and the opt-in `science.deterministic` mode flag. The worker still
applies `initial_sliders` after construction so live slider lanes are seeded,
but construction fields are not sourced from those lanes.

TypeScript consumes generated mirrors in
`app/web/src/generated/world-config.ts`: `DEFAULT_WORLD_CONFIG`,
`WORLD_CONFIG_PRESETS`, and `DEFAULT_LIVE_SLIDER_VALUES`. Settings defaults
derive construction values from `DEFAULT_WORLD_CONFIG` and live slider values
from `DEFAULT_LIVE_SLIDER_VALUES`; the Playwright defaults-drift spec compares
the generated live defaults against `WorldHandle::sliders_defaults_json()`.

`WorldConfig.master_seed = 0` means "resolve a random nonzero master seed".
The wasm constructor derives a string sim-RNG seed and a numeric internal
`world_seed` from that master seed via `derive_world_seed`. Biome, grass
clump boot, grass scatter, and species/founder setup currently share the
derived internal `world_seed` lane inside `World`; the stream names exist in
`WorldSeedStream`, but splitting those internal lanes requires a future
`World` constructor change. The old `world_seed` slider lane remains in the
SAB/default table for compatibility; externally, the Settings key stores the
master seed.

**Wrap-awareness** (gated on `dims.wrap_world`): position/movement step,
`SpatialGrid` rebuild + neighbour queries, proximity sector distances +
starburst/AABB scans, split-child placement, and eat reach all wrap across
the seam when on. When off, they clamp to walled bounds and the 4
wall-proximity NN inputs are present.

## Biome and species

These are now separate subsystems with their own docs:

- **Biome** — blob generation, static biome grid, grass-capacity cap
  (the only surviving biome effect) → [`biome.md`](biome.md)
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
| 19 | `world_seed` | construction compatibility lane; external config uses `master_seed` |
| 20 | `wrap_world` | construction |
| 21 | `_reserved_legacy_water_movement_penalty` | no-op |
| 22 | `_reserved_legacy_desert_movement_penalty` | no-op |
| 23 | `_reserved_legacy_trait_mutation_sigma_multiplier` | no-op |
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
| 67 | `crowding_strength` | live |
| 68 | `crowding_radius` | live |
| 69 | `starvation_threshold` | live |
| 70 | `starvation_drain_rate` | live |
| 71 | `grass_capacity_scale` | live |
| 72 | `grass_regrowth_rate` | live |

`SLIDER_COUNT = 73`. Authoritative list: `app/crates/evosim/src/wasm_api/mod.rs` →
`SLIDER_NAMES`. `set_slider(name, value)` is the sole mutation entry
point; bools ride the same path as `0|1`.

`grass_cell_size` (idx 62), the clump knobs, species construction knobs,
world shape, founder boosts, and NN topology ride `WorldConfig` because
`initial_sliders` is applied after construction and cannot resize the
already-built `WorldDims` or rebuild founder brains.
`WorldConfig.science.deterministic` also rides `WorldConfig`: it selects the
grass scatter implementation at world construction and is exposed in Settings
as a next-world-only science/replay option, not as a live slider lane.

Saved-world artifacts embed a resolved `WorldConfig` and validate it against
the serialized runtime state before constructing a resumed or forked world.
Artifact validation is independent of app settings migration and does not use
`initial_sliders` as a source for construction fields.

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
- The `MAX_POP_FOR_SIM` safety backstop: `World::handle_births` includes a
  random-cull pass after every birth phase that fires only if
  `pop > MAX_POP_FOR_SIM`. Under the food-limited equilibrium defaults this
  backstop effectively never fires; the equilibrium is held by
  crowding upkeep + starvation drain. Crowding uses the spatial grid only as
  a candidate scan and then applies exact, wrap-aware radius filtering.
- The deterministic chunking scheme for the parallel NN pass:
  `chunk_count = clamp(pop / 32, MIN_CHUNKS=4, min(MAX_CHUNKS=16, workers))`.
- Per-creature heritable state: brain weights + lineage `hue` (f32 ∈ [0,1),
  `CreatureSoA.hue` column). Combat, speed cap, metabolism/upkeep, energy cap,
  body radius, and graze yield are now **constants** — no per-creature genome
  trait modulation. See `crates/evosim/src/creature.rs` for the `hue` column;
  use-site constants are in `crates/evosim/src/constants.rs`.
- Lineage color: `lineage_color_u32(hue)` in `wasm_api/mod.rs` derives the
  display color from the heritable `hue` (HSV: hue×360°, sat=0.8, val=0.9).
  Founders get evenly spaced seed hues; children inherit parent hue with a
  small Gaussian nudge. Packed into the same RGBA8-LE lane-3 snapshot word
  (render format unchanged).
- The action ring-flash (`FlashTag` + `flash_ticks` countdown;
  priority: Killed > CreatedChild > Attacked > Grazed > Born).
- Versioned world runtime state (`WorldRuntimeStateV1`) for saved artifacts:
  tick, dimensions, `world_seed`, RNG state, live sliders, NN topology,
  creature SoA + compact base64 brain weights, grass density + propagation
  metadata, biome grid bytes, species registry, `next_id`, and
  `world_ended`. `World::to_runtime_state_v1` serializes it and
  `World::from_runtime_state_v1` validates lengths/topology/seeds before
  rebuilding derived caches such as spatial grids, grass pyramids, profiler
  scratch, and NN stats.

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
WorldHandle::newWithConfigJson(config_json: &str) -> Result<WorldHandle>
WorldHandle::newFromArtifactJson(artifact_json: &str, load_mode: &str) -> Result<WorldHandle>
WorldHandle::set_slider(name: &str, value: f32)
WorldHandle::sliders_defaults_json() -> String
WorldHandle::creature_at(wx, wy, tol) -> Option<f64>
WorldHandle::creature_idx_by_id(id: f64) -> Option<u32>
WorldHandle::creature_inspect_json(idx: u32) -> Option<String>
WorldHandle::creature_nn_inspect_json(idx: u32) -> Option<String>
WorldHandle::profile_enable(on: bool)
WorldHandle::profile_clear()
WorldHandle::profile_report_json() -> String
WorldHandle::telemetry_report_json() -> String
WorldHandle::world_artifact_json() -> String
WorldHandle::nn_worker_stats_json() -> String
WorldHandle::tps / jank_count / tick / population / world_ended / world_size
// Free functions:
max_pop_for_sim() -> u32
rayon_current_num_threads() -> u32
```

`creature_nn_inspect_json` returns a JSON object with the full labeled NN
input vector (group, label, value per slot) and the current output (vx, vy,
logits for graze/attack/split_or_mate, chosen action). Implemented via
`crates/evosim/src/world/nn.rs` → `build_labeled_nn_inspect`. Used by the
per-creature inspector UI.

`WorldHandle` also owns bounded, in-memory run telemetry: aggregate samples
every fixed sampling period, a capped low-cardinality event log, and the
worst whole-tick jank observation since the last jank reset. Saved-world
artifacts include telemetry policy/metadata (`includes_samples: false`) but
do not serialize the telemetry sample/event buffers by default; telemetry
export remains the path for run-history downloads.

## Tick step order

```text
fn step(&mut self) -> bool {
    let _tick = profile_span!(&self.profile, "tick");

    // Thin path: once world_ended (or pop==0) only grass_step +
    // bitset rebuild + tick++ run, then return false.

    // 1. tick.grid.rebuild      — SpatialGrid::rebuild
    // 2. tick.nn                — chunked Brain::forward (LEAF)
    // 4. tick.movement          — integrate + repulsion + apply
    // 5. tick.graze             — multi-cell density consume
    // 6. tick.attack            — per-bite energy transfer
    // 7. tick.grass_step        — compute_propagation (Scatter or Blur)
    //                             + rebuild_row_bitset (LEAF)
    // 7b.tick.pyramid_refresh   — cadence-gated full mip recompute
    // 8. tick.energy_bookkeeping — age upkeep + digestion; PLUS:
    //                             crowding mortality (∝ exact neighbours in radius)
    //                             starvation drain (below energy floor)
    // 9a.(tick.handle_births)   — species_mode only: handle_mating (sexual Mate)
    // 9b.tick.collect_deaths
    //10. tick.handle_births     — single-pool asexual Split
    //                             RANDOM CULL (backstop only, effectively
    //                             never fires under food-limited equilibrium)
    //12. tick.color_ema         — ring-flash decay (span name kept)
    //    tick.bookkeeping_tail  — last_action promote, tick++, world-end check
}
```

`tick.nn` and `tick.grass_step` measure the main sim worker's wall-clock
wait for the compute dispatch only. In normal threaded builds that is the
rayon wait; deterministic science scatter is an ordered reduction and reports
its sequential dispatch under the same `tick.grass_step` leaf. See
[`profiler.md`](profiler.md).

The spatial grid is rebuilt once, from start-of-tick positions. Movement,
repulsion, attack, and species mating reuse that grid; post-movement
consumers re-check exact distance against current positions. This trades an
occasional one-tick missed interaction near a bucket boundary for avoiding
two full `hash_dim²` rebuilds. `tick.movement` is decomposed into
`tick.movement.integrate`, `tick.movement.repulsion`, and
`tick.movement.apply`.

## NN topology (runtime input width → hidden layers → 5)

`app/crates/evosim/src/brain/mod.rs` → `NnTopology`, `Brain`
`crates/evosim/src/world/nn.rs` → `NnInputLayout`, `NnInputGroup`, `build_nn_input`

Input width is a **runtime field** of `NnTopology`. The active width is
computed by the composable `NnInputLayout` descriptor
(`NnInputLayout::for_settings(wrap_world, species_mode, grass_multisight)`)
then fed to the first matmul as `fan_in`. Width must be a multiple of 8
and `≤ MAX_NN_INPUTS`; see `crates/evosim/src/constants.rs` for the ceiling value
(`MAX_NN_INPUTS = 48`).

**Active input groups** (always-on): `SelfMemory` (8), `CreatureSectors` (8
single-pool / 16 species mode), `GrassSectors` (8), `CurrGrass` (1),
`Bias` (1).
**Conditional groups**: `WallProximity` (4) when `wrap_world = false`;
`GrassBandsFar` (8 far sectors at `GRASS_FAR_SIGHT_RADIUS`, mip level
`GRASS_FAR_MIP_LEVEL`) when `grass_multisight = true`. All 8 combinations fit
within `MAX_NN_INPUTS = 48` — no fallback is needed.

**8-way width table:**

Single-band (`grass_multisight=false`):

| `wrap_world` | `species_mode` | real | pad to |
|---|---|---|---|
| on | off | 26 | **32** |
| off | off | 30 | **32** |
| on | on | 34 | **40** |
| off | on | 38 | **40** |

Multi-band (`grass_multisight=true`, `GrassBandsFar` +8):

| `wrap_world` | `species_mode` | real | pad to |
|---|---|---|---|
| on | off | 34 | **40** |
| off | off | 38 | **40** |
| on | on | 42 | **48** |
| off | on | 46 | **48** |

Default (wrap on, species off, `grass_multisight=true`): real 34 → padded **40**.

Outputs: `out[0]=vx`, `out[1]=vy`, `out[2..5]` = action logits for
`{Graze=0, Attack=1, Split=2}`. In species mode `action[2]` decodes to
`Mate` (same logit slot, mode-dependent validity gate — see
`species.md`). Hidden layers use Leaky ReLU (slope 0.01). Per-layer
founder init: He-uniform `r = sqrt(6 / fan_in)` at runtime.

There is no direct biome-type NN input. Biomes carry no movement/energy
effects and shape learning only by changing grass carrying capacity; the brain
reads that through `GrassSectors`, optional `GrassBandsFar`, and `CurrGrass`.
See [`biome.md`](biome.md).

`GrassBandsFar`: 8 directional sectors at far radius, sampled at mip
level 3 from `GrassPyramid::sample_clamped` — O(1) per tap regardless of
cells under the creature.

## Grass mechanic

`app/crates/evosim/src/grass/mod.rs` → `GrassGrid`

The density field is `Vec<AtomicU8>` (`GrassGrid::density`), stored on the
snapshot quantization scale — a raw u8 IS the renderable density byte.
Accessors `dget`/`dset` encode/decode via `encode_density`/`decode_density`.

`GrassGrid::compute_propagation` dispatches on a `GrassPropagation` selector:

**`Scatter`** (the live path): stochastic u8 scatter. Once per tick the whole
density field is snapshotted into a `scatter_prev` buffer (one bulk memcpy
before the parallel dispatch, when no worker is writing). Then over the
active-tile set (rayon-parallel), each worker reads its source bytes from
`scatter_prev` (a stable, globally-consistent, no-intra-cascade snapshot) and
for each non-zero source cell rolls a **decay** event and a **spread** event,
writing the results back into the live field via atomic RMW. (This replaced an
older per-tile stack-freeze that re-read the field on every active tile —
~20% slower at full coverage.) Spread amount is **density-weighted**: denser source
cells push harder, producing tighter self-reinforcing clumps. The spread
target is picked from a precomputed round disc (`DiscTable`,
`GRASS_SPREAD_RADIUS` cells), with isotropic angular distribution via
radial ring weights. Cross-tile writes are lossy relaxed RMW (accepted
noise). The per-cell hash RNG is `grass_hash_fused_4` — stateless
SplitMix64-finalizer keyed on `(world_seed, cell, tick)`, never touches
`SimRng`. No spontaneous spawn; dead cells are never spread sources.

When `WorldConfig.science.deterministic = true`, scatter keeps the same
tick-start `scatter_prev` source and per-cell hash RNG but replaces relaxed
RMW writes with deterministic per-cell decay/add delta buffers and an
ascending cell-order commit. This removes cross-tile write-race dependence
without changing the default scatter mode.

**`Blur`** (retained for tests): original deterministic separable-Gaussian
path. Grid-direct test constructors default to `Blur`.

Both paths share the **32×32 active-tile frontier**: only frontier tiles
and their 8-neighbour fringe are processed; equilibrium tiles are skipped.
Snapshots read grass through `GrassPyramid::viewport_window`; the old
dirty-tile quantize path is gone.

**`GrassPyramid`**: u8 box-filter mip pyramid (clipmap). L0 aliases the
live density field; L1+ are owned, mean-downsampled. L1+ refresh every
`GRASS_PYRAMID_REFRESH_PERIOD` ticks at step 7b under
`tick.pyramid_refresh`; the span remains present on skipped ticks so its
reported cost is amortized. Serves render LOD + snapshot windowing and
far-grass NN sensing. Zoomed-out render and far sensing can lag by at most
the refresh period; default zoom render reads live L0.

**Boot seeding**: `grass_clump_count` (default 40) filled discs of radius
`grass_clump_size` (default 8 cells), disc centres derived deterministically
from `world_seed` via `grass_hash_u64`. Fallback to uniform-scatter when
`grass_clump_count = 0`.

Single-pool `founder_count` may seed up to `MAX_POP_FOR_SIM`; founders use a
Halton sequence for positions and evenly spaced seed hues. The UI exposes up to
8000 founders, and the live `max_population` cap applies at the first birth
phase.

**Graze**: per-bite transfer using u8 quantization. The live
`grass_bites_per_block` slider controls the density chunk size, with the
default remaining `GRASS_BITES_PER_BLOCK_DEFAULT = 2`. Six live
scatter-params sliders (indices 54–59); three construction-scoped grass
knobs (60–62); two construction-only clump knobs.

**Determinism**: normal/default mode remains optimized for the live app.
Single-threaded normal runs are deterministic, but under `--features threads`
the normal scatter kernel's lossy cross-tile relaxed RMW makes grass density
intentionally non-reproducible run-to-run. Opt-in science mode is the
reproducibility path. The covered promise is exact fixture-state/hash equality
for the same seed/config/artifact plus the same app version across
`cargo test --lib` and `cargo test --lib --features threads`; the pinned
world fixture hash is `0xda586f7d1ea01dbc`. This is not a promise that normal
mode, profiler/telemetry timing, browser event-loop timing, future app
versions, or arbitrary hardware/browser floating-point paths are bit-identical.

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
- `crates/evosim/src/world/nn.rs` → `nn_forward_all_chunks`, `build_nn_input`, `build_labeled_nn_inspect`, `NnInputLayout`, `NnInputGroup`, `ActionGate`, `decode_action`, `is_valid_action`, `chunk_ranges`, `dynamic_chunks`
- `crates/evosim/src/world/nn_stats.rs` → `NnStats`
- `crates/evosim/src/world/proximity.rs` → `LUT_RADIUS`, sector LUT build, creature + grass proximity helpers, `compute_creature_proximity_sectors_species`, `compute_grass_far_band_sectors`
- `app/crates/evosim/src/brain/mod.rs` → `Brain`, `Brain::forward`, `Brain::child_from`, `NnTopology`, `lrelu`
- `crates/evosim/src/creature.rs` → `CreatureSoA` (`hue` column), `FlashTag`, `Action`
- `app/crates/evosim/src/grass/mod.rs` → `GrassGrid`, `GrassPropagation`, `ScatterParams`, `DiscTable`, `GrassPyramid`, `compute_propagation`, `consume`
- `crates/evosim/src/rng.rs` → `SimRng`, `grass_hash_u64`, `grass_hash_fused_4`
- `crates/evosim/src/grid.rs` → `SpatialGrid`, `cell_of`, `rebuild`, `for_each_in_radius`
- `crates/evosim/src/constants.rs` → `WorldDims`, `MAX_POP_FOR_SIM`, `GRASS_CELL_SIZE`, `MAX_NN_INPUTS`, `MIN_CHUNKS`, `MAX_CHUNKS`, `GRASS_SPREAD_RADIUS`, `GRASS_BITES_PER_BLOCK_DEFAULT`, `GRASS_PYRAMID_MAX_LEVELS`, `CROWDING_STRENGTH_DEFAULT`, `CROWDING_RADIUS_DEFAULT`, `STARVATION_THRESHOLD_DEFAULT`, `STARVATION_DRAIN_RATE_DEFAULT`, `GRASS_CAPACITY_SCALE_DEFAULT`, `GRASS_REGROWTH_RATE_DEFAULT`, [etc — authoritative list in file]
- `app/crates/evosim/src/wasm_api/mod.rs` → `WorldHandle`, `WorldHandle::set_slider`, `WorldHandle::write_snapshot`, `WorldHandle::creature_nn_inspect_json`, `BiomePyramid`, `lineage_color_u32`, `max_pop_for_sim`
- `crates/evosim/src/world/equilibrium_tests.rs` — exit-gate test: death-cohort age-at-death trend + steady-state population band

## Update when

- A new tick phase is added, removed, or reordered (update the step order diagram).
- The NN topology or input layout changes (update the topology table + code anchors).
- `MAX_POP_FOR_SIM` changes, or the cull/backstop policy moves.
- The equilibrium mechanism changes (crowding, starvation, or grass dynamics).
- The `WorldHandle` wasm surface gains or loses a method.
- The `DevSliders` shape changes (update the slider table above).
- A new chunking constant lands (`MIN_CHUNKS`, `MAX_CHUNKS`, or the dynamic formula).
- The world-sizing model changes (`world_size`, `wrap_world`, `world_seed`, `grass_cell_size`, `HASH_CELL`, or the `WorldDims` derivation).
- The NN input group set changes (`GrassBandsFar` radii/mip level, `MAX_NN_INPUTS`).
- The heritable column set changes (currently: brain weights + `hue`).

## Why is it shaped this way

See [`decisions/sim.md`](../decisions/sim.md) — SoA-not-AoS choice, the
backstop cull and food-limited equilibrium design, deterministic chunking,
pure neuroevolution rationale, and the single-`set_slider`-entry-point rule.

## See also

- [`species.md`](species.md) — species registry, N-anchor seeding, sexual mating
- [`biome.md`](biome.md) — biome generation, movement + NN + grass effects
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md)
- [`worker-runtime.md`](worker-runtime.md)
- [`profiler.md`](profiler.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- Global authoring rules: `~/agent-docs/v1/rules/authoring-rules.md`
