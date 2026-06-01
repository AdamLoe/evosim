# Decisions — sim core

Declarative beliefs about how the simulation core is shaped. Each entry
is `Decision`, `Why`, `Applies to`, with optional `Alternatives
considered`, `Tradeoffs`, `Code anchors`, `Revisit when`.

---

### Single wasm instance, sim worker holds it

- **Decision**: Exactly one wasm module + one `WorldHandle` lives in the
  sim worker. Main thread holds no wasm.
- **Why**: One owner avoids reasoning about cross-thread wasm-memory
  races, and the renderer doesn't need wasm — it reads the SAB
  directly.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Alternatives considered**: Two wasm instances sharing linear memory
  (rejected — requires coordinating `--shared-memory` between two
  bundles + a second `initThreadPool`); main thread holds wasm and
  worker is a thread pool only (rejected — every UI event would block
  on the tick loop).

### Sim runs `step_n(1)` per loop iteration

- **Decision**: The worker's tick loop calls `world.step_n(1)`
  unconditionally per iteration; no batching.
- **Why**: Batching saved nothing meaningful (wasm-bindgen boundary cost
  is negligible compared to per-tick cost at any non-trivial pop) and
  cost snapshot freshness — at pop=8000 with a previous batching cap,
  the SAB flipped every ~1.2 s and the renderer repainted stale frames.
