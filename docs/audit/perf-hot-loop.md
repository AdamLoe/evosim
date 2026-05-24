# Performance audit — simulation hot loop (post perf-1..5 + threads)

Reviewer: Claude (opus 4.7), 2026-05-24.
Scope: `src/world.rs`, `src/creature.rs`, `src/grid.rs`, `src/vision.rs`,
`src/brain.rs` at HEAD `41a91a5`. Context: perf-1 (sector trig cache),
perf-2 (scratch pool), perf-3 (grid cursor), perf-5 (genome 7-field SoA
mirror), perf-threads (initThreadPool + parallel vision + rayon NN) have
already landed.

The findings below are the *next* layer down. Each carries a file:line,
before/after code sketch, and a low/med/high impact estimate (relative to
total tick time at N≈1k–2k creatures, which is the regime perf has been
tuned for).

Legend for impact: **H** = expected ≥5% of total tick wall-time on the
hot path; **M** = 1–5%; **L** = <1% / correctness-or-future-leverage.

---

## 1. NN forward pass — weight matrix loaded N times per tick (H)

**Location.** `src/brain.rs:108–132` (`Brain::forward`).

The forward pass walks `w_ih` as `weights[h * NN_INPUTS + i]` row-by-row,
but `weights` is a *per-creature* `Vec<f32>` (3 456 floats × 4 B = ~13.5 kB
per brain). Each creature gets a fresh L1 fill. At N=2 000 creatures the
NN forward pass alone touches **27 MB of brain weights per tick** — vastly
exceeds L2 and probably the per-core LLC slice. Sequential prefetch helps
within a creature, but there is zero reuse across creatures.

Two compounding problems on top:

- **`input` is reloaded 24× per creature.** The outer `for h in 0..24`
  re-reads `input[c*8..c*8+8]` for every hidden unit
  (`brain.rs:113`). The same 136 floats are loaded into SIMD regs 24
  times. With 17 chunks × 24 hidden = 408 `f32x8::from(&input[..])`
  calls per creature; the compiler may hoist some, but the row pointer
  arithmetic depends on `h` so it likely doesn't.

- **`acc.reduce_add()` after every dot product** (line 117). 24+8 = 32
  horizontal reductions per creature → ~64 k reductions/tick at N=2 000.
  Horizontal `f32x8` reduce is ~6 µops on x86 and worse on wasm SIMD128.

**Sketch — block the input loop outermost:**

```rust
// Layer 1: tile by input chunk so each input chunk is loaded once
//          across all 24 hidden units.
let mut acc = [f32x8::ZERO; NN_HIDDEN]; // 24 accumulators in regs/spill
for c in 0..INPUT_CHUNKS {
    let xv = f32x8::from(&input[c * 8..c * 8 + 8]);
    for h in 0..NN_HIDDEN {
        let row = &w_ih[h * NN_INPUTS + c * 8..h * NN_INPUTS + c * 8 + 8];
        let wv = f32x8::from(row);
        acc[h] += xv * wv;
    }
}
for h in 0..NN_HIDDEN { hidden[h] = acc[h].reduce_add().max(0.0); }
```

Or, far better, **transpose the weight matrix to hidden-contiguous-per-input**
so the inner loop is a single SIMD MAC across 24 hidden units and never
reduces:

```
w_ih_T[i * NN_HIDDEN + h]   // 136 rows × 24 cols (3 NN_HIDDEN/8 chunks each)
let mut acc = [f32x8::ZERO; HIDDEN_CHUNKS];  // 3 accumulators
for i in 0..NN_INPUTS {
    let xi = f32x8::splat(input[i]);
    for c in 0..HIDDEN_CHUNKS {
        let wv = f32x8::from(&w_ih_T[i * NN_HIDDEN + c*8 ..]);
        acc[c] = xi.mul_add(wv, acc[c]);   // FMA
    }
}
// 3 stores + ReLU; ZERO reductions.
```

This is the standard GEMV pattern. For 136×24 it removes all 24 reductions
on layer 1 and the 8 reductions on layer 2 (apply the same transpose to
`w_ho`). It also lets the inner loop be a clean FMA chain — ~4× IPC on
modern CPUs.

`perf-final-report` explicitly **defers** the weight transpose (#13 in
master plan §1 "Non-goals"). Reconsider: at 2 k creatures the NN forward
is probably the single largest tick consumer, and the transpose is a one-time
shape change applied at `Brain::founder` / `child_from` / save-load.

**Impact: H.** Likely 10–25% of tick wall-time at N=2 000.

---

## 2. Threaded NN path allocates `Vec<(f32,f32,Action)>` of length N per tick (H)

