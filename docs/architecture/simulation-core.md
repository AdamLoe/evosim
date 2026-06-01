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

### Biome map + movement penalty (v2.0 Wave 1b)

At construction the world generates a **static biome grid** (one `Biome` u8 per
grass cell: `Plains=0, Water=1, Desert=2`) deterministically from the numeric
`world_seed`, using a **dedicated SplitMix64 PRNG** that is *independent* of the
string/XxHash64 sim RNG (so the biome map pins to `world_seed` alone while the
live run still varies). The generator (`src/world/biome.rs`) scatters a few
large **blobs** — 2..4 water centers + 2..4 desert centers, each radius
`0.13..0.18 × world_size` — over a Plains background; a cell inside a water blob
is Water, inside a desert blob (and no water) is Desert, else Plains (water wins
overlaps). Distance to a blob center is **toroidal** when `wrap_world`. Tuned
proportions land near **plains ~60% / water ~20% / desert ~20%** (plains always
the plurality). The grid is stored sim-side on `World.biome_grid` for O(1)
per-tick `biome_at` / `movement_penalty_at` lookups and copied byte-for-byte
into the boot `biome_buf` SAB.

Each non-Plains biome carries a **base severity** `p ∈ [0, 1]` — the
live-tunable `water_movement_penalty` (default **0.8**) / `desert_movement_penalty`
(default **0.4**) sliders; Plains = 0. (Wave 2 will modulate `p` per genome via
`water_affinity` / `heat_tolerance`; in Wave 1b the penalty is
genome-independent.) While a creature stands in a penalized cell, `p` drives
**three effects each tick**, each scaled by a built-in balance-knob coefficient
(`src/constants.rs`):

- **reduced effective move speed** — the speed cap is `MOVE_SPEED_MAX ×
  (1 − K_BIOME_SPEED·p)`, `K_BIOME_SPEED = 0.6` (`apply_movement_and_repulsion`);
- **higher move-cost multiplier** — the per-distance move cost is `×
  (1 + K_BIOME_COST·p)`, `K_BIOME_COST = 1.0` (same pass);
- **extra per-tick upkeep** — `upkeep += K_BIOME_UPKEEP·p`, `K_BIOME_UPKEEP = 0.5`,
  added as a flat surcharge after the age multiplier (`energy_bookkeeping`).

Coefficients are tuned **survivable short-term**: at default sliders a
full-energy (100) creature drains ≈0.75 energy/tick on water (≈130 ticks) and
≈0.5/tick on desert (≈195 ticks), so a dash across is easily survivable but
sustained residence is not.

### Species + sexual mating mode (v2.0 Wave 3a)

`species_mode` (construction-only `DevSliders` flag, **default OFF**) is a hard
restart-required switch between two regimes that never coexist in one run:

| | `species_mode = off` (single-pool) | `species_mode = on` (species) |
|---|---|---|
| Population | Single pool, asexual. | N seeded species, founders clustered at random anchors. |
| Reproduction | Asexual `Split` (unchanged). | Sexual `Mate` (same-species, contact-radius, initiator-gated). |
| Action[2] | `Split` — valid iff `energy ≥ split_threshold`. | `Mate` — valid iff off mating cooldown AND `energy ≥ mating cost`. |
| Attack target | Any creature (v1 predation). | Other-species only (no cannibalism). |
| Body color | Genome-derived hue. | Per-species saturated color. |
| Creature NN sectors | 8 unified. | 16 (8 same-species + 8 other-species). |

The NN output shape is unchanged: `action[2]` decodes to the `Action::Split`
enum variant in BOTH modes (the logit order is stable), but its *meaning* and
validity gate differ by mode — see `ActionGate` in `src/world/nn.rs`
(`decode_action` / `is_valid_action` branch on `species_mode`). Continuous
`(vx, vy)` velocity is unchanged.

