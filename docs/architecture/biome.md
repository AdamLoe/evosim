# Biome generation and effects

Static biome map generation and the one surviving per-tick biome effect:
grass-capacity capping.

## What it is

At world construction a **static biome grid** is generated — one `Biome`
u8 per grass cell — and stored on `World.biome_grid` for O(1) per-tick
lookups. The grid never changes during a run; it is rebuilt only on world
reconstruction (restart).

`crates/evosim/src/constants.rs` → `Biome` enum (`Plains = 0`, `Water = 1`, `Desert = 2`)

## Blob generation

`crates/evosim/src/world/biome.rs` → `generate_biome_grid`, `biome_from_u8`

The generator uses a **dedicated SplitMix64 PRNG** seeded from `world_seed`
alone — independent of both the species-seeding stream and the sim RNG
(xoshiro). This pins the biome map to `world_seed` while the live run still
varies per launch.

Algorithm: scatter a few large blobs — `2..4` water centers and `2..4`
desert centers, each at radius `0.13..0.18 × world_size` — over a Plains
background. A cell inside a water blob is Water; inside a desert blob (and
no water) is Desert; otherwise Plains. Water wins overlaps. Blob-center
distances are **toroidal** when `wrap_world` is on. Tuned proportions land
near **plains ~60% / water ~20% / desert ~20%** (plains is always the
plurality).

`biome_from_u8` decodes a raw u8 to the `Biome` enum (bounds-checked; any
out-of-range byte maps to `Plains`). `capacity_factor_from_u8` maps a biome
byte to its grass carrying-capacity factor (Plains ×1.0, Desert ×0.30,
Water ×0.04).

## Static Biome Pyramid

The full biome grid stays sim-side on `World`. At `WorldHandle`
construction, `BiomePyramid::build` precomputes mode-downsampled static
biome levels that mirror the grass pyramid dimensions. Each snapshot copies
only the current window with `BiomePyramid::copy_window`, appending it after
the grass window in the snapshot slot. There is no separate full-field
biome buffer exposed to the web shell.

## Grass biome-capacity scaling (the ONLY surviving per-tick biome effect)

`app/crates/evosim/src/grass/mod.rs` → `compute_propagation_scatter`

Each grass cell's scatter spread is capped at a **biome-capacity byte**
derived from the cell's biome via `capacity_factor_from_u8`. Plains cells
cap at full capacity; Water and Desert cells cap lower. The cap is applied
to incoming spread writes: `scatter_add` clamps to the target cell's biome
cap. This limits food availability in non-Plains biomes, shaping grass
density by biome without any per-creature movement or energy cost.

Water and Desert cells are effectively **dead-grass avoidance zones** — the
only reason to avoid them is lack of food, not a movement tax. There are no
speed-cap, move-cost, or upkeep surcharges.

## NN signal

`crates/evosim/src/world/nn.rs` → `NnInputLayout::for_settings`, `build_nn_input`

There is no direct biome-type NN input. Biomes affect learning only by
modulating grass carrying capacity, so creatures sense the result through the
grass-sector, far-grass, and current-grass NN inputs.

## Invariants and gotchas

- The biome grid is **static per run**. No in-run mutation. Any code that
  accesses `World.biome_grid` after construction sees the same grid.
- The SplitMix64 generator is a **third independent RNG stream** (separate
  from the xoshiro sim RNG and the seeding PRNG). Sharing with either would
  break the invariant that `world_seed` pins both the biome map and the
  tick-0 species layout independently.
- Toroidal distance in the blob generator matches `wrap_world` — if
  `wrap_world` is off, blobs use Euclidean distance and can crowd the map
  edges differently.
- There is no genome-modulated penalty; every creature sees the same grass
  capacity in each biome.

## Code anchors

- `crates/evosim/src/world/biome.rs` → `generate_biome_grid`, `biome_from_u8`, `capacity_factor_from_u8`
- `crates/evosim/src/world/mod.rs` → `World.biome_grid` (the stored u8 grid)
- `crates/evosim/src/world/nn.rs` → `NnInputLayout::for_settings`, `build_nn_input`
- `crates/evosim/src/constants.rs` → `Biome` enum, `GRASS_CAPACITY_PLAINS`, `GRASS_CAPACITY_WATER`, `GRASS_CAPACITY_DESERT`
- `app/crates/evosim/src/grass/mod.rs` → `compute_propagation_scatter` (biome-capacity cap on spread writes)
- `app/crates/evosim/src/wasm_api/mod.rs` → `BiomePyramid` (`build`, `copy_window`)

## See also

- [`simulation-core.md`](simulation-core.md) — `World` overview, grass mechanic, `WorldDims` (world sizing)
- [`render-pipeline.md`](render-pipeline.md) — biome-id quad drawn under the grass texture
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) — per-slot biome window in wasm memory
- [`../decisions/sim.md`](../decisions/sim.md) — rationale for static biome map and food-only biome effect
- [`../../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- Global authoring rules: `~/agent-docs/v1/rules/authoring-rules.md`