**Location.** `src/world.rs:385–446` (the
`#[cfg(feature = "threads")]` branch of `nn_forward_all_chunks`).

```rust
let results: Vec<(f32, f32, Action)> = ranges
    .par_iter()
    .flat_map(|&(lo, hi)| {
        // …
        (lo..hi).map(|i| { … }).collect::<Vec<_>>()
    })
    .collect();
```

Two heap allocations per chunk **per tick** (the inner `collect::<Vec<_>>()`)
plus one final reassembly allocation, then a copy-back loop. At N=2 000 with
N_CHUNKS=8 that's 9 allocations + ~24 kB of churn per tick, and the cache
line traffic is double what it should be (write to `results`, then read
back to copy into `vx/vy/action_this_tick`).

This is *the same allocator-pressure pattern* perf-2 just removed elsewhere.

**Sketch — write directly into the SoA:**

The SoA write fields (`vx`, `vy`, `action_this_tick`) can be split into
N_CHUNKS disjoint slices and given to threads via `par_iter_mut` on a
zip of three `chunks_mut`:

```rust
let n = self.creatures.len();
let chunk = n.div_ceil(N_CHUNKS).max(1);
let vx = &mut self.creatures.vx[..n];
let vy = &mut self.creatures.vy[..n];
let act = &mut self.creatures.action_this_tick[..n];

vx.par_chunks_mut(chunk)
  .zip(vy.par_chunks_mut(chunk))
  .zip(act.par_chunks_mut(chunk))
  .enumerate()
  .for_each(|(k, ((vx_c, vy_c), act_c))| {
      let lo = k * chunk;
      let mut input_buf  = [0.0f32; NN_INPUTS];
      let mut hidden_buf = [0.0f32; NN_HIDDEN];
      let mut output_buf = [0.0f32; NN_OUTPUTS];
      for j in 0..vx_c.len() {
          let i = lo + j;
          // … inline carrion + wall + pick_action_d …
          vx_c[j]  = vxn;
          vy_c[j]  = vyn;
          act_c[j] = a;
      }
  });
```

Zero allocations. Each thread touches three contiguous output slices
(prefetcher-friendly) instead of producing then re-ingesting a tuple Vec.

Bit-identical to current output: rayon's `par_chunks_mut` produces a
deterministic partition; per-creature work is RNG-free.

**Impact: H** at N≥1 k in threaded build. The default build is unaffected.

---

## 3. `eat_and_scavenge` allocates `candidates: Vec<usize>` per Eat-attempting creature (M)

**Location.** `src/world.rs:676` inside the `Action::Eat` arm:

```rust
let mut candidates: Vec<usize> = Vec::with_capacity(8);
self.grid.for_each_in_radius(xi, yi, max_range, |j| {
    if j != i { candidates.push(j); }
});
for j in candidates { … }
```

This allocates a fresh `Vec` for *every* creature whose action this tick is
`Eat`. Even with `with_capacity(8)`, that's one `alloc()` per Eat-creature
per tick. perf-2 already added `scratch_neighbors` on `World` for the
movement step — the same idiom (with `std::mem::take` to dodge the
borrow) applies here.

**Sketch.**

Add `scratch_eat_candidates: Vec<usize>` next to the other scratch fields
in `World` (world.rs:106–114), then at the call site:

```rust
let mut candidates = std::mem::take(&mut self.scratch_eat_candidates);
candidates.clear();
self.grid.for_each_in_radius(xi, yi, max_range, |j| {
    if j != i { candidates.push(j); }
});
let mut best: Option<(usize, f32)> = None;
for &j in &candidates {
    // unchanged body
}
self.scratch_eat_candidates = candidates;
```

Better still: **inline the body inside the `for_each_in_radius` closure**
to avoid materializing `candidates` at all. The body needs `&self.creatures`
which is already borrowed read-only inside the loop, and `best` is a tiny
local. The current shape exists only because earlier code probably had
two-borrow issues; with the current `for_each_in_radius` signature (closure
captures by `&mut`), this works fine:

```rust
let mut best: Option<(usize, f32)> = None;
let xi = self.creatures.x[i]; let yi = self.creatures.y[i];
let radius_i = …; let reach = …; let max_range = …;
self.grid.for_each_in_radius(xi, yi, max_range, |j| {
    if j == i { return; }
    // dx/dy/d/rj/contact … same as before
    if contact <= reach { update_best(&mut best, j, d, self.creatures.id[j]); }
});
```

(if the borrow doesn't go through, use `mem::take` like above — but this
one-liner is closure-borrow-safe today because `self.grid` is taken by `&`
and `self.creatures` is also by `&`.)

