---
status: parked
owner: mixed
last_updated: 2026-06-02
okay_to_delete: false
long_lived: true
owning_docs: []
---

# Perf optimization ideas (sim / grass)

## Long-lived purpose

A running backlog of **performance ideas discussed but not committed** —
mostly grass propagation at scale, but open to any sim hot path. Each entry
records the idea, when it helps, when it doesn't, and the cost/risk that kept
it out of the active wave. This is a *menu*, not a plan; promote an entry into
a versioned plan when it's time to build it. Never deleted.

Shipped perf decisions, including the grid rebuild and pyramid-cadence
verdicts, are recorded in [`../decisions/perf.md`](../decisions/perf.md).

## The core tension

Grass propagation has two cost dimensions:

- **Per-cell cost** — how expensive one cell's spread/decay is. Reducible
  (alloc, instrumentation, RNG fusion, geometric-skip), but only ~2–4× of
  headroom, and that's the work going into v2.0.4.
- **Cell count processed** — how many cells we touch per tick. The active-tile
  frontier already skips empty-isolated tiles, but on a **uniformly dense /
  evenly-populated** map there is no sparsity to exploit and it's genuinely
  O(all cells). **Every idea below that beats the O(N) floor does so by
  processing fewer cells per tick — at some cost in fidelity, memory, or
  determinism.**

The honest ceiling: a full, evenly-spread grass+creature map is ~O(N) and the
only real levers are (a) cheaper cells, (b) fewer cells per tick via cadence,
or (c) simply less grass.

## Ideas

### 1. Cadence / visibility-gated simulation (`needs_cell_sim`)

Don't simulate every active tile every tick. Process a tile fully only when it
matters this frame; throttle the rest to every Nth tick.

```rust
let needs_full_sim =
    tile.frontier                          // growing edge — every tick
    || tile.near_creature(view)            // being grazed — every tick
    || tile.visible(view)                  // on-screen — every tick (or every few)
    || (tick % tile.background_period == 0); // quiet/saturated — every Nth tick
```

- **Why it's the real 10× lever:** it's a multiplicative `÷N` on the dominant
  cost, and it survives the "everything is frontier" case — even when every
  tile is frontier, most aren't near a creature or on-screen *this tick*.
- **Subsumes "skip saturated tiles":** we can't *skip* saturated tiles (decay
  makes them develop gaps) but we can *throttle* them — a saturated tile only
  needs occasional decay rolls.
- **Cost/risk:** off-screen living-noise updates slower (masked by coarse LOD
  mips off-screen); "near_creature" and "visible" channels need the camera +
  creature positions on the worker side (the camera SAB lane from v2.0.3 helps).
  Decay accumulated over N skipped ticks must be applied as one batched roll, not
  N separate ones, to keep rates correct.
- **Deferred** explicitly for v2.0.4 (user steer: skip frontier/visibility work
  for now).

### 2. Hybrid sparse/dense active-tile strategy

