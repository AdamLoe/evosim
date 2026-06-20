# Grass/no-grass zones

Static no-grass-zone generation and its one simulation effect:
grass-capacity capping.

## What it is

At world construction a **static zone grid** is generated — one `Biome` u8
per grass cell — and stored on `World.biome_grid`. Product-facing behavior is
binary: plains cells have normal grass capacity, and non-plains cells are
no-grass/dead-grass zones with lower capacity. The grid never changes during a
run; it is rebuilt only on world reconstruction (restart).

The construction setting `WorldConfig.grass.no_grass_zones` can disable this
effect for a new world. Disabled worlds publish an all-plains grid, so grass
capacity is normal everywhere and the renderer cannot show a dead-zone pattern.

`crates/evosim/src/constants.rs` → `Biome`

## Blob generation

`crates/evosim/src/world/biome.rs` → `generate_biome_grid`, `biome_from_u8`

The generator uses a **dedicated SplitMix64 PRNG** seeded from `world_seed`
alone — independent of both the species-seeding stream and the sim RNG
(xoshiro). This pins the biome map to `world_seed` while the live run still
varies per launch.

Algorithm: scatter a few large low-capacity blob groups over a Plains
background. The internal byte tags still distinguish two low-capacity classes
for capacity factors and stable saved-world compatibility, but the user-facing
model is simply normal grass capacity vs no-grass/dead-grass zones. Blob-center
distances are **toroidal** when `wrap_world` is on.

`biome_from_u8` decodes a raw u8 to the internal `Biome` enum
(bounds-checked; any out-of-range byte maps to `Plains`).
`capacity_factor_from_u8` maps a zone byte to its grass carrying-capacity
factor. Plains maps to full capacity; low-capacity tags map below full.

## Static Zone Pyramid

The full zone grid stays sim-side on `World`. At `WorldHandle`
construction, `BiomePyramid::build` precomputes mode-downsampled static
zone levels that mirror the grass pyramid dimensions. Each snapshot copies
only the current window with `BiomePyramid::copy_window`, appending it after
the grass window in the snapshot slot. There is no separate full-field zone
buffer exposed to the web shell.

## Grass capacity scaling (the only zone effect)

`crates/evosim/src/grass/mod.rs` → `compute_propagation_scatter`

Each grass cell's scatter spread is capped at a **capacity byte** derived from
the cell's zone via `capacity_factor_from_u8`. Plains cells cap at full
capacity; no-grass-zone tags cap lower. The cap is applied
to incoming spread writes: `scatter_add` clamps to the target cell's biome
cap. This limits food availability in no-grass zones without any per-creature
movement or energy cost.

No-grass-zone cells are **dead-grass avoidance zones** — the only reason to
avoid them is lack of food, not a movement tax. There are no speed-cap,
move-cost, or upkeep surcharges.

## NN signal

`crates/evosim/src/world/nn.rs` → `NnInputLayout::for_settings`, `build_nn_input`

There is no direct zone-type NN input. Zones affect learning only by modulating
grass carrying capacity, so creatures sense the result through the grass-sector,
far-grass, and current-grass NN inputs.

## Invariants and gotchas

- The zone grid is **static per run**. No in-run mutation. Any code that
  accesses `World.biome_grid` after construction sees the same grid.
- The SplitMix64 generator is a **third independent RNG stream** (separate
  from the xoshiro sim RNG and the seeding PRNG). Sharing with either would
  break the invariant that `world_seed` pins both the biome map and the
  tick-0 species layout independently.
- Toroidal distance in the blob generator matches `wrap_world` — if
  `wrap_world` is off, blobs use Euclidean distance and can crowd the map
  edges differently.
- `no_grass_zones = false` is construction-only. Existing saved worlds resume
  from their serialized zone grid and capacity state.
- There is no genome-modulated penalty; every creature sees the same grass
  capacity in each zone class.

## Code anchors

- `crates/evosim/src/world/biome.rs` → `generate_biome_grid`, `biome_from_u8`, `capacity_factor_from_u8`
- `crates/evosim/src/world/mod.rs` → `World`
- `crates/evosim/src/world/nn.rs` → `NnInputLayout::for_settings`, `build_nn_input`
- `crates/evosim/src/constants.rs` → `Biome`, `GRASS_CAPACITY_PLAINS`, `GRASS_CAPACITY_WATER`, `GRASS_CAPACITY_DESERT`
- `crates/evosim/src/grass/mod.rs` → `compute_propagation_scatter` (biome-capacity cap on spread writes)
- `crates/evosim/src/wasm_api/mod.rs` → `BiomePyramid`, `BiomePyramid::build`, `BiomePyramid::copy_window`

## See also

- [`simulation-core.md`](simulation-core.md) — `World` overview, grass mechanic, `WorldDims` (world sizing)
- [`render-pipeline.md`](render-pipeline.md) — zone-id quad drawn under the grass texture
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) — per-slot biome window in wasm memory
- [`../decisions/sim.md`](../decisions/sim.md) — rationale for static no-grass zones and the food-only effect
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- Global authoring rules: `~/agent-docs/v1/rules/authoring-rules.md`