**Impact: M.** Allocation pressure depends on the fraction of `Eat`
actions, but on populations where carnivory evolves this is real. Also
removes a needless copy of all candidate indices.

---

## 4. `Action::Scavenge` does a linear scan over **all carrion** per scavenger (H if carrion is large)

**Location.** `src/world.rs:720–730`:

```rust
for c in &mut self.carrion {
    let dx = c.x - xi;
    let dy = c.y - yi;
    let d2 = dx * dx + dy * dy;
    if d2 <= r_i * r_i {
        let take = c.pool.min(want);
        c.pool -= take;
        self.scratch_gain[i] += take;
        break;
    }
}
```

`O(scavengers × carrion)`. When carrion accumulates (high mortality
phases, or large worlds), this becomes the dominant term. `World` *already
maintains* `cell_to_carrion` for the vision pass (rebuilt at the top of
each tick), which is the exact index needed here.

**Sketch.**

```rust
let cx_cell = (xi / HASH_CELL).floor() as i32;
let cy_cell = (yi / HASH_CELL).floor() as i32;
let dim = HASH_DIM as i32;
let r2 = r_i * r_i;
'outer: for dy in -1i32..=1 {
    for dx in -1i32..=1 {
        let nx = cx_cell + dx; let ny = cy_cell + dy;
        if nx < 0 || ny < 0 || nx >= dim || ny >= dim { continue; }
        let cell = ny as usize * HASH_DIM + nx as usize;
        for &ci in &self.cell_to_carrion[cell] {
            let c = &mut self.carrion[ci as usize];
            let ddx = c.x - xi; let ddy = c.y - yi;
            if ddx*ddx + ddy*ddy <= r2 {
                let take = c.pool.min(want);
                c.pool -= take;
                self.scratch_gain[i] += take;
                break 'outer;
            }
        }
    }
}
```

(Caveat: `cell_to_carrion` was built earlier in the tick and would not
reflect carrion *added by deaths this tick* — but deaths happen in step 9,
after step 6, so this is safe today.)

**Impact: H** in carrion-rich states (e.g. after a die-off); **L** in
greenfield runs with no scavengers. Future-proofing.

---

## 5. `creatures.action_this_tick[i]` is matched in three sequential passes (M)

**Location.** Photosynth two-pass (`world.rs:594–636`) iterates the SoA
*twice* and branches on `action_this_tick`. `eat_and_scavenge` iterates a
third time. Bookkeeping iterates again. Each pass burns a full L1 scan of
the SoA columns (`x`, `y`, `g_size`, `g_photo_eff`, `energy`,
`action_this_tick`).

Because `action_this_tick` is set just once per tick in `nn_forward_all_chunks`,
you can **partition once** by action:

```rust
// After nn_forward_all_chunks:
self.scratch_idx_photo.clear();
self.scratch_idx_eat.clear();
self.scratch_idx_scav.clear();
self.scratch_idx_split.clear();
for i in 0..n {
    match self.creatures.action_this_tick[i] {
        Action::Photosynth => self.scratch_idx_photo.push(i as u32),
        Action::Eat        => self.scratch_idx_eat.push(i as u32),
        Action::Scavenge   => self.scratch_idx_scav.push(i as u32),
        Action::Split      => self.scratch_idx_split.push(i as u32),
        _ => {}
    }
}
```

Then `photosynth_two_pass` iterates `scratch_idx_photo` (skip the
`if action != Photosynth { continue; }` branch entirely; both demand and
payout pass), `eat_and_scavenge` iterates the eat/scav index lists, and
`handle_births` iterates `scratch_idx_split` (currently re-scans all N).

Today: 4 full SoA scans × N. After: 1 scan to bucket + 4 scans of K
(where K is action-set size). At N=2 000 the saved cache traffic is
modest but the branch elimination is real.

**Impact: M.** Also cleans up the branchy hot loops nicely.

---

## 6. `photosynth_two_pass` does the work twice; merge demand + payout (M)

**Location.** `src/world.rs:594–636`.

The current code:
1. iterates all N → push `want` onto `sun.demand[cell]`,
2. iterates all N → look up `demand`, divide, write `energy += want * factor`,
3. iterates all sun cells → drain.

Steps 1 + 2 both compute `want = PHOTO_GAIN_COEFF * g_photo_eff[i] * g_size[i]`
and the same `cell` index from the same `(x, y)`. Step 2 redoes both.

**Sketch — cache (cell, want) once into scratch, payout from there:**

