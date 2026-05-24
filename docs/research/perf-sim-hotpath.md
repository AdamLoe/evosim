# Evosim Rust Sim — Hot Tick Path Performance Analysis

> Research only — no code was modified. All line numbers refer to the current
> main branch at the time of this analysis.

---

## Summary Table (rank-ordered by savings/effort)

| # | Area | Estimated savings (ms/tick) | Effort | Determinism risk |
|---|------|-----------------------------|--------|-----------------|
| 1 | Vision: cache `sin/cos` per sector per creature (pre-compute once, not per-tick) | 0.4–0.8 | S | None |
| 2 | Repulsion: move `fx`/`fy` buffers off the heap onto `World` (reuse across ticks) | 0.05–0.10 | S | None |
| 3 | Eat/Scavenge: remove `Genome::clone()` inside hot loop | 0.03–0.07 | S | None |
| 4 | Vision: SIMD `f32x8` ray-vs-8-circles batch test per cell | 0.5–1.2 | M | None |
| 5 | Vision: DDA early-out when `best_dist < next_cell_entry_t` (already present but still walks cell 0 twice — see §1) | 0.1–0.3 | S | None |
| 6 | Grid rebuild: eliminate `cursors = self.starts.clone()` allocation (14 401 u32 copy every rebuild) | 0.02–0.04 | S | None |
| 7 | Birth: avoid `Brain::clone()` when child_from already copies parent (double-clone) | 0.01–0.05 | S | None |
| 8 | NN forward: transposed hidden-unit layout to eliminate horizontal reductions | 0.1–0.2 | M | Breaks golden hash |
| 9 | Repulsion: replace `neighbors: Vec` per creature with a flat scratch slice on `World` | 0.02–0.05 | S | None |
| 10 | Energy bookkeeping: `biggest_ever` species-name string clone per tick per creature | 0.01–0.03 | S | None |

---

## 1. Vision Raycast Hot Path

**Source:** `src/vision.rs`

### 1a. `sin`/`cos` computed per-tick, per-creature, per-active-sector

`fill_one` (vision.rs:86–99) walks all 24 sector slots and for each active sector
computes:

```
let theta_center = std::f32::consts::TAU * (s as f32) / (SECTORS as f32); // vision.rs:96
let theta_ray = theta_center + offset;                                       // vision.rs:97
let dx = theta_ray.cos();                                                    // vision.rs:98
let dy = theta_ray.sin();                                                    // vision.rs:99
```

`cos`/`sin` are ~10–20 ns each. A creature with 24 eyes fires 24 active sectors per
tick. At 1 500 creatures, worst-case `eye_count=24`:
- 1 500 × 24 × 2 transcendentals = 72 000 trig calls/tick
- At 15 ns each → ~1.1 ms/tick purely for trig

The eye offsets (`eye_offsets`) are genome-fixed between births. The sector center
angles are fixed per the formula. Neither changes tick-to-tick. Both can be
pre-computed once into a per-creature `[f32x2; 24]` (dx, dy) array whenever the
genome changes (on birth/mutation) and cached. During the tick this array is just
loaded — zero trig.

**Estimated savings:** 0.4–0.8 ms/tick at 1 500 creatures (proportional to mean
eye_count, which naturally concentrates below 12 due to upkeep pressure).
**Effort:** S (add a `SinCos` cache field to `World` or `CreatureSoA`; invalidate on
birth and when eye_offsets mutate).
**Determinism risk:** None — the sin/cos values are deterministic given the genome.

### 1b. `ray_circle_hit` called for every creature in every traversed cell — the inner-inner loop

`raycast` (vision.rs:127–254) walks DDA cells. For each cell, it iterates
`grid.starts[cell_idx]..grid.starts[cell_idx+1]` (vision.rs:192–209). For each
creature index `j` in the cell it calls `ray_circle_hit` (vision.rs:201). This is the
true inner-inner loop.

`ray_circle_hit` (vision.rs:260–277) is a scalar `b`, `c`, `disc`, `sqrt`
sequence — one creature at a time. If a cell contains 4 creatures, 4 sequential
circle tests are performed with 4 sequential `sqrt` calls.

