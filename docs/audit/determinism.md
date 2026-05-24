# Determinism & Threading Audit

Scope: `src/world.rs`, `src/vision.rs`, `src/rng.rs`, `src/snapshot_hash.rs`,
`src/grid.rs`, `src/brain.rs`. Cross-checked against
`tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt`
(both currently `0xb76e907c6221f7f5` — bit-identical today).

## Threading model summary

Only two parallel sites exist, both behind `cfg(feature = "threads")`:

1. **Vision** — `vision.rs:75` `out[..n].par_chunks_mut(chunk_size)`, where
   `chunk_size = n.div_ceil(N_CHUNKS).max(1)`. Each closure reads
   shared-immutable `&CreatureSoA`, `&[Carrion]`, `&SpatialGrid`, `&cell_to_carrion`
   and writes its own disjoint `&mut [VisionBuf]` slice. No RNG, no reductions.
2. **NN forward** — `world.rs:386` `ranges.par_iter().flat_map(...).collect::<Vec<_>>()`.
   `flat_map` is implemented over the indexed `ranges: &[(usize, usize); N_CHUNKS]`,
   producing per-creature `(vx, vy, action)` tuples that rayon assembles
   **in chunk order** (rayon `IndexedParallelIterator::collect` preserves index
   order). The drain at `world.rs:441` writes back sequentially by `i`.

Everything else (movement, repulsion, photosynth, eat/scavenge, energy,
deaths, births, decay, bookkeeping) is sequential `for i in 0..n` over the
SoA, in stable index order.

`SimRng` (`rng.rs`) is held by `&mut self.rng` and consumed only from the
single-threaded path (mutation in `handle_births`, `world.rs:944/947/951/952`).
**No clone-and-split, no sharing across threads**.

## Risks, ranked by likelihood-to-bite

### R1 — `par_chunks_mut` chunk count drifts from `chunk_ranges` when `n < N_CHUNKS` (LOW today, LANDMINE later)

`vision.rs:74` uses `par_chunks_mut(ceil(n/8).max(1))`, which yields
`ceil(n / chunk_size)` chunks. For `n >= 8` this is exactly 8 chunks and
matches `chunk_ranges` in `world.rs:1252`. For `1 <= n <= 7`, `chunk_size=1`
and rayon yields **n** chunks of size 1; `chunk_ranges` always yields 8
(some empty). Vision is RNG-free and writes only to its own `out[i]`, so
this divergence does **not** affect correctness today — but the loud
invariant comment at `world.rs:1247` is technically wrong for small `n`,
and any future change that piggybacks shared state on chunk identity (e.g.
a per-chunk scratch buffer keyed by chunk_idx) will silently corrupt.
Acceptance has 10k ticks of population well above 8 so this never trips.

**Mitigation:** assert `chunk_size * N_CHUNKS >= n` after the `div_ceil` and
debug-assert the chunk count.

### R2 — Float reduction order across rayon's work-stealing collect (LOW today, HIGH if anything moves into a reduction)

Today both parallel sites avoid cross-thread floating reductions:
- Vision writes disjoint slots; nothing summed across threads.
- NN forward `collect::<Vec<_>>()` is indexed → deterministic by chunk index,
  not by completion order. The per-creature `forward` runs entirely on one
  thread, so its `f32x8::reduce_add` sequence is fixed by code, not schedule.

Risk surface: if anyone adds `.par_iter().sum()` / `.reduce(|a,b| a+b, ...)`
or a `par_chunks_mut` that accumulates into a shared `AtomicU32`-via-bitcast
or a per-chunk partial that gets folded, the result will become
thread-count-dependent because f32 addition is non-associative.

Specific tempting future targets:
- `eat_and_scavenge` scratch accumulators (`scratch_damage`, `scratch_gain`)
  at `world.rs:704/705/727`. Today sequential. Parallelizing requires
  per-target locking or per-thread partials → cross-thread sum order.
