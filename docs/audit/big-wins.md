# Big Wins — Strategic Audit

> Companion to the narrow perf+UI audits (`docs/research/perf-*.md`,
> `docs/plans/perf-*`, `docs/plans/ui-*`). Those find the next 10–30%; this
> ranks the 10x opportunities that won't fall out of an inner-loop review.
> Opinionated.

Ranked by expected payoff × confidence. S = ~1 day, M = ~3–5 days, L = ~1–2
weeks, XL = >2 weeks. Effort estimates assume one engineer fluent in the
codebase.

---

## 1. Move the renderer to WebGL/WebGPU instanced-quads with a stable SoA view

Canvas2D in `web/src/render.ts` is the wrong primitive for this sim. Today it
does ~400 `fillRect`+string allocs for the sun grid every frame, an
`arc()`+`stroke()` per ring per creature (up to 5 rings × N creatures), no
off-screen cull, and re-reads `creatures_buffer()` every RAF. The data is
literally already in a `Float32Array` view onto wasm memory — a single
instanced quad draw call with a vertex pull from that buffer would render the
sun grid and bodies at >100k creatures with zero JS per-creature overhead, and
unlocks rings, energy halos, motion trails, and zoom-1-to-1000 without per-zoom
hand-tuning. This is the single change that lets us 10× peak population and 10×
sim speed simultaneously, because it removes the JS main thread as a bottleneck
in the player-visible regime (100× speed). Pairs naturally with
`OffscreenCanvas` on a worker to eliminate render contention with the sim.

- **Effort:** L
- **Payoff:** Unblocks 10k+ creature populations and 100× sim speed
  simultaneously; >5× frame-time headroom even at v1 sizes; enables visual
  effects that aren't economical in C2D (per-pixel sun field, soft shadows,
  energy glow, vision-cone overlay).
- **Risk:** WebGL2 is universal; WebGPU still flaky on iOS Safari. Pick WebGL2
  + Vite shader plugin to stay broad. Real risk is a `Float32Array::view`
  staleness footgun if wasm memory grows under us — fix with item 4 (memory
  budget) before shipping.

---

## 2. Pre-allocate everything: declare a hard MAX_POPULATION and remove all grow paths

