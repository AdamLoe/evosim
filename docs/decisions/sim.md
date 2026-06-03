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

### World wrap is a construction toggle (v2.0; default toroidal)

- **Decision**: `wrap_world` is a runtime construction setting (default **on**
  = toroidal). ON ⇒ positions/vision/neighbour queries wrap across the seam
  (`rem_euclid`, toroidal minimum-image), and the 4 wall-proximity NN inputs
  are dropped. OFF ⇒ the legacy walled world: movement clamps to
  `[ri, world_size-ri]`, queries don't wrap, and the 4 wall inputs reappear.
  (Pre-v2.0 the world was walled-only.)
- **Why**: Both are interesting evolutionary regimes worth A/B-ing. A torus
  removes the wall-camping pathology and smears population evenly; walls give
  creatures a usable "shore" feature (the wall_proximity inputs) and a
  population shape that hugs edges. Making it a toggle lets the user pick per
  world instead of baking one in.
- **Applies to**: `architecture/simulation-core.md`,
  `decisions/cross-cutting.md` (runtime-`world_size` model).
- **Code anchors**: `DevSliders.wrap_world`, `WorldDims.wrap_world`,
  `SpatialGrid::cell_of` / `for_each_in_radius` (wrap fold).

### Action enum is collapsed to `{Graze, Attack, Split}`

- **Decision**: Three discrete actions. NN outputs 5 values: `vx`, `vy`,
  and three action logits. No Rest, Scavenge, Signal, Armor, Pigment
  variants. v2.0 Wave 2a renamed `Eat → Attack` (same repr/logit order).
  v2.0 Wave 3a: `action[2]` (the `Split` variant) decodes to **Mate** in
  `species_mode` — same logit slot, mode-dependent validity gate
  (`ActionGate`), so the NN output shape is stable across modes.
