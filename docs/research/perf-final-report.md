# evosim v1.1 Perf Pass — Final Report

> Synthesizes four research docs into one ranked roadmap. Source docs (kept as
> reference material):
>
> - `docs/research/perf-sim-hotpath.md` — Rust sim hot path
> - `docs/research/perf-parallelism.md` — threads / workers / off-main-thread
> - `docs/research/perf-rendering.md` — JS render loop, Canvas, DOM
> - `docs/research/perf-layout.md` — data layout, cache, SoA/AoS, SIMD coverage
>
> §-references throughout this report point into those four docs; e.g.
> "(hotpath §1a)" means `perf-sim-hotpath.md` section 1a.

---

## 1. TL;DR

In priority order, the highest-leverage wins:

1. **Enable threads + parallel vision** (parallelism §1, §2a). NN forward is
   already `cfg(threads)`-gated; vision is embarrassingly parallel. ~30 LOC of
   JS init + ~50 LOC Rust. **Est. 4–8× on NN forward and 4–6× on vision** at
   ≥512 creatures. Effort: M. Requires golden re-bootstrap.
2. **Pre-compute eye sin/cos per creature** (hotpath §1a). Eliminates ~72 000
   trig calls/tick. **Est. 0.4–0.8 ms/tick saved (~17–35% of current 2.3 s
   acceptance time).** Effort: S. Determinism-safe.
3. **Genome hot-field SoA split** (layout §1). Drops per-tick genome working
   set 299 KB → 41 KB; warms `photosynth`, `energy_bookkeeping`, `vision`,
   `repulsion`. **Est. 2–5× reduction in cache-miss stalls on those passes.**
   Effort: M. Determinism-safe (purely additive).
4. **Pool the per-tick scratch Vecs onto `World`** (hotpath §2a/d/f, layout §3).
   `fx`/`fy`, six `eat_and_scavenge` vecs, and the grid cursor clone. **Est.
   0.10–0.20 ms/tick + ~300 KB/tick allocator pressure removed.** Effort: S.
   Determinism-safe.
5. **SIMD ray-vs-8-circles batch in vision DDA** (hotpath §1b). Highest
   ceiling left after the above. **Est. 0.5–1.2 ms/tick** at typical density.
   Effort: M. Determinism-safe.

Items 1–4 should ship as a v1.1 perf PR sequence; item 5 follows once the
perf-timing widget confirms vision is still dominant.

---

## 2. Current state baseline

**Acceptance test (v5 §16):** seed `evosim-test-001`, 10 000 ticks @ 1 500
creatures peak, single-threaded release build → **2.31 s wall-time on dev box**
(BUILD-REPORT.md, §16 row c). Budget is 8 s; current headroom ~3.5×. That
headroom is the only reason no perf work has shipped yet.

**v5 §3.6 budgeted tick cost** (PITCH-v5 §3.6, single-threaded, 1 500 creatures
@ 30 tps):

| Stage | Budgeted | Notes |
|---|---|---|
| Grid rebuild ×3 | ~0.2 ms | spec said ×2; actual code calls 3× (hotpath §3a) |
| Raycasts | ~3–5 ms (dominant) | 1 500 × ~12 active rays × ~10 cells/ray |
| NN forward | ~0.5 ms with SIMD | 1 500 × (130×24 + 24×8); confirmed (hotpath §4d) |
| Soft repulsion | ~0.3 ms | |
| Photosynth + eat + scavenge + energy + births | ~1 ms | |
| **Total** | **~5–7 ms / tick** | acceptance 0.23 ms/tick average |

**Reconciliation:** acceptance averages 0.23 ms/tick at 1 500 peak but the
peak-tick cost is what the budget is sized for. The four research docs all
converge on the same hot-path ranking:

- **Raycast dominates** (hotpath §1, parallelism §2a, layout §13). Consensus.
- **NN forward is small per-tick** (hotpath §4d: ~0.5 ms ceiling; parallelism
  §1: still the most parallelizable single block). No disagreement.