The whole engine is shaped around `Vec::push`/`swap_remove`/`with_capacity(2048)`
with no hard cap. There are >25 parallel SoA columns + 7 hot-mirror columns +
8 scratch Vecs + `eye_trig` + `vision` + `cell_to_carrion` Vec-of-Vec, every
one of which can independently realloc. This causes three problems: (a) wasm
linear memory grows mid-run, invalidating every `Float32Array::view` JS holds
(documented hazard in `perf-final-report.md` #18); (b) allocator pressure shows
up as random tick spikes in profiles; (c) the design forbids zero-copy slab
storage for genome/brain because Vec growth would move them. Pick a number
(e.g. 8192), allocate every parallel column to that capacity at `World::new`,
and add a `debug_assert!(n < MAX_POPULATION)` in `handle_births`. Cull on
overrun (oldest, weakest, or random — design choice). Everything downstream
simplifies: brains can live in one `Box<[f32; MAX_POPULATION * NN_WEIGHT_COUNT]>`
slab (item 5), wasm memory becomes a fixed quantity, and view-staleness goes
away forever.

- **Effort:** S to declare the cap and pre-alloc; M to add and tune the cull
  policy and surface it in the UI.
- **Payoff:** Eliminates an entire class of latent bugs (view staleness,
  allocator spikes); unlocks slab layouts that 2–4× the sim hot path.
- **Risk:** Picking the cull policy badly is a balance/UX question, not a perf
  one. Pop-cap at "fail loudly" first; add cull later behind a slider.

---

## 3. Replace fixed-topology MLP with a NEAT-style variable-topology brain (or strongly justify keeping it)

The current 136→24→8 MLP with 3,456 weights is the spec, and it works for the
acceptance test, but it's a strategic dead-end for "watch evolution actually
do something interesting." The genome can't grow new sensors, drop unused
ones, or evolve more complex behaviors beyond what a single hidden layer of
24 ReLU units can encode. Worse: per F.30 known-issue #1, the random init
collapses to all-zero hidden, requiring a hardwired energy sensor just to
boot. NEAT (or any minimal variable-topology scheme — start with N hidden
units, allow add-neuron / add-connection mutations) sidesteps the bootstrap
problem (start with direct sensor→action connections), gives the player
genuinely surprising emergent behaviors (which is the whole pitch), and
naturally compresses unused capacity. It costs the SIMD forward pass — but
honestly NN forward is already <1ms/tick on the perf budget, and the cost is
~1 ms ms/tick that we're not bottlenecked on. The 24-hidden architecture is
also a known balance footgun (see F.30 NN founder hardwiring).

- **Effort:** XL (new mutation operators, species distance over topology,
  forward pass for sparse graphs, save schema migration).
- **Payoff:** This is the difference between "evolution sandbox demo" and
  "evolution sandbox that produces stories." Removes the F.30 founder
  hardwiring spec deviation entirely. Doubles or 10×s engagement depending
  on how well topology mutations shake out.
- **Risk:** Highest of any item here. NEAT is research-territory; tuning
  speciation + complexity-cost can take weeks. Mitigation: keep the fixed
  MLP behind a feature flag during the experiment so we can A/B player
  engagement.

---

## 4. Treat wasm memory as a fixed budget and audit every allocation

`SaveV1` clones the entire world (creature SoA × all 18 columns + brains +
sun + species) into owned Vecs to serialize, then `serde_json` stringifies
~3MB+ at v1 sizes. Hall-of-fame stores cloned `Genome` (~120B) per slot.
`pending_extinction_check`, `events.all` (unbounded ring), `events_total_count`
across the JS bridge, JSON across the bridge every inspector frame, hot-mirror
SoA columns at 7×4 = 28 bytes × N alongside the original genomes (~120B × N).
Nobody has measured the total wasm memory footprint or written a budget
document; this is the kind of latent issue that goes from "fine" to
"out-of-memory in Safari iOS" in one population spike. Land an audit: (1)
declare a memory budget (e.g. 64MB), (2) count every long-lived allocation by
N, M, and constants, (3) for anything that scales with sim runtime
(`events.all`, `species.list` for extinct lineages, `carrion` decay debt),
add a hard cap or compaction. Bonus: snapshot via a binary format
(bincode/postcard) instead of JSON drops save size 5–10× and serialization
time even more.

- **Effort:** M (audit + budget doc); S to swap JSON→binary for save.
- **Payoff:** Prevents iOS Safari OOM at long runtimes; binary save knocks
  out the "snapshot_json is a tens-of-ms pulse" issue noted in main.ts;
  makes save/restore acceptable on phones.
- **Risk:** Binary save breaks F.26's load path for legacy saves. Keep a
  one-version JSON-load fallback (`tryLoadJsonV1 → tryLoadBinaryV2`).

---

## 5. Single contiguous slab for Brains, deterministic indexing, kill `Vec<Brain>`

Each `Brain` is `Vec<f32>` (3,456 floats = ~14KB) + a few scalars. With 1,500
creatures that's ~21MB of brain storage scattered across 1,500 heap
allocations — disastrous for L2 hit rate during NN forward pass and the cause
of "brain memcpy on birth" cost. Allocate one
`brains_slab: Vec<f32>` of length `MAX_POPULATION * NN_WEIGHT_COUNT` (once
item 2 lands). Birth = `slab[child_offset..].copy_from_slice(&slab[parent_offset..])`
+ in-place mutation. Death = `swap_remove`-equivalent on the slab via a chunk
swap (analogous to `swap_remove_chunk` already in `creature.rs`). This makes
NN forward genuinely cache-friendly, doubles the SIMD win at scale, and
removes the 1,500 mid-tick allocations on birth bursts. Same approach
extends to `Genome` (one struct-of-arrays for the 14 fields) once the hot
mirrors confirm the layout split was correct.

- **Effort:** M (mostly mechanical — Brain::forward already takes &self with
  bounded indices; thread a `&[f32]` slice through instead).
- **Payoff:** 1.5–3× NN forward at 1,500+ creatures (cache + zero alloc on
  birth); enables item 1's GPU path to bind brain weights as a single uniform
  buffer.
- **Risk:** Save schema change — serialize the slab in order. Mitigated by
  binary save migration (item 4).

---

## 6. Replace ad-hoc per-frame wasm↔JS chatter with one batched "frame snapshot" call

Today the render loop calls `creatures_buffer()`, `sun_buffer()`,
`sun_capacity_buffer()`, `carrion_buffer()`, `creature_ids_buffer()`,
plus inspector polls `creature_inspect_json(idx)` every frame, plus
`species_list_json()`, plus `stats_sample()`. Each is a separate wasm-bindgen
boundary crossing with its own argument marshalling. At 60fps with the
inspector open this is ~8 boundary crossings × 60 = 480/s of JS↔wasm chatter,
each of which prevents wasm from running. Replace with a single
`world.frame_snapshot(want_inspector_idx, want_stats, want_species)` →
`Float32Array` containing tagged sections, parsed JS-side. Or even better:
shared memory layout where JS reads directly from wasm memory through a
fixed-offset header (since we already need SAB for threads). This pattern
is what every mature wasm engine ends up doing.

- **Effort:** M.
- **Payoff:** Removes JS↔wasm as a render-loop bottleneck (currently
  invisible because raycasts dominate, but it'll appear immediately after
  item 1 lands). Probably 1-3ms/frame at high zoom.
- **Risk:** Low. The boundary layer is already plumbed; this is
  consolidation.

---

## 7. Delete the Events subsystem (or restore it deliberately) — decide

`EventLog`, `EventKind`, the `recent_events_json` API, the rail-events DOM,
`pollRail` event-diff logic, `EventSnapshot` in `SaveV1`, `events_enabled`
guard on every `events.push` call site in `world.rs`, three tests gated on
`events_enabled=true`, the `events_total_count` JS-bridge polling — **all of
this is shipped in v1.1 with `events_enabled=false` by default**. The user
disabled the UI for the Events tab but kept all the dead-weight: per-tick
guards, snapshot serialization, JSON APIs that return `[]`. This is hundreds
of LOC and meaningful per-tick cost (cheap individually but it adds up across
6 fire-sites per tick × N creatures). Either: (a) delete it cleanly,
recovering the LOC and cycles and shrinking the save schema, or (b) restore
the UI and the polling and actually use it (the player wants to see "Lineage
A3 just speciated!" — that's literally the pitch). Punting forever is the
worst option.

- **Effort:** S to delete; M to restore with a fresh, designed-for-the-UI
  event model.
- **Payoff:** Either a code-quality win (delete) or a UX win (restore).
  Neither big in isolation but the current state actively rots.
- **Risk:** Delete loses the save-compat fallback for old saves with events.
  Mitigation: bump SCHEMA_VERSION and skip the field on load.

---

## 8. Replace SpatialGrid with a quadtree or sweep-and-prune for size-heterogeneous worlds

The 5u uniform grid was sized for current SIZE_MAX=10 → max body radius=10u →
2-cell-radius queries. Once item 3 (NEAT brain) and balance tuning produce
the natural outcome — wildly heterogeneous creature sizes — the uniform grid
either has to shrink (more cells, more rebuild cost) or query large radii
(more cells touched per query). A quadtree handles 100x size variance
gracefully; sweep-and-prune is even faster for the typical "mostly static
positions with occasional moves" pattern in a slow sim. Vision DDA also gets
a major upgrade: traverse the quadtree along the ray instead of cells. Not
worth doing for current v1 balance, but it's the kind of architectural
foundation that has to land before "evolution sandbox where megafauna
emerge" is a real possibility. Pairs with item 3.

- **Effort:** L.
- **Payoff:** Unblocks heterogeneous size evolution (currently
  SIZE_MAX=10 cap is partly a perf artifact, not a balance choice).
  2–5× vision pass at sparse populations.
- **Risk:** Determinism is harder in a quadtree (tree rebalances). Need
  insertion order = creature index order.

---

## 9. Add a deterministic replay/scrubber by separating sim from save into an event stream

Today: there is only point-in-time save. The user can't watch a 10,000-tick
sim play back, can't scrub to "the moment Lineage A3 went extinct," can't
share an interesting world without sharing the seed + a 3MB save. Every
mature sim ends up with a sparse event stream (births, deaths, speciations,
extinctions, action choices for inspected creatures) plus periodic
keyframes — replay becomes free, sharing becomes a 10KB URL, and you can build
"highlight reel" UX on top of it. The sim is already deterministic from
seed, so a replay is conceptually just `seed + tick_range + inspect_indices`.
This is the single capability the player most clearly wants ("show me a tour
of the lineage tree") that doesn't exist today.

- **Effort:** L (event stream design + UI). M if you just want determinism-
  from-seed-and-tick scrubbing without event compression.
- **Payoff:** Transforms the product from "sandbox you watch live" to "world
  you explore." Big engagement multiplier. Enables sharing.
- **Risk:** Replay determinism breaks the moment you allow any
  user-interactive input that modifies sim state mid-run (dev sliders!).
  Decide upfront: dev-slider changes are out-of-band annotations or sim ends
  at first slider change. The former is friendlier.

---

## 10. Cut the dual single-threaded / threads codepaths down to one (and ship threads-on by default)

`world.rs::nn_forward_all_chunks`, `vision.rs::run`, and the build infra
all carry parallel paths gated by `cfg(feature = "threads")`. Per BUILD-REPORT
known-issue #7+#9, threads-on requires a separate golden snapshot and the
WSL2 dev path is 9× slower with rayon. The result: every change to a hot
path has to be re-verified on both paths, the golden bootstrap is dual, and
the parallel codepath has subtle differences (inlined carrion overlap math in
the threaded branch vs. function call in sequential). Pick one: ship the
threaded path always (with `RAYON_NUM_THREADS=1` as the single-threaded
escape hatch — already documented for WSL2). This collapses two codepaths
into one, removes a maintenance tax that grows with every perf change, and
makes the parallel path the path everyone debugs against. Per the perf-final
report the threaded path is already bit-identical on the bootstrap; the dual
codepath is defensive overhead at this point.

- **Effort:** S to delete the `cfg(not(threads))` branches; M to harden
  the rayon path on WSL2 (where it's currently a perf cliff).
- **Payoff:** ~150 LOC deleted; one golden file; one codepath to profile and
  optimize. Frees mental energy for items 1–9.
- **Risk:** Some users on locked-down browsers may lack SAB (no
  COOP/COEP). Fallback to `RAYON_NUM_THREADS=1` already handles this via
  the existing JS-side gate.

---

## Honorable mentions (not in the top 10 but worth tracking)

- **Fixed-point math for cross-platform determinism.** f32 + transcendentals
  is bit-stable across same-arch builds today but will bite if we ever
  WebGPU-port the sim. Defer until item 1 lands and we know whether GPU sim
  is on the table.
- **Replace `serde_json` with `bincode`/`postcard` for the snapshot.** Subset
  of item 4.
- **Replace `Vec<Vec<u32>>` in `cell_to_carrion` with a flat
  `Vec<u32>` + `starts: Vec<u32>` like SpatialGrid.** Narrow perf item, but
  the per-tick `cell.clear()` + Vec-of-Vec allocator pattern is the same
  smell as the grid that got fixed in perf-3.
- **Two-tier action selection (gate by "wants to act" before NN forward).**
  Most creatures rest/photosynth; if energy is high, gate against running
  the full 136→24→8 NN. ~5x speedup on idle populations.
- **Drop `last_action` one-hot + `cooldown_frac` + `carrion_overlap_norm`
  inputs and replace with a learned hidden-state feedback (RNN-lite).**
  Would force lineages to evolve memory rather than the fixed scalars we
  hand them. Risky but high-pitch.