- **Why**: Smaller search space, simpler invariants. Other action
  variants cost code without delivering observable behavioural payoff
  at the population sizes the sim reaches. Reusing `action[2]`'s logit for
  Mate keeps the SAB + brain output shape unchanged between modes.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/creature.rs → Action`, `Action::ALL`;
  `src/world/nn.rs → ActionGate / decode_action / is_valid_action`.

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
  `(2·PROXIMITY_RANGE)²`. The aggregation is `max` per sector, so once each
  sector has any hit, further cells whose closeness ceiling is below
  the worst lit sector cannot improve the result. The starburst order
  guarantees we hit the closest cells first, so the early-bail fires
  quickly in clumps. (v2.0 Wave 1a retuned `HASH_CELL` 2.5u → **10u** and the
  world to 64× area at the same pop, so creature density is ~64× sparser and a
  full-range walk is cheap either way; the early-bail still pays off in
  clumps. v2.0 Wave 3a: in species mode the scan lights **16** sectors — 8
  same-species + 8 other-species — so a mono-species clump never lights the
  other block; judged acceptable at the sparse coarse-cell density, with the
  fallback being to bail the two blocks independently.)
- **Applies to**: `architecture/simulation-core.md`,
  `src/world/proximity.rs → proximity_starburst`,
  `src/world/proximity.rs → compute_creature_proximity_sectors`,
  `compute_creature_proximity_sectors_species` (the 16-sector species variant).
- **Tradeoffs**: Output is bit-identical to the prior bbox walk (same
  max-aggregation semantics, same sector LUT, same bilinear split) per mode.
  The starburst list is built once via `OnceLock` and reused; in wrap mode
  the displacement is the toroidal minimum-image.
- **Revisit when**: `PROXIMITY_RANGE` or `HASH_CELL` changes by enough
  that the precomputed list needs a different cap, or a future sense
  uses sum-aggregation (where the early-bail invariant doesn't hold).

### No save/load, no events, no Hall of Fame

- **Decision**: Every page load is a fresh world. No persistence, no
  event log, no eulogy.
- **Why**: All three cost surface area without delivering observable
  benefits. Removing them lets the rest of the system get smaller.
- **Applies to**: `architecture/simulation-core.md`.
- **Note (v2.0 Wave 3a)**: a **species** registry now exists in
  `species_mode` (`World.species`) — but it's still in-memory, fresh per
  world; there's no cross-session persistence. The "no species tracking"
  clause referred to per-session durable lineage, which still doesn't exist
  (species-history breadcrumb is plumbed-but-empty until V2.1 splits).
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

### Species + sexual mating is an opt-in mode, not a replacement (v2.0 Wave 3a)

- **Decision**: `species_mode` is a construction-only `DevSliders` flag
  (default OFF, restart-required). OFF = today's single-pool asexual
  `Split` sim, fully unchanged. ON = N seeded species + sexual `Mate` +
  Attack-refuses-same-species + 16-sector creature proximity + per-species
  color. The two never coexist in a run.
- **Why**: Emergent single-pool drift is the v1 thing worth preserving;
  faction storytelling is the v2 thing worth adding. A single hard switch
  keeps both readable and lets the user A/B them.
- **Applies to**: `architecture/simulation-core.md`,
  `src/world/{mod,nn,tick,species}.rs`, `src/wasm_api.rs`.
- **Code anchors**: `DevSliders.species_mode`, `World.species`
  (`SpeciesRegistry`), the `species_id` SoA column.

### Mate: contact-radius, NN-initiated, first-found, initiator-only cost + cooldown (v2.0 Wave 3a)

- **Decision**: In species mode, `action[2]` (the `Action::Split` logit)
  decodes to **Mate**. It's gated ENTIRELY by the initiator — a legal pick
  only when off mating cooldown AND `energy ≥ mating cost` (= `split_gift`);
  an invalid pick falls through to Graze, like Split. On fire, the sim finds
  the **first** same-species creature (by `species_id` only) within a small
  contact radius `(r_i + r_j) × MATING_CONTACT_RADIUS_FACTOR` (≈ summed body
  radii, NOT the 20u perception range), first-found wins (no nearest-scan,
  matching Attack). **All cost + cooldown fall on the initiator only:** it
  pays the cost and enters a fixed `mating_cooldown_ticks` (default 200, live)
  cooldown via a dedicated `mating_cooldown` SoA column; the partner pays
  nothing, needs no energy, and is not required to be off cooldown. The child
  spawns at the parent midpoint (wrap-aware, no habitable-cell check) with the
  parents' shared `species_id`.
- **Why**: Contact (not perception range) makes mating a deliberate physical
  act that ties reproduction to staying clustered ("stay together or die out"
  is the intended selection pressure). Initiator-only cost/cooldown is a
  deliberate cold-start concession — requiring both parents ready-and-fed
  roughly squares the gating probability, which a clustered founder group with
  random brains rarely clears.
- **Applies to**: `World::handle_mating`, `ActionGate` /
  `decode_action` / `is_valid_action` in `src/world/nn.rs`,
  `energy_bookkeeping` (cooldown decrement).
- **Watch item**: with no partner cooldown, many initiators can mate one
  popular partner in a single tick — add a guard if births spike.

### Crossover is a construction enum applied to brain weights AND genome traits (v2.0 Wave 3a)

- **Decision**: `crossover_mode` (construction enum `{average, fifty_fifty}`,
  default `fifty_fifty`) is applied **identically** to brain weights and the 6
  genome traits, then mutation (the same per-birth bucket machinery as a normal
  child). `fifty_fifty` = per-slot 50/50 random pick from the two parents;
  `average` = per-slot elementwise midpoint. Single-pool `Split` is unchanged
  (asexual, no crossover). RNG draw order is fixed and deterministic: brain
  crossover (one `unit()`/weight for fifty_fifty, none for average) → brain
  bucket mutation → genome crossover → genome mutation, off the pair's pre-rolled
  seed.
- **Why**: Both are interesting evolutionary mechanics; one construction switch
  is cheap and lets the user observe both regimes. `fifty_fifty` preserves
  variance (standard GA); `average` is available for homogenization research.
  Per-weight crossover of trained nets is a known neuroevolution hazard, kept
  safe by the **aligned regime** (same-species-only mating + small per-birth
  brain mutation + frequent within-species mating) so crossover is allele-shuffling,
  not basin-mixing — crossover's job here is narrative, not raw optimization.
- **Applies to**: `Brain::child_from_crossover_with_sigma` (`src/brain.rs`),
  `Genome::crossed` (`src/creature.rs`), `World::handle_mating`.
- **Revisit when**: playtest shows one mode strictly dominant, or brains
  demonstrably fail to evolve under mating.

### Cannibalism stays off in species mode — `Attack` refuses same-species (v2.0 Wave 3a)

- **Decision**: In species mode, `World::attack` refuses same-`species_id`
  targets (a sim-side gate, not learned). Single-pool has no species, so
  `Attack` hits any creature — the v1 predation mechanic, unchanged. "No
  cannibalism" is a species-mode rule, not a global one.
- **Why**: Short networks drift into "eat anything moving" within a few
  generations; without a hard gate every species converges to cannibalism. Our
  brains aren't deep enough to encode evolved anti-cannibalism, so we guardrail it.
- **Applies to**: `World::attack` (`src/world/tick.rs`).
- **Revisit when**: brains grow deep enough that anti-cannibalism could plausibly
  evolve and be retained against drift.

### Species seeding + identity: dynamic registry, hand-spread palette, biome-adapted N-anchor founders (v2.0 Wave 4)

- **Decision**: `World.species` is a **dynamic** `SpeciesRegistry`
  (`Vec<Species { id, color_u32, name }>`, add/remove/recolor-capable) even
  though v2.0 seeds a fixed `starting_species_count` (default 10). Real
  **N-anchor biome-adapted seeding** replaces the Wave-3 placeholder:
  1. Pick anchors spread across the world (rejection-sampled min spacing).
  2. Per anchor, roll a canonical "first member" = random founder brain + a
     genome **biased to survive the biome under the anchor**
     (`Genome::canonical_for_biome`): Water → high `water_affinity`, Desert →
     high `heat_tolerance`, Plains → moderate both; the other four traits stay
     random per species.
  3. Spawn `starting_species_member_count - 1` more founders clustered near the
     anchor, each = the canonical brain+genome through **one per-birth bucket
     draw** with that bucket's **rate AND sigma both ×
     `starting_species_member_variance`** (default 3.0, now active;
     `Brain::founder_spread_with_sigma`).
  All of seeding is **deterministic from `world_seed`** via a *dedicated* setup
  PRNG (`SimRng::from_u64(world_seed ^ SEEDING_PRNG_SALT)`) — separate from the
  biome generator and from the sim RNG, so the same `world_seed` reproduces the
  same tick-0 layout while the run still varies per launch. Species colors come
  from a **hand-spread 10-hue saturated palette** (`SPECIES_PALETTE`),
  single-sourced for both the per-creature `color_u32` lane and the polled
  species→color table. Labels are `Species-A..J`. Species extinct = extinct.
- **Why**: Dynamic registry means V2.1 splits/merges need no protocol change.
  Biome-adapting the founder genome (a deliberate reversal of "random founders,
  let the world filter them") keeps a clump alive through the opening moves — at
  gen 0 there's no learned behavior to keep a maladapted clump alive long enough
  to evolve. Biasing only the genome, not the anchor *position*, keeps the map
  honest. The dedicated seeding PRNG satisfies the determinism guarantee without
  coupling the whole run to `world_seed`. A hand-spread palette (vs an even-hue
  ring) keeps adjacent species readable.
- **Applies to**: `src/world/species.rs` (palette + registry),
  `Genome::canonical_for_biome` (`src/creature.rs`),
  `Brain::founder_spread_with_sigma` (`src/brain.rs`), `pick_anchors` +
  `World::new_with_sliders_topology` seeding branch (`src/world/mod.rs`),
  `fill_creature_bytes` (species color lane).
- **Revisit when**: v2.1 adds splits/merges (split-aware "one color → two
  similar-but-distinct" generator); founders survive comfortably and the gen-0
  pre-adapt masks rather than enables interesting selection.

### Polled species→color/name/count table (v2.0 Wave 4 producer)

- **Decision**: A cadence-written SAB report
  (`WorldHandle::species_table_json` → `CTRL_SPECIES_TABLE_EPOCH/LEN` + the
  `SPECIES_TABLE` byte window, written every ~45 ticks in species mode) exposes
  every **live** species as `{ id, color_u32, name, count }` + the world tick.
  Per-species counts are aggregated **sim-side**; the table is **dynamic**
  (extinct species drop out). Mirrors the `nn_worker_stats_json`
  cadence-producer pattern (no request side). This is the **producer**; the
  Monitor pop-graph + canvas color-table **consumers are Wave 5**.
- **Why**: Sending `species_id` per creature + a shared polled table (rather
  than per-creature color bytes) keeps the snapshot lean and single-sources the
  palette so canvas and chart can't drift. Building it dynamic now saves a
  protocol break when v2.1 splits arrive.
- **Applies to**: `src/wasm_api.rs` (`species_table_json`), `src/control_sab.rs`
  (slots + byte window), `web/src/sim-worker.ts` (`maybeWriteSpeciesTable`),
  `web/src/generated/control-sab.ts` (regenerated).

### Population balance: ship sane defaults, the cap never binds via random cull (v2.0 Wave 4)

- **Decision**: Ship the plan's reproduction-economy defaults unchanged —
  mating energy cost = `split_gift` (30), initiator `mating_cooldown_ticks`
  (200), birth gift = the mating cost (30), 10 species × 10 founders ×
  variance 3.0. An in-process long run confirms the soft pop cap (8k) **never
  binds via random cull**: at the default 9600u world the population sits well
  under 200 at all times (boot 100 → low-tens steady state), and at a denser
  2400u world it holds ~100–122. Multiple species persist for thousands of
  ticks (no extinct-end). Cold-start did **not** fire, so no cold-start lever
  was applied (per the plan, change defaults only *if* a default run extinct-ends
  before learning).
- **Why**: Per the contract, balance is the *user's* — the implementation job is
  "runs without the cap binding via random cull + the mechanism demonstrably
  works," not hand-tuned fun. At 64× area with the same pop ceiling the cap is
  nowhere near binding; the observed population *decline* at the default world is
  an encounter-rate (geographic-sparsity) characteristic the user tunes via the
  live/construction knobs (`world_size`, biome severities, grass richness,
  `mating_cooldown_ticks`), not a mechanism bug.
- **Applies to**: `MATING_COOLDOWN_TICKS_DEFAULT`, `SPLIT_GIFT_MAX_DEFAULT`
  (= mating cost + gift), the `starting_species_*` defaults (`src/constants.rs`).
- **Revisit when**: playtest wants growth at the default world — first lever is
  cranking `starting_species_member_count` (denser clumps); next is the 8-dir
  mate-proximity NN inputs (`v2-possible-next-steps.md`).

### Why one binary mode toggle, and why species ⇒ mating (v2.0)

- **Decision**: `species_mode` is a *single* construction switch with **no
  finer parameterization** — there is no "sexual without species" or "species
  without asexual-mating" combination. (What each mode *contains* is covered by
  the Wave-3a "opt-in mode" entry above; this entry records only why the axis
  is a single binary.)
- **Why**: Most v2.0 ideas reduce to "another setting," but the emergent
  single-pool drift (v1) and the faction storytelling (v2) are each worth
  preserving whole. One hard switch keeps both readable and A/B-able; the
  intermediate combinations were rejected as too many to balance. Species
  *without* mating in particular has no mechanism for species to evolve
  independently — asexual lineages would each just be their own drift line,
  which the v1 sim already does without the species label.
- **Applies to**: `architecture/simulation-core.md`, boot payload, NN input
  layout composition.
- **Alternatives**: parameterize finer (asexual+species, sexual+no-species, …)
  — rejected as too many combinations to balance.

### Child spawns at parent midpoint, no habitable-cell check (v2.0)

- **Decision**: A mated child spawns at the wrap-aware midpoint of the two
  parents' positions, even if that cell is hostile (deep water / desert). No
  retry, no nearest-habitable search.
- **Why**: Simple, and honest selection — most cross-biome mating attempts
  produce children that die quickly, which is the point, not a bug. A
  habitable-cell search would add a scan and quietly mask cross-biome mating
  as a failure mode.
- **Applies to**: `World::handle_mating` (`src/world/mod.rs`).

### Species extinct = extinct; `Species-A..J` names; species-only history (v2.0)

- **Decision**: When a species hits population 0 it stays gone — no respawn
  pod, no founder injection. Names are the fixed `Species-A..J` (random-name
  generator deferred). The inspector shows species id / color / **species**
  history (parent-species breadcrumb), never a per-creature ancestor chain.
- **Why**: New species arise from splits in v2.1+, so in v2.0 "how do new
  species appear" has the simplest answer: they don't. Species color does the
  visual-identity work, so names aren't load-bearing yet. A 10-generation
  individual-ancestor chain is unreadable; species history is the storytelling
  primitive that matters once splits ship (plumbed-but-empty in v2.0).
- **Applies to**: `World.species` (`src/world/species.rs`), the inspector
  species block (`#ins-species-*`), `creature_inspect_json`.