- **Repulsion is the third hottest** (hotpath §3, layout §12, parallelism §2d).
- **Allocator churn is ~300 KB/tick** even with high water-marks held — the
  three sources (hotpath §2, layout §3, layout §11) line up cleanly.

**Already optimized:**

- SIMD NN forward via `wide::f32x8`, 17-chunk layout (brain.rs).
- Prefix-sum + cursor scatter spatial grid (grid.rs); rebuild is O(N+cells).
- SoA for hot scalars (`x`, `y`, `vx`, `vy`, `energy`, `age`, …) on
  `CreatureSoA` (creature.rs). Layout §7 confirms this is the right default.
- DDA early-out on `best_dist < next_t` (vision.rs:224–226).
- Inactive-sector skip in `fill_one` (vision.rs:86–88).
- Symmetric-pair repulsion write (hotpath §3d).
- Per-creature `Vec<f32>` reused across ticks for `creatures_buffer` /
  `sun_buffer` (no per-frame Rust-side allocation).
- IDB write off-main-thread via `web/src/persistence/worker.ts`.

**Left on the table** (ranked, addressed in §3 below):

- Per-creature trig recomputed every tick.
- AoS `Genome` inside `CreatureSoA` forces 204-byte lines for 4-byte reads.
- Three `Vec` clones per `grid.rebuild` = 168 KB/tick of allocator traffic.
- `cfg(threads)` is wired in Rust but never initialized from JS — full NN +
  vision parallelism is dormant.
- No SIMD on ray-circle test; scalar inner-inner loop.
- 400 `fillRect` + 400 string allocs in `drawSunMap` every frame.
- Inspector re-serializes a 20-field JSON every RAF when open.
- No off-screen creature cull.

---

## 3. Prioritized roadmap

Items ranked by impact / effort. Cross-references in the **Deps** column point
to other items where one enables another. Determinism column: **safe** = no
golden re-bootstrap required; **breaks** = `tests/golden_snapshot_t10000.txt`
must be regenerated. **TBD** = needs a measurement decision.

| # | Item | Source | Effort | Risk / Det. | Est. ms/tick saved | % of 2.31 s acceptance | Deps |
|---|---|---|---|---|---|---|---|
| 1 | Pre-compute eye `sin`/`cos` per creature | hotpath §1a | S | **safe** | 0.4–0.8 | 17–35% | — |
| 2 | Pool per-tick scratch vecs onto `World` (`fx`,`fy`, eat/scavenge six, eat candidates) | hotpath §2a,§2c,§2d; layout §11 | S | **safe** | 0.05–0.15 | 2–7% | — |
| 3 | Grid cursor scratch buffer (eliminate `starts.clone()` ×3/tick) | hotpath §2f; layout §3 | S | **safe** | 0.05–0.10 | 2–4% | — |
| 4 | Remove `Genome::clone()` in `eat_and_scavenge` | hotpath §6a | S | **safe** | 0.03–0.07 | 1–3% | — |
| 5 | Enable `threads` feature; `initThreadPool` in JS; SAB feature-detect fallback | parallelism §1 | M | **breaks** | NN goes 4–8× parallel | (multi-stage; see §5) | — |
| 6 | Parallelize `VisionPass::run` (par_iter_mut over 8 chunks) | parallelism §2a | M | **breaks** | Vision goes 4–6× parallel | (multi-stage; see §5) | 5 |
| 7 | Genome hot-field SoA split (7 scalars) | layout §1 | M | **safe** | indirect: 0.3–0.8 via cache | 13–35% (estimate) | enables 11, 12 |
| 8 | SIMD `f32x8` ray-vs-8-circles batch in DDA inner loop | hotpath §1b | M | **safe** | 0.5–1.2 | 22–52% | — |
| 9 | Pre-allocate `CreatureSoA` to `MAX_POPULATION` at init (eliminate mid-run grow) | layout §10 | S | **safe** | <0.01 (safety win, not perf) | — | — |
| 10 | `biggest_ever_size: f32` flat field (skip `species.get().name.clone()` per tick) | hotpath §8 | S | **safe** | 0.01–0.03 | <1% | — |
| 11 | SIMD photosynth two-pass `want = K·eff·size` | layout §5 | S (post-7) | **safe** | <0.01 | <1% | 7 |
| 12 | SIMD energy bookkeeping upkeep terms | layout §6 | S (post-7) | **safe** | <0.01 | <1% | 7 |
| 13 | NN weight transpose for layer 1 (kill horizontal `reduce_add`) | hotpath §4c | M | **breaks** | 0.1–0.2 | 4–9% | — |
| 14 | Inspector throttle to 5 Hz + drop the per-frame `new Set` in `drawCreatures` | render §1,§6 | S | **safe** (JS-only) | 0.3–1 ms/frame render | render-side | — |
| 15 | `drawSunMap` → `putImageData` (collapse 400 fillRect into 1 blit) | render §3 | M | **safe** (JS-only) | 0.5–1.5 ms/frame render | render-side | — |
| 16 | Off-screen creature culling (4-cmp viewport reject) | render §8 | S | **safe** (JS-only) | 1–4 ms/frame at zoom ≥4 | render-side | — |
| 17 | Stats chart wall-clock gate (cap `drawCharts` at 1 Hz) | render §5 | S | **safe** (JS-only) | 0.2–0.5 ms/frame render | render-side | — |
| 18 | Re-fetch `ids` view as last buffer call (or `.slice()` it) — safety fix | render §9 | S | **safe** (JS-only) | correctness, not perf | render-side | — |