**Species registry.** `World.species` is a `SpeciesRegistry`
(`src/world/species.rs`) — a `Vec<Species>` keyed by id (`Species { id: u16,
color_u32 }`), built **dynamic** (add/remove-capable) so the v2.1 split/merge
work needs no protocol change even though v3 seeds a fixed set. Each creature
carries a `species_id: u16` SoA column (0/unused in single-pool). Seeding
(Wave-3 **placeholder**; Wave 4 does biome-appropriate founders): seed
`starting_species_count` (default 10) species each with an evenly-spread
saturated **placeholder hue** (`hue = id/total × 360°`); for each, cluster
`starting_species_member_count` (default 10) founders near a random Halton
anchor with **basic uniform-random** founder genomes. `starting_species_member_variance`
(default 3.0) is **accepted but inert** in Wave 3 — wired + plumbed, applied in
Wave 4.

**Mate mechanic** (`World::handle_mating`, runs in the `tick.handle_births` span
in species mode). Gated ENTIRELY by the **initiator**: it chose `action[2]`, is
off mating cooldown, and has `energy ≥ mating cost` (= the `split_gift`). On fire
it scans for the **first** same-species creature (by `species_id` only — no
genome-distance gate) within a small **contact radius** `(r_i + r_j) ×
MATING_CONTACT_RADIUS_FACTOR` (≈ summed body radii, **NOT** the 20u perception
range) — first-found wins, no nearest-scan (consistent with Attack). **All cost +
cooldown fall on the initiator only:** it pays the mating cost and enters a fixed
`mating_cooldown_ticks` (default 200, live slider) cooldown via the dedicated
`mating_cooldown` SoA column (decremented in `energy_bookkeeping`, distinct from
`digestion_cooldown`). The partner pays nothing, needs no energy, and is **not**
required to be off cooldown — it's purely a target. The child spawns at the
parent **midpoint** (wrap-aware; no habitable-cell check) via
**crossover-then-mutate**, with the parents' shared `species_id`. An invalid
`Mate` pick falls through to Graze exactly like Split.

**Crossover** (`crossover_mode` construction enum `{average, fifty_fifty}`,
default `fifty_fifty`) is applied **identically to brain weights AND the 6 genome
traits**, then mutation (the same per-birth bucket machinery as a normal child):
`fifty_fifty` = per-slot 50/50 random pick from the two parents; `average` =
per-slot elementwise midpoint. Single-pool `Split` is unchanged (asexual, no
crossover). RNG draw order is fixed: brain crossover (one `unit()`/weight for
fifty_fifty; none for average) → brain bucket mutation → genome crossover → genome
mutation, all off the pair's pre-rolled seed (`Brain::child_from_crossover_with_sigma`
+ `Genome::crossed`).

**Attack same-species gate.** In species mode `Attack` refuses same-species
targets (a sim-side gate in `World::attack` — no cannibalism). Single-pool
`Attack` hits any creature (v1 predation, unchanged). Effectiveness still scales
with `diet` + `body_size` (Wave 2).

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
`max_population` moves to 17. `full_grass_on_init` remains a `DevSliders` field
carried through the boot construction call (`newWithFounderCount`), just not a
live slider.
**v2.0 Wave 1b slider changes:** two live biome movement-penalty sliders are
added — `water_movement_penalty` (**21**, default 0.8) and
`desert_movement_penalty` (**22**, default 0.4).
**v2.0 Wave 2a slider change:** `trait_mutation_sigma_multiplier` (**23**, default 0.3).
**v2.0 Wave 3a slider changes:** six species/mating sliders added at indices
**24–29** — `species_mode` (24, bool, construction), `crossover_mode` (25, enum
0=average/1=fifty_fifty, construction), `starting_species_count` (26),
`starting_species_member_count` (27), `starting_species_member_variance` (28,
inert in W3), `mating_cooldown_ticks` (29, live, default 200) — so
`SLIDER_BUCKET_BASE` shifts 24→**30** and `SLIDER_COUNT = 54`. The slider region
`[16, 70)` stays below the inspect block (slot 80), so **no control-SAB byte-slot
shift** this wave. Regenerate `web/src/generated/slider-ids.ts` via `cargo run
--bin gen-bindings` (the `bindings_in_sync` test guards it). `species_mode` +
`crossover_mode` + the three `starting_species_*` settings also ride the explicit
construction call (`newWithFounderCount`) since they shape the world topology +
seeding at construction; `mating_cooldown_ticks` is live.

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
    // 5. tick.attack            — per-bite energy transfer (v2.0 Wave 2a: Eat→Attack)
    // 6. tick.grass_step        — grass propagation + bitset rebuild (LEAF)
    // 7. tick.energy_bookkeeping
    // 8. tick.collect_deaths
    // 9. tick.handle_births     — single-pool: split; species_mode: handle_mating
    //                             (sexual Mate) + RANDOM CULL back to MAX_POP_FOR_SIM
    //10. tick.color_ema         — ring-flash decay (v2.0 Wave 2a; span name kept)
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
(`NnInputLayout::for_settings(wrap_world, species_mode)`, v2.0 Wave 3a): the
`ReservedPredator` group is **dropped**, `WallProximity` (4 slots) is present
**only when `wrap_world == false`**, the two biome groups — `BiomeDir` (4:
N/S/E/W) + `CurrCellPenalty` (1) — are **always-on in BOTH wrap modes**, and the
`CreatureSectors` group is **8 in single-pool / 16 in species_mode** (8
same-species + 8 other-species). The topology's `input_width` is reconciled to
the layout width at construction (old brains are discarded, no save/load).

