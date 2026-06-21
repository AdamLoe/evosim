# Species and sexual mating

Species registry, N-anchor seeding, sexual Mate crossover, and the
same-species attack gate.

## What it is

`species_mode` is a construction-only `DevSliders` switch (default off)
that selects between two regimes:

| | single-pool (off) | species (on) |
|---|---|---|
| Population | Asexual, one gene pool. | N seeded species, founders clustered at anchors. |
| Reproduction | Asexual `Split`. | Sexual `Mate` (same-species, contact radius, initiator-gated). |
| Attack target | Any creature. | Other-species only (no cannibalism). |
| Body color | Lineage hue (heritable). | Per-species palette color. |
| Creature NN sectors | 8 unified. | 16 (8 same-species + 8 other-species). |

The two modes never coexist in one run — switching requires a restart.

## SpeciesRegistry

`crates/evosim/src/world/species.rs` → `Species`, `SpeciesRegistry`

The registry is a `Vec<Species>` keyed by `id: u16`. Each entry holds the
id, a `color_u32`, and a `name: String`. It is designed **dynamic**
(add/remove/recolor-capable) so future split/merge work needs no protocol
change, even though the current mode seeds a fixed set at construction.

Colors come from `SPECIES_PALETTE` — a hand-spread 10-hue saturated
palette that is the single source of truth for both the per-creature
`color_u32` the sim writes and the polled species-to-color table. The
canvas and the monitor read the same palette; they cannot drift. Labels
are `Species-A..J`.

Each creature carries a `species_id: u16` SoA column (unused/0 in
single-pool mode) — see `crates/evosim/src/creature.rs` → `CreatureSoA`.

## N-anchor seeding

`crates/evosim/src/world/mod.rs` → `new_with_sliders`, `new_with_sliders_topology`

Seeding is **deterministic from `world_seed`** via a dedicated setup PRNG
(`SimRng::from_u64(world_seed ^ SEEDING_PRNG_SALT)`) — a distinct stream
from both the biome generator (raw-SplitMix64) and the sim RNG
(string-seeded xoshiro). The same `world_seed` reproduces the same tick-0
layout; the run still varies per launch.

Algorithm:

1. **Anchors** — pick `starting_species_count` (default 10) points spread
   across the world by rejection-sampling against a minimum spacing
   (`ANCHOR_MIN_SPACING_FRAC × world_size`, relaxed when infeasible).
2. **Canonical first member** per anchor — random founder brain with a
   distinct seed hue evenly spread across the color wheel
   (`canonical_hue = species_index / species_count`). There is no genome
   bias in seeding.
3. **`starting_species_member_count - 1` more founders** clustered within
   `FOUNDER_CLUSTER_RADIUS_FRAC × world_size` of the anchor (contact-mating
   reachable at tick 0). Each is the canonical brain passed through
   one per-birth bucket draw, with that bucket's rate and sigma both
   multiplied by `starting_species_member_variance` (default 3.0). This
   spreads the founding brains so selection acts from tick 0.
   Helper: `Brain::founder_spread_with_sigma`.

Defaults: 10 species × 10 founders = 100 starting pop. A species that goes
extinct stays extinct — no respawn.

## Mate mechanic

`crates/evosim/src/world/mod.rs` → `handle_mating` (runs in the `tick.handle_births` span in species mode)

The initiator triggers `Mate` when: it chose `action[2]`, is off mating
cooldown, and has `energy ≥ mating cost` (= `split_gift`). On fire it scans
for the **first** same-species creature (by `species_id` only — no
genome-distance gate) within the contact radius
`(r_i + r_j) × MATING_CONTACT_RADIUS_FACTOR × mate_reach_multiplier`.
First-found wins (consistent with Attack; no nearest scan).

All cost and cooldown fall on the initiator only:
- pays the mating cost;
- enters `mating_cooldown_ticks` (default 200, live slider) cooldown via
  the `mating_cooldown` SoA column (decremented in
  `crates/evosim/src/world/tick.rs` → `energy_bookkeeping`, distinct from
  `digestion_cooldown`).

The partner pays nothing, needs no energy, and need not be off cooldown. An
invalid `Mate` pick falls through to Graze, exactly like Split.

