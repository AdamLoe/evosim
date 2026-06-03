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

Near-term actionable work lives in
[`v2.0.4-grass-tuning-perf.md`](v2.0.4-grass-tuning-perf.md). Non-perf v2
backlog lives in [`v2-possible-next-steps.md`](v2-possible-next-steps.md).

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
"Consumer 3 / stage 2e"). **Now in scope** — see
[`v2.0.4-grass-tuning-perf.md`](v2.0.4-grass-tuning-perf.md) section E. Kept here
as a pointer only.

### 7. Deterministic-scatter redesign (perf-relevant tradeoff)

If seed-reproducibility ever becomes a feature, the lossy cross-tile atomic RMW
(the source of run-to-run divergence) would need replacing. Cheapest candidate
is CAS-add-with-cap (additive spread commutes → may restore determinism at
modest contention cost). Listed here because the determinism/perf tradeoff is
entangled. See the closeout's ratification item #1.

## See also

- [`v2.0.4-grass-tuning-perf.md`](v2.0.4-grass-tuning-perf.md) — the active wave.
- [`grass-perf-closeout.md`](grass-perf-closeout.md) — shipped scatter+LOD state.
- [`v2-possible-next-steps.md`](v2-possible-next-steps.md) — non-perf v2 backlog.