- Photosynth `sun.demand[cell] += want` at `world.rs:607`. Same hazard.
- Repulsion `scratch_fx/fy` at `world.rs:540-554`. Same hazard, plus the
  `if j > i` guard would have to be re-derived.

**Mitigation:** when parallelizing reductions, write per-creature partials
into a `Vec<f32>` indexed by `i`, then sequentially sum in index order.

### R3 — `initThreadPool` not awaited → silent serial fallback on SAB-less browsers (LOW for determinism, HIGH for confusion)

`vision.rs:71-73` and `lib.rs:30-31` rely on `wasm-bindgen-rayon`'s contract
that without `await initThreadPool(...)`, rayon runs work on the calling
thread. That keeps results bit-identical (good) but means `--features
threads` on a SAB-less host runs *identical* code to the sequential build
yet still asserts the threaded golden. If rayon ever changes its fallback
to "spawn a default pool of N=cores", the threaded golden could diverge
from the SAB-enabled golden on machines with a different core count.
Today both goldens equal `0xb76e907c6221f7f5`, which proves the threaded
arithmetic is order-stable across whatever core counts you've tested — but
you have no test that asserts this across `RAYON_NUM_THREADS=1, 2, 4, 8, 16`.

**Mitigation:** add a CI matrix run that pins `RAYON_NUM_THREADS` to 1, 2,
and ≥cores and re-asserts the threaded golden.

### R4 — `XxHash64::with_seed(0).finish()` over `serde_json::to_vec(&w.rng)` (LOW, but fragile)

`snapshot_hash.rs:76` serializes the RNG via `serde_json::to_vec`. xoshiro's
`Xoshiro256PlusPlus` serializes its state as 4 numeric fields; serde_json's
JSON output is deterministic for derived `Serialize` on plain structs, but
this is contingent on:
- field order being struct order (it is, today),
- numeric formatting being stable across serde_json versions (it has been),
- no upstream change adding a non-deterministic field (e.g. version tag).

If serde_json or `rand_xoshiro` ever change their derived Serialize output
(e.g. switch to a compact array form), the golden flips with **no source
change** in this repo. The risk is invisible in code review.

**Mitigation:** hash the four u64s of `xoshiro` state directly
(`self.rng.0.get_state()` if exposed, or `bincode` with explicit
little-endian) instead of going through JSON.

### R5 — Iteration over `HashSet`/`HashMap` for determinism-relevant outputs (NONE found, but two near-misses)

Audited every `HashSet`/`HashMap` use:
- `world.rs:1008` `HashSet<u32> alive` — **only** used for `contains()` lookups,
  iteration is over the `Vec` `pending_extinction_check`. Safe.
- `wasm_api.rs:398` `HashMap<u32, u32> counts` — used only for `get()` lookups
  during a `Vec` iteration. Output is `Vec<_>`. Safe (and not part of the
  sim/snapshot anyway).
- `profiler.rs:60` `HashMap<(NodeId, String), NodeId> child_index` — runtime
  span tree only; not in snapshot.

No `BTreeMap` substitution needed today, but any new code that does
`HashMap.iter()` into the snapshot or into sim state would be a hard bug
on the first run with a different hash seed (HashMap's RandomState seeds
per-process via getrandom).

### R6 — `getrandom` is enabled with the `js` feature (LOW)

`Cargo.toml:14` enables `getrandom/js`. The only Rust-side consumer in a
deterministic path would be std's `HashMap` RandomState; everything sim-
critical uses `SimRng`. As long as no `HashMap.iter()` feeds the snapshot
(R5), this is fine — but it's a standing landmine because *any* new
`HashMap.into_iter().collect::<Vec<_>>()` that touches sim state will be
seeded by browser RNG and break the golden on every page reload.

**Mitigation:** lint rule / grep gate forbidding `HashMap::iter`/`into_iter`
in `src/world.rs`, `src/snapshot_hash.rs`, `src/species.rs`,
`src/creature.rs`.

### R7 — `f32` vs `f64` mixing in vision/movement (NONE — confirmed)

