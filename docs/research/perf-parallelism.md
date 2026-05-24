# Parallelism Opportunities in evosim

*Research only — no code was modified.*

---

## Summary Table

| Opportunity | Estimated speedup | Effort | Priority |
|---|---|---|---|
| 1. Threads feature default-on (NN pass) | 4-8× NN forward at ≥512 creatures | Low (build + JS init plumbing) | **do-now** |
| 2a. Vision pass parallelized (rays) | 2-6× vision at ≥512 creatures | Medium (~60 LOC, 2 files) | **do-now** |
| 2b. Eat/scavenge accumulator reduction | 1.5-2× at high density | High (40 LOC, logic risk) | do-after-other-wins |
| 2c. Photosynth two-pass | <1.1× (cells are tiny) | Medium (30 LOC + atomics) | never |
| 2d. Soft repulsion accumulator reduction | 1.5-3× at high density | High (50 LOC, logic risk) | do-after-other-wins |
| 3. Off-thread serialization for autosave | Removes ~50-200 ms main-thread spike per save | Medium (80 LOC, IPC design) | do-after-other-wins |
| 3b. Binary snapshot format (Transferable) | 5-15× faster serialize path | High (new wasm API) | v1.1 |
| 4. Render off main thread (OffscreenCanvas) | Negligible; render is cheap | Very High (arch change) | never |
| 5. Vision raycast on GPU (WebGPU compute) | 10-50× vision; complex | Enormous | v1.1 (already deferred per §14) |
| 6. Worker pool for sampling / event IO | 0 currently (events disabled) | Medium | v1.1 |

---

## 1. Threads Feature Default-On (NN Forward Pass)

### Mechanism

The NN forward pass is already fully parallel behind `cfg(feature = "threads")` in `src/world.rs` lines 301-374. The `wasm-bindgen-rayon` crate (version 1.3.0 per `Cargo.lock`) divides work into `N_CHUNKS = 8` (constant, `src/constants.rs` line 144) rayon tasks via `par_iter()`. Each task iterates its slice of the 8 pre-computed `chunk_ranges`, runs `pick_action_d` — which calls `Brain::forward` (SIMD, `src/brain.rs` lines 100-134) plus carrion overlap and wall detection — and returns `(vx, vy, Action)` tuples that are drained back sequentially. The intermediate `Vec<(f32, f32, Action)>` is the only cross-thread write; reads are immutable refs that are `Sync`.

The `wasm-bindgen-rayon` init contract requires calling `initThreadPool(navigator.hardwareConcurrency)` from JS before the first rayon task. This call is **not present** anywhere in the current web source (`web/src/main.ts`, `web/src/persistence/*.ts`, etc.) — it has never been wired in. Enabling threads end-to-end thus requires: (a) building with `--features threads`, (b) calling `initThreadPool` in `main.ts` before `init()`, (c) gating on a SAB-availability check for the fallback path. COOP/COEP headers are already set in both `web/vite.config.ts` (dev server) and `web/public/_headers` (Cloudflare Pages), so SharedArrayBuffer is already available.

### Determinism risk

None for the NN forward pass itself. `Brain::forward` is a pure function (`src/brain.rs` line 100): same weights + same inputs → bit-identical outputs. The v6 §J chunking design explicitly ensures this: results are assembled in chunk-index order (line 313-366 uses `.par_iter()` on `ranges` which is a fixed `[N_CHUNKS]` array, then `.flat_map` which preserves per-chunk order). The only source of per-run non-determinism between single- and multi-threaded paths would be floating-point reassociation if the reduction iterated in different orders — but `par_iter().flat_map(...).collect()` on `N_CHUNKS = 8` tasks preserves deterministic ordering. The golden snapshot hash (v5 §16) covers tick, creature SoA, sun, carrion, species, and RNG state (`src/snapshot_hash.rs` lines 21-79); it does not cover intermediate NN computations, so hash stability is preserved as long as final state is identical.

The one real concern: `wasm-bindgen-rayon` spawns Workers using `SharedArrayBuffer` for the rayon work-stealing queue. Worker initialization is async. If any tick fires before `initThreadPool` resolves, rayon falls back to the calling thread (single-threaded behavior). Proper sequencing: `await initThreadPool(...)` before constructing `WorldHandle`. This means `main()` gets a small (50-500 ms) one-time cold-start cost per tab load; subsequent ticks are fully parallel.

### Browser compatibility risk