Maintain the active-tile list while grass area is small; once it exceeds a
threshold X, stop maintaining the list and just stride all cells (the
maintenance bookkeeping costs more than it saves when nearly everything is
active). Cheap to add; largely subsumed by cadence (#1) but a simpler stopgap.

### 3. Event-sampling O(events) propagation

Instead of visiting every cell and rolling, draw `N_events ≈ total_grass × p`
random cells directly and apply spread/decay only to those. Beats the O(N)
floor outright — work scales with *events*, not cells.

- **Blocker (why the original plan rejected it):** needs a per-cell indexable
  live list (`cell→position`), which is ~4 GB at the aspirational 1B-cell scale.
- **Reconsider at realistic scale:** at 4M cells a `Vec<u32>` live-cell list is
  only ~16 MB — entirely affordable. The 1B-hostility that killed this may not
  apply to the worlds actually being run. Strong candidate for a "small/medium
  world fast path." (Geometric-skip in v2.0.4 is the per-tile cousin of this.)

### 4. Grass-block mip-skip (with throttle, not skip)

A coarse occupancy mip: if a 16×16 block is uniformly all-grass or all-empty,
it usually needs no per-cell work.

- All-empty: already handled (empty-isolated skip).
- All-grass: **cannot skip** (decay), but **can throttle** (→ #1).
- The hard part is the boundary between a uniform block and its neighbours
  (spread crossing the seam) — must still process block fringes. Likely not
  worth it over #1's tile-cadence, which gets the same win more simply.

### 5. Interior-non-atomic writes

Most spreads land inside the source tile (range ≤3 ≪ tile 32). Do plain `u8`
reads/writes for tile interior and reserve relaxed atomics for the ≤3-cell
cross-tile halo. Removes atomic overhead for ~80–90% of writes. Smaller win on
wasm (relaxed ≈ plain there); measure before building. (Listed in v2.0.4 C-notes
as a stretch; parked here if C0 shows atomics aren't the bottleneck.)

### 6. Far-grass NN sensing (multi-band mip taps) — PROMOTED to v2.0.4

Multi-band near/med/far grass sensing off the LOD pyramid (the dropped v2.0.3
"Consumer 3 / stage 2e"). **Shipped in v2.0.4** — see
[`../decisions/render.md`](../decisions/render.md) (grass LOD). Kept here
as a pointer only.

### 7. Deterministic-scatter redesign (perf-relevant tradeoff)

If seed-reproducibility ever becomes a feature, the lossy cross-tile atomic RMW
(the source of run-to-run divergence) would need replacing. Cheapest candidate
is CAS-add-with-cap (additive spread commutes → may restore determinism at
modest contention cost). Listed here because the determinism/perf tradeoff is
entangled. See the closeout's ratification item #1.

## v2.0.6 S9 snapshot-write measured verdicts (2026-06-03)

**Setup:** default scale (world_size=9600, grass_dim=1920), mip_level=0,
1920×1620 viewport window, 100 creatures. Criterion native bench
(`benches/snapshot_write.rs`).

### Biome-window recompute (DOMINANT cost, SHIPPED fix)

- **Before:** `write_snapshot` = 13.74 ms/call; isolated `biome_window_kernel` = 11.43 ms/call.
- **Verdict:** biome-window loop was **83% of total snapshot cost**.
- **Fix shipped:** `BiomePyramid` precomputed at construction; `write_snapshot` now does
  a row-by-row `copy_from_slice` instead of the nested mode-accumulation loop.
- **After:** `write_snapshot` = 2.35 ms/call. **5.9× speedup** on the total snapshot write.
- **Correctness:** 7 output-equivalence tests pass (byte-identical across L0–L3,
  subwindows, multiple seeds, tie-break behaviour). See `app/crates/evosim/src/biome_pyramid_tests.rs`.
- **Profiler span added:** `write_output_sab.snapshot.biome` sub-span is now recorded
  (alongside existing `snapshot.creatures` and `snapshot.grass_copy`).

### Creature-pack copy (not dominant, SKIPPED)

- After the biome precompute, creature pack cost is ~0.1 ms/call (4% of remaining 2.35ms).
  Parallelization within the rayon pool would not yield a measurable win at 100 creatures.
  Not worth the complexity. Deferred.

### Grass pyramid copy (remaining dominant cost, DEFERRED)

- After the biome precompute, grass pyramid viewport_window is ~2.2ms/call (94% of remaining).
  This is a real bounded memcpy (~3MB); no obvious cheap fix without a different
  data layout or further LOD coarsening. Deferred to a future pass.

### v2.0.7 unpark signal

The snapshot cost was large (~13ms/call) and was largely due to the biome recompute.
After the precompute, `write_snapshot` costs ~2.35ms. This is still non-trivial but
is now dominated by the grass pyramid copy (~2.2ms), which is a bounded memcpy with
no redundant recompute. The case for unparking v2.0.7 (snapshot worker) is **weaker**
than before: the total snapshot cost dropped 5.9×. The remaining grass copy cost
could be reduced by a tighter LOD budget or by making the copy async in the existing
worker (no second worker thread needed). Recorded for the v2.0.7 lead to factor in.

### Movement / grid rebuild / grass step (not measured, DEFERRED)

S9 scope originally included movement, duplicate grid rebuild, and grass step.
The biome precompute was the measured prime suspect and its fix absorbed the full
S9 budget. Movement and grid rebuild remain unprobed. Deferred items:

- **Grid rebuild cost:** `SpatialGrid` is rebuilt every tick (`O(N)` with N creatures).
  At 1500 creatures, unverified but likely small vs snapshot. Measure if TPS budgets tighten.
- **Movement + repulsion:** rayon-parallel already; any remaining cost is per-creature NN output.
  Unverified as a hotspot. Measure if TPS budgets tighten.
- **Grass step:** already parallelized. Per-cell cost explored in v2.0.4; main levers are
  cadence gating (#1 above) and event-sampling (#3). No new data.

## v2.0.8 grid + pyramid measured verdicts (2026-06-04)

**Setup:** `benches/tick_profile.rs` (native, `--features threads`, env-tunable
world/grass/pop/full-grass). Numbers are this dev box (WSL2) — ~8× slower
per-cell than the user's machine, so read ratios not absolutes.

### Spatial-grid rebuild (SHIPPED — Wave A)

- The grid rebuild ran **3× per tick** (step 1 + two inside
  `apply_movement_and_repulsion`), each ~900µs, and was `O(hash_dim²)` not
  `O(creatures)` — a 960² grid for ~1500 creatures is 99.8% empty array sweep.
- **Lever 1 (HASH_CELL 10→20u):** behavior-neutral (coarser cells change only
  bucket granularity; exact-distance filter returns the identical neighbour set).
  grid.rebuild 969→191µs, movement 2026→492µs.
- **Lever 2 (single rebuild/tick):** repulsion + attack + mating run on the
  step-1 grid (≤0.25-cell stale at MOVE_SPEED_MAX=5u). Safe-by-construction —
  exact-distance on current positions means a stale bucket only *misses* a target
  (benign), never a false hit. Combined grid+movement 2995→404µs (7.4×).

### Grass mip-pyramid refresh (SHIPPED — Wave B)

- `pyramid_refresh` was a **full recompute every tick** (`O(grass_cells×4/3)`),
  feeding only zoomed-out render (default zoom=1 reads L0=density, NOT the
  pyramid) + far-grass NN sensing at L3 — neither needs per-tick freshness.
- **Cadence (`GRASS_PYRAMID_REFRESH_PERIOD=4`):** refresh every Nth tick;
  amortized 3675→870µs (4.2×) at the dense 9600/full-grass regime. Widens the
  pre-existing 1-tick NN lag to ≤N ticks. Robust even when the whole map changes
  (unlike dirty-keyed partial refresh, whose win collapses at steady-state living
  noise). The dirty-keyed partial refresh (the old `refresh()` TODO) is the
  behavior-neutral alternative but loses to cadence at this scale.

### Grass scatter `grass_step` (NOT optimized — at the floor)

- At 14.7M fully-grassed cells `grass_step` is **memory-bandwidth-bound on
  touching every occupied cell**. The per-cell path is already optimal (fused RNG,
  stack freeze, no alloc, active-tile frontier). The frontier gives little here:
  **decay keeps every grass-bearing tile permanently active** (a saturated tile
  keeps developing gaps → never skippable), so once grass covers the map the
  frontier ≈ the whole map.
- **No behavior-neutral 10× exists.** The levers are all fidelity/memory trades:
  cadence/throttle (§1, the menu's "real 10×", batch N ticks of decay/spread into
  one rate-preserving roll), event-sampling (§3, now affordable — a 14.7M live-cell
  `Vec<u32>` is ~59MB, not the 1B-cell hostility that originally shelved it), or
  simply coarser `grass_size` (5→10u = free ~4× on both grass_step AND pyramid).
- **User steer (2026-06-04):** ship pyramid cadence only; leave grass_step. Revisit
  with event-sampling (§3) if a true 10× is needed.

## See also

- [`../decisions/perf.md`](../decisions/perf.md) — shipped perf decisions (scatter attribution, LOD, S2R NO-GO).
- [`../decisions/render.md`](../decisions/render.md) — shipped scatter + LOD render state.