- **Revisit when**: v2.1 splits/merges land (then extinction, naming, and the
  history breadcrumb all gain real behavior).

### Action set is `{Graze, Attack, Split | Mate}`; `Eat`→`Attack`; velocity stays continuous (v2.0)

- **Decision**: Three actions, NN output shape unchanged at 5 (`vx, vy` + 3
  logits). `action[2]` is `Split` (single-pool) or `Mate` (species), chosen at
  construction. `Eat` is renamed `Attack` in both modes (same repr/logit
  order). A discrete N/S/E/W movement-output option was floated and **cut** —
  velocity stays continuous `(vx, vy)`.
- **Why**: Reusing `action[2]`'s logit for Mate keeps the SAB + brain output
  shape stable across modes (mode-dependent validity gate, not a new slot).
  "Attack" reads correctly once the species gate exists. Discrete movement
  didn't justify a new setting + decode path + balance surface for the thin
  "they all migrate east" readability win.
- **Applies to**: `src/creature.rs → Action`, `src/world/nn.rs → decode_action`.

### NN input layout is settings-derived; no vestigial slots; one bias slot (v2.0)

- **Decision**: The input vector composition is computed at world init from the
  active construction settings (`NnInputLayout::for_settings`) and SIMD-padded
  to a multiple of 8. No slot is preserved for compatibility — inputs constant
  for a brain's whole run (the dropped "action-set / mode indicator" slot, the
  always-zero wall inputs in wrap mode) collapse to a per-output bias and are
  removed. One slot stays pinned to `1.0` as the classical bias-learning
  constant (distinct from the dropped mode flag). Old brains are discarded on
  any settings change (no save/load to migrate).