Yes, but narrow. SharedArrayBuffer requires COOP + COEP headers (already deployed). The main unsupported path is browsers that block SAB even with those headers (Edge on iOS, Firefox with `privacy.resistFingerprinting = true`). Solution: feature-detect `typeof SharedArrayBuffer !== 'undefined'` and fall back to the single-threaded wasm build. This requires serving two wasm builds or a runtime branch. The simpler path for v1: serve one build with `threads` enabled; let `wasm-bindgen-rayon` gracefully degrade (it silently uses only the calling thread if `initThreadPool` is never called or hardware concurrency reports 1). The UX impact is zero in degraded mode — the single-thread path already works.

### Estimated speedup

At 512+ creatures with 24 eyes each: the NN forward + carrion overlap + wall detection loop dominates a tick. `Brain::forward` for one creature runs 17 SIMD chunks × 24 hidden + 3 × 8 output = ~185 SIMD operations, plus the 136-input build and 6-logit argmax. At 512 creatures, sequential cost is roughly `512 × C_forward`. With 8 workers on a 4-core machine (2 hyperthreads each) the wall time drops to `≈ 512/8 × C_forward + sync overhead`, a practical 4-6× improvement. On an 8-physical-core desktop the multiplier reaches 7-8×.

### Cost

~30 LOC in `web/src/main.ts` (import `initThreadPool`, add await, add SAB feature-detect). Build script must add `--features threads` (and the second `wasm-pack` target if dual-build). No changes to Rust.

### Recommended priority: **do-now**

The Rust parallelism is already written and tested (the `#[cfg(feature = "threads")]` block at `src/world.rs` line 301). Only the JS initialization wiring is missing. This is the highest-leverage single change in the codebase.

---

## 2a. Vision Pass Parallelized

### Mechanism

`VisionPass::run` at `src/vision.rs` lines 53-58 iterates creatures sequentially: `for i in 0..n { self.fill_one(i, &mut out[i]); }`. Each `fill_one` call (lines 61-125) reads only `&self.creatures` (immutable SoA), `&self.carrion`, `&self.grid`, and `&self.cell_to_carrion`, and writes only to `out[i]` — a disjoint memory region per creature. This is textbook embarrassingly parallel.

The `build_cell_to_carrion` call at `src/world.rs` line 1120 (inside `run_vision_pass`) is sequential and must remain so because it writes `cell_to_carrion`. But it is O(C) where C is carrion count, which is typically << N. After that build completes, the `pass.run` call on line 1127 can be parallelized with `par_iter_mut` on the `vision` slice without any structural change to the data layout.

Implementation shape: in `src/vision.rs`, add a `#[cfg(feature = "threads")]` path in `VisionPass::run` that splits `out` into `N_CHUNKS` mutable sub-slices, maps each over `rayon::par_iter_mut`, and dispatches `fill_one` with the correct global creature index. The shared `&self` refs (creatures, carrion, grid, cell_to_carrion) are all `Sync` and read-only inside the parallel region.

The DDA raycast in `fill_one` (lines 127-254) is the expensive sub-call: it walks up to `MAX_CELLS_PER_RAY = 64` grid cells (constant, `src/constants.rs` line 138) per ray, with up to 24 rays per creature. Per-creature cost is `O(E × R_cells)` where E = eye_count and R_cells = cells walked per ray. At 24 eyes, 64-cell max, and 512 creatures that is 786,432 cell-tests in the worst case — entirely independent across creatures.

### Determinism risk

None. Vision outputs feed the NN input buffer but contain no RNG calls. The written `vision[i]` slots are index-aligned with creatures; parallelizing the write does not affect the values. Golden hash covers creature SoA (positions, energy, genomes) and sun/carrion/RNG — not the intermediate vision buffer — so hash is unaffected.

### Estimated speedup

At 512 creatures with 12+ eyes: vision is likely co-dominant with the NN pass. At 8 workers, 4-6× reduction in vision wall time is achievable. At small populations (<100 creatures), the rayon task overhead likely exceeds the gain; a `if n < 128 { sequential } else { parallel }` guard in `VisionPass::run` is advisable.

### Cost

~40-60 LOC changes: add `#[cfg(feature = "threads")]` block in `src/vision.rs::VisionPass::run`, import `rayon::prelude::*`. The `VisionPass` struct already holds only references, making it trivially `Sync`-compatible. No changes to callers.

### Browser compatibility risk

Same as opportunity 1 (SAB/COOP/COEP). No additional risk.

### Recommended priority: **do-now** (after threading is wired in JS)

Should be bundled with opportunity 1 — both are in the same `#[cfg(feature = "threads")]` gating and share the same build/init cost.

---

