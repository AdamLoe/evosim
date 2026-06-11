# Biome generation and effects

Static biome map generation, biome cell lookup, and the three per-tick
effects biome applies to creatures.

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
out-of-range byte maps to `Plains`).

## Static Biome Pyramid

The full biome grid stays sim-side on `World`. At `WorldHandle`
construction, `BiomePyramid::build` precomputes mode-downsampled static
biome levels that mirror the grass pyramid dimensions. Each snapshot copies
only the current window with `BiomePyramid::copy_window`, appending it after
the grass window in the snapshot slot. There is no separate full-field
biome buffer exposed to the web shell.

## Per-tick movement and energy effects

`crates/evosim/src/world/mod.rs` → `movement_penalty_for`, `biome_at`

Each non-Plains biome carries a base severity `p ∈ [0, 1]` — the live-tunable
`water_movement_penalty` (default 0.8) and `desert_movement_penalty` (default
0.4) sliders; Plains = 0. While a creature stands in a penalized cell, `p`
drives three effects each tick (balance-knob coefficients in
`crates/evosim/src/constants.rs`):

- **Reduced speed cap** — `MOVE_SPEED_MAX × (1 − K_BIOME_SPEED · p)`;
  `K_BIOME_SPEED` in `constants.rs` (`apply_movement_and_repulsion` in
  `crates/evosim/src/world/tick.rs`).
- **Higher move-cost multiplier** — per-distance move cost scaled by
  `(1 + K_BIOME_COST · p)`; `K_BIOME_COST` in `constants.rs` (same pass).
- **Extra per-tick upkeep** — flat surcharge `K_BIOME_UPKEEP · p` added after
  the age multiplier; `K_BIOME_UPKEEP` in `constants.rs` (`energy_bookkeeping`
  in `crates/evosim/src/world/tick.rs`).

The penalty is **genome-modulated** per creature via
`World::movement_penalty_for`: `water_affinity` reduces the effective penalty
on Water cells; `heat_tolerance` reduces it on Desert cells. The same
modulation is applied to the biome NN inputs so the creature's perceived
penalty matches what it pays.

## NN biome inputs

`crates/evosim/src/world/nn.rs` → `BiomeSampler`, `NnInputGroup` (`BiomeDir`, `CurrCellPenalty`)

Two always-on NN input groups expose biome to the creature brain:
- **`BiomeDir`** (4 slots): genome-modulated movement penalty one cell
  N/S/E/W from the creature (wrap-aware).
- **`CurrCellPenalty`** (1 slot): penalty under the body.

These groups are present in **both** `wrap_world` modes.

## Grass biome-capacity scaling

`app/crates/evosim/src/grass/mod.rs` → `compute_propagation_scatter`

Each grass cell's scatter spread is capped at a **biome-capacity byte**
derived from the cell's biome. Water cells cap lower than Plains; Desert
cells cap at their own level. The cap is applied to incoming spread writes
(`scatter_add` clamps to the target cell's biome cap). This shapes grass
density distributions by biome without per-tick overhead beyond the cap
lookup.

## Invariants and gotchas

- The biome grid is **static per run**. No in-run mutation. Any code that
  calls `biome_at` after construction sees the same grid.
- The SplitMix64 generator is a **third independent RNG stream** (separate
  from the xoshiro sim RNG and the seeding PRNG). Sharing with either would
  break the invariant that `world_seed` pins both the biome map and the
  tick-0 species layout independently.
- Toroidal distance in the blob generator matches `wrap_world` — if
  `wrap_world` is off, blobs use Euclidean distance and can crowd the map
  edges differently.
- Genome modulation (affinity traits) is applied at the penalty *lookup*
  site (`movement_penalty_for`), not at generation time. A creature with
  `water_affinity = 1.0` pays zero penalty on Water cells even though the
  biome grid still marks those cells as Water.

## Code anchors

- `crates/evosim/src/world/biome.rs` → `generate_biome_grid`, `biome_from_u8`
- `crates/evosim/src/world/mod.rs` → `movement_penalty_for`, `biome_at`, `biome_grid_bytes`
- `crates/evosim/src/world/tick.rs` → `apply_movement_and_repulsion` (speed + cost effects), `energy_bookkeeping` (upkeep effect)
- `crates/evosim/src/world/nn.rs` → `BiomeSampler`, `NnInputGroup` (`BiomeDir`, `CurrCellPenalty`)
- `crates/evosim/src/constants.rs` → `Biome` enum, `K_BIOME_SPEED`, `K_BIOME_COST`, `K_BIOME_UPKEEP`, `WATER_MOVEMENT_PENALTY_DEFAULT`, `DESERT_MOVEMENT_PENALTY_DEFAULT`, `NN_BIOME_DIRS`
- `app/crates/evosim/src/grass/mod.rs` → `compute_propagation_scatter` (biome-capacity cap on spread writes)
- `app/crates/evosim/src/wasm_api/mod.rs` → `BiomePyramid` (`build`, `copy_window`)

## See also

- [`simulation-core.md`](simulation-core.md) — `World` overview, grass mechanic, `WorldDims` (world sizing)
- [`render-pipeline.md`](render-pipeline.md) — biome-id quad drawn under the grass texture
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) — per-slot biome window in wasm memory
- [`../decisions/sim.md`](../decisions/sim.md) — rationale for static biome map and genome-modulated penalties
- [`../../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- Global authoring rules: `~/.claude/agent-docs/v1/rules/authoring-rules.md`