```rust
self.scratch_photo_cell.clear();
self.scratch_photo_want.clear();
self.sun.demand.fill(0.0);
for i in 0..n {
    if self.creatures.action_this_tick[i] != Action::Photosynth { continue; }
    let want = PHOTO_GAIN_COEFF * self.creatures.g_photo_eff[i] * self.creatures.g_size[i];
    if want <= 0.0 {
        self.scratch_photo_cell.push(u32::MAX); // sentinel
        self.scratch_photo_want.push(0.0);
        continue;
    }
    let cell = SunMap::cell_index_for(self.creatures.x[i], self.creatures.y[i]) as u32;
    self.sun.demand[cell as usize] += want;
    self.scratch_photo_cell.push(cell);
    self.scratch_photo_want.push(want);
}
// payout
let mut k = 0;
for i in 0..n {
    if self.creatures.action_this_tick[i] != Action::Photosynth { continue; }
    let cell = self.scratch_photo_cell[k]; let want = self.scratch_photo_want[k]; k += 1;
    if cell == u32::MAX { continue; }
    let demand = self.sun.demand[cell as usize];
    let factor = (self.sun.current[cell as usize] / demand).min(1.0);
    self.creatures.energy[i] += want * factor;
}
```

If item #5 (partition-by-action) lands first, the index list of photo-actors
already exists and this becomes even tighter — no `if` filter at all on the
payout pass.

**Impact: M.** Eliminates one multiplication and one cell-index computation
per photosynth-actor per tick.

---

## 7. `Vec<Vec<u32>>` for `cell_to_carrion` — pointer chase per ray cell (M)

**Location.** `src/world.rs:94` (field declaration) and `vision.rs:42`
(use). The current structure is `Vec<Vec<u32>>`, length HASH_DIM² = 14 400.

For every cell a DDA ray crosses, the code does
`for &ci in &self.cell_to_carrion[cell_idx]` — that loads the outer
`Vec<u32>` header (ptr/len/cap = 24 B), then chases the heap pointer to the
inner buffer. With 14 400 inner Vecs, the headers alone span
345 kB — not L1-resident.

Even worse: rebuilding via `cell.clear()` in `build_cell_to_carrion`
(vision.rs:328–332) keeps 14 400 separate heap allocations alive even when
most cells are empty (typical: 99% empty since carrion is sparse).

**Sketch — CSR layout, same shape as `SpatialGrid`:**

```rust
pub struct CellToCarrion {
    pub starts: Vec<u32>,   // length = HASH_DIM*HASH_DIM + 1
    pub indices: Vec<u32>,  // length = num_carrion
}

impl CellToCarrion {
    pub fn rebuild(&mut self, carrion: &[Carrion]) {
        let total = HASH_DIM * HASH_DIM;
        self.starts.resize(total + 1, 0);
        for s in &mut self.starts { *s = 0; }
        for c in carrion {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            self.starts[cell + 1] += 1;
        }
        for k in 1..self.starts.len() { self.starts[k] += self.starts[k-1]; }
        self.indices.resize(carrion.len(), 0);
        let mut cursor = self.starts.clone(); // or use the same pre-alloc cursor trick as perf-3
        for (i, c) in carrion.iter().enumerate() {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            let pos = cursor[cell] as usize;
            self.indices[pos] = i as u32;
            cursor[cell] += 1;
        }
    }
    pub fn cell(&self, cell: usize) -> &[u32] {
        let s = self.starts[cell] as usize; let e = self.starts[cell+1] as usize;
        &self.indices[s..e]
    }
}
```

Single heap buffer for the indices, no pointer chase in the inner ray loop,
and `cursor` can be pre-allocated using the same trick perf-3 just applied
to `SpatialGrid`.

**Impact: M.** Vision ray loops touch many cells per ray × 24 sectors ×
~10% sighted creatures.

---

## 8. `for_each_in_radius` re-reads `self.starts[c]` + `self.starts[c+1]` per cell (L)

**Location.** `src/grid.rs:81–87`.

```rust
for iy in lo_y..=hi_y {
    for ix in lo_x..=hi_x {
        let c = iy * HASH_DIM + ix;
        let s = self.starts[c] as usize;
        let e = self.starts[c + 1] as usize;
        for &idx in &self.indices[s..e] {
            f(idx as usize);
        }
    }
}
```

Two random `self.starts[]` loads per cell. `starts` is 14 401 u32s = 56 kB,
fits in L2 but not L1. A bounding-box query for repulsion search at
radius ≈11 cells (size+SIZE_MAX = 11u, /5u cell) hits ~121 cells per
query × N creatures.

Restructure so the outer loop walks rows and grabs a row-slice of `starts`
once. Tiny win but trivial:

```rust
for iy in lo_y..=hi_y {
    let row = &self.starts[iy * HASH_DIM ..];
    for ix in lo_x..=hi_x {
        let s = row[ix] as usize;
        let e = row[ix + 1] as usize;
        for &idx in &self.indices[s..e] { f(idx as usize); }
    }
}
```

(Boundary: `iy * HASH_DIM + hi_x + 1` must be `≤ starts.len() - 1`. Add
an `iy + 1 < HASH_DIM || ix + 1 ≤ HASH_DIM` check or just let the row span
HASH_DIM+1 — `starts` is HASH_DIM² + 1 long so the last row has the
sentinel.)

**Impact: L.** Marginal; mostly about removing two index multiplications
per cell.

---

## 9. `grid.rebuild` computes `cell_of` twice per creature (L)

**Location.** `src/grid.rs:50–66`.

```rust
for k in 0..n {
    let c = Self::cell_of(xs[k], ys[k]);  // call #1
    self.starts[c + 1] += 1;
}
// …
for k in 0..n {
    let c = Self::cell_of(xs[k], ys[k]);  // call #2 — identical
    let pos = self.cursors[c] as usize;
    self.indices[pos] = k as u32;
    self.cursors[c] += 1;
}
```

`cell_of` does two divides, two casts, two `min`s. Across 3 rebuilds × N
creatures × 2 redundant calls, this is 6 unnecessary cell-of evaluations
per creature per tick. Cache the cell once:

```rust
// stash cells into a scratch Vec<u32> (add cells: Vec<u32> field).
self.cells.resize(n, 0);
for k in 0..n {
    let c = Self::cell_of(xs[k], ys[k]) as u32;
    self.cells[k] = c;
    self.starts[c as usize + 1] += 1;
}
// prefix-sum loop unchanged
for k in 0..n {
    let c = self.cells[k] as usize;
    let pos = self.cursors[c] as usize;
    self.indices[pos] = k as u32;
    self.cursors[c] += 1;
}
```

Same trick perf-3 used for `cursors`. The new `cells: Vec<u32>` joins the
pool of pre-allocated SoA-sized scratch and is excluded from save/hash.

**Impact: L.** Real but small — `grid.rebuild` runs 3× per tick.

---

## 10. `apply_movement_and_repulsion` rebuilds the grid *twice* per tick (M)

**Location.** `src/world.rs:501` (after velocity apply) and
`src/world.rs:591` (after wall clamp), plus `step()`'s rebuild at
`world.rs:210`.

The first internal rebuild (after velocity apply) feeds the
repulsion-neighbor query. The second (after wall clamp) is presumably for
the *next* phase's queries. But the next phase is `photosynth_two_pass`,
which uses `SunMap::cell_index_for`, **not** the spatial grid. The next
spatial-grid users are `eat_and_scavenge` (step 6) — which doesn't run
until creatures have already been clamped by the wall logic. So step 5's
final rebuild is needed; the rebuild at the *start* of step 4 might be
skippable since step 3 (NN forward) doesn't move creatures.

Actually trace it: step 1 rebuilds from start-of-tick positions
(`world.rs:210`). Step 4 applies velocity, then rebuilds (line 501), then
runs repulsion using the new positions, then clamps walls and rebuilds
again (line 591). That's 3 rebuilds per tick.

Walls clamp positions slightly, so eat/scavenge needs that final rebuild —
it's load-bearing. But: the post-velocity rebuild (line 501) is consumed
only by the repulsion-neighbor query in the *same function*. Could that
query use a coarser data structure (e.g. iterate creatures pairwise within
the same wider-radius hash cell using start-of-tick positions + a slop
margin)?

A safer optimization: skip the final rebuild (line 591) **iff no walls
actually fired**. Add a `wall_fired: bool` flag inside the clamp loop and
gate the final `grid.rebuild` on it. In typical mid-world runs (creatures
not at the boundary), wall hits are rare → save one rebuild per tick.

```rust
let mut any_wall = false;
for i in 0..n {
    // … clamp logic …
    if x < r || x > WORLD_SIZE - r || y < r || y > WORLD_SIZE - r {
        any_wall = true;
        // adjust + zero velocity component
    }
    // …
}
if any_wall { self.grid.rebuild(&self.creatures.x, &self.creatures.y); }
```

**Impact: M** in typical states (no wall hits); **L** when creatures are
crowded against walls. Each `grid.rebuild` is ~O(N + HASH_DIM²) and right
now happens unconditionally.

---

## 11. Action enum + struct padding waste (L)

**Location.** `src/creature.rs:15–24`.