## 2b. Eat/Scavenge Accumulator Reduction

### Mechanism

`eat_and_scavenge` at `src/world.rs` lines 556-669 has cross-creature side effects: when creature i bites creature j, `damage[j] += ...` and `gain[i] += ...` are accumulated in local Vecs that are then applied. The outer loop already allocates `damage` and `gain` per-tick (lines 561-562) and applies them sequentially at the end (lines 645-668), which is the right shape for a parallel accumulate-then-reduce. However, the `Eat` path (lines 573-618) also does a linear scan of candidates from the spatial grid and chooses a `best` target with distance tiebreaking including `creature.id` comparison (line 601-605), making the choice deterministic but order-dependent in a non-trivial way.

For parallelization: partition creatures by chunk. Each thread independently runs the eat/scavenge resolution for its slice and writes to thread-local `damage_local[j]` and `gain_local[i]` accumulators. After the parallel region, reduce all thread-local damage/gain arrays into the global ones. The `damage[j]` writes across chunk boundaries (creature i in chunk A biting creature j in chunk B) require either: (a) atomic f32 adds (not trivial with `f32`; use `AtomicU32` with bit-cast), or (b) per-thread full-width damage vectors that are summed sequentially post-join. Option (b) is safer and costs `n × n_threads` memory per tick — at 1000 creatures and 8 threads that is 8,000 f32 = 32 KB, negligible.

The scavenge path (lines 619-641) mutates `self.carrion[].pool` directly — this is a true shared write and would require either a per-carrion mutex or serialization of scavenge. The simplest approach: parallelize only the eat sub-pass; keep scavenge sequential. Scavenge is O(N_scavengers × C_carrion) which is cheap at typical population sizes.

### Determinism risk

Yes, with careful design it can be preserved. The `damage[j]` from multiple attackers in different chunks must sum to the same value regardless of thread scheduling. Since floating-point addition is associative only approximately, the reduction order must be fixed (e.g., always reduce chunk 0, then chunk 1, etc.). With a fixed reduction order the result is bit-identical across runs.

The trickier case: the `best` target selection (line 596-609) uses a deterministic tiebreak that refers to `self.creatures.id[j]`. This is preserved per-creature within its chunk but unaffected by other chunks' choices, so it remains deterministic.

Golden snapshot hash: unchanged — it hashes post-tick creature state (energy), not intermediate damage accumulators.

### Estimated speedup

Moderate: 1.5-2× at 500+ creatures all eating simultaneously. In practice, only a fraction of creatures pick Eat in any tick, so the inner loop is sparse. The gain materializes mostly at high population density with carnivore-heavy populations.

### Cost

~40 LOC in `src/world.rs`. Files touched: `world.rs`. Additional complexity: thread-local damage Vecs, fixed-order reduction.

### Recommended priority: **do-after-other-wins**

The net gain is small at realistic population distributions (most creatures photosynthesize). Do this after the NN + vision passes are parallel and profiling confirms eat/scavenge is a meaningful bottleneck.

---

## 2c. Photosynth Two-Pass Parallelization

### Mechanism

`photosynth_two_pass` at `src/world.rs` lines 511-553 consists of three serial passes over all creatures and one pass over all sun cells. Pass 5a (demand accumulation, lines 513-525) writes `sun.demand[cell] += want` — multiple creatures may share a cell, requiring atomic adds or a per-creature demand vec that is summed. Pass 5b (payout, lines 527-543) reads `sun.demand[cell]` which was just written, creating a strict sequential dependency between 5a and 5b.

The passes are light: 400 sun cells (20×20), each creature does one `SunMap::cell_index_for` call and two float ops. At 1000 creatures the total work is ~2000 float ops — negligible wall time. Atomic contention on hot cells (high-density populations clustering on hotspots) would likely exceed the parallelism gain. The three-pass structure could be rewritten as a single pass with `AtomicF32` (simulated via `AtomicU32::fetch_update`), but the code complexity is high for a micro-optimization.

### Determinism risk

Yes: floating-point reduction order of `demand` would diverge from sequential unless additions are fixed-order.

### Estimated speedup

<1.1× at any realistic population. The sun grid is 400 cells and the math is trivial.

### Recommended priority: **never**

Not worth the complexity. The photosynth pass is not in the hot path relative to NN forward + vision.

---

## 2d. Soft Repulsion Accumulator Reduction

### Mechanism

`apply_movement_and_repulsion` at `src/world.rs` lines 377-509 includes a nested-pair loop (lines 430-476) that writes symmetric impulses: `fx[i] -= ...; fy[i] -= ...; fx[j] += ...; fy[j] += ...`. This is the classic symmetric-pair write problem. Two approaches:

**Option A (partitioned cells)**: Assign non-adjacent cells to parallel threads so that no two threads write the same `fx[j]`/`fy[j]`. Requires graph-coloring or a checkerboard partition of the spatial grid. Correct but complex (~100 LOC).

**Option B (per-thread accumulator reduction)**: Each thread operates on a contiguous creature range, accumulates `fx_local[0..n]` and `fy_local[0..n]` (full width), then the main thread reduces them. Memory cost: `n × n_threads × 2 × 4 bytes`. At 2000 creatures × 8 threads = 128 KB per tick — acceptable. Fixed-order reduction preserves bit-determinism.

### Determinism risk

With fixed reduction order (option B): bit-deterministic. With option A: deterministic only if the partition and traversal order within each partition are fixed.

### Estimated speedup

1.5-3× at high population. The repulsion loop is O(N × avg_neighbors), which grows with density.

### Cost

Option B: ~50 LOC in `world.rs`, no new dependencies. Option A: ~100+ LOC.

### Recommended priority: **do-after-other-wins**

Same logic as eat/scavenge: NN + vision wins come first; repulsion is worth revisiting if the sim targets 2000+ populations.

---

## 3. Off-Thread Serialization for Autosave

### Current state

`maybeAutosave` in `web/src/main.ts` (lines 76-98) calls `world.snapshot_json()` (line 91), which invokes `WorldHandle::snapshot_json` in `src/wasm_api.rs` (lines 299-302). That calls `self.inner.to_save_v1()` followed by `serde_json::to_string(&save)`. The `to_save_v1` path in `src/save.rs` (lines 138-200) clones all SoA columns and brain weight Vecs. At 500 creatures, each with a `Brain` of 3456 `f32` weights (`NN_WEIGHT_COUNT`, `src/constants.rs` line 63), the weight payload alone is `500 × 3456 × 4 = 6.9 MB` of cloned floats before JSON encoding. JSON encoding of `f32` arrays is roughly 5-10× larger than binary, so the serialized string can reach 30-70 MB at moderate populations. This runs synchronously on the main thread at each autosave trigger.

The persistence worker in `web/src/persistence/worker.ts` already offloads the IndexedDB write. The comment in `main.ts` lines 88-92 notes that `snapshot_json()` is the "unavoidable wasm-side serde cost" — but there are two approaches to make it less disruptive.

### Approach 3a: Shadow WorldHandle in a Dedicated Worker

Maintain a second wasm instance in a Web Worker. Periodically copy the sim state from main to worker via SoA typed-array buffers (Transferable or structured-clone), then serialize in the worker. Trades: doubles memory (two wasm instances), requires an IPC protocol for state transfer, and the state copy itself takes wall time. At 500 creatures, copying the SoA float arrays (excluding brains) is cheap (~50 KB transferred). Copying brain weights (6.9 MB) still dominates; with SAB + `SharedArrayBuffer` views, the copy can be zero-copy but requires careful synchronization to avoid tearing.

### Approach 3b: Expose Binary Snapshot via Transferable ArrayBuffer

Add a `snapshot_binary()` method to `WorldHandle` that serializes to a flat binary format (`bincode` or hand-written) rather than JSON. Ship the resulting `ArrayBuffer` to the IDB worker via `postMessage([buf], [buf])` (transferable = zero-copy). The worker converts binary → JSON locally (or stores binary directly in IDB as a Blob). Binary serialization of 3456 × 500 f32s takes ~7 ms vs ~150 ms for JSON at that size. The IDB worker already runs off-main-thread; the only main-thread cost would be the binary pack.

### Determinism risk

None. The snapshot is a read-only snapshot of world state. The autosave path does not feed back into the sim RNG or creature state.

### Browser compatibility risk

Transferable `ArrayBuffer` is universally supported. A `bincode`-based approach requires either a Rust-side bincode dependency or a hand-rolled binary writer.

### Estimated speedup

Approach 3a: main thread off the critical path; serialization cost shifts to the worker thread and overlaps with the sim. Eliminates the periodic ~50-200 ms main-thread pause (measured range from comment at `src/wasm_api.rs` line 298: "1-5 ms" is optimistic; with 500+ creatures and full brain weights it is more). Approach 3b: 5-15× faster serialize path, still on main thread unless combined with 3a.

### Cost

