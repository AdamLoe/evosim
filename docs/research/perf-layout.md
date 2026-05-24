# evosim — Data Layout, Cache, and SIMD Perf Research

**Date:** 2026-05-24  
**Scope:** `src/creature.rs`, `src/genome.rs`, `src/brain.rs`, `src/world.rs`, `src/vision.rs`, `src/grid.rs`, `src/wasm_api.rs`, `src/events.rs`  
**Population model:** 1 500 creatures (typical peak), target 2 000 (milestone cap)

---

## Summary Table

| # | Opportunity | Expected Impact | Effort | Risk |
|---|---|---|---|---|
| 1 | Genome SoA split (hot fields into parallel arrays) | High — cuts genome working set from 299 KB to 41 KB for per-tick loops | Medium | Low: purely additive; AoS `Genome` stays for mutation/speciation |
| 2 | Contiguous brain weight slab (`Vec<f32>` pool, stride = 3456) | Medium — eliminates heap fragmentation; improves prefetcher locality for multi-creature NN burst | Medium | Medium: changes Brain ownership model |
| 3 | Grid rebuild cursor scratch avoidance (reusable scratch buffer) | Medium — eliminates 56 KB heap alloc × 3 per tick | Low | Low: purely internal to `SpatialGrid` |
| 4 | Vision copy elimination in `build_nn_input` | Low-Medium — saves 703 KB of memcpy per tick at 1 500 creatures | Medium | Low (wasm) / High (spec layout): breaks v6 §E NN input ordering |
| 5 | SIMD on photosynth two-pass (SoA prerequisite) | Low-Medium — requires SoA genome as prerequisite (#1); then ~8x throughput on `want = coeff * eff * size` | Low (once SoA exists) | Low |
| 6 | SIMD on energy bookkeeping (SoA prerequisite) | Low — saves ~0.08 ms/tick at 1 500 creatures | Low (once SoA exists) | Low |
| 7 | Pre-allocate `CreatureSoA` to `MAX_POPULATION` at init | Low — eliminates realloc and wasm memory growth during play | Low | Low |
| 8 | Brain clone pool / in-place mutation to reduce birth alloc | Low-Medium — at 50 births/tick saves ~675 KB heap pressure | High | High: complicates ergonomics |
| 9 | Cap `EventLog::all` with ring-overwrite | Low (currently disabled) | Low | Low |

**Top two to ship first:** [#1 Genome SoA split] then [#3 Grid cursor buffer] — highest ratio of impact to risk.

---

## 1. Genome AoS-within-SoA: the Biggest Cache Problem

### Current layout

`CreatureSoA` (`src/creature.rs:43–63`) stores `genomes: Vec<Genome>` — a contiguous array of `Genome` structs in AoS form.

`Genome` (`src/genome.rs:54–70`) layout (with Rust's default repr):

```
offset   0:  size              f32   4 B
offset   4:  max_age           u32   4 B
offset   8:  photosynth_eff    f32   4 B
offset  12:  eat_efficiency    f32   4 B
offset  16:  scavenge_eff      f32   4 B
offset  20:  move_speed        f32   4 B
offset  24:  eye_count         u8    1 B
offset  25:  [pad]                   3 B
offset  28:  eye_offsets    [f32;24] 96 B
offset 124:  vision_range      f32   4 B
offset 128:  armor             f32   4 B
offset 132:  bite_reach        f32   4 B
offset 136:  pigment_r         f32   4 B
offset 140:  pigment_g         f32   4 B
offset 144:  pigment_b         f32   4 B
offset 148:  mutation_rates  [14×f32] 56 B
TOTAL:  204 bytes
```

At 1 500 creatures the full `genomes[]` working set is **299 KB** — exceeds a typical 256 KB L2 cache.

### Hot fields and their offsets

Every tick, the following passes touch `genomes[i]`:

| Pass | Fields read | `world.rs` location |
|---|---|---|
| `photosynth_two_pass` (5a + 5b) | `size`, `photosynth_efficiency` | lines 519, 533 |
| `apply_movement_and_repulsion` | `move_speed`, `size` | lines 383, 435, 446, 481 |
| `energy_bookkeeping` | `size`, `max_age`, `move_speed`, `eye_count`, `vision_range`, `eat_efficiency`, `scavenge_efficiency`, `armor` | lines 676–694 |
| `build_nn_input` | `size`, `max_age`, `move_speed`, `eye_count` | lines 1170–1177 |
| `count_carrion_overlap` / `compute_is_at_wall` | `size` | lines 1068, 1101 |
| `eat_and_scavenge` | `eat_efficiency`, `size`, `bite_reach`, `scavenge_efficiency` | lines 576–628 |

The seven hottest per-tick fields (`size`, `photosynth_efficiency`, `eat_efficiency`, `scavenge_efficiency`, `move_speed`, `vision_range`, `eye_count`) total **29 bytes** per creature (counting `eye_count` as 1 B). In SoA those seven arrays fit in **≈41 KB** — inside L2. The full AoS genome needs **299 KB** — L3 territory.

The `eye_offsets [f32; 24]` at offset 28 occupies 96 bytes and sits between the six tightly-packed front fields and the trailing hot fields (`vision_range`, `armor`). This forces the CPU to drag three extra cache lines even when it only needs `size` and `photosynth_efficiency`.

### Proposed split

```rust
// src/creature.rs — add alongside genomes:
pub photo_eff:   Vec<f32>,  // genomes[i].photosynth_efficiency
pub eat_eff:     Vec<f32>,  // genomes[i].eat_efficiency
pub scav_eff:    Vec<f32>,  // genomes[i].scavenge_efficiency
pub move_speed:  Vec<f32>,  // genomes[i].move_speed
pub size:        Vec<f32>,  // genomes[i].size
pub vision_range: Vec<f32>, // genomes[i].vision_range
pub eye_count_hot: Vec<u8>, // genomes[i].eye_count
```

Keep `genomes: Vec<Genome>` for mutation (`mutate_in_place`), speciation (`species_distance`), birth cloning, and serialization — those paths touch the whole struct and are not in the hot inner loop.

Sync the hot arrays on every mutation and birth (trivial: 7 field writes). Add a `debug_assert!` helper that checks the hot arrays match `genomes[i]` for N-to-verify.

**Estimated gain:** At 1 500 creatures, the per-tick genome-read working set drops from 299 KB (cold in L3, ~40 ns latency) to ≤41 KB (warm in L2, ~4 ns latency). Passes that were bottlenecked on genome loads (`energy_bookkeeping`, `photosynth_two_pass`, `apply_movement_and_repulsion`) should see a 2–5× reduction in cache-miss stalls. This is the single highest-impact layout change.

---

## 2. Brain Weight Layout: Scattered Heap vs. Contiguous Slab

### Current layout

`Brain` (`src/brain.rs:34–37`):

```rust
pub struct Brain {
    pub weights: Vec<f32>,   // length = NN_WEIGHT_COUNT = 3456
    pub nn_mutation_rate: f32,
}
```

Each `Brain` owns a separately heap-allocated `Vec<f32>`. At 1 500 creatures, there are 1 500 distinct heap allocations of 13 824 bytes each, totalling **≈20.7 MB** of brain weights.

The NN forward pass (`src/brain.rs:101–134`) streams linearly through `self.weights[0..NN_WEIGHT_COUNT]`. Because the weights are in one contiguous slice per brain, the hardware prefetcher can stream them effectively — there is no gather/scatter penalty within a single forward call. The penalty is **between creatures**: after finishing creature 0's brain, the next access is `brains[1].weights.ptr` → a pointer chase to a completely different heap region, flushing the stream prefetch.

### Contiguous slab layout

Alternative: allocate all brain weights in a single `Vec<f32>` with stride `NN_WEIGHT_COUNT`:

```rust
// Conceptually:
pub struct BrainSlab {
    pub data: Vec<f32>,  // length = n * NN_WEIGHT_COUNT
    pub nn_mutation_rate: Vec<f32>,  // length = n
}
// Access: &data[i * NN_WEIGHT_COUNT .. (i+1) * NN_WEIGHT_COUNT]
```

Benefits:
- Eliminates 1 500 individual heap allocations; reduces allocator overhead and fragmentation.
- Sequential `for i in 0..n { forward(&data[i*N..(i+1)*N], ...) }` lets the prefetcher pipeline the next brain's weights while computing the current one — because the physical memory is contiguous.
- `Brain::child_from` currently calls `parent.clone()` (`src/brain.rs:169`), which allocates 13 824 bytes per birth. With a slab, a child slot is pre-allocated; mutation is in-place with a memcpy from parent slice to child slice. No heap allocation on birth.

Concerns:
- Changes the `Brain` ownership model significantly; `World` or `CreatureSoA` must own the slab, not `Brain`.
- `swap_remove` on creature death requires swapping two fixed-size slabs, not just a Vec pointer — O(N_WEIGHTS) instead of O(1). At 3 456 f32 × 4 bytes = 13 824 bytes per swap, at ~10 deaths/tick this is 138 KB of memory moves. Still cheaper than 1 500 allocator round-trips.
- Serialization and snapshot paths (`src/save.rs`, `src/world.rs:967–996`) rely on per-creature `Brain` structs. Would need adaptation.

**Estimated gain:** Primarily allocator pressure and fragmentation relief. The forward-pass itself is already SIMD-accelerated (`wide::f32x8`). This is a medium-effort refactor with moderate impact.

---

## 3. Grid Rebuild: Cursor Vec Clone (Low-Effort Win)

### Current code

`SpatialGrid::rebuild` (`src/grid.rs:37–57`):

```rust
pub fn rebuild(&mut self, xs: &[f32], ys: &[f32]) {
    // ...
    let mut cursors = self.starts.clone();  // line 51: allocates 14 401 × 4 = 56 KB
    // ...
}
```

Every call to `rebuild` heap-allocates a fresh 56 KB `Vec<u32>` for the cursor scratch, then drops it. `rebuild` is called **three times per tick** (`src/world.rs:176, 428, 508`):

1. Before vision pass (`grid.rebuild` at line 176)
2. After applying velocities, before repulsion (`line 428`)
3. After applying repulsion forces (`line 508`)

That is **168 KB of heap alloc/free churn per tick** for cursors alone, purely to avoid adding a field to `SpatialGrid`.

### Fix

Add `cursors: Vec<u32>` as a field on `SpatialGrid`, pre-allocated to `HASH_DIM * HASH_DIM + 1`. In `rebuild`, replace `self.starts.clone()` with a `self.cursors.copy_from_slice(&self.starts)`. No allocation, no drop.

```rust
pub struct SpatialGrid {
    pub starts: Vec<u32>,
    pub indices: Vec<u32>,
    cursors: Vec<u32>,  // scratch; same length as starts
}
```

**Estimated gain:** Eliminates 168 KB of allocator round-trips per tick. At 1 GHz effective allocator throughput, 168 KB of allocation + zeroing takes ~0.17 ms. This is a trivial code change with zero correctness risk.

---

## 4. VisionBuf Copy in `build_nn_input`

### Current code

`build_nn_input` (`src/world.rs:1148–1191`) copies the 120-float vision buffer into the NN input array:

```rust
buf[10..130].copy_from_slice(vision);  // line 1184: 480-byte memcpy per creature
```

At 1 500 creatures: **703 KB** of memory copies per tick, just for the vision passthrough.

### Elimination options

**Option A (spec-breaking):** Reorder the NN input layout so vision is at `[0..120]`, self-state at `[120..130]`, one-hot at `[130..136]`. Then `Brain::forward` can accept `(&vision[i], &self_state)` as two slices and process them without copying. This breaks the v6 §E layout contract and would require retraining any saved weights.

**Option B (zero-spec-change):** Change `Brain::forward` to accept two input slices that it concatenates logically during the dot-product, matching the existing layout. The SIMD loop in layer 1 processes `INPUT_CHUNKS = 17` chunks of 8 floats. The first chunk covers inputs 0–7, etc. If inputs 0–9 are self-state and 10–129 are vision, you can process them without materializing the concatenated buffer by splitting the chunk loop at the boundary. This is a minor refactor to `Brain::forward` and `build_nn_input` with no external visible change.

**Option C (accepted status quo):** The 703 KB/tick memcpy is 15 cache lines × 1 500 = fits in L1/L2 bandwidth budget at typical frequencies (~50 GB/s = can copy ~50 MB/tick). The copy is not a throughput bottleneck today but will become noticeable at 3 000+ creatures.

**Recommendation:** Implement Option B when population grows to 2 000+. Not urgent at v1.

---

## 5. SIMD Coverage: Photosynth Two-Pass

### Current code

`photosynth_two_pass` (`src/world.rs:511–553`) has two scalar loops over all creatures:

**Pass 5a (demand):**
```rust
for i in 0..self.creatures.len() {
    if self.creatures.action_this_tick[i] != Action::Photosynth { continue; }
    let g = &self.creatures.genomes[i];
    let want = PHOTO_GAIN_COEFF * g.photosynth_efficiency * g.size;  // line 519
    // ...
    self.sun.demand[cell] += want;
}
```

**Pass 5b (payout):** Same pattern, reads `want` again (recomputed).

The computation `PHOTO_GAIN_COEFF * photosynth_eff * size` is a scalar multiply on two per-creature floats. If `photosynth_efficiency` and `size` were in SoA arrays (from improvement #1 above), this becomes trivially vectorizable with `f32x8`:

```rust
// After SoA split:
for chunk in photo_creatures.chunks(8) {
    let eff = f32x8::from_slice(&creatures.photo_eff[chunk]);
    let sz  = f32x8::from_slice(&creatures.size[chunk]);
    let want = f32x8::splat(PHOTO_GAIN_COEFF) * eff * sz;
    // scatter to demand cells (not vectorizable, but much less frequent)
}
```

The scatter (demand[cell] +=) is the non-vectorizable part, but it runs once per photosynthesizing creature and is inherently sparse. The multiply is the part that benefits from SIMD.

**Estimated gain:** With SoA prerequisite in place, SIMD gives ~8x throughput on the FMA itself. At 1 500 creatures fully photosynthesizing, the demand pass currently does 1 500 scalar multiplies (~750 ns at 2 GFLOPS scalar); SIMD cuts it to ~90 ns. The absolute saving is small (~0.7 μs/tick) but illustrative of the broader SoA payoff.

---

## 6. SIMD Coverage: Energy Bookkeeping

### Current code

`energy_bookkeeping` (`src/world.rs:671–730`) loops over all creatures and computes upkeep from ~8 genome fields:

```rust
for i in 0..self.creatures.len() {
    let g = &self.creatures.genomes[i];
    let mut up = UPKEEP_BASE;
    if g.size > 1.0 { up += UPKEEP_SIZE_PER_UNIT * (g.size - 1.0); }
    if g.move_speed > 0.0 { up += UPKEEP_MOBILITY_FLAG + UPKEEP_MOVE_SPEED_PER_UNIT * g.move_speed; }
    up += UPKEEP_PER_EYE * g.eye_count as f32;
    up += UPKEEP_VISION_COEFF * g.vision_range * g.vision_range;
    // ...
}
```

With SoA genome hot fields (improvement #1), this loop can be SIMDified for the arithmetic-heavy `size`, `move_speed`, `vision_range` terms. The conditional branches (`if g.size > 1.0`, `if g.move_speed > 0.0`) become branchless SIMD masks. The `past_lifespan` penalty branch fires rarely.

**Estimated gain:** At 1 500 creatures, ~1 500 scalar upkeep computations. Each does ~8 multiplies and adds. At 2 GFLOPS scalar: ~6 μs. With AVX2/SIMD at 8× FLOPS: ~0.75 μs. Saving ~5 μs/tick. At 30 ticks/s this is 0.15 ms/s — small but free once SoA is in place.

---

## 7. Hot Scalar Packing: SoA vs. AoS for Multi-Field Access

### The tension

The existing `CreatureSoA` is good for single-field sequential passes (e.g., iterating `x[]` for grid rebuild, or `energy[]` for death check). But several tick phases access **multiple fields per creature in a tight loop**:

- `apply_movement_and_repulsion` reads `vx[i]`, `vy[i]`, then writes `x[i]`, `y[i]`, `energy[i]`, `distance_travelled[i]` — six SoA arrays.
- The inner repulsion loop reads `x[j]`, `y[j]`, `genomes[j].size` for each neighbor.

For the multi-field sequential passes, AoS packing of the co-accessed fields would reduce the total cache lines fetched. A `PerCreatureHot` struct:

```rust
#[repr(C)]
struct PerCreatureHot {
    x: f32,           // 4
    y: f32,           // 4
    vx: f32,          // 4
    vy: f32,          // 4
    energy: f32,      // 4
    action: Action,   // 1 (u8)
    _pad: [u8; 3],    // 3
}                     // = 24 bytes; ~2.7 per cache line
```

At 1 500 creatures: 24 × 1 500 = **35 KB** — L2 resident. Single pass reading x, y, vx, vy, energy, action would touch `ceil(35/64) = 547` cache lines instead of `6 × 94 = 564` lines (approximately the same). The win is marginal when fields are accessed together in the same loop body, and is reversed for the common single-field passes.

**Verdict:** The current SoA layout is the right default. The multi-field access patterns in the repulsion pass are bounded by the grid query (O(k neighbors)), not by memory bandwidth. Do not AoS-pack hot scalars; SoA is correct here. Focus genome work (#1) on the actual bottleneck.

---

## 8. `Brain::clone()` Heap Thrash at Births

### Current code

`handle_births` (`src/world.rs:828–906`) calls:

```rust
let child_brain = Brain::child_from(&self.creatures.brains[i], &mut self.rng, ...);
```

`Brain::child_from` (`src/brain.rs:168–186`) calls `parent.clone()`:

```rust
pub fn child_from(parent: &Brain, rng: &mut SimRng, sigma: f32, multiplier: f32) -> Self {
    let mut child = parent.clone();  // allocates 13 824 bytes
    // then geometrically mutates child.weights in place
}
```

At peak population (1 500 creatures × some fraction splitting per tick), birth rate can be significant. At 50 births/tick: **675 KB of heap allocation and initialization per tick** just for child brain copies.

### Options

1. **Brain slab (overlaps #2):** With a contiguous slab, the "clone" is a `slice::copy_from_slice` within the pre-allocated buffer at a pre-reserved child index. No allocator involvement.

2. **Vec pool:** Maintain a free-list `Vec<Vec<f32>>` of recycled brain weight buffers. On birth, pop one (if available) and `copy_from_slice` into it. On death, push the buffer back. Reduces allocator calls at the cost of some complexity.

3. **Accept as-is:** At current population scales (< 1 500 creatures), the Rust allocator handles 675 KB/tick efficiently with jemalloc or mimalloc (typical in wasm environments). The GC-pause equivalent is minimal because these are short-lived intermediate allocations that get freed before the next tick's allocations.

**Verdict:** Not worth a dedicated fix unless profiling shows allocator contention above 10% of tick time. The slab approach (#2) handles this as a side-effect.

---

## 9. `EventLog::all` Unbounded Growth

### Current code

`EventLog` (`src/events.rs:41–66`) has two stores:

- `recent: VecDeque<Event>` with `ring_cap = 200` — correctly bounded.
- `all: Vec<Event>` — grows without bound; used for save-file serialization.

Currently `events_enabled = false` by default (`src/world.rs:139`), so no events are pushed in production runs. The comment at line 58–60 notes this will flip to `true` in v1.1.

When events are re-enabled, `all` will accumulate every speciation, extinction, and milestone event indefinitely. In a long run with rapid speciation (common with `mutation_rate_multiplier` > 5.0), this could grow to tens of thousands of entries. `snapshot_json` (`src/wasm_api.rs:299`) serializes the full `EventLog` to JSON, so the save file grows proportionally.

### Fix

Cap `all` at a configurable `MAX_EVENTS_HISTORY` (e.g., 10 000). On overflow, overwrite the oldest entry (ring buffer semantics), or drop the oldest half. The `recent` ring already demonstrates this pattern. Alternatively, keep `all` for the session but truncate it during serialization.

**Estimated gain:** Save file size stays bounded; `snapshot_json` does not encode O(ticks) data. No tick-loop impact (pushes are O(1) amortized).

---

## 10. WASM Memory Growth and Float32Array View Lifetime

### Current code

`WorldHandle::creatures_buffer` (`src/wasm_api.rs:103–133`) and sibling buffer methods:

```rust
pub fn creatures_buffer(&mut self) -> js_sys::Float32Array {
    self.creature_buf.clear();
    self.creature_buf.resize(n * stride, 0.0);
    // ... fill ...
    unsafe { js_sys::Float32Array::view(&self.creature_buf) }
}
```

The pattern is: rebuild the Rust-side buffer from scratch on every call, then return a view. The view is a zero-copy alias into wasm linear memory. If wasm memory grows between calls (e.g., a large birth batch causes `Vec::push` to reallocate), any `Float32Array` view the JS side cached from a previous call is invalidated and silently reads garbage.

### Current safety analysis

The current code is safe **as long as JS callers do not cache the Float32Array across `step_n` or `step` calls**. The view is created fresh each call, so the caller always gets a valid slice pointing at current wasm memory. The comment at `src/creature.rs:1–7` acknowledges this design.

However, there is no enforcement: nothing in the wasm API prevents a JS caller from holding a reference to an old `Float32Array` and reading it after `step_n` has run. This is a documentation/contract issue, not a code defect.

### Pre-allocation mitigation

`World::new` allocates `CreatureSoA::with_capacity(2048)` (`src/world.rs:105`). The `with_capacity` reserves the initial `Vec` backing store but does not prevent later growth beyond 2048. If population exceeds 2048, all SoA `Vec`s will reallocate, and wasm linear memory may grow.

To eliminate mid-run reallocations: reserve `MAX_POPULATION` (e.g., 3 000) at construction:

```rust
// src/world.rs, World::new:
let mut creatures = CreatureSoA::with_capacity(MAX_POPULATION);
```

With sufficient initial reservation, no `Vec::push` reallocation will occur, wasm memory will not grow mid-run, and JS-side Float32Array views remain valid until the next explicit buffer call. The current `2048` capacity was chosen to match the `POPULATION_MILESTONES` max of 2000; bumping to 3000 costs ~200 KB additional static wasm memory at startup.

**Estimated gain:** Zero-allocation births (for population < capacity) and safer JS interop. Low-effort, low-risk.

---

## 11. Grid Rebuild Frequency and Cursor Scratch (Detailed)

As noted in #3, `grid.rebuild` is called three times per tick:

1. `src/world.rs:176` — before vision pass
2. `src/world.rs:428` — after velocity application, before repulsion
3. `src/world.rs:508` — after repulsion force application

Each rebuild clones `self.starts` (`src/grid.rs:51`) for the cursor scratch — a 56 KB heap allocation that lives for ~10 μs then is freed. Three of these per tick = **168 KB of allocation/deallocation churn per tick** at 30 ticks/s = 5 MB/s of allocator traffic from grid cursors alone.

The fix (add `cursors` field, `copy_from_slice` instead of `clone`) is a 5-line change with zero correctness risk and meaningful allocator pressure relief. It also makes the `rebuild` function signature compatible with future parallelism (no hidden allocation).

Additionally, the number of rebuilds could be reduced. Rebuild 2 (after velocity) is needed before repulsion so the grid reflects post-velocity positions. Rebuild 3 is needed so the grid reflects post-repulsion positions for the next tick's vision pass (which comes first in tick ordering). There is no obvious way to eliminate any of the three without changing tick semantics.

---

## 12. Repulsion: SIMD Potential and Grid Query Bottleneck

`apply_movement_and_repulsion` (`src/world.rs:430–476`) computes pair forces:

```rust
for i in 0..n {
    // ...
    for j in neighbors {
        let dx = x[j] - x[i];
        let dy = y[j] - y[i];
        let d2 = dx*dx + dy*dy;
        let rsum = ri + rj;
        if d2 < rsum * rsum {
            let d = d2.sqrt();
            let overlap = rsum - d;
            let f = (REPULSION_K * overlap).clamp(0.0, REPULSION_MAX);
            // ...
        }
    }
}
```

The inner pair computation is FMA-heavy and SIMD-able in principle. However, the bottleneck is **the gather pattern**: neighbor indices come from the grid's `indices[]` slice in arbitrary order, so `x[j]`, `y[j]`, `genomes[j].size` are gathered from non-sequential locations. SIMD gather instructions exist (AVX2 `_mm256_i32gather_ps`) but are expensive on most x86 microarchitectures (about 2× slower than sequential loads). In wasm there are no gather intrinsics.

The more impactful fix for repulsion is ensuring `ri = genomes[i].size` is read from a SoA array (#1) rather than the AoS genome struct, reducing the per-outer-iteration genome cache miss from 4 cache lines to 1.

**Verdict:** Do not SIMD the repulsion pair loop. Fix the genome AoS access (#1) first.

---

## 13. Vision Raycast: DDA and Cache Access Pattern

The DDA raycast (`src/vision.rs:128–254`) walks grid cells and for each cell fetches:

- `self.grid.starts[cell_idx]` and `self.grid.starts[cell_idx + 1]` — 8 bytes, almost always in L2 (grid starts is 56 KB, L2-resident if warm)
- `self.grid.indices[start..end]` — typically 2–5 creatures per cell
- For each candidate: `self.creatures.x[j]`, `self.creatures.y[j]`, `self.creatures.genomes[j].size`

The `genomes[j].size` read (for the body radius `rj`) is the same AoS penalty as discussed in #1: it drags in the full 204-byte genome struct for a single f32. With the SoA split, this becomes `creatures.size[j]` — a sequential array that is likely prefetcher-visible.

SIMD for ray-circle intersection: `ray_circle_hit` (`src/vision.rs:260–275`) performs 7 scalar ops per candidate. With 2–5 candidates per cell and up to 24 active sectors per creature, SIMDing across multiple candidates simultaneously is possible but requires batching candidates, which complicates the early-exit (`best_dist` cutoff) logic. The current DDA early-exit is a correctness optimization; removing it would allow SIMD batching but at the cost of checking more candidates than necessary.

**Verdict:** SIMD the cell-level candidate batch only if profiling shows the raycast at >15% of tick time. The SoA genome fix (#1) provides a simpler gain with no algorithmic risk.

---

## Closing Recommendations

**Ship first: Genome SoA split (#1)**

The genome AoS-within-SoA is the widest bottleneck across the most tick phases: `photosynth_two_pass`, `energy_bookkeeping`, `apply_movement_and_repulsion`, `build_nn_input`, vision raycast, and `eat_and_scavenge` all touch `genomes[i]` in inner loops. Splitting the seven hot fields into parallel SoA arrays drops the per-tick genome working set from 299 KB (L3) to 41 KB (L2). The change is purely additive — the full `Genome` struct survives for mutation, speciation, and serialization. Estimated implementation: 200–300 lines across `creature.rs`, `world.rs`, `vision.rs`, and `wasm_api.rs`. All uses of `g.field` in hot loops become `creatures.field[i]` from the new array.

**Ship second: Grid cursor pre-allocation (#3)**

Five lines of code, zero correctness risk, eliminates 168 KB/tick of allocator churn. The payoff is immediate and measurable with any allocator profiler. Do this before the SoA split to establish a timing baseline.

After those two, the brain slab (#2) is the next meaningful structural change, but requires a broader refactor of `Brain` ownership and is best deferred to a dedicated milestone.