**SIMD opportunity:** if the grid cell can produce up to 8 creature indices, those 8
`cx`, `cy`, `r` values can be loaded into `f32x8` vectors, and the entire
`ray_circle_hit` logic (no branches besides the early-out on `c > 0 && b > 0`)
can be executed in 1 pass using `wide::f32x8`, producing 8 `t` values in parallel.
The minimum non-negative `t` across the 8 lanes is the cell winner. Cells with fewer
than 8 occupants can be padded with `cx = f32::MAX` (guaranteed miss).

This trades 4–8 sequential circle tests + branches for 1 SIMD pass + a lane-minimum
reduce. At high density zones (3+ creatures/cell) the speedup is 3–6x for this
sub-step.

**Estimated savings:** 0.5–1.2 ms/tick (density-dependent; near 0 at sparse
populations, meaningful at 1 500+ creatures in a 600×600 world = avg 0.1
creature/cell but with clustering).
**Effort:** M (requires padding cells to 8, loading positions/radii into f32x8,
restructuring the hit-accumulation loop).
**Determinism risk:** None — SIMD min is associative and the function is
deterministic given positions.

### 1c. DDA traversal: does the code re-visit the start cell?

Yes. The loop begins at the start cell (vision.rs:187) without any advance step. The
first iteration always tests creatures in cell `(cx, cy)` including the caster itself
(filtered by `j == self_idx`, vision.rs:196). The DDA advances at the end of the
loop body (vision.rs:230–236). This is correct, not a bug. The caster filters its
own index.