Detail on the items that need explanation beyond the table:

**#1 Pre-compute eye `sin`/`cos`** (hotpath §1a). Today `fill_one` does
`theta_ray.cos()` + `theta_ray.sin()` for every active sector every tick.
That's ~72 000 transcendentals/tick at 1 500 creatures × 24 eyes worst case →
~1.1 ms purely for trig at 15 ns each. Sector centers are fixed; `eye_offsets`
is genome-fixed between births. Cache `[(f32,f32); 24]` on `CreatureSoA` or
`World`, invalidate on birth/genome mutation. Best impact-per-LOC item in the
report. **Priority 1.**

**#2 + #3 Scratch-buffer pooling** (hotpath §2a/c/d/f, layout §11). Eight
per-tick `vec!` calls (`fx`, `fy`, `damage`, `gain`, `cooldown_set`,
`attempted_eat`, `attempted_scavenge`, `got_a_bite`) plus three `starts.clone()`
in `grid.rebuild` together account for ~300 KB/tick of allocator round-trips
and zeroing. Promote all eight Vecs to fields on `World`; add `cursors: Vec<u32>`
to `SpatialGrid`. One commit, all four items, fully additive. **Priority 2
(bundle items 2–4 + 10).**

**#5 + #6 Threads end-to-end** (parallelism §1, §2a). The Rust path under
`cfg(feature = "threads")` already exists in `world.rs:301–374`; the missing
piece is `await initThreadPool(navigator.hardwareConcurrency)` in
`web/src/main.ts` before `WorldHandle` construction, plus a SAB feature-detect
fallback. COOP/COEP headers are already deployed (BUILD-REPORT.md confirms).
Vision parallelization adds ~50 LOC in `src/vision.rs` (`par_iter_mut` over
the disjoint `out` slices). Together these are **the single biggest sim
throughput win available** — but they require regenerating the golden hash
(`tests/golden_snapshot_t10000.txt`) because the sub-RNG path differs from the
sequential path. Schedule deliberately. **Priority 3 (after items 1–4 land
without golden churn).**