**4-way width table** (always-on block = SelfMemory 8 + GrassSectors 8 + BiomeDir
4 + CurrCellPenalty 1 + CurrGrass 1 + Bias 1 = 23; + WallProximity 4 when walled;
+ CreatureSectors 8 or 16):

| `wrap_world` | `species_mode` | real | pad to |
|---|---|---|---|
| on | off | 31 | **32** |
| off | off | 35 | **40** |
| on | on | 39 | **40** |
| off | on | 43 | **48** (the `MAX_NN_INPUTS` ceiling) |

The SIMD-vs-scalar drift guard exercises **all four** compositions (widths
32/40/48 — width-40 covers both wrap-off/species-off and wrap-on/species-on).

Default (wrap **on**, species **off**) — real width 31 → padded 32:

| Slot | Group / inputs |
|---|---|
| `[0..8)` | `SelfMemory`: hunger, age_frac, prev_vx, prev_vy, is_last_graze, is_last_attack, ticks_since_split_norm, cooldown_ready |
| `[8..16)` | `CreatureSectors` × 8 world-aligned sectors (range `PROXIMITY_RANGE = 20u`); in `species_mode` this group is **16** (8 same-species + 8 other-species), shifting the groups after it |
| `[16..24)` | `GrassSectors` × 8 sectors (range `GRASS_PROXIMITY_RANGE = 20u`) |
| `[24..28)` | `BiomeDir`: movement penalty one cell N/S/E/W (base severity, wrap-aware) |
| `[28]` | `CurrCellPenalty`: movement penalty under the body |
| `[29]` | `CurrGrass`: curr_grass_density (under the body) |
| `[30]` | `Bias` — `1.0` (bias-learning constant) |
| `[31]` | trailing pad (0.0) — SIMD alignment to 32 |

Walled (wrap **off**) — real width 35 → padded **40** — inserts
`WallProximity` N/S/E/W (range `WALL_PROXIMITY_RANGE = 50u`) at `[8..12)`,
shifting `CreatureSectors`→`[12..20)`, `GrassSectors`→`[20..28)`,
`BiomeDir`→`[28..32)`, `CurrCellPenalty`→`[32]`, `CurrGrass`→`[33]`,
`Bias`→`[34]`, pad `[35..40)`.

Width table (wrap × {on, off}): **wrap on = 26+5 = 31 → pad 32; wrap off =
30+5 = 35 → pad 40** (the `+5` is the always-on `BiomeDir`+`CurrCellPenalty`).
The biome inputs are penalties in `[0, 1]`, sampled wrap-aware alongside the
existing input build (`BiomeSampler` in `src/world/nn.rs`). **v2.0 Wave 2a:
these biome inputs are GENOME-MODULATED per creature** — the `BiomeDir` +
`CurrCellPenalty` slots read `base_severity * (1 - water_affinity)` on water
cells and `* (1 - heat_tolerance)` on desert cells, so a high-affinity creature
reads the same lower penalty it pays in the tick effects (this closes the Wave
1b seam).