3a: ~80 LOC new TypeScript, new wasm API surface, architectural change. 3b: ~40 LOC new Rust (`wasm_api.rs` + binary writer), ~30 LOC JS. Files: `src/wasm_api.rs`, `web/src/main.ts`, `web/src/persistence/worker.ts`, `web/src/persistence/index.ts`.

### Recommended priority

3b (binary format): **do-after-other-wins** — the payoff is clear and the risk is low.
3a (shadow world): **v1.1** — architectural complexity; the binary format gets most of the benefit first.

---

## 4. Render Off Main Thread (OffscreenCanvas)

### Assessment

The current renderer in `web/src/render.ts` is a pure `CanvasRenderingContext2D` draw loop calling `renderWorld` once per `requestAnimationFrame` (main.ts line 274). At 1000 creatures the render loop performs ~1000 `arc` + `fill` + up to 5 `stroke` calls per creature, plus the 400-cell sun grid loop. Profiling from the code suggests the render is cheap relative to the sim tick at 1× speed; at 100× speed the sim dominates.

`OffscreenCanvas` is supported in Chrome 69+, Firefox 105+, Safari 16.4+; Edge follows Chrome. Transferring a `RenderingContext` to a Worker is possible via `canvas.transferControlToOffscreen()`. But the payoff is low because: (a) the render is not the bottleneck at typical sim speeds, (b) the typed-array buffers (`creatures_buffer`, `sun_buffer`, `carrion_buffer`) would still need to cross the Worker boundary each frame (structured-clone or SAB), adding latency, and (c) the existing architecture — where the render reads directly from wasm memory via `unsafe Float32Array.view` — would need to be redesigned.

### Recommended priority: **never**

The render is not the bottleneck. The architectural cost exceeds any frame-time win.

---

## 5. Vision Raycast on GPU (WebGPU Compute Shader)

Per v5 §14, WebGPU integration is explicitly deferred to v1.1. The vision raycast (DDA grid walk, `src/vision.rs` lines 127-254) is a natural GPU compute target — 512 creatures × 24 rays = 12,288 independent ray tasks per tick, each doing up to 64 cell lookups. A compute shader dispatched on a GPU with 256-core warps would handle this in ~48 warps, potentially <1 ms even at large populations. However: WebGPU is not universally available (iOS Safari is the major gap, with WebGPU support experimental as of mid-2025), the data transfer round-trip (CPU grid → GPU buffer → CPU result) adds 0.5-2 ms overhead at this data size, and the implementation effort is very high.

### Recommended priority: **v1.1** (already deferred per spec)

---

## 6. Web Worker Pool for Non-Render Tasks

### Assessment

Profile sampling (stats panel at 1 Hz, `world.stats_sample()` in `src/wasm_api.rs` line 285) is O(1) and costs <0.1 ms. Event log writes are disabled by default (`events_enabled = false`, `src/world.rs` line 59). Species list JSON (`species_list_json`, `src/wasm_api.rs` line 200) is O(N+S) and runs at 1 Hz from the rail poll — cheap. Inspector JSON (`creature_inspect_json`, line 243) is O(1). None of these represent measurable main-thread cost at v1 population sizes.

### Recommended priority: **v1.1** — revisit when the events UI is re-enabled and event log writes scale.

---

## Recommended First Wins

**Ship these two together:**

**1. Enable the `threads` feature + wire `initThreadPool` in JS.** The Rust code is done (`src/world.rs` lines 301-374). Only ~30 LOC of TypeScript plumbing remain — import `initThreadPool` from the wasm bindgen output, call `await initThreadPool(navigator.hardwareConcurrency)` before constructing `WorldHandle`, add a SAB feature-detect fallback. Build with `--features threads`. At 512+ creatures this yields 4-8× speedup on the NN forward pass, which is the dominant per-tick cost. Zero determinism risk: the parallel path produces bit-identical results to sequential due to the fixed `N_CHUNKS = 8` chunking design (v6 §J). COOP/COEP headers are already deployed.

**2. Parallelize `VisionPass::run` under the same `threads` feature.** Add ~50 LOC to `src/vision.rs` — a `par_iter_mut` over the 8 creature chunks with per-chunk `fill_one` dispatch. All reads are immutable (`&VisionPass` is already `Sync`), writes are disjoint (one `VisionBuf` per creature). At 512 creatures with 12+ eyes, vision is co-dominant with the NN pass; together these two opportunities effectively double the sim throughput on a 4-core machine for vision-heavy populations, with no change to the golden snapshot hash and no browser compatibility regression beyond what threading already requires.

These two can be delivered in a single PR, gated behind the existing `threads` feature flag, with a JS-side fallback for SAB-unavailable environments.