However, the start cell is also the cell that already contains the caster. Testing
every creature in that cell for the ray is correct but is O(density of start cell).
There is no skip for the starting cell beyond the self-skip. This is unavoidable for
correctness (another creature may overlap the caster's start cell).

### 1d. Early-out correctness

The existing early-out (vision.rs:224–226) returns as soon as `best_dist < next_t`
(the distance to the next cell boundary). This is correct: if the closest hit so far
is nearer than the nearest point of the next cell, no cell further along can produce
a closer hit. The logic is present. One minor note: the check at vision.rs:242–244
uses `t_max_x.min(t_max_y) > max_dist` rather than `next_t > max_dist`. These are
equivalent but the `next_t` variable is re-computed at line 224 as a local; the loop
re-computes `t_max_x.min(t_max_y)` again at line 243. A single local `next_t`
hoisted out of the duplicated min would be cleaner (micro-optimization only).

### 1e. Empty-sector early-out

`fill_one` (vision.rs:86–88) skips inactive sectors via `if s % stride != 0 {
continue; }`. No ray is fired for inactive sectors. This early-out is already present
and correct.

### 1f. Batching multiple rays from one creature into SIMD

Eight rays from a single creature could be batched: load 8 `(dx, dy)` pairs into
SIMD, run 8 parallel DDAs. The challenge is that DDAs have data-dependent control
flow (which axis steps next), so they cannot advance in lock-step. This is a much
harder transformation than the circle-test SIMD above and the benefit is unclear
because each ray traverses different cells. Not recommended as a first step.

---

## 2. Per-Tick Allocations

**Source:** `src/world.rs`, `src/grid.rs`

Every allocation that fires on the hot tick path is listed below at 1 500 creatures.

### 2a. `fx: vec![0.0_f32; n]` and `fy: vec![0.0_f32; n]` in `apply_movement_and_repulsion`

**Location:** world.rs:431–432.
Two `vec!` macros, each allocating `n × 4` bytes. At n=1 500:
- 2 × 1 500 × 4 = 12 000 bytes allocated and zeroed per tick.
- `n` can spike to 2 000+ during population peaks.

These are pure scratch buffers. They can be promoted to fields on `World`:
`World::fx: Vec<f32>` and `World::fy: Vec<f32>` initialized with `Vec::new()` and
grown via `resize` at the start of each tick to `n` (which only re-allocates if `n`
exceeds the previous high-water mark, i.e., almost never).

**Estimated bytes/tick at 1 500:** 12 000 bytes allocated + zeroed.
**Estimated ms savings:** 0.05–0.10 ms/tick (allocator overhead + zeroing cost).
**Effort:** S.
**Determinism risk:** None.

### 2b. `neighbors: Vec<usize>` in `apply_movement_and_repulsion`

**Location:** world.rs:437. Allocated `with_capacity(16)` inside the per-creature
loop. At 1 500 creatures: 1 500 small allocations per tick. Typically stays within
the 16 element inline capacity, so no realloc — but the heap round-trip (malloc +
free) still occurs.

Promoted to a single reusable `Vec<usize>` on `World`, cleared at the top of the
creature loop body.

**Estimated bytes/tick:** 1 500 × ~64 bytes (malloc metadata) = ~96 kB heap
churn.
**Effort:** S.
**Determinism risk:** None.

### 2c. `candidates: Vec<usize>` in `eat_and_scavenge`

**Location:** world.rs:585. `Vec::with_capacity(8)` per eating creature inside the
eat loop. Only allocated for `Action::Eat` creatures, so at any given tick the
fraction is small. Still, same pattern: pool this into a reusable scratch vec on
`World` or pass it as a `&mut Vec<usize>` scratch parameter.

**Estimated bytes/tick:** (% of 1 500 choosing Eat) × 8-element vec.
**Effort:** S.
**Determinism risk:** None.

### 2d. `damage`, `gain`, `cooldown_set`, `attempted_eat`, `attempted_scavenge`, `got_a_bite` in `eat_and_scavenge`

**Location:** world.rs:561–566. Six `vec!` calls per tick, each length `n`:
- `damage: Vec<f32>` = n × 4 bytes
- `gain: Vec<f32>` = n × 4 bytes
- `cooldown_set: Vec<bool>` = n × 1 byte
- `attempted_eat: Vec<bool>` = n × 1 byte
- `attempted_scavenge: Vec<bool>` = n × 1 byte
- `got_a_bite: Vec<bool>` = n × 1 byte

At n=1 500: 6 × (4+4+1+1+1+1) × 375 = 4 500 + 4 500 + 1 500 × 4 = ~15 000
bytes allocated + zeroed per tick. All six can be moved to reusable `World` fields.

**Effort:** S.
**Determinism risk:** None.

### 2e. `dead: Vec<usize>` and `species_lost: Vec<u32>` in `collect_deaths`

**Location:** world.rs:734–737. Two `Vec::new()` per tick. Size bounded by
death count per tick (usually small; rarely > 50). Keep as-is or pool — low
priority.

**Effort:** S, low priority.

### 2f. `cursors = self.starts.clone()` in `SpatialGrid::rebuild`

**Location:** grid.rs:51. `self.starts` has length `HASH_DIM * HASH_DIM + 1` =
14 401 elements × 4 bytes = 57 604 bytes cloned and allocated every `rebuild` call.
`rebuild` is called three times per tick (step 1, inside step 4 after movement,
again at end of step 4 — actually twice, see world.rs:428 and world.rs:508).

Total: 3 × 57 604 bytes = ~173 kB cloned per tick. Fix: add a `cursors:
Vec<u32>` field to `SpatialGrid`, sized identically to `starts`, reused across
rebuilds via `copy_from_slice`.

```rust
// proposed replacement for grid.rs:51
self.cursors.copy_from_slice(&self.starts); // reuse existing allocation
```

**Estimated savings:** 0.02–0.04 ms/tick (allocator + memcopy).
**Effort:** S.
**Determinism risk:** None.

### 2g. `vision: Vec<VisionBuf>` resize in `run_vision_pass`

**Location:** world.rs:1116–1117. Only triggers when `n` exceeds the previous
capacity. After warm-up this is a no-op. Not a per-tick concern.

---

## 3. Soft Repulsion (`apply_movement_and_repulsion`)

**Source:** world.rs:377–509

### 3a. Three grid rebuilds per tick

Step 4 calls `self.grid.rebuild` at world.rs:428 (after position update) and
world.rs:508 (after wall clamp). Step 1 calls `rebuild` at world.rs:176. Total = 3
rebuilds per tick. The second rebuild at line 428 is needed so repulsion queries use
post-movement positions. The third rebuild at line 508 is needed so subsequent steps
(eat, scavenge) use post-clamp positions. There is no redundancy here; all three are
load-bearing. Dirty-tracking would not help because positions always change on
movement.

### 3b. `for_each_in_radius` closure overhead

`for_each_in_radius` (grid.rs:62) is an `impl FnMut(usize)` closure. The call at
world.rs:438 pushes into a local `neighbors: Vec`. Because the closure is
`impl FnMut` (monomorphized at call site), this should be inlined by the compiler.
Verify with `#[inline(always)]` on `for_each_in_radius` if not already inlined — the
function is small (grid.rs:62–79) and the cost is the inner cell traversal, not the
call overhead.

### 3c. Density hotspots causing quadratic pairs within a cell

The repulsion loop (world.rs:432–476) does `j > i` filtering to avoid double-counting.
If a 5-unit cell contains k creatures, the pair count is k(k-1)/2. At density 10
creatures/cell (possible near the world center or birth-cluster hotspots), that's 45
pairs per cell. The `for_each_in_radius` search radius is `ri + SIZE_MAX *
BODY_RADIUS_PER_SIZE` = `ri + 10.0`, which at default size 1.0 spans a 3×3 to 5×5
cell block. The quadratic behavior is bounded by the search radius, but dense birth
clusters (all children jittered from the same parent, FOUNDER_SPLIT_JITTER=50u)
can cause transient local density spikes. The fix — a cap on pairwise repulsion
evaluations per creature — would reduce accuracy but is unlikely to be needed until
population exceeds 3 000.

### 3d. Two-pass structure locks out symmetry optimization

The current code accumulates forces into `fx[i], fy[i]` and `fx[j], fy[j]` within
the `j > i` loop (world.rs:456–473). This is already the symmetric-write pattern
(one pair evaluation writes both sides). It is correct. No improvement available
here.

---

## 4. NN Forward Pass

**Source:** src/brain.rs, src/world.rs:270–374

### 4a. Layout and weight reuse

Each creature has its own `Brain::weights: Vec<f32>` of length `NN_WEIGHT_COUNT =
3 456` (brain.rs:35). Layout is row-major, hidden-unit-contiguous (brain.rs:93–96):
`w_ih(h, i) = weights[h * NN_INPUTS + i]`. The forward pass iterates hidden units
in the outer loop (brain.rs:109) and input chunks in the inner loop (brain.rs:112).
Each hidden unit's row is 136 f32 = 544 bytes, fitting in 9 cache lines (64 bytes
each). 24 hidden units = 24 × 544 = 13 056 bytes for the input-hidden block.