`GRASS_PROXIMITY_RANGE` was raised **8u → 20u** so each of the 8 grass sectors
spans `ceil(20/5) = 4` grass cells (≥3) at the 5u cell; the sector LUT radius
`LUT_RADIUS` is recomputed to **4** (was 16 at the old 1.25u cell). The
`NnInputLayout::legacy()` width-32 anchor (the historical 7 groups,
ReservedPredator at `[28]`, no biome groups) survives as a **test-only**
calibration baseline. Group offsets + total width are **recomputed by the
descriptor** when the active group set changes — no hand-maintained slot
constants in the build path. The SIMD-vs-scalar drift guard now exercises BOTH
input widths (32 and 40). Old brains are discarded (no save/load).

Outputs: `out[0] = vx`, `out[1] = vy`, `out[2..5]` = action logits for
`{Graze=0, Attack=1, Split=2}` (v2.0 Wave 2a: `Eat` renamed to `Attack`; the
repr indices and logit order are unchanged, so NN numerics are stable). v2.0
Wave 3a: `action[2]` (the `Split` variant) decodes to **Mate** in `species_mode`
— same logit slot, mode-dependent validity gate (`ActionGate`). Hidden layers use
Leaky ReLU (slope 0.01).
Per-layer founder init follows He-uniform `r = sqrt(6 / fan_in)`, computed at
runtime per matmul (the first matmul's `fan_in` is the runtime input width).
Founder weights are pure uniform-random — no hardwiring.

## Body genome (v2.0 Wave 2a, single-pool)

Each creature carries a `Genome` (`src/creature.rs`) — **six `f32 ∈ [0,1]`**
traits, stored as a SoA column (`creatures.genome`) alongside the brain. The
genome is **sim-side only**; it never rides the snapshot (the render color is
*derived* from it at write time). Each trait is rescaled at its use site, with
ranges centered so a **median genome (all 0.5) ≈ today's constants**:

| trait | rescale → factor | plugs into |
|---|---|---|
| `body_size` | `0.5 + t` ∈ [0.5, 1.5] | sprite + repulsion radius, energy-cap factor, attack damage |
| `max_speed` | `0.5 + t` ∈ [0.5, 1.5] | speed cap × factor; move-cost × factor |
| `metabolism` | `0.5 + t` ∈ [0.5, 1.5] | per-tick idle-upkeep multiplier |
| `diet` | `t` (0 grazer … 1 predator) | graze yield × `(1 - 0.5·diet)`; attack effectiveness × `(0.5 + diet)` |
| `water_affinity` | `t` | water move-penalty × `(1 - t)` |
| `heat_tolerance` | `t` | desert move-penalty × `(1 - t)` |

Attack effectiveness combines diet + body: `effectiveness = (0.5 + diet) *
body_size_factor` (so a median genome's effectiveness is `1.0 * 1.0 = 1.0`,
reproducing the pre-genome transfer). The genome-modulated movement penalty
(`World::movement_penalty_for`) reduces the Wave 1b base severity by the affinity
trait of the cell's biome, and the **same modulation is applied to the biome NN
inputs** — closing the Wave 1b seam.

**Founders** draw a genome uniformly in `[0,1]` per trait (single-pool: visible
diversity from tick 0; drawn from the world RNG right after the founder brain).
**Mutation on split:** the genome mutates off the **same per-birth mutation
bucket** as the brain (`Brain::child_from_with_sigma` returns the chosen bucket's
sigma); each trait takes one Gaussian nudge `+= normal() * (bucket.sigma *
trait_mutation_sigma_multiplier)` then is clamped to `[0,1]`. Same bucket, scaled
sigma — the genome draws happen *after* the brain weights on the same RNG so the
stream stays deterministic. `trait_mutation_sigma_multiplier` is a live slider
(default **0.3**).

## Action ring-flash (v2.0 Wave 2a, sim-side state)

Each creature carries a transient `flash_tag` (`FlashTag`) + `flash_ticks`
countdown (5 ticks). A creature records a flash when it acts; when several fire
in one tick the **highest priority wins**: **Killed (red) > CreatedChild (blue) >
Attacked (yellow) > Grazed (green)**; **Born (teal)** is set at spawn. The
countdown decrements each tick in `flash_decay` (the `tick.color_ema` span, kept
for profiler stability — it replaced the old action-EMA color update). The flash
is packed into the snapshot for the renderer (2b draws the ring); see
[`shared-memory-and-protocol.md`](shared-memory-and-protocol.md).

## Code anchors

- `src/world/mod.rs` → `World`, `DevSliders`, `World::step`,
  `World::handle_births` (single-pool split), `World::handle_mating` (species
  sexual mate), `World::new_with_sliders` (single-pool + species seeding).
- `src/world/tick.rs` → `apply_movement_and_repulsion`, `graze`, `attack`
  (same-species gate in species mode), `energy_bookkeeping` (mating-cooldown
  decrement), `collect_deaths`, `flash_decay`.
- `src/creature.rs` → `Genome` (6-trait body genome + rescale-factor helpers +
  `crossed` per-trait crossover), `FlashTag` (ring-flash priority), `CreatureSoA`
  (`genome` / `flash_*` / `species_id` / `mating_cooldown` columns).
- `src/world/species.rs` → `Species`, `SpeciesRegistry` (dynamic id→color
  registry), `placeholder_species_color` (Wave-3 evenly-spread hue).
- `src/world/nn.rs` → `nn_forward_all_chunks`, `build_nn_input`,
  `NnInputLayout` / `NnInputGroup` (composable input-layout descriptor, incl.
  the `BiomeDir` + `CurrCellPenalty` groups; `for_settings(wrap, species)`),
  `BiomeSampler` (biome NN-input view), `decode_action` / `is_valid_action` /
  `ActionGate` (mode-dependent action[2] gate), `chunk_ranges`, `dynamic_chunks`,
  `PickTimings`.
- `src/world/biome.rs` → `generate_biome_grid` (SplitMix64 blob generator from
  `world_seed`), `biome_from_u8`. `World::biome_at` / `movement_penalty_at` /
  `biome_grid_bytes` live in `src/world/mod.rs`.
- `src/world/nn_stats.rs` → `NnStats`.
- `src/world/proximity.rs` → `LUT_RADIUS = 4` / `LUT_DIM`, sector LUT build,
  wall + creature + grass proximity helpers (all wrap-aware via the grid/grass
  `dims`), `compute_creature_proximity_sectors_species` (16-sector same/other
  block fill for species mode).
- `src/brain.rs` → `Brain`, `Brain::founder`, `Brain::forward`,
  `Brain::child_from` (asexual), `Brain::child_from_crossover_with_sigma`
  (sexual per-weight crossover then bucket mutation), `NnTopology` (runtime
  `input_width` field + `with_input_width`), the `lrelu` helper, the
  `NN_OUTPUTS == 5` compile-assert. Width tests: `src/brain_width_tests.rs`.
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
  (u8; generated from `world_seed` in `src/world/biome.rs`),
  the biome movement-penalty balance knobs `K_BIOME_SPEED = 0.6` /
  `K_BIOME_UPKEEP = 0.5` / `K_BIOME_COST = 1.0` + slider defaults
  `WATER_MOVEMENT_PENALTY_DEFAULT = 0.8` / `DESERT_MOVEMENT_PENALTY_DEFAULT = 0.4`
  and `NN_BIOME_DIRS = 4`,
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
  `FULL_GRASS_ON_INIT_DEFAULT = false`,
  `SPECIES_MODE_DEFAULT = false`, `CrossoverMode` enum + `CROSSOVER_MODE_DEFAULT =
  FiftyFifty`, `NN_CREATURE_SECTORS_SPECIES = 16`,
  `STARTING_SPECIES_COUNT_DEFAULT = 10`, `STARTING_SPECIES_MEMBER_COUNT_DEFAULT = 10`,
  `STARTING_SPECIES_MEMBER_VARIANCE_DEFAULT = 3.0` (inert in W3),
  `MATING_COOLDOWN_TICKS_DEFAULT = 200`, `MATING_CONTACT_RADIUS_FACTOR = 1.5`.
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
