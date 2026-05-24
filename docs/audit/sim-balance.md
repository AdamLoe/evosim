# Simulation Balance Audit

Scope: behavioral knobs in `src/constants.rs` + logic in `creature.rs`, `genome.rs`,
`brain.rs`, `sun.rs`, `carrion.rs`, `species.rs`, `hof.rs`, plus relevant
`world.rs` sites (photosynth, split, deaths, upkeep, HoF, speciation).

## 1. Energy economy — split is a near-death sentence for the parent

`world.rs:935` requires `energy >= SPLIT_THRESHOLD=50`, then subtracts `SPLIT_THRESHOLD`
*plus* a gift (clamped to `SPLIT_GIFT_MAX=30`). Net: a parent splitting at the
threshold ends at **0 energy** and dies next tick to upkeep. Even at full energy
the parent is left at `E - 80` (≤ 20 for a creature splitting at E=100).

- **Fix:** decouple gating from cost. Use a higher gating threshold
  (e.g. `SPLIT_THRESHOLD=80`, `COST_SPLIT=30`, keep `SPLIT_GIFT_MAX=30`) so a parent
  that chose Split walks away with ≥ 20 e and a real survival margin. The NN
  founder wiring already biases Split at high energy, so a higher gate is in-spec.

## 2. Photosynth payout under-rewards size, demand crowding starves big plants

`PHOTO_GAIN_COEFF=0.5` × eff × size; upkeep grows ~`0.04 × (size-1)` per unit *plus*
`0.04 × move_speed` etc. A size-10 photo specialist demands 5.0 e/cell yet a single
cell's `current` saturates at gradient + hotspot (~3–7 e). Two big photo creatures in
one cell drop `factor` to ~0.3 and both lose to upkeep. Sun cells are 30×30u — easy
for 5+ creatures to share.

- **Fix:** raise `PHOTO_GAIN_COEFF` to ~0.8–1.0, *or* reduce `SUN_CELL` to 20
  so contention is more local, *or* sub-linear demand
  (`want = COEFF × eff × sqrt(size)`). Otherwise size > ~3 is selected against
  whenever neighbors are nearby.

## 3. Past-lifespan penalty is effectively infinite in one "day"

`PAST_LIFESPAN_MULT=4.0` per 1000 ticks past `max_age`. Founder max_age=5000;
common evolved lifespans 100–50000. 1000 ticks past → 4×, 5000 ticks → 1024×.
Combined with `up.min(1e6)` the death is instant. Fine as a hard cap, but means
`max_age` mutation is *all upside* (10% σ trivially reaches 50k cap) — no real
trade-off. Lifespan upkeep tax `0.01/1000 ticks` is too small to push back.

- **Fix:** raise `UPKEEP_LIFESPAN_PER_1K` to ~0.05–0.10 so genome-encoded long
  lifespan is metabolically expensive. Currently a max_age=50000 creature pays
  only +0.5/tick for 10× the lifespan — trivially affordable.

## 4. Mutation σ is multiplicative ⇒ trapped at zero

`genome.rs:101–112` sets `sigma_X = 0.1 * value.max(floor)`. Founder has
`eat_efficiency=0`, `scavenge_efficiency=0`, `move_speed=0`, `vision_range=0`,
`eye_count=0`, `armor=0`. The `.max(0.1)` floor on eat/scav/move keeps σ ≥ 0.01 so
they can escape; **but** `sigma_vision = 0.1 * vision_range.max(1.0)` only allows
±~0.3 jumps from zero. With `eye_count=0` no eye_offsets ever mutate
(loop is `0..n_active`). A lineage that loses all eyes via mutation can never grow
them back to non-zero usefulness because vision_range starts at near-floor.

- **Fix:** Either (a) add absolute floors to every σ (e.g. `sigma_X = (0.1 * x).max(0.05)`),
  or (b) when a trait is at the min bound, force an initial "kick" σ. Currently the
  founder can almost never evolve carnivory because `eat_eff=0` plus
  `UPKEEP_BASE`-only upkeep makes photosynth strictly dominant.

## 5. Brain founder wiring + linear outputs ⇒ split spam at high energy

`brain.rs:62–75` adds `+10` to `w_ih[0][0]` and `+10` to `w_ho[Split][hidden0]` and
`+5` to all Photo output weights. With energy_frac=1.0 and hidden[0] = ReLU(10·1 +
small noise) ≈ 10, Split logit = ~100 vs Photo ≈ 5×something. So **every founder
splits the instant energy crosses 50** with no exploration window. The "high
energy → split" intent is right but the magnitude crushes any learned alternative.