- **Why**: A constant input wastes weights and trains against a dishonest
  surface. Mode information that matters (8 vs 16 creature sectors, present vs
  absent wall inputs) is carried *implicitly* by the layout, so a separate
  "are we in species mode" input buys nothing. The bias-learning constant earns
  its slot (every hidden unit gets a learnable threshold) and is standard
  practice. (The full per-mode width table — 32/40/40/48 across wrap×species —
  lives in `architecture/simulation-core.md`; the cross-language safety
  implication is in `decisions/cross-cutting.md`.)
- **Applies to**: `architecture/simulation-core.md`,
  `src/world/nn.rs → NnInputLayout / NnInputGroup`, `src/brain.rs → NnTopology`.
- **Alternatives**: keep a constant action-set-indicator input for
  "portability" (rejected — a brain selected against one `action[2]` semantic
  doesn't transfer just because one constant signal says which mode it's in);
  8-direction biome sectors instead of 4 cardinal (rejected — doubles inputs
  for marginal signal).

### Body genome: 6 floats, not buckets; dropped trait set; sprite/repulsion not collision (v2.0)

- **Decision**: The genome is exactly six **continuous** `f32 ∈ [0,1]` traits
  (`body_size, max_speed, metabolism, diet, water_affinity, heat_tolerance`),
  not quantized `u8` buckets. The larger ChatGPT-era trait set (social, fear,
  aggression, cold, humidity, fertility, photosynthesis) is **not** in v2.0.
  `body_size` scales sprite + repulsion strength, **not** collision (there is
  no collision).
- **Why**: These six are the minimum that distinguish the Grazer / Hunter /
  Swimmer / Desert archetypes against the 3-biome world, and each does real
  work in at least one mechanic. The dropped traits either have no biome to
  express them, duplicate `diet`+`metabolism`, or open a new energy channel
  (photosynthesis) that destabilizes balance. Floats are honest about drift —
  buckets read cleaner but hide the small differences accumulating over
  generations, the very thing the sim exists to expose. Collision means physics
  + pathfinding (out of scope); repulsion approximates "big creatures own more
  space."