- **Applies to**: `architecture/worker-runtime.md`.
- **Alternatives considered**: `step_n(floor(tickBudget))` with a
  fractional-budget accumulator (shipped briefly; reverted — main.ts
  batched because it shared the render thread, the worker doesn't).
- **Code anchors**: `web/src/sim-worker.ts → simLoop`.

### Pacing uses `Atomics.waitAsync` with a 1 ms floor on `timeoutMs`

- **Decision**: The async tick loop awaits `Atomics.waitAsync(ctrl,
  CTRL_FUTEX, before, timeoutMs)`. `timeoutMs` is clamped to
  `max(1, 1000/targetTPS - elapsed)` when running and `Infinity` when
  paused. Synchronous `Atomics.wait` is forbidden.
- **Why**: `Atomics.wait` blocks the worker event loop and dark-holes
  every `onmessage`. `Atomics.waitAsync(..., 0)` returns synchronously
  with `{async: false, value: "timed-out"}` per the Web Atomics spec —
  no Promise, no microtask, no event-loop yield — so a 0-timeout would
  spin without yielding and reproduce the same dark-hole. The 1 ms
  floor forces the async path; the sim was already underrunning when
  the floor kicks in, so the throughput cost is zero.
- **Tradeoffs**: 1 ms minimum loop period caps pacing-bound throughput
  at ≤ 1000 iter/s. Only matters at target TPS > 1000, which the
  slider does not expose.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/sim-worker.ts → simLoop`,
  `web/src/sim-bridge.ts → SimBridge.postMessage`.
- **Revisit when**: a browser ships a different `Atomics.waitAsync(0)`
  semantics, or a wholly different pacing primitive becomes available.

### Macrotask yield on the `Atomics.waitAsync` not-equal path

- **Decision**: When `Atomics.waitAsync` returns synchronously
  (`r.async === false`, i.e., main mutated the futex between our load
  and the wait call), the loop must `await new Promise(r =>
  setTimeout(r, 0))` before continuing.
- **Why**: `onmessage` dispatches as a macrotask, not a microtask. A
  `await Promise.resolve()` would resolve before the postMessage task
  runs, so the loop would loop back to `drainMessages()` with an empty
  queue and lose the wake.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/sim-worker.ts → simLoop`.

### Slider drain ordering: drain at the top of every iteration

- **Decision**: `drainMessages()` runs at the very top of every
  `simLoop` iteration, before `step_n` and before
  `writeSnapshotToSAB`.
- **Why**: A slider sent at tick T takes effect for tick T+1
  deterministically. If the drain happened after `step_n`, the slider
  would skip a tick.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/sim-worker.ts → simLoop`,
  `web/src/sim-worker.ts → drainMessages`.

### `set_slider(name, value)` is the sole external mutation entry point

- **Decision**: One wasm-bindgen method dispatches every dev-panel
  slider by string name. There are no per-typed `set_max_age` /
  `set_split_threshold` / etc. exports. Bools (`full_grass_on_init`)
  ride the same path as `0|1` via the `try_set_slider`
  `"full_grass_on_init" => apply_full_grass_on_init(value != 0.0)` arm.
- **Why**: One small protocol surface, no per-slider message type, no
  carve-out for bools. Adding a slider is a 1-line
  `apply_X` + 1-line `try_set_slider` arm + a TS widget — no protocol
  change.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Tradeoffs**: Unknown slider names throw a `JsValue` error — typos
  surface as a `[sim] set_slider("X", v) rejected` warning rather than
  silently no-op'ing. (Considered a feature.)
- **Code anchors**: `src/wasm_api.rs → WorldHandle::set_slider`,
  `src/wasm_api.rs → try_set_slider`.

### `MAX_POP_FOR_SIM` is a hard sim invariant, enforced by random cull

- **Decision**: `World::handle_births` performs a uniform-random cull
  draw from `self.rng` after the birth phase whenever
  `pop > MAX_POP_FOR_SIM`, removing creatures back to the cap. The
  snapshot writer asserts `pop <= MAX_POP_FOR_SIM` in dev builds.
- **Why**: A previous design let the sim run past the cap and the
  snapshot writer silently truncated. The renderer didn't show the
  extras — silent state divergence between sim and view, the worst
  kind of bug. Making the cap a sim invariant means every observer
  (snapshot writer, future save-load, future test) sees the same
  population.
- **Tradeoffs**: One tick of slight EMA-color noise at the cull
  boundary when a newborn gets swapped into a culled existing slot
  (scratch state for that index is stale for one tick; the next NN
  phase rebuilds cleanly). Sub-perceptual.
- **Splitters get prioritized structurally** — they reproduced before
  the random sample, so their genes are over-represented in the
  surviving pool. No special carve-out for newborns.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`,
  `decisions/cross-cutting.md`.
- **Code anchors**: `src/world/mod.rs → World::handle_births`,
  `src/constants.rs → MAX_POP_FOR_SIM`,
  `src/wasm_api.rs → write_creatures_each` (debug_assert),
  `World::scratch_cull_pool`.
- **Revisit when**: a feature wants persistent per-creature identity
  guarantees that random cull would break (e.g., a Hall of Fame), or
  the cap value moves past where allocator pressure becomes a problem.

### Sim worker handles restart by terminate + respawn

- **Decision**: Restart is `worker.terminate()` + `new Worker(...)` on
  main; the worker has no re-boot path on a live instance.
- **Why**: Cheapest possible reset semantics — every piece of
  worker-local state (wasm heap, rayon pool, SABs, message queue) is
  GC'd with the worker. The replacement boots fresh.
- **Tradeoffs**: ~500 ms target / ~600 ms realistic blip (rayon
  re-init dominates). The renderer keeps painting the last-good frame
  during the gap because the previous bridge's SABs stay GC-rooted
  until the new `boot_ready` lands.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/main.ts → restart`,
  `web/src/main.ts → spawnSimWorker`.

### Restart sources sliders from in-memory widget state, not localStorage

- **Decision**: `boot.initial_sliders` is sourced from
  `devpanel.ts::currentSliderState()` (reading the widgets' current
  `.value`), not from `getSettings()` (localStorage).
- **Why**: A mid-drag restart should carry the dragged value, not the
  last-persisted one. The drag write to localStorage only happens on
  release.
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/widgets/devpanel.ts → currentSliderState`,
  `web/src/main.ts → spawnSimWorker`.

### `WorldHandle` exposes no wasm-side `cross_origin_isolated` getter

- **Decision**: Both main and the worker read `crossOriginIsolated`
  from their own JS global. No Rust export.
- **Why**: It's a per-realm JS property; a Rust round-trip adds nothing
  except a wasm-bindgen call.
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/build-and-deploy.md`.
- **Code anchors**: `web/src/main.ts → main` (`globalThis...`),
  `web/src/sim-worker.ts → handleBoot` (`(self as ...).crossOriginIsolated`).

### Deterministic chunking for the parallel NN pass

- **Decision**: Per-tick NN chunk count =
  `clamp(pop / 32, MIN_CHUNKS=4, min(MAX_CHUNKS=16, workers))`.
- **Why**: Locks parallel results to be bit-identical to sequential for
  any given `(pop, workers)` combination — chunk boundaries are a
  function of inputs, not of rayon's runtime scheduling.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/world/nn.rs → chunk_ranges`,
  `src/world/nn.rs → dynamic_chunks`,
  `src/constants.rs → MIN_CHUNKS`, `MAX_CHUNKS`.

### Brain is `32 → 48 → 24 → 5` Leaky ReLU pyramid, no biases

- **Decision**: Three matmul layers with per-layer He init ranges
  (`0.433 / 0.354 / 0.500`); Leaky ReLU (slope 0.01) in the two hidden
  layers; no bias vectors; SIMD via `wide::f32x8`. Total
  `NN_WEIGHT_COUNT == 2808` (compile-asserted).
- **Why**: Sign-preservation (Leaky vs ReLU) gives smoother mutation
  fitness under no-gradient evolution. SIMD requires layer widths to
  be multiples of 8 (the topology choice is partly a consequence). No
  biases keeps the weight count parametric and the founder init
  uniform-random.
- **v2.0 update**: the **input width** (like the hidden-layer list) is now a
  *runtime* field of `NnTopology`, a multiple of 8 in `[8, MAX_NN_INPUTS = 48]`,
  fed to the first matmul as `fan_in`. The compile-time `NN_INPUTS == 32` assert
  is replaced by runtime checks in `NnTopology::with_input_width`. `build_nn_input`
  is now a composable, offset-computing `NnInputLayout` descriptor; the legacy
  layout reproduces width 32 (weight_count `2808`) bit-for-bit. (The
  `NN_HIDDEN_*`/`NN_WEIGHT_COUNT`/`NN_INIT_RANGE_L*` anchors below are stale from
  v1.12 — left for the broader doc-migration pass.)
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/brain.rs → Brain`, `Brain::founder`,
  `Brain::forward`; `src/constants.rs → NN_INPUTS`, `NN_HIDDEN_1`,
  `NN_HIDDEN_2`, `NN_OUTPUTS`, `NN_WEIGHT_COUNT`, `NN_INIT_RANGE_L*`.

### Founder NN is pure uniform-random — no hardwired priors

- **Decision**: `Brain::founder` initializes weights from `rng.uniform`
  per layer with the He ranges above. No "energy → split" wiring, no
  grass-detector hot slot, no Move baseline.
- **Why**: Previous founder-bias hardwiring (in superseded versions)
  was brittle to topology / slot-layout changes. Uniform-random with a
  reasonable multi-founder count (default 8 via Halton placement)
  gives the sim enough lineages that at least one survives
  generation 0.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/brain.rs → Brain::founder`,
  `src/constants.rs → FOUNDER_COUNT_DEFAULT`.
- **Revisit when**: a session with default sliders routinely
  extinct-ends before generation 1; that signals the founder pool
  needs the prior back, or wider He ranges.

### World is walled (not toroidal)

- **Decision**: Movement clamps to `[0, WORLD_SIZE]`. Vision and
  neighbour queries do not wrap.
- **Why**: Walls give creatures a usable feature (the wall_proximity NN
  inputs) and a population shape that doesn't smear evenly across the
  field.
- **Applies to**: `architecture/simulation-core.md`.

### Action enum is collapsed to `{Graze, Eat, Split}`

- **Decision**: Three discrete actions. NN outputs 5 values: `vx`, `vy`,
  and three action logits. No Rest, Scavenge, Signal, Armor, Pigment
  variants.
- **Why**: Smaller search space, simpler invariants. Other action
  variants cost code without delivering observable behavioural payoff
  at the population sizes the sim reaches.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/creature.rs → Action`, `Action::ALL`.

### Creature color is genome-derived; per-action highlight is a ring-flash (v2.0 Wave 2a)

- **Decision**: The action-EMA `color_r/g/b` channels are **removed**.
  Display color is now **derived from the genome** at snapshot write
  (`color_u32`: HSV hue←diet, sat←body_size, value←max_speed). Per-action
  highlight moved to a transient **ring-flash** — a `flash_tag` +
  5-tick countdown packed into the snapshot's `packed_u32`. Priority when
  several fire in one tick: Killed (red) > CreatedChild (blue) > Attacked
  (yellow) > Grazed (green); Born (teal) is set at spawn.
- **Why**: With an evolving body genome, identity-color (lineage/morphology)
  and behaviour-highlight (a brief ring) are *separable* — the steady color
  reads "what is this creature" and the flash reads "what did it just do".
  The old EMA conflated the two and rode 3 f32 lanes.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `src/creature.rs → Genome, FlashTag`,
  `src/world/tick.rs → flash_decay`,
  `src/wasm_api.rs → genome_color_u32, pack_render_u32`.

### Body genome reintroduced — 6 traits, single-pool, bucket-coupled mutation (v2.0 Wave 2a)

- **Decision**: Each creature carries a 6-trait body `Genome` (`body_size,
  max_speed, metabolism, diet, water_affinity, heat_tolerance`, each `f32 ∈
  [0,1]`), rescaled at each use site with ranges **centered so a median (0.5)
  genome ≈ today's constants**. It is a SoA column, **never on the snapshot**
  (the render color is derived from it). Founders seed **uniform-random per
  trait** (single-pool diversity from tick 0). On split the genome mutates off
  the **same per-birth mutation bucket** as the brain, with the trait Gaussian
  step using `bucket.sigma * trait_mutation_sigma_multiplier` (default **0.3**),
  drawn *after* the brain weights on the same RNG (deterministic). Traits clamp
  to `[0,1]`. The biome movement penalty (and the matching biome NN inputs) are
  genome-modulated by the affinity traits — closing the Wave 1b seam.
- **Why**: Reintroduces morphology as a real selection axis (the D3 deletion
  flattened every creature to constants). Coupling the genome step to the brain's
  bucket keeps one mutation knob set, and the ×0.3 multiplier keeps trait drift
  gentler than weight drift so behaviour and body don't co-mutate at the same
  scale. Single-pool (no species/crossover) keeps Wave 2a scoped — W3 adds those.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `src/creature.rs → Genome`,
  `src/brain.rs → child_from_with_sigma`,
  `src/world/mod.rs → handle_births / movement_penalty_for`,
  `src/world/tick.rs → graze / attack / energy_bookkeeping`.

### Attack picks first-valid in a per-attacker-rotated cell walk

- **Decision**: `World::attack` (v2.0 Wave 2a: renamed from `eat`) does not scan
  every in-radius candidate to pick the *closest* in-reach target. Instead it
  takes the first target whose squared distance falls inside the genome-scaled
  reach, with the per-cell iteration starting at `attacker_idx % K_cell` and
  wrapping. Drops the closest + lowest-id tiebreak the previous impl used.
- **Why**: At high pop with `repulsion_max=0` (or any large clump),
  every predator's bbox returned dozens of candidates and the inner
  scan dominated the tick. The cells in a clump are small enough that
  "first valid" and "closest valid" are physically near-identical
  bites. The rotation seed de-correlates "first" from creature-index,
  which otherwise biases toward biting the oldest cell-mate every time
  (SoA-index order ≈ age order).
- **Applies to**: `architecture/simulation-core.md`,
  `src/grid.rs → SpatialGrid::find_first_in_radius`,
  `src/world/tick.rs → World::attack`.
- **Tradeoffs**: Bite-target is no longer the strict argmin-distance
  prey; in a tight stack the picked prey is whichever sits at
  `(predator_idx + k) mod K_cell` first. Determinism is preserved
  (rotation is a pure function of predator index + cell layout).
- **Revisit when**: A future world is much sparser (so the
  closest-prey scan is cheap again) or visualisation reveals an
  unexpected pattern from the rotation bias — at that point the
  cheapest mitigation is a starburst cell walk so "first" approximates
  "closest" without sorting.
- **Code anchors**: `src/grid.rs → SpatialGrid::find_first_in_radius`,
  `src/world/tick.rs → attack`, `AttackPick`.

### Creature proximity scan walks cells in starburst order with sector-saturation early-bail

- **Decision**: `compute_creature_proximity_sectors` walks a precomputed
  starburst list of integer cell offsets sorted by lower-bound distance
  from the home cell. Per sector it tracks `best_intensity` and bails
  as soon as every sector has been lit *and* the current cell's max
  achievable intensity (`1 - min_dist/range`) can no longer beat the
  worst-lit sector.
- **Why**: The naive bbox walk visits every cell in
  `(2·PROXIMITY_RANGE)²`, which at `PROXIMITY_RANGE=20` and
  `HASH_CELL=2.5` is ~256 cells per predator, dominated by candidates
  in dense clumps. The aggregation is `max` per sector, so once each
  sector has any hit, further cells whose closeness ceiling is below
  the worst lit sector cannot improve the result. The starburst order
  guarantees we hit the closest cells first, so the early-bail fires
  quickly in clumps.
- **Applies to**: `architecture/simulation-core.md`,
  `src/world/proximity.rs → proximity_starburst`,
  `src/world/proximity.rs → compute_creature_proximity_sectors`.
- **Tradeoffs**: Output is bit-identical to the prior bbox walk (same
  max-aggregation semantics, same sector LUT, same bilinear split).
  The starburst list is built once via `OnceLock` and reused.
- **Revisit when**: `PROXIMITY_RANGE` or `HASH_CELL` changes by enough
  that the precomputed list needs a different cap, or a future sense
  uses sum-aggregation (where the early-bail invariant doesn't hold).

### No save/load, no species tracking, no events, no Hall of Fame

- **Decision**: Every page load is a fresh world. No persistence, no
  species registry, no event log, no eulogy.
- **Why**: All four cost surface area without delivering observable
  benefits. Removing them lets the rest of the system get smaller.
- **Applies to**: `architecture/simulation-core.md`.
- **Revisit when**: a use case appears that genuinely needs durable
  per-creature identity or cross-session continuity.

### Biome map: a few large blobs from `world_seed` via a dedicated PRNG (v2.0 Wave 1b)

- **Decision**: The static biome grid is generated from a **dedicated
  SplitMix64 PRNG seeded by `world_seed`**, independent of the
  string/XxHash64 sim RNG. The generator scatters a few **large blobs**
  (2..4 water + 2..4 desert, radius `0.13..0.18 × world_size`) over a
  Plains background; water wins overlaps; distance is toroidal when
  `wrap_world`. Tuned to ~plains 60% / water 20% / desert 20%.
- **Why**: A separate stream lets the map pin to `world_seed` alone (so
  it is reproducible / shareable) while the live run still varies on the
  sim RNG. Large blobs (not noise speckle) give creatures a navigable,
  legible landscape and let the directional biome NN inputs carry a real
  gradient. SplitMix64 is trivial, fast, and never touches the sim RNG
  draw order.
- **Applies to**: `architecture/simulation-core.md`,
  `src/world/biome.rs → generate_biome_grid`, `src/world/mod.rs`.
- **Tradeoffs**: Blob count/radius are balance knobs (constants in
  `biome.rs`); proportions are statistical, not exact per seed. The grid
  is `grass_cell_count` u8 — same size as the snapshot grass region.
- **Revisit when**: biomes need finer structure (rivers, gradients) or a
  third+ biome kind, or the target proportions change.

### Biome movement penalty: one base severity → three effects, survivable short-term (v2.0 Wave 1b)

- **Decision**: Each non-Plains biome carries a single live-tunable base
  severity `p ∈ [0, 1]` (`water_movement_penalty` 0.8 /
  `desert_movement_penalty` 0.4; Plains 0). While a creature is on that
  cell, `p` drives three effects — reduced speed cap (`× (1 −
  0.6·p)`), higher move cost (`× (1 + 1.0·p)`), and extra upkeep (`+
  0.5·p`) — each with a built-in coefficient constant (`K_BIOME_*`).
  Genome modulation (`water_affinity` / `heat_tolerance`) is deferred to
  Wave 2; in Wave 1b the penalty is genome-independent.
- **Why**: One knob per biome keeps the live tuning surface tiny while
  the three effects make a biome both *slower to cross* and *costly to
  linger in*. The coefficients are tuned **survivable short-term**: a
  full-energy creature can dash across water/desert (~130 / ~195 ticks of
  budget) but cannot homestead there. The directional biome NN inputs let
  brains learn to route around penalized cells.
- **Applies to**: `architecture/simulation-core.md`,
  `src/world/tick.rs → apply_movement_and_repulsion / energy_bookkeeping`,
  `src/constants.rs` (the `K_BIOME_*` balance knobs).
- **Tradeoffs**: `K_BIOME_*` are global balance knobs, not per-biome; the
  per-biome severity is the live knob. The penalty samples the cell under
  the body (and one cell N/S/E/W for the NN), not a sub-cell gradient.
- **Revisit when**: Wave 2 adds genome modulation, or the
  survivability/cross-time balance needs retuning.

### Biome NN inputs: always-on `BiomeDir` (4) + `CurrCellPenalty` (1) (v2.0 Wave 1b)

- **Decision**: Two input groups are added to the composable layout and
  are **always-on in both wrap modes**: `BiomeDir` (4 = the penalty one
  cell N/S/E/W, base severity, wrap-aware) and `CurrCellPenalty` (1 = the
  penalty under the body). New active widths: **wrap on = 32, wrap off =
  40** (was 32 in Wave 1a — old brains discarded).
- **Why**: A creature can't evolve to avoid water/desert without sensing
  it. Directional inputs give a local gradient (route around); the
  current-cell input gives an "I'm being penalized now" signal. Always-on
  keeps the two wrap modes' sensoria comparable.
- **Applies to**: `architecture/simulation-core.md`,
  `src/world/nn.rs → NnInputGroup / NnInputLayout::for_settings /
  build_nn_input`, `src/brain.rs` (drift guard now covers widths 32 + 40).
- **Tradeoffs**: Wrap-off NN width grows 32→40, so the wrap-off first
  matmul is wider; the SIMD invariants (multiple-of-8) still hold (40 is
  a multiple of 8). No genome inputs yet (Wave 2).
- **Revisit when**: more biome senses are added, or the input width
  approaches `MAX_NN_INPUTS = 48`.

## See also

- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`cross-cutting.md`](cross-cutting.md)