`Action` is `#[repr(u8)]`, so `Vec<Action>` is dense (good). But it's then
stored in `last_action: Vec<Action>` and `action_this_tick: Vec<Action>`
— two parallel u8 vecs. These are *always* accessed together
(`last_action[i] = action_this_tick[i]` at world.rs:295) and the one-hot
embed reads `last_action[i]`. Merging into a single `Vec<(Action, Action)>`
(2 bytes/creature) wouldn't help and complicates the API — leave as is.

The struct fields on `CreatureSoA` itself are 7+ scalar `Vec<f32>`/`Vec<u32>`
each pointing at heap; no struct-padding issues (the SoA pattern is
inherently efficient).

`Genome` (`genome.rs:54–70`) has `eye_offsets: [f32; EYE_SLOTS]` = 24
floats = 96 B inline. The whole `Genome` is ~200 B (founder + mutation
rates table). It's now cold-path-only (perf-5 mirrors hot fields), so size
is non-load-bearing.

**Impact: L.** No action needed; noted for completeness.

---

## 12. `last_action` mirror copy can be folded into NN forward (L)

**Location.** `src/world.rs:294–296`.

```rust
for i in 0..self.creatures.len() {
    self.creatures.last_action[i] = self.creatures.action_this_tick[i];
}
```

A full N-element copy at end-of-tick that exists solely so next tick's
`build_nn_input` can read `last_action[i]`. Since `nn_forward_all_chunks`
sets `action_this_tick[i]` and `build_nn_input` reads `last_action[i]`,
we could just `swap(&mut last_action, &mut action_this_tick)` at end-of-tick
and reuse the buffer. O(1) instead of O(N), and removes one full scan.

```rust
// bookkeeping_tail:
std::mem::swap(&mut self.creatures.last_action, &mut self.creatures.action_this_tick);
// (any code that reads action_this_tick before nn_forward must be checked;
//  currently nothing does — nn_forward overwrites it first thing.)
```

Verify the borrow surface: `action_this_tick` is read by `photosynth_two_pass`,
`eat_and_scavenge`, `handle_births`, all of which run *before* this swap.
Next tick's NN sets it first. Safe.

**Impact: L.** One eliminated full-N memory scan per tick.

---

## 13. `decode_action` sort + `Action::ALL` lookup in tight loop (L)

**Location.** `src/world.rs:1322–1339`.

For every creature every tick:
```rust
let mut order = [0u8, 1, 2, 3, 4, 5];
order.sort_by(|&a, &b| { … logits[b].partial_cmp(&logits[a]).unwrap_or(Equal).then(a.cmp(&b)) });
for &k in &order {
    let act = Action::ALL[k as usize];
    if is_valid_action(act, genome, energy, cooldown) { return act; }
}
```

`sort_by` on a 6-element array is fine (it goes to insertion sort) but the
closure captures `logits` by ref, the comparator does a NaN-aware
`partial_cmp` per pair (~5 ops with branch), and `Action::ALL[k]` is a
lookup with a bounds check (LLVM may elide).

For 6 elements, **argmax-fallthrough is faster than sort-then-find**:
the typical case is "the top logit is the chosen action and it's valid"
— don't sort the rest at all unless you need to.

```rust
// Pre-rank lazily: find argmax over `valid mask`.
fn decode_action_fast(logits: &[f32; 6], genome: &Genome, energy: f32, cooldown: u32) -> Action {
    let valid_mask = action_valid_mask(genome, energy, cooldown); // 6-bit
    let mut best_k: u8 = 6;
    let mut best_l = f32::NEG_INFINITY;
    for k in 0..6u8 {
        if valid_mask & (1 << k) == 0 { continue; }
        let l = logits[k as usize];
        // first-index tiebreak on equality
        if l > best_l { best_l = l; best_k = k; }
    }
    Action::ALL[best_k as usize] // valid_mask guarantees Rest=0 is always set
}
```

`action_valid_mask` is a 6-bit constant-time computation
(`Rest|Photo|Signal|Split-if-energy|Eat-if-eff-and-no-cd|Scav-if-eff`).

**Impact: L.** Per-creature but tiny. Removes ~20 ops/creature/tick.

---

## 14. `EYE_VALID.iter().position(...)` runs every vision sector eval (L)

**Location.** `src/vision.rs:104` (inside `fill_one`) and the symmetric
call at `src/creature.rs:267` (inside `recompute_eye_trig_at`).

`vision.rs:104`:
```rust
let k_idx = EYE_VALID.iter().position(|&v| v as usize == k).unwrap_or(0);
```

Called once per creature in `fill_one`. Cheap, but **also stored** — you
could mirror `g_eye_stride: Vec<u8>` next to the existing 7 hot mirrors
(perf-5) so the per-tick read is a single load:

```rust
let stride = self.creatures.g_eye_stride[i] as usize;
if stride == 0 { *buf = [0.0; VISION_LEN]; return; }
```

Updated alongside `g_eye_count` in `push_hot_mirrors` and
`resync_hot_mirrors_at`. Removes the 8-element linear scan and the
`unwrap_or` branch.

Same in `recompute_eye_trig_at` (creature.rs:267) — but that runs on
birth/save-load only, cold path.

**Impact: L.** One fewer branch and a few cycles saved per sighted creature
per tick.

---

## 15. `bookkeeping_tail`'s biggest-ever clone is per-tick (L)

**Location.** `src/world.rs:808–821`.

```rust
let current_best = self.biggest_ever.as_ref().map_or(0.0, |h| h.captured_size);
if size_i > current_best {
    let species_name = self.species.get(self.creatures.species_id[i]).name.clone();
    self.biggest_ever = Some(HallOfFame { …, genome: g.clone(), … });
}
```

Per-creature per-tick, this clones a `String` (species name) and a `Genome`
(~200 B) any time the size record is exceeded. For typical evolution
trajectories `biggest_ever` is updated rarely after the founder generation,
so this is fine. BUT — the check inside the per-creature `energy_bookkeeping`
loop fires the comparison every creature every tick. The body of the `if`
is rare; the comparison itself is cheap. Fine as-is.

The fix that would matter: gate the `biggest_ever` update to *only* fire
once per tick (track tick-max-size in a scalar, then if it beat the global
record do the clone). One clone per tick max instead of one comparison per
creature per tick of unchanged value.

**Impact: L.** Skip unless allocation-profiler shows `Genome::clone` /
`String::clone` from this site is hot.

---

## 16. `collect_deaths` allocates two fresh Vecs per tick (L)

**Location.** `src/world.rs:832–833`.

```rust
let mut dead: Vec<usize> = Vec::new();
…
let mut species_lost: Vec<u32> = Vec::new();
```

Both are short-lived and typically tiny (`dead` often empty). Promote to
World scratch (`scratch_dead: Vec<usize>`, `scratch_species_lost: Vec<u32>`)
for consistency with perf-2. Tick-loop allocation pressure from these is
real but small.

**Impact: L.** Pattern consistency more than throughput.

---

## 17. `finalize_extinctions` builds a `HashSet<u32>` from scratch each call (L)

**Location.** `src/world.rs:1007–1011`.

```rust
let mut alive = std::collections::HashSet::<u32>::new();
for i in 0..self.creatures.len() {
    alive.insert(self.creatures.species_id[i]);
}
```

`HashSet` for a tiny set of species ids (typically <100) where we only do
`contains` is overkill. A sorted `Vec<u32>` with binary search, or a
species-indexed `bool` bitset (since species ids are dense and capped at
`species.list.len()`), is much faster and allocation-free.

```rust
// reuse a pooled Vec<bool>
self.scratch_alive_species.resize(self.species.list.len(), false);
for v in &mut self.scratch_alive_species { *v = false; }
for &sid in &self.creatures.species_id {
    self.scratch_alive_species[sid as usize] = true;
}
…
if !self.scratch_alive_species[cand as usize] { … }
```

**Impact: L.** This runs once per `tick_once` (via `finalize_extinctions`)
and only when `pending_extinction_check` is non-empty, but the HashSet
allocator churn shows up under flame profiling.

---

## 18. SIMD ray-vs-circle batch (deferred in master plan; reconsider) (M)

**Location.** `src/vision.rs:283–300` (`ray_circle_hit`) called inside the
DDA traversal at line 223/237.

Each cell crossed by a ray runs `ray_circle_hit` once per creature/carrion
in the cell. The closed-form math is 4 muls + 1 sub + 1 sqrt branch. For a
single ray hitting a cell with say 4 creatures, that's 4 scalar calls.

`f32x4` (or `f32x8`) lane-parallel batch: gather 4 creatures' (cx, cy, r),
compute b/c/disc/t in one shot, pick the min where `disc ≥ 0 && t ≥ 0`.
The `sqrt` is the throughput bottleneck — SIMD sqrt is ~the same latency
as scalar but quarters the per-batch cost.