- **Applies to**: `src/creature.rs → Genome`, `src/world/tick.rs` (use sites).
- **Revisit when**: new biomes ship and need new trait axes (forest →
  social/fear, tundra → cold-tolerance).

### Grass density field is `Vec<AtomicU8>` on the snapshot quantization scale (v2.0.2)

- **Decision**: The in-memory grass density field is `Vec<AtomicU8>` (1
  byte/cell). The encoding scale is the same one the snapshot has always
  used: `round(clamp(d / GRASS_MAX, 0, 1) × 255)`, so `GRASS_MAX → 255`
  and `0 → 0`. The decode inverse is `u8 / 255 × GRASS_MAX`. Because the
  live field now holds those same bytes, the snapshot copy-out is a
  trivial byte copy and the rendered picture is unchanged. All reads
  `load(Relaxed) → decode → f32`; all writes `encode → store(Relaxed)`.
- **Why**: Storing the snapshot scale in-memory eliminates the quantize
  step from the hot write path (byte copy replaces per-cell multiply +
  round). It also enables the stochastic scatter kernel (below) to
  operate entirely in u8, avoiding f32 double-buffering entirely. 1 byte
  vs 4 bytes per cell also reduces working-set pressure at 1920²
  (3.7 MB vs 14.7 MB).
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `src/grass.rs → GrassGrid::density` (field),
  `encode_density`, `decode_density`, `GrassGrid::dget`, `GrassGrid::dset`,
  `GrassGrid::dget_u8`, `GrassGrid::density_u8_snapshot`.

### Stochastic u8 scatter kernel is the live propagation path; blur retained behind a selector (v2.0.2)

- **Decision**: `compute_propagation_scatter` is the **live propagation
  path** — `World` boot sets `GrassPropagation::Scatter` on the grid.
  The old separable-Gaussian `compute_propagation_blur` is **retained**
  behind the `GrassPropagation::{Scatter,Blur}` selector; grid-direct
  test constructors default to `Blur` so existing blur tests keep passing
  without rewrite. Deletion of blur is a future lead call.
- **Scatter mechanics**: per active tile, rayon-parallel over the active
  tile set — (1) freeze the tile's source bytes into a transient local
  buffer (no intra-tile cascade); (2) for each source cell that has grass,
  roll using a per-cell **SplitMix64 hash RNG** keyed on
  `(world_seed, cell, tick, salt)` (grass never touches `SimRng`):
  **decay roll** — prob `decay_pct`, lossy relaxed RMW subtract
  `decay_amount` (floor ≥ 1 byte when non-zero), saturating at 0;
  **spread roll** — prob `spread_pct`, pick one offset from the
  **precomputed round disc table** (radial band by ring1/2/3 weights,
  uniform angle → round footprint), lossy relaxed RMW add `spread_amount`
  (floor ≥ 1 byte), clamp to the target cell's biome-cap byte. No
  spontaneous spawn — a zero-byte cell is skipped as a source. (3)
  Frontier: tiles written to are marked active+dirty next tick via atomic
  `fetch_or`; cross-tile spread lands in the immediate 8-fringe neighbour
  (spread radius ≤ `GRASS_TILE_SIZE = 32`). Measured ~34× faster than
  blur, linear O(N) over active tiles, does not balloon past the 2048
  budget.
- **Why**: The blur path is O(frontier × cells/tile × blur-taps) and
  must run the full logistic-growth + propagation-rate math per cell, with
  an f32 double-buffer. Scatter replaces all that with 4 hash queries + 2
  atomic RMW ops per cell, in place, no scratch buffer. The visual result
  is organic-looking patches with a tunable radial spread profile; the
  disc table (not Chebyshev rings) keeps the footprint round at all zoom
  levels. The tile_class equilibrium heuristic (EMPTY/SATURATED/MIXED) is
  NOT maintained on the scatter path — the frontier is driven by
  grass-presence only, which is sufficient.
- **Applies to**: `architecture/simulation-core.md`.
- **Tradeoffs**: The blur is NOT deleted (perf hit ~34× but not the
  50–100× stretch target; deletion deferred). The blur's tile_class
  equilibrium logic is inert on the scatter path.
- **Code anchors**: `src/grass.rs → compute_propagation_scatter`,
  `compute_propagation_blur`, `GrassPropagation`, `DiscTable`,
  `ScatterParams`, `GrassGrid::scatter_params`;
  `src/constants.rs → GRASS_DECAY_PCT_DEFAULT`, `GRASS_DECAY_AMOUNT_DEFAULT`,
  `GRASS_SPREAD_PCT_DEFAULT`, `GRASS_SPREAD_AMOUNT_DEFAULT`,
  `GRASS_SPREAD_RING1_PCT_DEFAULT`, `GRASS_SPREAD_RING2_PCT_DEFAULT`,
  `GRASS_SPREAD_RING3_PCT_DEFAULT`, `GRASS_SPREAD_RADIUS`.