Spot-checked all hot-path math in `vision.rs` (raycast, ray_circle_hit) and
`world.rs` (movement, photosynth, eat). All scalars are `f32`. The angles
in `eye_trig` are precomputed `(dx, dy)` pairs (perf-1), so vision no
longer calls `sin`/`cos` per ray — eliminating that family of cross-build
drift (libm vs platform).

`ray_circle_hit` uses `disc.sqrt()` — `f32::sqrt` is IEEE-754 correctly
rounded on every modern target. Safe.

`(-2.0 * s.ln() / s).sqrt()` in `rng.rs:57` (Box-Muller) uses `f32::ln`,
which is **not** IEEE-754 mandated and can vary across platforms (libm vs
musl vs MSVC). The mutation path consumes this, and the golden was
recorded on a specific platform. If you've never run the golden on a
non-Linux target, this is a sleeping cross-platform bug.

**Mitigation:** if the golden ever needs to validate on macOS/Windows
native, swap `f32::ln` for a polynomial approximation or a portable
implementation (e.g. `libm` crate, which is bit-identical across targets).

### R8 — `wide::f32x8::reduce_add` order is fixed but lane-permutation may differ across ISAs (LOW)

`brain.rs:117/132` uses `acc.reduce_add()`. `wide` uses a fixed software
reduction tree, not a hardware horizontal-sum, so the order is
target-independent. But this is library policy, not a contract — a `wide`
version bump that switches to `_mm256_hadd_ps` on AVX2 would change the
sum order and flip both goldens at once. Pinned `wide = "0.7"` in Cargo.toml
helps, but not against patch bumps.

**Mitigation:** assert that `forward` and `forward_scalar` agree within
exact equality (not 1e-5) for a battery of fixed inputs in CI. Currently
the test allows 1e-5 relative error.

### R9 — `pending_extinction_check` order depends on death order, which depends on tick-time index order (NONE — safe)

`collect_deaths` walks `0..n` and pushes to `species_lost` in index order;
`finalize_extinctions` iterates that Vec. Since the SoA is index-ordered
(swap_remove churn is deterministic per tick), this is safe.

## Concrete near-term watchouts

If parallelizing more of the tick (the obvious next moves are photosynth,
eat/scavenge accumulation, repulsion):

- The "scratch accumulator" pattern (`scratch_damage[j] += dmg`) **cannot**
  go behind a rayon parallel iterator without per-thread partials, because
  multiple `i`s may target the same `j`. See R2.
- The `for_each_in_radius` callback in `world.rs:520` captures
  `&mut neighbors` — closure isn't `Send`, so the obvious "just par-iter
  the outer i loop" attempt won't compile, which is a small mercy.
- Any `events.push(...)` in a parallel section needs to collect into a
  per-chunk `Vec<Event>` and merge in chunk order; pushing into a shared
  `Mutex<Vec<Event>>` would yield non-deterministic ordering, and
  `events_enabled = true` paths feed into snapshot only via tick
  counters, but `first_*_fired` flags (`world.rs:478, 742`) are
  order-sensitive guards that would flip non-deterministically if their
  parent loops were parallelized.

## Bottom line

The current threaded build is genuinely bit-identical to the sequential
build (goldens match). The two parallel sites were chosen carefully:
disjoint writes (vision) and indexed-collect into a Vec (NN). The
landmines are not in the code that exists, but in the patterns this code
*invites* future contributors to extend: scratch accumulators, event
buffers, shared sun-cell demand. Top three to fix preemptively:

1. **R4** — replace `serde_json::to_vec(&rng)` in snapshot with a direct
   little-endian hash of the four xoshiro u64s. Eliminates a silent
   dependency on serde_json output stability.
2. **R3** — add a `RAYON_NUM_THREADS={1,2,8}` matrix to the acceptance
   suite. Proves order-stability empirically.
3. **R1** — tighten the chunk-count invariant doc in `world.rs:1247` (or
   actually enforce it) so future small-n bugs are caught at compile/debug
   time rather than slipping through a 10k-tick run with 200+ creatures.