- **Fix:** drop `NN_FOUNDER_SPLIT_ENERGY_WEIGHT` from 10 → 2 (~tanh-scale signal),
  and `NN_FOUNDER_ENERGY_SENSOR_STRENGTH` from 10 → 3. Mutation σ=0.02 cannot
  budge a +10 weight in a reasonable number of generations.

## 6. NN architecture: 24 hidden, ReLU, no biases — weak gradient

136→24→8 with ReLU is very small *and* has no biases. Hidden units that drift
below zero are dead (can't recover via bias). With σ=0.02 mutations, dead
hidden units are absorbing states. Combined with the +10 founder wiring,
most of the brain is effectively a single-feature linear policy.

- **Fix:** bump `NN_HIDDEN` to 32 (still 8-aligned), and/or switch to LeakyReLU
  in `forward`/`forward_scalar` (`sum.max(0.1 * sum)` — same SIMD-friendly shape).
  Alternative: add a single learned bias vector (24 floats), still <1% weight
  count growth.

## 7. Speciation threshold won't fire from body drift alone

`SPECIES_THRESHOLD=6.0`, `SPECIES_W_BODY=3.0`. A child differs from anchor by at
most ~0.1 × value per trait per birth — single-mutation steps are tiny. Single
`eye_count` jump = `SPECIES_W_BODY × sqrt(SPECIES_EYE_JUMP_COST²) = 4.5` →
still below threshold without other drift. A few key traits drifting in tandem
needed.

Since species *anchor* is fixed at speciation time, distance accumulates with
*every* generation against the same anchor — speciation will fire eventually.
But: `species_distance` uses sigma denominators that don't match mutation σ scales
(e.g. `max_age / 2000`: a 50000-tick lineage is 25 units from a 0-tick anchor).
Eye_count jumps already cost 4.5; pair with any single ~moderate drift = speciation.
Risk: speciation explodes once first eye_count change happens because every later
birth-from-anchor is far. Healthier?

- **Fix:** raise `SPECIES_EYE_JUMP_COST` to ~2.0 (cost² = 4.0) and add anchor
  *re-centering* when child diverges enough — currently anchors never move so old
  species become dust. Also: `species_distance` doesn't weight brain by sqrt(N)
  the way body uses σ — `brain = ||Δw|| / sqrt(N)` averages to ~one weight unit;
  with `SPECIES_W_BRAIN=1.0` brain contribution maxes ~0.6 vs body's 20+.
  Brain is effectively unused for speciation. Either drop `SPECIES_W_BRAIN` to
  0 (admit it) or raise to ~5 and stop dividing by sqrt(N).

## 8. Hall-of-Fame seeding bugs

- `biggest_ever` (`world.rs:805–821`) is updated *inside* `energy_bookkeeping` and
  comments say "v1 has no in-life growth" — but `size_i > max_size_reached[i]`
  is the only growth signal *in genome*, and genome.size can only change at
  birth. So `biggest_ever` is set per tick for every alive creature whose
  `g_size` exceeds the global champion; **it captures the current biggest
  creature alive**, not the biggest ever born. That works while the champion
  is alive, but when it dies the HoF is never demoted — fine. However the
  scan runs O(N) per tick when the comparison only ever changes on birth.
  - **Fix:** lift to `handle_births` (check newborn only).

- `weirdest` requires `age >= 500` (line 881) and only updates at death. A
  creature that lives forever never enters the HoF. Combine with §3 — anything
  past `max_age` dies in <1000 extra ticks — and `weirdest` is effectively only
  for short-lived divergent lines. Long-lived weird creatures are invisible.
  - **Fix:** check `weirdest` on a per-N-tick scan of the living too (e.g. every
    DAY_TICKS=1000), or drop the age gate.

- `last_survivor` is overwritten on every death (line 858). The "final overwrite
  is the latest death" comment is correct only if deaths are processed in tick
  order — they are. Fine, but `captured_size` reads `g_clone.size`, not
  `max_size_reached`. Cosmetic.

- `first_mover_snapshot` is captured only once (line 482), inside the
  movement loop — fires at first creature whose `distance_travelled >= 5.0`.
  Founder `move_speed=0` so first mover is the first *mutated* offspring
  with non-zero move_speed. Sound.

## 9. Carrion math edge cases

- `CARRION_POOL_COEFF=0.3 × cumulative_upkeep`, capped at `CARRION_POOL_CAP=60`.
  A long-lived size-1 creature (5000 ticks × 0.15 upkeep) = 750 upkeep × 0.3
  = 225 → caps at 60. **Cap dominates almost immediately** (after ~1300 ticks);
  longevity stops contributing carrion. Fine if intended, but `cumulative_upkeep`
  past the cap is wasted bookkeeping.
- `decay_carrion` returns *remaining* pool to the sun cell only when `age >
  CARRION_MAX_AGE`. If `pool` drains to 0 first (scavenged out), the early exit
  branch sets `remaining = self.carrion[i].pool.max(0.0)` then refunds *nothing*
  because pool is already 0. Correct.
- **Edge case:** carrion is dropped at exact cell of death (`sun_cell` index
  captured at death); if the creature died near a wall the cell is fine. No issue.
- Sun gradient west=3.0, east=1.0 — the west half is 3× more productive.
  Hotspots peak +4 on top of gradient (so up to 7). The whole eastern half
  approaches the photo-payout/upkeep equilibrium calculated in §2 — expect a
  starvation-on-east bias. Possibly intentional ("press west"), but worth
  surfacing as a default.

## 10. Misc

- `CARRION_OVERLAP_NORM_BASE=4.0`: body radius = size × 1.0, so size-1 creatures
  see only carrion within ~1u. With `HASH_CELL=5.0`, every overlap test scans
  one cell. Norm base of 4 means input saturates at 4 overlapping corpses — fine.
- `EYE_VALID=[0,2,3,4,6,8,12,24]`: eye_count=0 is "blind". From `eye_count=0` the
  only adjacent mutation is `EYE_VALID[1]=2` — irreversible to 0 unless we add
  it back as adjacent. But the founder *is* at 0 with `vision_range=0`. So
  the founder's `recompute_eye_trig_at` short-circuits (line 264) and the
  founder will never benefit from random vision-range mutation until eye_count
  also flips to 2 *and* vision_range mutates above 0 in the same lineage.
  Probability gated by both mutation rates AND threshold magnitudes. See §4.
- `UPKEEP_VISION_COEFF * vision_range²`: vision_range=80 ⇒ +6.4 upkeep — that's
  ~40× the base. Vision is *very* expensive. Combined with eye upkeep
  (24 eyes × 0.02 = 0.48), a fully-decked predator pays ~7+ per tick *before*
  movement and size. Realistic eat_efficiency to pay this back needs huge prey.
  May explain "no carnivores evolve" symptom.
  - **Fix:** drop `UPKEEP_VISION_COEFF` to `0.0005` (4× cheaper) or make it linear.

## Summary of recommended constant changes

| Constant | Current | Proposed | Reason |
|---|---|---|---|
| `SPLIT_THRESHOLD` | 50 | 80 | §1 give parent margin after split |
| `COST_SPLIT` | 50 | 30 | §1 (use threshold as gate, not cost) |
| `PHOTO_GAIN_COEFF` | 0.5 | 0.8 | §2 size selection currently negative |
| `UPKEEP_LIFESPAN_PER_1K` | 0.01 | 0.05 | §3 max_age mutation has no cost |
| `UPKEEP_VISION_COEFF` | 0.001 | 0.0005 | §10 vision economically unaffordable |
| `NN_FOUNDER_ENERGY_SENSOR_STRENGTH` | 10.0 | 3.0 | §5 too dominant vs σ=0.02 |
| `NN_FOUNDER_SPLIT_ENERGY_WEIGHT` | 10.0 | 2.0 | §5 same |
| `NN_HIDDEN` | 24 | 32 | §6 widen + consider LeakyReLU |
| `SPECIES_W_BRAIN` | 1.0 | 0.0 or 5.0 | §7 currently dead weight |
| `SPECIES_EYE_JUMP_COST` | 1.5 | 2.0 | §7 stronger categorical signal |

## Logic fixes

- `Genome::mutate_in_place` (genome.rs:101+): replace `0.1 * value.max(floor)` σ with
  `0.1 * value + 0.05` so traits at zero can still take real steps (§4).
- `decode_action` / `pick_action_d`: argmax has no temperature, NN is fully
  deterministic — no exploration. Consider Boltzmann sampling at low population
  (pop < 10) using the dev RNG. Currently extinction is hard to recover from.
- `World::collect_deaths` HoF update path (world.rs:881–899) should also fire
  for live long-lived creatures (e.g. in a per-DAY_TICKS sweep), not only on
  death (§8).
- `biggest_ever` update should move from `energy_bookkeeping` (per-tick O(N))
  to `handle_births` (O(births)) — size only changes at birth (§8).
- Add anchor re-centering for species (or periodic anchor refresh after K
  generations) so distance accumulation against frozen founder anchors doesn't
  trap the lineage tree (§7).