**#7 Genome SoA split** (layout §1). Per-tick genome working set drops from
299 KB (L3) to 41 KB (L2). Six tick phases benefit:
`photosynth_two_pass`, `energy_bookkeeping`, `apply_movement_and_repulsion`,
`build_nn_input`, `count_carrion_overlap`/`compute_is_at_wall`, and
`eat_and_scavenge`. Keep `genomes: Vec<Genome>` for mutation, speciation,
serialization (those paths read the full struct). Sync the seven hot arrays on
mutation and birth. Estimated cache-miss reduction is 2–5× on the affected
passes; absolute ms savings are harder to peg without a profiler but likely
0.3–0.8 ms/tick. **Determinism-safe — values are identical; only storage
layout differs.** Also unlocks items 11 and 12 (SIMD on
photosynth/energy_bookkeeping) for free.

**#8 SIMD ray-vs-8-circles** (hotpath §1b). The vision DDA's inner-inner loop
(`ray_circle_hit` per creature in cell) is the single biggest scalar block
left after items 1–7. Pad cell occupants to 8 with `cx = f32::MAX`, load into
`wide::f32x8`, run the entire circle test in one SIMD pass, lane-min reduce.
3–6× on dense cells; 0.5–1.2 ms/tick total at 1 500 creatures. Higher ceiling
than #7 but M effort and only attractive if vision is still the bottleneck
after threading + sin/cos cache land. **Defer until perf-timing widget
confirms.**

**#13 NN weight transpose** (hotpath §4c). Eliminates 24 → 3 horizontal
`reduce_add` calls per creature; 0.1–0.2 ms/tick. **Breaks golden hash and
breaks the save-schema** (weight order in `SaveV1` would change). Don't
schedule unless the broader determinism budget has already been spent on
items 5/6.

**#14–18 Render-side items** (render §1, §3, §5, §6, §8, §9) are pure JS — no
wasm or Rust changes — and have zero impact on the golden hash. Bundle into a
single "render polish" PR. The inspector throttle alone removes the worst
per-frame JSON round-trip when the inspector is open. `putImageData` for the
sun grid removes 400 `fillStyle = rgb(...)` string allocations and 400
`fillRect` calls per frame. Off-screen culling pays the most at high zoom.
None of these change tick wall-time; they affect frame budget, which matters
at high sim speeds when render fights for the same main thread.

**Items deliberately omitted** from the roadmap (covered in source docs):

- Eat/scavenge accumulator parallelization (parallelism §2b). Low realized
  speedup; only a fraction of creatures choose Eat per tick.
- Soft repulsion accumulator-reduction parallelism (parallelism §2d). Only
  pays at 2 000+ population.
- Brain slab (layout §2). Architectural; defer to a separate milestone.
- Brain clone pool on birth (layout §8). Bundled into the slab refactor or
  skipped.
- `EventLog::all` ring-cap (layout §9). Events disabled by default; not on
  hot path yet.
- VisionBuf copy elimination (layout §4). Breaks v6 §E NN-input layout
  contract; not worth it at v1 populations.

---

## 4. Items NOT to do