- **Revisit when**: the blur deletion decision is made by the lead (the
  bench is the trigger).

### Threaded scatter is intentionally non-reproducible; single-threaded runs are deterministic — PENDING LEAD RATIFICATION (v2.0.2)

- **Decision**: Single-threaded runs are deterministic (same seed →
  same output). Under `--features threads` and the live wasm sim (always
  threaded), the scatter lossy cross-tile relaxed RMW makes grass density
  — and hence creature behavior that senses it — **intentionally
  non-reproducible** run-to-run. This is the accepted perf-vs-reproducibility
  trade-off. The 2 determinism tests assert exact equality only under
  `#[cfg(not(feature="threads"))]`; threaded tests assert liveness +
  bounded population delta only.
- **Why**: A fully-reproducible scatter under threads would require either
  tile-local-only spread (no cross-tile writes), per-tile inbox merge
  (queue incoming cross-tile adds, apply after all tiles finish), or
  CAS-add (retry loop on every RMW). All three alternatives impose
  significant complexity or cost: tile-local spread narrows the effective
  disc; inbox merge adds per-tile allocation; CAS-add contends heavily at
  dense frontiers. The lossy relaxed RMW is a wholesale collision on rare
  concurrent cross-tile writes — the per-run grass trajectory differs but
  the statistical properties (patch density, frontier speed) are stable.
  This trade-off is accepted at the implementation level but **requires
  lead ratification** for whether determinism is a durable contract.
- **Alternatives considered**: tile-local spread only (rejected — shrinks
  effective disc radius, breaks long-range seeding); per-tile inbox merge
  (viable but complex; deferred as the preferred fix-path if ratification
  requires reproducibility); CAS-add loop (rejected — contention too high
  at dense frontiers).
- **Applies to**: `architecture/simulation-core.md`.
- **Revisit when**: the lead ratifies or rejects the reproducibility
  trade-off; if rejected, implement per-tile inbox merge.

### GRASS_EQ_EPS = 1/255 (one u8 quantum) (v2.0.3)

- **Decision**: `GRASS_EQ_EPS` (the epsilon below which a cell is
  classified EMPTY and above `cap − EPS` is classified SATURATED) is
  `1.0 / 255.0` — exactly one u8 quantization step.
- **Why**: The original value of `1e-4` was 39× smaller than one u8
  quantum (1/255 ≈ 3.92e-3). Water-cap cells (cap=0.04, byte 10/255 ≈
  0.0392) never classified SATURATED because `0.0392 < 0.04 − 1e-4 =
  0.0399`. All Water tiles remained permanently MIXED, preventing the
  frontier skip. Raising to exactly one u8 step means any cell whose f32
  value round-trips to within one quantum of `cap − EPS` is correctly
  classified saturated, restoring the frontier skip for biome-capped cells.
- **Applies to**: `architecture/simulation-core.md`,
  `src/grass.rs → GRASS_EQ_EPS`.

### 6 live grass sliders (SLIDER_NAMES 54–59); GRASS_BITES_PER_BLOCK = 2 constant (v2.0.2)

- **Decision**: Six live sliders wire into `GrassGrid.scatter_params` via
  `WorldHandle` apply_ setters: `grass_decay_pct` (54),
  `grass_decay_amount` (55), `grass_spread_pct` (56),
  `grass_spread_amount` (57), `grass_spread_ring1_pct` (58),
  `grass_spread_ring2_pct` (59). Ring 3 is implicit (disc table
  normalizes all three; ring3 = max(0, 1 − ring1 − ring2)).
  `GRASS_BITES_PER_BLOCK = 2` is a **compile-time constant** (not a live
  slider) so that `density_chunk = GRASS_MAX / 2 = 0.5` encodes to
  exactly 128 u8 bytes — 128 u8 levels per bite, satisfying the
  `≥ 8 levels/bite` u8-floor guard. The `.max(1)` floor on `consume`
  means a grass-present cell never yields 0 energy.