The child spawns at the parent midpoint (wrap-aware; no habitable-cell
check) via crossover-then-mutate with the parents' shared `species_id`.

**Mating reach slider:** `mate_reach_multiplier` (live f32, default 1.0)
widens the contact radius, letting creatures mate from further apart.

## Crossover

`crates/evosim/src/brain/mod.rs` → `child_from_crossover_with_sigma`

Crossover (`crossover_mode`: `average` or `fifty_fifty`, default
`fifty_fifty`) is applied to **brain weights** only, then the same per-birth
bucket mutation machinery runs. RNG draw order is fixed: brain crossover →
brain bucket mutation, all off a pre-rolled per-pair seed. Lineage `hue` is
inherited as the midpoint of the two parents' hues plus a small Gaussian
nudge.

Single-pool `Split` is unchanged (asexual, no crossover).

## Same-species attack gate

`crates/evosim/src/world/tick.rs` → `attack`

In species mode, `Attack` refuses same-species targets (sim-side gate; no
cannibalism). Single-pool `Attack` hits any creature (unchanged). Attack
effectiveness is a constant — there is no genome to modulate it.

## Action gate (mode-dependent action[2])

`crates/evosim/src/world/nn.rs` → `ActionGate`, `decode_action`, `is_valid_action`

The NN output shape is unchanged: `action[2]` decodes to `Action::Split` in
both modes, but its **meaning** and validity gate differ by mode. In
single-pool this slot is `Split`; in species mode the same logit is `Mate`.
The logit order is stable across modes.

## Invariants and gotchas

- `species_mode` and `crossover_mode` are **construction-only** — they shape
  world topology and seeding at construction; changing them requires a restart.
- The mating contact radius uses **summed body radii**, not the 20u
  perception range. A newly seeded world has founders close enough to mate
  at tick 0.
- The seeding PRNG is a *third* independent stream (distinct from the biome
  generator's raw-SplitMix64 and the sim's xoshiro). Mixing them would break
  the invariant that `world_seed` pins the biome layout and the tick-0
  species layout independently.
- `starting_species_member_variance` scales both rate and sigma — tuning it
  high gives early selection pressure but also increases variance in which
  founding lineages survive.
- Seeding is no longer genome-biased. Founders get distinct hues and random
  brains; the biome under the anchor no longer shapes the founder genetics.

## Code anchors

- `crates/evosim/src/world/species.rs` → `Species`, `SpeciesRegistry`, `SPECIES_PALETTE`
- `crates/evosim/src/world/mod.rs` → `handle_mating`, species seeding in `new_with_sliders_topology`
- `crates/evosim/src/world/tick.rs` → `attack` (same-species gate), `energy_bookkeeping` (cooldown decrement)
- `crates/evosim/src/world/nn.rs` → `ActionGate`, `decode_action`, `is_valid_action`
- `crates/evosim/src/brain/mod.rs` → `child_from_crossover_with_sigma`, `founder_spread_with_sigma`
- `crates/evosim/src/creature.rs` → `CreatureSoA` (`species_id`, `mating_cooldown`, `hue` columns)
- `crates/evosim/src/constants.rs` → `SPECIES_MODE_DEFAULT`, `CrossoverMode`, `STARTING_SPECIES_COUNT_DEFAULT`, `STARTING_SPECIES_MEMBER_COUNT_DEFAULT`, `STARTING_SPECIES_MEMBER_VARIANCE_DEFAULT`, `MATING_COOLDOWN_TICKS_DEFAULT`, `MATING_CONTACT_RADIUS_FACTOR`, `MATE_REACH_MULTIPLIER_DEFAULT`

## See also

- [`simulation-core.md`](simulation-core.md) — tick step order, `World`/`CreatureSoA` overview, lineage `hue`, FlashTag
- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) — snapshot creature stride, species color in the packed render word
- [`render-pipeline.md`](render-pipeline.md) — how species colors appear on canvas
- [`../decisions/sim.md`](../decisions/sim.md) — rationale for the mode-switch design and seeding choices
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- Global authoring rules: `~/.agentdocs/rules/authoring-rules.md`