Since each creature has unique weights, there is no weight reuse across creatures.
The per-creature forward pass touches its own 13.7 kB weight block cold on most
ticks (brains are not access-hot relative to the creature's other tick processing).

### 4b. `acc.reduce_add()` — horizontal sum bottleneck

Layer 1 (brain.rs:109–119): for each of the 24 hidden units, 17 SIMD chunks are
accumulated (`acc += xv * wv`) then reduced via `acc.reduce_add()`. That's 24
horizontal sums per creature. At 1 500 creatures: 36 000 horizontal sums per tick.
On x86 `_mm_hadd_ps` / `_mm256_hadd_ps` sequences take 3–6 cycles each. Assuming
4 cycles, 36 000 × 4 = 144 000 cycles ≈ 0.06 ms on a 3 GHz processor.

This is small relative to the raycast cost. The horizontal sum is not the bottleneck.

### 4c. Transposed layout for layer 1 to eliminate reduce_add

If the weights were stored column-major (input-chunk-contiguous across all hidden
units simultaneously), one SIMD chunk of the input could multiply against 8 hidden
units' weight rows simultaneously, accumulating 8 partial sums in parallel without
any horizontal reduction during the main loop. The final result for 8 hidden units
would be in 8 SIMD lanes and only one extraction per lane is needed.

Concretely: with the current layout, for each hidden unit, 17 FMAs + 1
`reduce_add`. With transposed layout: for each input chunk (17 total), one SIMD
load of input chunk × 8 weight rows → 8 accumulators, no reduce_add until the end.
This halves the number of horizontal reductions (from 24 to 3) and potentially
improves throughput on CPUs where horizontal add is a bottleneck.

**Trade-off:** transposing the weight layout breaks the current golden snapshot hash
(the hash includes `weights` in their current order) and requires transposing both
storage and mutation. This is a non-trivial change.

**Estimated savings:** 0.1–0.2 ms/tick.
**Effort:** M.
**Determinism risk:** HIGH — changes `weights` storage order; requires golden hash
re-bootstrap and save-schema migration.

### 4d. FMA count sanity check

Layer 1: 17 chunks × 8 lanes × 24 hidden = 3 264 FMAs per creature.
Layer 2: 3 chunks × 8 lanes × 8 outputs = 192 FMAs per creature.
Total: ~3 456 FMAs per creature.
At 1 500 creatures: ~5.18 M FMAs per tick.

Modern SIMD can sustain ~8 FMAs/cycle in 256-bit mode. At 3 GHz: 24 × 10⁹
FMAs/s / 8 = 3 × 10⁹ FMA-groups/s. 5.18 M / 3 × 10⁹ = 1.7 ms at 100% SIMD
utilization. In practice, with memory latency for per-creature weight loading, actual
cost is higher but the spec estimate of 0.5 ms assumes pipelining and weights
staying warm, which is plausible if creatures are processed sequentially and
weights fit in L2. Measured acceptance test time of ~2.3 s / 10 000 ticks = 0.23
ms/tick total, with raycasts dominant, puts NN cost well below 0.5 ms — consistent
with the spec.

---

## 5. Grid Rebuild

**Source:** src/grid.rs:37–57

The prefix-sum + cursor approach is efficient: one pass to count (grid.rs:39–43),
one prefix sum (grid.rs:44–47), one scatter (grid.rs:48–57). O(N + cells) = O(1 500
+ 14 400) per rebuild.

**The one avoidable cost:** `cursors = self.starts.clone()` (grid.rs:51) clones 14 401
u32 values. This should be a `copy_from_slice` into a pre-allocated `Vec<u32>` field
on `SpatialGrid`, as noted in §2f above.

**Dirty-tracking:** would not help. Every tick, every mobile creature moves (velocity
is always written back). Non-mobile creatures (`move_speed = 0`) still undergo
repulsion which can shift their position. The grid must be rebuilt fresh every time.

---

## 6. Eat and Scavenge

**Source:** world.rs:556–668

### 6a. `Genome::clone()` inside the eat/scavenge match arm

**Location:** world.rs:575 (`let g_i = self.creatures.genomes[i].clone();`) and
world.rs:622 (`let g_i = self.creatures.genomes[i].clone();`).

`Genome` contains:
- ~15 `f32` fields
- 1 `u32`
- 1 `u8`
- `eye_offsets: [f32; 24]` = 96 bytes
- `mutation_rates: TraitMutationRates` = 14 × 4 = 56 bytes

Total Genome size: ~240 bytes. Each `clone()` memcopies ~240 bytes on the heap.
At 1 500 creatures all eating: 1 500 × 240 = 360 kB copied per tick. In practice
only a fraction choose `Action::Eat` or `Action::Scavenge`, so real cost is lower.

The `clone()` is unnecessary. The fields actually used are:
- `g_i.eat_efficiency` → `self.creatures.genomes[i].eat_efficiency`
- `g_i.size` → `self.creatures.genomes[i].size`
- `g_i.bite_reach` → `self.creatures.genomes[i].bite_reach`
- `g_i.scavenge_efficiency` → `self.creatures.genomes[i].scavenge_efficiency`

Replace with direct borrows or copy only the needed fields:

```rust
// Instead of:
let g_i = self.creatures.genomes[i].clone();
// Use:
let eat_eff = self.creatures.genomes[i].eat_efficiency;
let size_i  = self.creatures.genomes[i].size;
let reach   = self.creatures.genomes[i].bite_reach;
```

The borrow checker requires care because later in the loop body `self.creatures.genomes[j]`
is also read. Since `i != j` these are non-overlapping indices — Rust won't allow
simultaneous `&mut self.creatures` and reads through it, but the current code already
handles this by cloning. The fix is to read the needed scalar fields before the inner
loop rather than cloning the whole struct.

**Estimated savings:** 0.03–0.07 ms/tick (reduced allocations + memcopy).
**Effort:** S.
**Determinism risk:** None.

---

## 7. Handle Births

**Source:** world.rs:828–906

### 7a. Double `Brain` clone in `child_from`

`Brain::child_from` (brain.rs:168–186) begins with:

```rust
let mut child = parent.clone(); // brain.rs:169
```

This clones the parent's `weights: Vec<f32>` (3 456 floats = 13.8 kB) plus
`nn_mutation_rate`. The caller in world.rs:847:

```rust
let child_brain = Brain::child_from(
    &self.creatures.brains[i],
    ...
```

passes a reference to the parent, so `Brain::child_from` clones it internally. This
is correct and unavoidable — the child needs a new copy of the weights. The clone
here is load-bearing.

**However**, if speciation occurs (world.rs:875–877):

```rust
let new_species_id = self.species.speciate(
    parent_species,
    child_genome.clone(),
    child_brain.weights.clone(), // second clone of the 13.8 kB weights
    self.tick,
);
```

`child_brain.weights.clone()` at world.rs:876 clones the weights a second time for
the species anchor. This is only triggered on speciation events (rare), so the
amortized cost is low. Still, the `speciate` function signature takes
`Vec<f32>` by value, so the clone is forced at the call site. If `speciate` were
changed to take `&[f32]` and cloned internally, the pattern would be cleaner but
identical in cost.

### 7b. `Genome::mutate_in_place` clones `mutation_rates`

`Genome::mutate_in_place` (genome.rs:100): `let r = self.mutation_rates.clone();`
— clones 14 × 4 = 56 bytes of `TraitMutationRates`. This is fine; it's a small
stack-like copy. No improvement needed.

### 7c. `species_distance` per birth

`handle_births` calls `species_distance` (world.rs:864) for every split attempt,
whether speciation occurs or not. `species_distance` (species.rs:120–132) iterates
`nn1.len()` = 3 456 f32 pairs (the weight subtraction loop, species.rs:126–130).
At `n` births per tick (say 20 births at steady state), 20 × 3 456 = 69 120 f32
subtractions + multiply-adds. This is fast and not a bottleneck. The
`trait_body_distance_sq` (species.rs:134–170) is pure scalar arithmetic with no
allocations. Fine as-is.

---

## 8. Energy Bookkeeping

**Source:** world.rs:671–731

The `energy_bookkeeping` loop is simple scalar arithmetic per creature (upkeep
terms, age, digestion_cooldown decrement). The one non-trivial piece is
`biggest_ever` tracking (world.rs:710–723), which fires a
`self.species.get(...).name.clone()` (a `String` clone) every tick for every
creature whose `size > current_best`. After the first tick, `current_best` converges
quickly and this branch is almost never taken. In early ticks (small population,
monotonically growing sizes from mutations), it fires frequently.

**Potential fix:** store `biggest_ever_size: f32` as a separate field so the `>
current_best` check is a float comparison before dereferencing `biggest_ever`. This
is already partially present (`weirdest_distance: f32` on World at world.rs:81), but
`biggest_ever` currently requires `as_ref().map_or(0.0, |h| h.captured_size)` which
dereferences an `Option<HallOfFame>`. A flat `biggest_ever_size: f32 = 0.0` field
would make this a two-instruction branch.

**Estimated savings:** 0.01–0.03 ms/tick.
**Effort:** S.
**Determinism risk:** None.

The rest of the energy_bookkeeping loop is a textbook linear scan over hot SoA
fields (`energy`, `cumulative_upkeep`, `age`, `digestion_cooldown`). All these
fields are `Vec<f32>` or `Vec<u32>` in `CreatureSoA`. Cache behavior is excellent;
no improvements needed.

---

## 9. HoF / Weirdest Distance Check

**Source:** world.rs:783–800, species.rs:120–132

The weirdest check runs only when a creature dies (`collect_deaths`, world.rs:783)
AND that creature has `age >= 500`. It calls `species_distance` which iterates 3 456
floats. This is O(deaths with age ≥ 500 per tick), typically 0–5 per tick. The
`trait_body_distance_sq` function (species.rs:134–170) allocates nothing. The brain
distance loop (species.rs:126–130) allocates nothing. Total cost is negligible.

---

## 10. Determinism Budget

These items exist solely to preserve determinism per §16 and carry a real but
quantifiable cost:

### 10a. Single `SimRng` threaded through all tick steps

The single `SimRng` (xoshiro256++) is passed by `&mut self.rng` through every tick
function in fixed order. This prevents parallel execution of any step that consumes
RNG (genome mutation, NN mutation, birth jitter). The rayon path works around this
via per-chunk sub-RNGs, but this requires enabling the `threads` feature and
re-bootstrapping the golden hash.

**Cost:** The RNG itself is cheap (xoshiro256++ is ~1 ns/call). The constraint is
architectural: vision and NN forward (no RNG) are safe to parallelize even in single-
threaded mode from a pure physics standpoint, but the current sequential chunk loop
keeps them on the critical path. Enabling the `threads` feature is the right
mitigation and is already spec'd.

### 10b. Three `grid.rebuild()` calls per tick (required, not redundant)

As noted in §5, all three rebuilds are load-bearing. The third rebuild (world.rs:508)
uses post-wall-clamp positions which differ from post-repulsion positions for
creatures that hit a wall. Removing it would use stale positions for eat/scavenge
queries, breaking determinism relative to spec §3.5 step 6.

### 10c. `finalize_extinctions` runs `HashSet::new()` every tick call

`finalize_extinctions` (world.rs:909–934) allocates a `HashSet<u32>` every tick_once
call (world.rs:910). At 1 500 creatures it is O(N) with constant-factor overhead from
hash insertions. This is only called from `tick_once` (world.rs:937), not from
the inner step function. Cost: small but non-zero. It could be replaced with a
bitset (species IDs are small integers) or a sorted scratch Vec, but the savings
would be marginal (< 0.01 ms/tick).

---

## Recommendations: Top 3 to Ship First

**1. Pre-compute sector `sin`/`cos` per creature (§1a).**
Pure win: no determinism risk, S effort, 0.4–0.8 ms/tick savings from eliminating
1 500+ transcendental pairs on the dominant raycast hot path. Add `[(f32, f32); 24]`
to `CreatureSoA` (or `World`), recompute on birth/genome change. This alone can
shave ~10–20% off the acceptance test wall time.

**2. Promote `fx`/`fy` and the six `eat_and_scavenge` scratch vecs to `World`
fields (§2a, §2d).**
All are S effort, collectively eliminate ~30 kB of per-tick heap churn and the
associated allocator round-trips. No determinism risk. Group the fix into a single
"scratch buffer" commit — add 8 fields to `World`, replace the in-function `vec!`
with `resize_with(n, ...)`.

**3. Eliminate `Genome::clone()` in `eat_and_scavenge` (§6a).**
S effort, no risk. The pattern (`let g_i = self.creatures.genomes[i].clone()`) copies
~240 bytes that are almost entirely unused — only 3–4 fields are read. Replace with
direct field reads. Small savings individually, but it also cleans up the borrow
structure and removes a subtle implicit dependency on `Genome` being `Clone` inside
a loop that also reads `genomes[j]`.

The SIMD circle-test batching (§1b) is the highest-ceiling optimization (~1.2
ms/tick) but is M effort and requires careful cell padding and SIMD load
restructuring. Ship the three S-effort changes first to confirm the performance
floor, then evaluate whether the SIMD circle-test is worth the complexity.