- **Why**: Making bites-per-block constant avoids tracking the u8
  representation of an arbitrary slider value (arbitrary chunk sizes can
  produce u8 encodings near 0, making the `.max(1)` floor non-trivial to
  reason about). The six scatter sliders let the operator tune decay vs
  spread dynamics and the radial distribution live without a restart.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/constants.rs → GRASS_BITES_PER_BLOCK`,
  `GRASS_BITES_PER_BLOCK_DEFAULT`;
  `src/wasm_api.rs → SLIDER_NAMES` (indices 54–59), `try_set_slider`;
  `src/grass.rs → GrassGrid::consume`.

### Grass pyramid: u8 box-filter mip clipmap over the density field (v2.0.3)

- **Decision**: `GrassPyramid` is a u8 box-filter mip pyramid over
  `GrassGrid`. L0 aliases the live `density` field (not stored in the
  pyramid). L1+ are owned, MEAN-downsampled, edge-clamped u8 buffers.
  Each Lk cell is the arithmetic mean of its (up to) 4 in-bounds Lk-1
  parent cells: `(sum + count/2) / count` (round-before-truncate). At the
  default `grass_dim = 1920` this yields ~11 levels before reaching 1×1
  (well under the `GRASS_PYRAMID_MAX_LEVELS = 16` cap). Refreshed each
  tick in `World::step` at step 7b — after grass advances, before energy
  — as a full recompute. Serves render LOD + snapshot windowing; a
  dirty-subtree partial refresh keyed off `tile_active` is a tracked TODO.
- **Why**: The snapshot's write path publishes a **clipmap window** of
  the pyramid (not the whole field) — the LOD level and window are chosen
  to fit a 2048² render budget at any zoom. At default scale (zoom=1,
  grass_dim=1920 < 2048) the window is byte-identical to the pre-effort
  full-field copy. At zoomed-out views, higher LOD levels are sampled and
  the window covers more world space, keeping the snapshot size fixed.
  The pyramid's L0 alias avoids a copy of the full density field on every
  tick; only L1+ (O(N/3) total) need recomputing.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Tradeoffs**: Refresh is a full recompute each tick (O(N×1/3) over all
  owned levels). Active-set-keyed partial refresh is deferred; see the
  `TODO(stage3-pyramid-perf)` anchor in `src/grass.rs`.
  Toroidal wrap-seam: a window straddling the world wrap on a toroidal
  world (grass_dim > 2048, non-default zoom) shows the wrong region —
  documented TODO in `GrassPyramid::viewport_window`, not yet fixed.
- **Code anchors**: `src/grass.rs → GrassPyramid`, `GrassGrid::pyramid`,
  `GrassGrid::refresh_pyramid`;
  `src/constants.rs → GRASS_PYRAMID_MAX_LEVELS`.

### Far multi-band grass NN sight: GrassBandsFar default ON; single-band toggle kept (v2.0.4 S6)

- **Decision**: Grass NN sensing now includes a second group —
  `NnInputGroup::GrassBandsFar` (8 sectors at `GRASS_FAR_SIGHT_RADIUS = 160u`,
  read at `GRASS_FAR_MIP_LEVEL = 3` from `GrassPyramid::sample_clamped`).
  This is **default ON** via the `grass_multisight` construction slider (idx 61,
  default true). Both the multi-band and single-band layouts stay compiled;
  the dev-panel toggle lets the user A/B them. The A/B is a regression guard,
  not a ship gate: multi-band was confirmed not-worse than single-band.
- **Why**: O(1) pyramid taps cost nothing at mip level 3 (one sample per sector).
  Creatures sensing grass at 160u radius can navigate toward distant patches
  before local exhaustion. Shipping default ON keeps all born creatures on the
  same layout as the rest of the world; the toggle is for the A/B and debugging.
- **Constraint**: the walled+species combination hits 51 real → 56 padded inputs,
  exceeding `MAX_NN_INPUTS = 48`. The layout **automatically falls back** to
  single-band for that configuration; documented in `NnInputLayout::for_settings`.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/world/nn.rs → NnInputGroup::GrassBandsFar`,
  `NnInputLayout::for_settings`; `src/world/proximity.rs →
  compute_grass_far_band_sectors`; `src/constants.rs → GRASS_FAR_SIGHT_RADIUS`,
  `GRASS_FAR_MIP_LEVEL`.
- **Revisit when**: band count, radii, or mip level need retuning; or the
  MAX_NN_INPUTS ceiling becomes a bottleneck for future input groups.

### Fused RNG for scatter kernel (grass_hash_fused_4) (v2.0.4 S3)

- **Decision**: The scatter kernel replaces 4 separate `grass_hash_u64` calls
  (salts 1–4) with `grass_hash_fused_4` (`src/rng.rs`): 2 `grass_hash_u64`
  words, bit-sliced into non-overlapping windows for decay/spread/band/pick.
  The property "grass never perturbs `SimRng`" is preserved; distribution
  statistics match within tolerance (tested in `src/grass_fused_rng_tests.rs`).
- **Why**: Attribution bench (S0) measured RNG as the dominant per-cell cost at
  dense fill (3.05 ns/cell, 47% of 6.42 ns/cell total). Fusing to 2 calls saves
  ~0.86 ns/cell at dense fill. Non-overlapping bit windows avoid cross-salt
  correlation while sharing the two-hash work.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/rng.rs → grass_hash_fused_4`; `src/grass.rs →
  compute_propagation_scatter` (the 2-hash dispatch replacing the 4-call path).

### Density-weighted ("fertility") spread: variant A, amount ∝ source (v2.0.4 S4)

- **Decision**: The scatter spread amount is now **proportional to the source
  cell's density**: `add_byte = (spread_byte × s + 127) / 255` where `s` is the
  raw u8 source value. Spread *probability* (`spread_pct`) is unchanged, keeping
  `p` constant (geometric-skip-compatible if that ships). This is **variant A**
  (variant B — probability ∝ density — was rejected because it makes `p` per-cell,
  forcing rejection sampling on top of the walk with no observed benefit over A).
  `GRASS_DECAY_PCT_DEFAULT` is raised **0.02 → 0.04**: the self-reinforcing clumps
  that density weighting produces refill their centers, so decay headroom improves
  and a higher decay rate produces living-noise without global collapse.
- **Why**: Denser source cells push harder → self-reinforcing patch centers →
  tighter, more organic-looking clumps. The persistence invariant (plains
  super-critical, water sub-critical at shoreline) is preserved (tested in
  `src/grass_fertility_tests.rs`). The 0.04 decay default is **feel-tunable** —
  the lead should confirm it reads well in the browser.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/grass.rs → compute_propagation_scatter`
  (the `(spread_byte * s + 127) / 255` add_byte line);
  `src/constants.rs → GRASS_DECAY_PCT_DEFAULT`.