This was deferred (perf-final-report item #8). Worth revisiting once items
1, 2 ship — vision raycast may then be the next biggest term.

**Impact: M.** Conditional on items 1–2 landing first.

---

## 19. `cells_walked` counter inside DDA loop (L)

**Location.** `src/vision.rs:208`, `269–272`.

Per-ray loop has:
```rust
cells_walked += 1;
if cells_walked >= MAX_CELLS_PER_RAY { break; }
```

`MAX_CELLS_PER_RAY = 64` and vision_range_max / hash_cell ≈ 16 diagonal-ish
so this guard is rarely hit. The counter + compare is small but in a hot
inner loop. Could be replaced by precomputing a `t_max_overall` from
`max_dist` and breaking once `t_max_x.min(t_max_y) > t_max_overall` —
which the loop *already* does at line 266. The counter is redundant
defensive code.

Remove `cells_walked` entirely. The "stop at `next_t > max_dist`" check is
the real bound; the counter only kicks in for pathological degenerate rays
which the bounds check + `f32::MAX` `t_delta` already handle.

**Impact: L.** One fewer branch + counter per cell crossing.

---

## 20. `f32x8::from(&slice)` may not be aligned — `Vec<f32>` only guarantees 4B (L)

**Location.** `src/brain.rs:113, 114, 128, 129`.

`weights: Vec<f32>` is heap-aligned to 4 bytes. `wide::f32x8::from(&[f32; 8])`
does a load that the compiler will lower to an unaligned `_mm256_loadu_ps`
(or wasm SIMD `v128.load`). Aligned loads (`_mm256_load_ps`) are faster on
older Intel chips; modern uops merge them. **In wasm SIMD128, alignment
hints are ignored** so this is x86-native-only.

If you're targeting native runs in profiling (e.g. perf-final-report's
desktop measurements), you'd want `weights` to be 32-byte aligned. Options:

1. Use `aligned-vec` crate (adds a dep) for the brain weights buffer.
2. Allocate one big aligned buffer for all brains' weights (single
   `Box<[f32]>` aligned via `alloc::Layout::from_size_align`), index by
   `weights[creature_idx * NN_WEIGHT_COUNT ..]`. This *also* unlocks
   future GEMV-batching across creatures.

Option 2 would simultaneously fix item #1's "weights touched 27 MB
randomly per tick" problem by giving the prefetcher a predictable stride.

**Impact: L** on its own; **M** if combined with item #1's transpose.

---

# Top-5 punch list

Prioritized by `(expected impact × shipping risk)⁻¹`, assuming the goal is
"the next 2× on the tick path".

1. **NN forward — transpose weights + tile to remove reductions.**
   `src/brain.rs:108–132`. Item #1. Highest single-piece return. Affects
   founder/child_from/save-load (transposed layout must be applied on load
   and on `child_from` mutation site). Bit-identical output up to FMA
   ordering — likely needs a new threaded-golden + a new sequential-golden
   regen, so plan it as its own PR. Expected: 10–25% tick wall-time.

2. **Threaded NN — write directly into SoA via `par_chunks_mut`.**
   `src/world.rs:385–446`. Item #2. Removes ~9 allocations/tick + N
   tuple-buffer copies in the threaded build. Bit-identical to current
   threaded golden (deterministic chunked partition). Localized to one
   function. Expected: 5–10% threaded tick wall-time.

3. **`cell_to_carrion` → CSR layout.** `src/world.rs:94, vision.rs:42`.
   Item #7. Single contiguous indices buffer instead of 14 400-Vec
   pointer-chase. Touches `vision.rs::build_cell_to_carrion`, all readers
   in `vision.rs::raycast`, and `world.rs::count_carrion_overlap` + the
   threaded inlined equivalent. Drops 345 kB of header memory pressure.
   Expected: 3–5% tick wall-time; cleaner code.

4. **`Action::Scavenge` — use `cell_to_carrion` index.**
   `src/world.rs:720–730`. Item #4. Turns O(scavengers × carrion) into
   O(scavengers × ~9 cells). Local change, no API churn, no determinism
   risk. Expected: H *conditional* on carrion population — quasi-free
   future-proofing.

5. **Partition by action once, then narrow loops.** `src/world.rs:594–760`.
   Item #5 (+ #6 falls out for free). Drops 3 full-N action-match scans
   per tick and removes branchy hot loops. Mid-size refactor (~80 lines
   touched) but no algorithmic change. Expected: 2–5% tick wall-time + a
   noticeable code-clarity win.

**Punch list runners-up** (worth doing when convenient, low effort each):
item #3 (eat candidates scratch), item #10 (skip-rebuild-on-no-wall),
item #12 (last_action swap), item #14 (g_eye_stride mirror), item #17
(bitset for alive-species), item #19 (drop cells_walked).

**Punch list things NOT to do yet:**

- Item #20 alone (alignment) — needs item #1 first to matter.
- Item #18 (SIMD ray-vs-circle batch) — only after items #1+#7 ship and
  re-profile to confirm vision is still on top.
- Item #15 (biggest_ever clone gating) — premature unless allocator
  profiling implicates it.