| Idea | Source | Why skip |
|---|---|---|
| Render off main thread (`OffscreenCanvas`) | parallelism §4 | Render is not the bottleneck. Architectural rework cost exceeds any frame-time win. Cross-worker buffer transfer adds latency. |
| Photosynth two-pass parallelization with atomics | parallelism §2c | Sun grid is 400 cells; total work ~2 000 float ops/tick. Atomic contention would exceed the parallelism gain. <1.1× theoretical. |
| Vision raycast on WebGPU compute shader | parallelism §5 | Already deferred per v5 §14. iOS Safari support still experimental; CPU↔GPU round-trip adds 0.5–2 ms for this data size. Revisit post-v1.1. |
| Web worker pool for sampling / event IO | parallelism §6 | Stats sample is O(1) <0.1 ms; event log writes are disabled by default. Re-evaluate when events UI is re-enabled. |
| Shadow `WorldHandle` in a worker for snapshot serialization | parallelism §3a | Architecture-heavy; binary snapshot format (parallelism §3b) gets most of the benefit at a fraction of the cost — and snapshot isn't blocking yet (autosave is already off-main-thread for the IDB write). |
| SIMD per-creature 8-ray batched DDA | hotpath §1f | Rays from one creature traverse different cells with data-dependent control flow; cannot advance in lockstep. Unclear payoff vs. complexity. The cell-level SIMD (#8) is the right granularity. |
| AoS pack of `(x,y,vx,vy,energy,action)` hot scalars | layout §7 | Marginal win on multi-field passes; reverses the win for the more common single-field passes. SoA stays as the default. |
| SIMD soft-repulsion pair loop | layout §12 | Bottleneck is the gather pattern (neighbor indices from grid scatter to non-sequential `x`/`y`/`size`). WASM has no gather intrinsic; x86 gather is ~2× slower than sequential. Fix genome SoA (#7) first; revisit only if profiling demands. |
| `tracing` / `tracy` / `puffin` integration for the profiler | perf-timing plan | Explicitly excluded — the perf-timing widget is a hand-rolled rolling-window tree. |

---

## 5. Determinism budget

The §16 golden snapshot hash (`tests/golden_snapshot_t10000.txt`,
`0xc35be8a7905c7f05`) covers tick + creature SoA + sun + carrion + species +
RNG state (per `src/snapshot_hash.rs`, v6 §M). Items split clean into two
buckets:

### 5a. Golden-safe (ship without re-bootstrap)

Items **1, 2, 3, 4, 7, 9, 10, 11, 12, 14, 15, 16, 17, 18**.

All of these either:
- Cache derived values that are mathematically identical to recomputed ones
  (item 1 `sin`/`cos`, item 7 SoA mirror of `Genome`).
- Move scratch buffers to long-lived storage without changing the values
  computed (items 2, 3).
- Remove a redundant clone of values already accessible by reference
  (items 4, 10).
- Pre-allocate capacity without changing post-tick state (item 9).
- SIMD-vectorize an op whose scalar result is bit-identical
  *because the addition tree is the same* (items 11, 12 — same single-pass
  accumulator order).
- Are JS-only and never touch the wasm sim state (items 14–18).

Schedule all of these **before** the threaded items so the golden stays
green and a regression elsewhere in the codebase is bisectable.

### 5b. Requires golden re-bootstrap

Items **5, 6, 13**.

- **Item 5 (threads)** changes the RNG path (`SimRng` → per-chunk sub-RNGs).
  Even though the result is deterministic, the rng-draw ordering differs from
  the sequential path, so the post-tick state hash differs.
- **Item 6 (parallel vision)** by itself produces identical output (vision is
  side-effect-free per-creature) and would NOT require re-bootstrap if it
  could ship without item 5 — but it gates on the same `cfg(threads)` flag,
  so in practice they ship together.
- **Item 13 (NN weight transpose)** changes the storage order of
  `Brain::weights`, which is included in the snapshot hash. Also requires a
  `SaveV1` schema migration (or a `SCHEMA_VERSION` bump).

Bootstrap protocol: land items 1–4, 7, 9, 10 first; confirm the golden still
matches and the acceptance test stays under budget. Then in a separate PR,
land items 5+6 together, regenerate the golden with
`cargo test --release --test acceptance -- --ignored --nocapture` (or the
equivalent bootstrap path), update `tests/golden_snapshot_t10000.txt`, and
note the re-bootstrap in the commit message + DECISIONS.md.

Item 13 is deferred indefinitely — only schedule if profiling after items
1–8 still shows NN forward as a meaningful contributor.

---

## 6. Suggested commit shape for v1.1 perf pass

A reviewable sequence. Each commit name follows the per-milestone style
(`perf(area): action`). Commits are ordered so the golden stays green through
commit 4 (the natural review checkpoint), then re-bootstraps once at commit 6.

1. `perf(sim): pre-compute sector sin/cos per creature` *(item 1)*
   — Adds `[(f32,f32); 24]` cache to `CreatureSoA` or `World`; invalidate on
   birth/mutation. ~80 LOC. Golden stays green. Expected: 17–35% acceptance
   wall-time reduction.

2. `perf(sim): pool per-tick scratch vecs on World and SpatialGrid`
   *(items 2 + 3)* — Promote `fx`, `fy`, the six `eat_and_scavenge` Vecs,
   and `grid.cursors` to long-lived fields. ~120 LOC across `world.rs` and
   `grid.rs`. Golden green.

3. `perf(sim): drop Genome and Brain clones from hot loops`
   *(items 4 + 10)* — Replace `let g_i = genomes[i].clone()` with scalar
   field reads in `eat_and_scavenge`; flatten `biggest_ever_size` to an `f32`
   field. ~60 LOC. Golden green.

4. `perf(layout): pre-allocate CreatureSoA to MAX_POPULATION at init`
   *(item 9)* — Bump `with_capacity(2048)` → `with_capacity(MAX_POPULATION)`
   (or 3000). ~5 LOC. Eliminates mid-run wasm memory growth → also fixes the
   `Float32Array::view` staleness hazard (item 18 partially). Golden green.

5. `perf(layout): genome hot-field SoA split (7 scalars)` *(item 7)* —
   Add `photo_eff`, `eat_eff`, `scav_eff`, `move_speed`, `size`,
   `vision_range`, `eye_count_hot` parallel arrays on `CreatureSoA`. Update
   all six hot-path read sites. Keep full `genomes: Vec<Genome>` for the cold
   paths. ~250 LOC across `creature.rs`, `world.rs`, `vision.rs`. Golden
   green. Also unlocks the easy SIMD wins of commits 7+8.

   **🔍 Natural review checkpoint.** Re-run acceptance test; expect <1.5 s
   wall-time. If acceptance still passes here, the rest of the PR series can
   land on its own cadence.

6. `perf(threads): wire initThreadPool in main.ts; build with --features threads`
   *(items 5 + 6)* — Adds `await initThreadPool(navigator.hardwareConcurrency)`
   before `WorldHandle` construction; SAB feature-detect fallback;
   `par_iter_mut` block in `VisionPass::run`. ~80 LOC TS + ~60 LOC Rust.
   **Re-bootstraps `tests/golden_snapshot_t10000.txt`.** Commit message must
   note the regen; DECISIONS.md entry required.

7. `perf(sim): SIMD photosynth and energy_bookkeeping (post-SoA)`
   *(items 11 + 12)* — Wraps the per-creature arithmetic loops in
   `wide::f32x8` over the new SoA arrays. ~80 LOC. Golden green (single-pass
   accumulator → bit-identical).

8. `perf(vision): SIMD ray-vs-8-circles in DDA inner loop` *(item 8)* —
   Pad cell occupants to 8; load into `f32x8`; lane-min reduce. ~120 LOC in
   `vision.rs`. Golden green. Expected: 0.5–1.2 ms/tick. Schedule only if
   the perf-timing widget (separate effort) confirms vision is still
   dominant.

9. `perf(render): inspector throttle, drawSunMap putImageData, off-screen cull`
   *(items 14 + 15 + 16 + 17 + 18)* — Bundle all render-side wins. Pure JS.
   ~150 LOC across `render.ts`, `rail/inspector.ts`, `rail/stats.ts`,
   `main.ts`. No wasm changes; golden unaffected. Expected: 2–6 ms/frame
   at high zoom.

10. *(optional, deferred)* `perf(brain): transpose layer-1 weight layout`
    *(item 13)* — Only if items 1–9 still leave NN forward in the top three.
    Re-bootstraps the golden a second time AND requires a `SaveV1` schema
    migration.

Commits 1–4 can ship as a single PR ("perf pass v1 — scratch + clones") with
zero determinism risk. Commits 5+6 ship as a second PR ("genome SoA + threads")
with the golden regen called out. Commit 7 is opportunistic. Commits 8+9 are
follow-on PRs scoped per-area.

---

## 7. Open questions / unknowns

Items the research couldn't pin down without measurement. The forthcoming
perf-timing widget (planned in `docs/plans/perf-timing.md`) instruments each
of the 12 tick sub-functions with rolling-window spans and will answer most
of these.

| Question | Source | How perf-timing resolves it |
|---|---|---|
| What is the real per-phase breakdown at 1 500 creatures vs. the v5 §3.6 estimates? | hotpath §10c, layout closing | Widget tags each step 1–12 from `World::step()`; values feed `#rail-stats`. |
| Is the actual NN forward cost really ~0.5 ms (spec) or higher with memory-latency weight loads? | hotpath §4d | NN forward span timed directly; compare cold vs. warm runs. |
| At what population does the eat/scavenge accumulator-reduction parallelism (parallelism §2b) start paying? | parallelism §2b | Eat/scavenge span timed; threshold visible. |
| Does `creature_buf` ever actually grow mid-run after the first peak, triggering the `Float32Array::view` staleness hazard? | render §9, layout §10 | Item 9 (pre-alloc to `MAX_POPULATION`) makes this moot; widget can still confirm via wasm memory size sampling. |
| At zoom 4× vs. 16×, what fraction of creatures are off-screen and what's the actual render-loop time? | render §8 | Widget should include a render-loop top-level span. |
| Is the photosynth/energy-bookkeeping SIMD worth the SoA prerequisite work in absolute terms? | layout §5, §6 | Per-step spans pre- and post-item-7 quantify this directly. |
| Does the rayon overhead at small populations (<128 creatures) actually exceed the gain? | parallelism §2a | Widget at varying population samples answers; informs the `if n < 128 { sequential }` guard. |
| At 100× speed, is render or sim the dominant frame cost? | render §3 | Top-level `frame()` span partitions `step_n` vs. `renderWorld`. |
| Does `species_distance` on weight diff (3 456 f32 subtracts) ever become non-negligible at high birth rates? | hotpath §7c | Births span; if births dominate, drill into species_distance. |

**Not resolvable by the widget** (still genuine unknowns):

- Allocator behavior under wasm — whether `mimalloc`/`dlmalloc` reuse the
  freed scratch Vecs in O(1) or pay full heap traversal. Only `console.time`
  + manual profiling would tell. Items 2+3 make this moot regardless.
- Exact dense-cell distribution in `vision.rs` (how often does a single cell
  hold 5+ creatures?). Determines the realized speedup of item 8. The widget
  can grow a histogram counter if needed.
- Browser-by-browser cost of the JS↔wasm boundary crossings for the many
  small getters (`world.tick`, `world.population`, etc.) — render §4 flags
  these. Likely sub-microsecond each but unverified.

---

## Appendix: where to drill in

- **Tick budget context**: `docs/archive/PITCH-v5.md` §3.6 (lines 89–105).
- **Acceptance test definition**: `docs/archive/PITCH-v5.md` §16
  (lines 371–388). Current results in `BUILD-REPORT.md` §16.
- **Module map**: `docs/architecture.md` "Rust modules" and "Tick ordering"
  sections.
- **Outstanding v1.1 priorities** (pre-research): `BUILD-REPORT.md` §
  "Suggested v1.1 priorities" — items 5 (per-tick allocations), 7 (WebGPU),
  9 (threads-by-default) all land in this report.
- **Perf-timing widget plan** (resolves §7 questions):
  `docs/plans/perf-timing.md`.
- **Source docs reference**: `docs/research/perf-sim-hotpath.md`,
  `perf-parallelism.md`, `perf-rendering.md`, `perf-layout.md`.