### grass_size slider: configurable cell size as perf lever (v2.0.4 S2)

- **Decision**: `GRASS_CELL_SIZE` is no longer compile-time only — it is also
  a **construction slider** (`grass_size`, idx 62, range 5–20u, step 1, default
  5.0). The value flows via `WorldDims::from_world_size_with_cell_size`; changing
  it resizes `grass_dim`, `grass_cell_count`, `capacity[]`, the biome grid, and
  the snapshot slot. Restart/full-world-rebuild scoped.
- **Why**: Cell count scales ~1/size², so a larger cell is the bluntest available
  perf lever — reducing grass_step work without architectural change. It is the
  principled form of "use less grass" and pairs with the LOD/scale resolution theme.
- **Tradeoffs**: `LUT_RADIUS = 4` (in `src/world/proximity.rs`) is pinned to the
  5u default — a safe over-approximation across the full 5–20u range. The grass
  `bilinear_sample` + circle-overlap uses the runtime `grass_cell_size`; so do
  `biome.rs`, `nn.rs` BiomeSampler, and the `render-gl.ts` UV transform (all
  previously hard-coded 5.0).
- **Applies to**: `architecture/simulation-core.md`, `architecture/render-pipeline.md`.
- **Code anchors**: `src/constants.rs → WorldDims::from_world_size_with_cell_size`,
  `GRASS_CELL_SIZE_DEFAULT`; `web/src/render-gl.ts → renderWorldImpl`
  (`grass_cell_size` param from `boot_ready`).
- **Revisit when**: `LUT_RADIUS` needs retuning for cell sizes > 20u, or the
  sim's proximity semantics change to depend on absolute cell size.

### Geometric-skip (C4) dropped after attribution bench shows net harm (v2.0.4 S5 drop)

- **Decision**: C4 (geometric-skip spread sampler) and C5 (rare-event spread
  tuning) were **dropped** and will not ship. The S0 attribution bench
  (`benches/grass_attribution.rs`, 512², 256 tiles) measured:
  - **RNG is dominant at dense fill**: 4-hash adds 3.05 ns/cell = 47% of
    6.42 ns/cell total; freeze floor 3.08 ns/cell (48%) is irreducible O(tile).
  - **Geometric-skip is net-harmful**: +0.49 ns/cell at 6.25% fill and
    **+18.7 ns/cell at ~90% fill**. The compact-list build is O(grassy) and
    the freeze+decay O(tile) floor is untouched; the cheap spread-gate-reducer
    form always loses.
  - The only path where skip pays is restructuring the freeze loop around an
    event list — the full event-sampling refactor in
    [`perf-optimization-ideas.md`](../plans/perf-optimization-ideas.md) §3
    — which is an **explicit non-goal** for this wave.
  - **C3 (fused RNG)** was vindicated and shipped (see above).
- **Why**: The measured data contradicts the theoretical gain because the
  freeze+decay O(tile) floor is irreducible. Skip removes only the spread-gate
  hash; it cannot remove the freeze read of every cell.
- **Applies to**: `architecture/simulation-core.md`;
  `perf-optimization-ideas.md §3` (S0 verdict annotates #3 with a measured
  verdict: event-sampling is the prerequisite, not skip alone).
- **Code anchors**: `benches/grass_attribution.rs` (the attribution bench);
  `docs/plans/v2.0.4-s0-attribution.md` (the bench findings doc).
- **Revisit when**: the event-sampling refactor is in scope, OR a different
  architecture changes the freeze floor.

### Grass spreads as a disc via separable blur + 8-neighbour max; active-tile frontier (v2.0.1)

- **Decision**: The `Blur` propagation path (retained behind the
  `GrassPropagation` selector) uses a two-pass separable `[1,2,1]/4`
  blur (horizontal then vertical = a 3×3 Gaussian) and spills via the
  **max of that blur over a cell's 8 neighbours**. Logistic growth
  `r·v·(1−v/K)` and the `[0,K]` clamp are unchanged. Work is scoped by a
  **32×32 active-tile frontier**: only tiles in `(ε, K−ε)` (plus their
  fringe) are processed. This is no longer the live propagation path
  (Scatter is); it is retained for benchmarking and as the default for
  low-level grid tests.
- **Why**: The 4-neighbour kernel grows grass in unnatural diamonds
  (L1 reach). The separable blur rounds the front; the 8-neighbour max is
  what achieves isotropy. Keeping it as a **max** (not a sum) preserves
  the anti-cascade rule. The active-tile frontier makes the full-grid pass
  unnecessary; equilibrium tiles are skipped exactly (equivalence-tested).
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `src/grass.rs → compute_propagation_blur`, `blur_at`,
  `GRASS_TILE_SIZE`; `src/grass_v201_tests.rs`.
- **Revisit when**: the lead decides to delete the blur path (Stage 3 of
  the grass-perf effort).

## See also

- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`cross-cutting.md`](cross-cutting.md)
