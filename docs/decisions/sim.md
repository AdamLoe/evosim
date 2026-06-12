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
- **Code anchors**: `app/web/src/sim/worker.ts → simLoop`.

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
- **Code anchors**: `app/web/src/sim/worker.ts → simLoop`,
  `app/web/src/sim/bridge.ts → SimBridge.postMessage`.
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
- **Code anchors**: `app/web/src/sim/worker.ts → simLoop`.

### Slider drain ordering: drain at the top of every iteration

- **Decision**: `drainMessages()` runs at the very top of every
  `simLoop` iteration, before `step_n` and before
  `writeSnapshotToSAB`.
- **Why**: A slider sent at tick T takes effect for tick T+1
  deterministically. If the drain happened after `step_n`, the slider
  would skip a tick.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `app/web/src/sim/worker.ts → simLoop`,
  `app/web/src/sim/worker.ts → drainMessages`.

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
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → WorldHandle::set_slider`,
  `app/crates/evosim/src/wasm_api/mod.rs → try_set_slider`.

### `MAX_POP_FOR_SIM` is a safety backstop, not the equilibrium mechanism

- **Decision**: `World::handle_births` retains a uniform-random cull pass
  that fires whenever `pop > MAX_POP_FOR_SIM`, but under the food-limited
  equilibrium defaults the backstop **effectively never fires** — population
  is held well below the cap by crowding upkeep + starvation drain. The
  snapshot writer asserts `pop <= MAX_POP_FOR_SIM` in dev builds, preserving
  the state-consistency invariant.
- **Why**: The random cull is an **anti-selection mechanism** — it kills good
  and bad foragers equally, so selection never compounds. Demoting it to a
  safety backstop lets food-limited mortality do the work: creatures that
  can't find food (high crowding, low energy) die preferentially, so foraging
  skill is a real selection axis. See the food-limited equilibrium entry below
  for the mechanism and the verified dynamics.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`,
  `decisions/cross-cutting.md`.
- **Code anchors**: `app/crates/evosim/src/world/mod.rs → World::handle_births`,
  `app/crates/evosim/src/constants.rs → MAX_POP_FOR_SIM`,
  `app/crates/evosim/src/wasm_api/mod.rs → write_creatures_each` (debug_assert),
  `World::scratch_cull_pool`.
- **Revisit when**: a feature wants persistent per-creature identity
  guarantees (e.g., a Hall of Fame), or the cap value moves past where
  allocator pressure becomes a problem.

### Food-limited, density-dependent equilibrium via crowding + starvation

- **Decision**: Population equilibrium is held by two food-limited mortality
  mechanisms in `energy_bookkeeping`, plus grass-supply limits:
  1. **Crowding upkeep** — extra energy drain proportional to the count of
     exact-distance neighbours within `crowding_radius`
     (`crowding_strength × count`). The spatial grid only bounds candidates;
     the tick path applies wrap-aware radius filtering.
  2. **Starvation drain** — below `starvation_threshold` energy, an extra
     `starvation_drain_rate` drain fires each tick.
  3. **Grass capacity scale + regrowth rate** — `grass_capacity_scale`
     multiplies the per-cell biome carrying-cap; `grass_regrowth_rate`
     multiplies spread probability + in-cell growth rate. Together they
     bound total food supply. Six live sliders: idx 67–72
     (`crowding_strength`=0.010, `crowding_radius`=20, `starvation_threshold`=15,
     `starvation_drain_rate`=0.30, `grass_capacity_scale`=0.7,
     `grass_regrowth_rate`=1.0).
- **Why the random cull was the wrong mechanism**: random cull kills good and
  bad foragers with equal probability, so selection never compounds — a
  well-fed, long-lived forager has the same chance of dying as a starving one.
  Food-limited mortality makes death track foraging skill: a creature that
  cannot find food (high crowding, low energy) dies preferentially; a skilled
  forager at good energy level survives. This is the selection argument for
  replacing the cull.
- **Verified dynamics** (headless harness, T=3000, K=4 seeds × both RNG paths;
  exit test `world/equilibrium_tests.rs`): steady-state mean ~1600–1760,
  CoV 0.007–0.025, never extinct, never cap-pinned, backstop cull never fires.
  The startup transient dips to ~38 founders before recovery on some seeds —
  a UX artifact, not extinction. **Foraging learning signal confirmed**:
  death-cohort age-at-death up 3.1–4.6× from early to late cohorts (real
  selection, not monotone artifact). Energy-gathered-per-tick is flat (food is
  the constraint) — skill manifests as *living longer on the same budget*.
  Age-at-death is therefore the correct foraging proxy.
  The browser-sized default world also has a Playwright startup smoke in
  `app/web/tests/e2e/sim-bridge.spec.ts`: at high TPS after fresh boot, the
  page remains alive and below the default population cap.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/tick.rs → energy_bookkeeping` (crowding + starvation drain);
  `app/crates/evosim/src/world/mod.rs → DevSliders` (six new fields);
  `app/crates/evosim/src/constants.rs → CROWDING_STRENGTH_DEFAULT`, `CROWDING_RADIUS_DEFAULT`,
  `STARVATION_THRESHOLD_DEFAULT`, `STARVATION_DRAIN_RATE_DEFAULT`,
  `GRASS_CAPACITY_SCALE_DEFAULT`, `GRASS_REGROWTH_RATE_DEFAULT`;
  `app/crates/evosim/src/world/equilibrium_tests.rs` (death-cohort + steady-state band gate).
- **Revisit when**: observed steady-state band or age-at-death trend differs
  materially in-browser (threaded scatter changes grass texture, headless is
  single-threaded); re-tune `grass_capacity_scale` first.

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
- **Code anchors**: `app/web/src/main.ts → restart`,
  `app/web/src/main.ts → spawnSimWorker`.

### Restart sources sliders from in-memory widget state, not localStorage

- **Decision**: `boot.initial_sliders` is sourced from
  `devpanel.ts::currentSliderState()` (reading the widgets' current
  `.value`), not from `getSettings()` (localStorage).
- **Why**: A mid-drag restart should carry the dragged value, not the
  last-persisted one. The drag write to localStorage only happens on
  release.
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/web/src/widgets/devpanel.ts → currentSliderState`,
  `app/web/src/main.ts → spawnSimWorker`.

### `WorldHandle` exposes no wasm-side `cross_origin_isolated` getter

- **Decision**: Both main and the worker read `crossOriginIsolated`
  from their own JS global. No Rust export.
- **Why**: It's a per-realm JS property; a Rust round-trip adds nothing
  except a wasm-bindgen call.
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/build-and-deploy.md`.
- **Code anchors**: `app/web/src/main.ts → main` (`globalThis...`),
  `app/web/src/sim/worker.ts → handleBoot` (`(self as ...).crossOriginIsolated`).

### Deterministic chunking for the parallel NN pass

- **Decision**: Per-tick NN chunk count =
  `clamp(pop / 32, MIN_CHUNKS=4, min(MAX_CHUNKS=16, workers))`.
- **Why**: Locks parallel results to be bit-identical to sequential for
  any given `(pop, workers)` combination — chunk boundaries are a
  function of inputs, not of rayon's runtime scheduling.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/nn.rs → chunk_ranges`,
  `app/crates/evosim/src/world/nn.rs → dynamic_chunks`,
  `app/crates/evosim/src/constants.rs → MIN_CHUNKS`, `MAX_CHUNKS`.

### Brain is `32 → 48 → 24 → 5` Leaky ReLU pyramid, no biases

- **Decision**: Three matmul layers with per-layer He init ranges
  (`0.433 / 0.354 / 0.500`); Leaky ReLU (slope 0.01) in the two hidden
  layers; no bias vectors; SIMD via `wide::f32x8`. Total
  `NN_WEIGHT_COUNT == 2808` (compile-asserted).
- **Why**: Sign-preservation (Leaky vs ReLU) gives smoother mutation
  fitness under no-gradient evolution. SIMD requires layer widths to
  be multiples of 8 (the topology choice is partly a consequence). No
  biases keeps the weight count parametric and the founder init
  uniform-random. The **input width** is a *runtime* field of
  `NnTopology`, a multiple of 8 in `[8, MAX_NN_INPUTS = 48]`, fed to
  the first matmul as `fan_in`. `build_nn_input` is a composable,
  offset-computing `NnInputLayout` descriptor.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/brain/mod.rs → Brain`, `Brain::founder`,
  `Brain::forward`; `app/crates/evosim/src/constants.rs → NN_OUTPUTS`,
  `NN_WEIGHT_COUNT`, `NN_INIT_RANGE_L1`, `NN_INIT_RANGE_L2`, `NN_INIT_RANGE_L3`.

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
- **Code anchors**: `app/crates/evosim/src/brain/mod.rs → Brain::founder`,
  `app/crates/evosim/src/constants.rs → FOUNDER_COUNT_DEFAULT`.
- **Revisit when**: a session with default sliders routinely
  extinct-ends before generation 1; that signals the founder pool
  needs the prior back, or wider He ranges.

### World wrap is a construction toggle (default toroidal)

- **Decision**: `wrap_world` is a runtime construction setting (default **on**
  = toroidal). ON ⇒ positions/vision/neighbour queries wrap across the seam
  (`rem_euclid`, toroidal minimum-image), and the 4 wall-proximity NN inputs
  are dropped. OFF ⇒ the legacy walled world: movement clamps to
  `[ri, world_size-ri]`, queries don't wrap, and the 4 wall inputs reappear.
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
  variants. `action[2]` is `Split` (single-pool) or `Mate` (species_mode)
  — same logit slot, mode-dependent validity gate (`ActionGate`), so the
  NN output shape is stable across modes. `Attack` (the logit formerly
  `Eat`) uses the same repr/logit order in both modes.
- **Why**: Smaller search space, simpler invariants. Other action
  variants cost code without delivering observable behavioural payoff
  at the population sizes the sim reaches. Reusing `action[2]`'s logit for
  Mate keeps the SAB + brain output shape unchanged between modes.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/creature.rs → Action`, `Action::ALL`;
  `app/crates/evosim/src/world/nn.rs → ActionGate / decode_action / is_valid_action`.

### Creature color is lineage-hue-derived; per-action highlight is a ring-flash

- **Decision**: Display color derives from the heritable `hue` column
  (`lineage_color_u32(hue)`: HSV hue = hue×360°, sat = 0.8, val = 0.9).
  The `hue` column replaces the old genome color: it is inherited at split
  (or mating) with a small Gaussian nudge, and founders receive evenly spaced
  seed hues so distinct lineages bloom/die in distinct colors. Per-action
  highlight remains a transient **ring-flash** — a `flash_tag` + 5-tick
  countdown packed into the snapshot's `packed_u32`. Priority: Killed (red)
  > CreatedChild (blue) > Attacked (yellow) > Grazed (green); Born (teal)
  at spawn.
- **Why**: Lineage color makes evolution legible on canvas — descendants share
  a hue, so a successful lineage blooms visibly. Identity-color (lineage) and
  behaviour-highlight (a brief ring) are separable: the steady color reads
  "which lineage is this" and the flash reads "what did it just do".
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `app/crates/evosim/src/creature.rs → CreatureSoA` (`hue` column),
  `app/crates/evosim/src/world/tick.rs → flash_decay`,
  `app/crates/evosim/src/wasm_api/mod.rs → lineage_color_u32, pack_render_u32`.

### Pure neuroevolution: genome removed; brain is the only heritable thing

- **Decision**: The 6-trait body `Genome` struct and the `CreatureSoA.genome`
  column are gone. Combat effectiveness, speed cap, metabolism/upkeep, energy
  cap, body radius, and graze yield are now **constants** (their median-genome
  values). Brain weights + lineage `hue` are the only heritable state.
- **Why**: The genome coupled many levers that made equilibrium untunable and
  muddied the "is the brain getting smarter?" question. With genome traits
  modulating biome penalties, combat, and metabolism simultaneously, it was
  impossible to isolate whether a population improvement came from brain
  learning or genome drift. Removing the genome makes the learning signal
  clean: any improvement in foraging efficiency, lifespan, or energy
  management is traceable to the brain alone. The constants are set at the
  median-genome values so behaviour is unchanged from a neutral genome run.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/creature.rs → CreatureSoA` (`hue` column replaces `genome`),
  `app/crates/evosim/src/constants.rs` (the former genome use-site constants),
  `app/crates/evosim/src/world/tick.rs → graze / attack / energy_bookkeeping`.

### Attack picks first-valid in a per-attacker-rotated cell walk

- **Decision**: `World::attack` does not scan
  every in-radius candidate to pick the *closest* in-reach target. Instead it
  takes the first target whose squared distance falls inside the constant
  body-radius reach, with the per-cell iteration starting at `attacker_idx % K_cell` and
  wrapping. Drops the closest + lowest-id tiebreak the previous impl used.
- **Why**: At high pop with `repulsion_max=0` (or any large clump),
  every predator's bbox returned dozens of candidates and the inner
  scan dominated the tick. The cells in a clump are small enough that
  "first valid" and "closest valid" are physically near-identical
  bites. The rotation seed de-correlates "first" from creature-index,
  which otherwise biases toward biting the oldest cell-mate every time
  (SoA-index order ≈ age order).
- **Applies to**: `architecture/simulation-core.md`.
- **Tradeoffs**: Bite-target is no longer the strict argmin-distance
  prey; in a tight stack the picked prey is whichever sits at
  `(predator_idx + k) mod K_cell` first. Determinism is preserved
  (rotation is a pure function of predator index + cell layout).
- **Revisit when**: A future world is much sparser (so the
  closest-prey scan is cheap again) or visualisation reveals an
  unexpected pattern from the rotation bias — at that point the
  cheapest mitigation is a starburst cell walk so "first" approximates
  "closest" without sorting.
- **Code anchors**: `crates/evosim/src/grid.rs → SpatialGrid::find_first_in_radius`,
  `app/crates/evosim/src/world/tick.rs → attack`, `AttackPick`.

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
  quickly in clumps. `HASH_CELL` is 20u and the world is 64× area at the
  same pop, so creature density is ~64× sparser and a full-range walk is
  cheap either way; the early-bail still pays off in clumps. In species
  mode the scan lights **16** sectors — 8 same-species + 8 other-species
  — so a mono-species clump never lights the other block; judged
  acceptable at the sparse coarse-cell density, with the fallback being
  to bail the two blocks independently.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/proximity.rs → proximity_starburst`,
  `compute_creature_proximity_sectors`,
  `compute_creature_proximity_sectors_species` (the 16-sector species variant).
- **Tradeoffs**: Output is bit-identical to the prior bbox walk (same
  max-aggregation semantics, same sector LUT, same bilinear split) per mode.
  The starburst list is built once via `OnceLock` and reused; in wrap mode
  the displacement is the toroidal minimum-image.
- **Revisit when**: `PROXIMITY_RANGE` or `HASH_CELL` changes by enough
  that the precomputed list needs a different cap, or a future sense
  uses sum-aggregation (where the early-bail invariant doesn't hold).

### Spatial grid rebuilds once per tick

- **Decision**: Rebuild `SpatialGrid` once from start-of-tick positions;
  movement, repulsion, attack, and species mating reuse that grid and
  post-movement consumers re-check exact distance against current positions.
- **Why**: A rebuild sweeps `hash_dim²`, which dominated the serial tick at the
  sparse default world; coarsening `HASH_CELL` to 20u and removing two
  movement-time rebuilds reduced grid plus movement cost by about 7.4× in the
  native tick profile.
- **Tradeoffs**: A creature can move into interaction range from a bucket the
  stale grid query does not visit, causing a benign one-tick missed repulsion,
  attack, or mate opportunity. Exact-distance checks prevent false hits.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/constants.rs → HASH_CELL`;
  `crates/evosim/src/world/mod.rs → World::step`;
  `crates/evosim/src/world/tick.rs → World::apply_movement_and_repulsion`.
- **Revisit when**: interaction fidelity near bucket boundaries matters more
  than the two avoided rebuilds, or movement/query ranges change materially.

### No save/load, no events, no Hall of Fame

- **Decision**: Every page load is a fresh world. No persistence, no
  event log, no eulogy.
- **Why**: All three cost surface area without delivering observable
  benefits. Removing them lets the rest of the system get smaller.
- **Applies to**: `architecture/simulation-core.md`.
- **Note**: a **species** registry now exists in `species_mode`
  (`World.species`) — but it's still in-memory, fresh per world; there's
  no cross-session persistence. The decision covers per-session durable
  lineage, which still doesn't exist (species-history breadcrumb is
  plumbed-but-empty until V2.1 splits).
- **Revisit when**: a use case appears that genuinely needs durable
  per-creature identity or cross-session continuity.

### Biome map: a few large blobs from `world_seed` via a dedicated PRNG

- **Decision**: The static biome grid is generated from a **dedicated
  SplitMix64 PRNG seeded by `world_seed`**, independent of the
  string/XxHash64 sim RNG. The generator scatters a few **large blobs**
  (2..4 water + 2..4 desert, radius `0.13..0.18 × world_size`) over a
  Plains background; water wins overlaps; distance is toroidal when
  `wrap_world`. Tuned to ~plains 60% / water 20% / desert 20%.
- **Why**: A separate stream lets the map pin to `world_seed` alone (so
  it is reproducible / shareable) while the live run still varies on the
  sim RNG. Large blobs (not noise speckle) give creatures a navigable,
  legible landscape with distinct food-rich and food-poor regions. SplitMix64
  is trivial, fast, and never touches the sim RNG draw order.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/biome.rs → generate_biome_grid`;
  `app/crates/evosim/src/world/mod.rs`.
- **Tradeoffs**: Blob count/radius are balance knobs (constants in
  `biome.rs`); proportions are statistical, not exact per seed. The grid
  is `grass_cell_count` u8 — same size as the snapshot grass region.
- **Revisit when**: biomes need finer structure (rivers, gradients) or a
  third+ biome kind, or the target proportions change.

### Biomes are food-only: no movement/energy penalties

- **Decision**: Water and Desert biomes carry **no movement, cost, or upkeep
  penalties**. The `K_BIOME_SPEED/COST/UPKEEP` constants, the
  `water_movement_penalty` / `desert_movement_penalty` sliders, and the
  `movement_penalty_for` function are all removed. The only surviving biome
  effect is the **grass-capacity cap** on scatter writes (Plains ×1.0,
  Desert ×0.30, Water ×0.04). Water/Desert are dead-grass avoidance zones:
  the brain learns to avoid them because there is no food, not because
  crossing them is expensive.
- **Why**: Movement tax + genome modulation coupled biome selection and body
  evolution, muddying the learning signal. With the genome gone and no penalty,
  biomes do exactly one thing — absence of food — which is the cleanest
  possible selection pressure. Avoidance remains learnable through grass-sector
  and current-grass inputs.
- **Applies to**: `architecture/biome.md`, `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/biome.rs → capacity_factor_from_u8`;
  `app/crates/evosim/src/grass/mod.rs → compute_propagation_scatter` (the cap write);
  `app/crates/evosim/src/constants.rs → GRASS_CAPACITY_PLAINS/WATER/DESERT`.

### Biomes have no direct NN type input

- **Decision**: The active NN layout has no direct biome-type input. Biomes
  influence the brain only through grass-sector, far-grass, and current-grass
  density inputs.
- **Why**: Biomes now only change grass carrying capacity. A raw biome id is a
  redundant proxy for the food signal and makes the learning surface wider
  without adding an effect the creature can act on directly.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/nn.rs → NnInputLayout::for_settings`,
  `build_nn_input`; `app/crates/evosim/src/world/proximity.rs →
  compute_grass_density_sectors`, `compute_grass_far_band_sectors`.
- **Revisit when**: a new biome effect other than grass capacity is added.

### Species + sexual mating is an opt-in mode, not a replacement

- **Decision**: `species_mode` is a construction-only `DevSliders` flag
  (default OFF, restart-required). OFF = today's single-pool asexual
  `Split` sim, fully unchanged. ON = N seeded species + sexual `Mate` +
  Attack-refuses-same-species + 16-sector creature proximity + per-species
  color. The two never coexist in a run.
- **Why**: Emergent single-pool drift is the v1 thing worth preserving;
  faction storytelling is the v2 thing worth adding. A single hard switch
  keeps both readable and lets the user A/B them.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/mod.rs`, `app/crates/evosim/src/world/nn.rs`,
  `app/crates/evosim/src/world/tick.rs`, `app/crates/evosim/src/world/species.rs`,
  `app/crates/evosim/src/wasm_api/mod.rs`.
- **Code anchors**: `DevSliders.species_mode`, `World.species`
  (`SpeciesRegistry`), the `species_id` SoA column.

### Mate: contact-radius, NN-initiated, first-found, initiator-only cost + cooldown

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
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/mod.rs → World::handle_mating`,
  `app/crates/evosim/src/world/nn.rs → ActionGate`, `decode_action`, `is_valid_action`,
  `app/crates/evosim/src/world/tick.rs → energy_bookkeeping` (cooldown decrement).
- **Revisit when**: births spike in a way that traces to many initiators mating one
  popular partner in a single tick — add a partner-cooldown guard at that point.

### Crossover is a construction enum applied to brain weights only

- **Decision**: `crossover_mode` (construction enum `{average, fifty_fifty}`,
  default `fifty_fifty`) is applied to **brain weights only**, then mutation
  (the same per-birth bucket machinery as a normal child). `fifty_fifty` =
  per-slot 50/50 random pick from the two parents; `average` = per-slot
  elementwise midpoint. Lineage `hue` is inherited as the wrap-aware midpoint
  of both parents' hues plus a small Gaussian nudge. Single-pool `Split` is
  unchanged (asexual, no crossover). RNG draw order is fixed and deterministic:
  brain crossover → brain bucket mutation, off the pair's pre-rolled seed.
- **Why**: The genome is gone, so crossover applies to brain weights only.
  Both modes are interesting evolutionary mechanics; one construction switch
  is cheap. `fifty_fifty` preserves variance (standard GA); `average` is
  available for homogenization research. Per-weight crossover is kept safe by
  the aligned regime (same-species-only mating + small per-birth mutation).
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/brain/mod.rs → Brain::child_from_crossover_with_sigma`,
  `app/crates/evosim/src/world/mod.rs → World::handle_mating`.
- **Revisit when**: playtest shows one mode strictly dominant, or brains
  demonstrably fail to evolve under mating.

### Cannibalism stays off in species mode — `Attack` refuses same-species

- **Decision**: In species mode, `World::attack` refuses same-`species_id`
  targets (a sim-side gate, not learned). Single-pool has no species, so
  `Attack` hits any creature — the v1 predation mechanic, unchanged. "No
  cannibalism" is a species-mode rule, not a global one.
- **Why**: Short networks drift into "eat anything moving" within a few
  generations; without a hard gate every species converges to cannibalism. Our
  brains aren't deep enough to encode evolved anti-cannibalism, so we guardrail it.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/tick.rs → World::attack`.
- **Revisit when**: brains grow deep enough that anti-cannibalism could plausibly
  evolve and be retained against drift.

### Species seeding + identity: dynamic registry, hand-spread palette, N-anchor founders

- **Decision**: `World.species` is a **dynamic** `SpeciesRegistry`
  (`Vec<Species { id, color_u32, name }>`, add/remove/recolor-capable) even
  though today it seeds a fixed `starting_species_count` (default 10). **N-anchor
  seeding** (no genome):
  1. Pick anchors spread across the world (rejection-sampled min spacing).
  2. Per anchor, roll a canonical first member = random founder brain with a
     **distinct seed hue** evenly spread across the color wheel
     (`canonical_hue = species_index / species_count`). No genome bias.
  3. Spawn `starting_species_member_count - 1` more founders clustered near the
     anchor, each = the canonical brain through **one per-birth bucket draw**
     with that bucket's **rate AND sigma both ×
     `starting_species_member_variance`** (default 3.0;
     `Brain::founder_spread_with_sigma`).
  All of seeding is **deterministic from `world_seed`** via a *dedicated* setup
  PRNG (`SimRng::from_u64(world_seed ^ SEEDING_PRNG_SALT)`) — separate from the
  biome generator and from the sim RNG, so the same `world_seed` reproduces the
  same tick-0 layout while the run still varies per launch. Species colors come
  from a **hand-spread 10-hue saturated palette** (`SPECIES_PALETTE`),
  single-sourced for both the per-creature `color_u32` lane and the polled
  species→color table. Labels are `Species-A..J`. Species extinct = extinct.
- **Why**: Dynamic registry means future splits/merges need no protocol change.
  The dedicated seeding PRNG satisfies the determinism guarantee without
  coupling the whole run to `world_seed`. A hand-spread palette (vs an even-hue
  ring) keeps adjacent species readable. Biome-adapted genome seeding was removed
  with the genome; species survival now depends entirely on the brain and the
  food-limited equilibrium, which is the intended selection environment.
- **Applies to**: `architecture/simulation-core.md`, `architecture/species.md`.
- **Code anchors**: `app/crates/evosim/src/world/species.rs → SpeciesRegistry`, `SPECIES_PALETTE`;
  `app/crates/evosim/src/brain/mod.rs → Brain::founder_spread_with_sigma`;
  `app/crates/evosim/src/world/mod.rs → pick_anchors`, `World::new_with_sliders_topology`
  (seeding branch); `app/crates/evosim/src/wasm_api/mod.rs → fill_creature_bytes` (species color lane).
- **Revisit when**: future splits/merges land (split-aware color generator needed);
  founders cold-start-extinct before brain evolution kicks in.

### Polled species→color/name/count table

- **Decision**: A cadence-written SAB report
  (`WorldHandle::species_table_json` → `CTRL_SPECIES_TABLE_EPOCH/LEN` + the
  `SPECIES_TABLE` byte window, written every ~45 ticks in species mode) exposes
  every **live** species as `{ id, color_u32, name, count }` + the world tick.
  Per-species counts are aggregated **sim-side**; the table is **dynamic**
  (extinct species drop out). Mirrors the `nn_worker_stats_json`
  cadence-producer pattern (no request side). The Monitor pop-graph + canvas
  color-table are the consumers (see `decisions/render.md → Per-species
  population graph`).
- **Why**: Sending `species_id` per creature + a shared polled table (rather
  than per-creature color bytes) keeps the snapshot lean and single-sources the
  palette so canvas and chart can't drift. Building it dynamic now saves a
  protocol break when future species splits arrive.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → species_table_json`;
  `crates/evosim/src/control_sab.rs` (slots + byte window);
  `app/web/src/sim/worker.ts → maybeWriteSpeciesTable`;
  `app/web/src/generated/control-sab.ts` (regenerated).

### Species-mode population balance: sane defaults for the mating economy

- **Decision**: Species-mode mating-economy defaults are unchanged —
  mating energy cost = `split_gift` (30), initiator `mating_cooldown_ticks`
  (200), birth gift = the mating cost (30), 10 species × 10 founders ×
  variance 3.0. The cap never binds via random cull in this mode (population
  under the encounter-rate / geographic-sparsity equilibrium). Cold-start did
  not fire at defaults, so no cold-start lever was applied.
- **Why**: Balance is the user's tuning task. The implementation contract is
  "mechanism demonstrably works" — the mating economy is one such mechanism.
  Single-pool equilibrium is now held by the food-limited mortality sliders
  (see the food-limited equilibrium entry above), not by this entry.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/constants.rs → MATING_COOLDOWN_TICKS_DEFAULT`,
  `SPLIT_GIFT_MAX_DEFAULT`, `starting_species_member_count` (default), `starting_species_count` (default).
- **Revisit when**: playtest wants growth at the default world — first lever is
  cranking `starting_species_member_count` (denser clumps).

### Why one binary mode toggle, and why species ⇒ mating

- **Decision**: `species_mode` is a *single* construction switch with **no
  finer parameterization** — there is no "sexual without species" or "species
  without asexual-mating" combination. (What each mode *contains* is covered by
  the "opt-in mode" entry above; this entry records only why the axis is a
  single binary.)
- **Why**: The emergent single-pool drift and the faction storytelling (species
  mode) are each worth preserving whole. One hard switch keeps both readable and
  A/B-able; the intermediate combinations were rejected as too many to balance.
  Species *without* mating in particular has no mechanism for species to evolve
  independently — asexual lineages would each just be their own drift line,
  which the single-pool sim already does without the species label.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md` (boot payload, NN input layout composition).
- **Alternatives**: parameterize finer (asexual+species, sexual+no-species, …)
  — rejected as too many combinations to balance.

### Child spawns at parent midpoint, no habitable-cell check

- **Decision**: A mated child spawns at the wrap-aware midpoint of the two
  parents' positions, even if that cell is hostile (deep water / desert). No
  retry, no nearest-habitable search.
- **Why**: Simple, and honest selection — a child spawning in a food-poor zone
  is exposed to the same selection pressure as any other creature. A
  habitable-cell search would add a scan and quietly mask cross-biome mating
  as a failure mode.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/mod.rs → World::handle_mating`.

### Species extinct = extinct; `Species-A..J` names; species-only history

- **Decision**: When a species hits population 0 it stays gone — no respawn
  pod, no founder injection. Names are the fixed `Species-A..J` (random-name
  generator deferred). The inspector shows species id / color / **species**
  history (parent-species breadcrumb), never a per-creature ancestor chain.
- **Why**: In this version new species don't arise mid-run — they're seeded at
  construction. Species color does the visual-identity work, so names aren't
  load-bearing yet. A 10-generation individual-ancestor chain is unreadable;
  species history is the storytelling primitive that matters once splits ship
  (plumbed-but-empty until then).
- **Applies to**: `architecture/simulation-core.md`, `architecture/species.md`.
- **Code anchors**: `app/crates/evosim/src/world/species.rs → World.species`;
  `app/crates/evosim/src/wasm_api/mod.rs → creature_inspect_json`; inspector species block `#ins-species-*`.
- **Revisit when**: splits/merges land (then extinction, naming, and the
  history breadcrumb all gain real behavior).

### Action set is `{Graze, Attack, Split | Mate}`; velocity stays continuous

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
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/creature.rs → Action`;
  `app/crates/evosim/src/world/nn.rs → decode_action`.

### NN input layout is settings-derived; no vestigial slots; one bias slot

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
  practice. (The full 8-way width table across wrap×species×multisight —
  32/32/40/40/40/40/48/48 — lives in `architecture/simulation-core.md`; the
  cross-language safety implication is in `decisions/cross-cutting.md`.)
- **Applies to**: `architecture/simulation-core.md`,
  `app/crates/evosim/src/world/nn.rs → NnInputLayout / NnInputGroup`, `app/crates/evosim/src/brain/mod.rs → NnTopology`.
- **Alternatives**: keep a constant action-set-indicator input for
  "portability" (rejected — a brain selected against one `action[2]` semantic
  doesn't transfer just because one constant signal says which mode it's in);
  directional biome penalty sectors (rejected — penalties are gone and grass
  density carries the remaining biome effect).

### Body radius and repulsion: constants; sprite/repulsion not collision

- **Decision**: Body radius, repulsion strength, and all creature mechanics
  previously modulated by genome traits are now constants. `body_size` no
  longer exists as a trait; the constant radius scales sprite + repulsion
  strength, **not** collision (there is no collision).
- **Why**: Removing per-creature body-size variance eliminates one axis of
  non-brain variation from the learning signal. Repulsion still approximates
  "creatures own space" without requiring per-creature spatial tracking.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/constants.rs` (body radius constant);
  `app/crates/evosim/src/world/tick.rs` (repulsion use site).

### Grass density field is `Vec<AtomicU8>` on the snapshot quantization scale

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
- **Code anchors**: `app/crates/evosim/src/grass/mod.rs → GrassGrid::density` (field),
  `encode_density`, `decode_density`, `GrassGrid::dget`, `GrassGrid::dset`,
  `GrassGrid::dget_u8`, `GrassGrid::density_u8_snapshot`.

### Stochastic u8 scatter kernel is the live propagation path; blur retained behind a selector

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
  Frontier: tiles written to are marked active next tick via atomic
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
- **Code anchors**: `app/crates/evosim/src/grass/mod.rs → compute_propagation_scatter`,
  `compute_propagation_blur`, `GrassPropagation`, `DiscTable`,
  `ScatterParams`, `GrassGrid::scatter_params`;
  `app/crates/evosim/src/constants.rs → GRASS_DECAY_PCT_DEFAULT`, `GRASS_DECAY_AMOUNT_DEFAULT`,
  `GRASS_SPREAD_PCT_DEFAULT`, `GRASS_SPREAD_AMOUNT_DEFAULT`,
  `GRASS_SPREAD_RING1_PCT_DEFAULT`, `GRASS_SPREAD_RING2_PCT_DEFAULT`,
  `GRASS_SPREAD_RING3_PCT_DEFAULT`, `GRASS_SPREAD_RADIUS`.
- **Revisit when**: the blur deletion decision is made by the lead (the
  bench is the trigger).

### Threaded scatter is intentionally non-reproducible; single-threaded runs are deterministic — PENDING LEAD RATIFICATION

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

### GRASS_EQ_EPS = 1/255 (one u8 quantum)

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
  `app/crates/evosim/src/grass/mod.rs → GRASS_EQ_EPS`.

### Grass scatter sliders plus live bites-per-block

- **Decision**: Six live sliders wire into `GrassGrid.scatter_params` via
  `WorldHandle` apply_ setters: `grass_decay_pct` (54),
  `grass_decay_amount` (55), `grass_spread_pct` (56),
  `grass_spread_amount` (57), `grass_spread_ring1_pct` (58),
  `grass_spread_ring2_pct` (59). Ring 3 is implicit (disc table
  normalizes all three; ring3 = max(0, 1 − ring1 − ring2)). The existing
  `grass_bites_per_block` slider is also live: graze computes
  `density_chunk = GRASS_MAX / max(1, grass_bites_per_block)` each tick.
- **Why**: Scatter parameters and graze bite granularity are simulation
  balance knobs that do not resize buffers or rebuild topology, so they can
  update on the running world. The default bite count remains 2, preserving the
  shipped default transfer.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/constants.rs → GRASS_BITES_PER_BLOCK_DEFAULT`;
  `app/crates/evosim/src/wasm_api/mod.rs → SLIDER_NAMES` (indices 54–59), `try_set_slider`;
  `app/crates/evosim/src/world/tick.rs → graze`;
  `app/crates/evosim/src/grass/mod.rs → GrassGrid::consume`.

### Grass pyramid: u8 box-filter mip clipmap over the density field

- **Decision**: `GrassPyramid` is a u8 box-filter mip pyramid over
  `GrassGrid`. L0 aliases the live `density` field (not stored in the
  pyramid). L1+ are owned, MEAN-downsampled, edge-clamped u8 buffers.
  Each Lk cell is the arithmetic mean of its (up to) 4 in-bounds Lk-1
  parent cells: `(sum + count/2) / count` (round-before-truncate). At the
  default `grass_dim = 1920` this yields ~11 levels before reaching 1×1
  (well under the `GRASS_PYRAMID_MAX_LEVELS = 16` cap). L1+ refresh as a
  full recompute every `GRASS_PYRAMID_REFRESH_PERIOD` ticks in `World::step`
  at step 7b. Serves render LOD + snapshot windowing and far-grass NN sensing;
  a dirty-subtree partial refresh keyed off `tile_active` is a tracked TODO.
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
- **Tradeoffs**: Refresh is a full recompute when it runs (O(N×1/3) over all
  owned levels), amortized across the refresh period. Zoomed-out render and
  far-grass sensing may lag by at most that period; default zoom render reads
  live L0. Active-set-keyed partial refresh is deferred; see the
  `TODO(stage3-pyramid-perf)` anchor in `app/crates/evosim/src/grass/mod.rs`.
- **Code anchors**: `app/crates/evosim/src/grass/mod.rs → GrassPyramid`, `GrassGrid::pyramid`,
  `GrassGrid::refresh_pyramid`;
  `app/crates/evosim/src/constants.rs → GRASS_PYRAMID_MAX_LEVELS`,
  `GRASS_PYRAMID_REFRESH_PERIOD`;
  `app/crates/evosim/src/world/mod.rs → World::step`.

### Far multi-band grass NN sight: GrassBandsFar default ON; single-band toggle kept

- **Decision**: Grass NN sensing now includes a second group —
  `NnInputGroup::GrassBandsFar` (8 sectors at `GRASS_FAR_SIGHT_RADIUS = 160u`,
  read at `GRASS_FAR_MIP_LEVEL = 3` from `GrassPyramid::sample_clamped`).
  This is **default ON** via the `grass_multisight` construction slider (idx 61,
  default true). Both the multi-band and single-band layouts stay compiled;
  the dev-panel toggle lets the user A/B them. Because the setting changes the
  NN input layout, it rides the explicit boot payload before founder brains and
  topology are constructed; applying `initial_sliders` after construction is too
  late.
- **Why**: O(1) pyramid taps cost nothing at mip level 3 (one sample per sector).
  Creatures sensing grass at 160u radius can navigate toward distant patches
  before local exhaustion. Shipping default ON keeps all born creatures on the
  same layout as the rest of the world; the toggle is for the A/B and debugging.
- **Constraint**: without direct biome-type inputs, all
  wrap×species×multisight combinations fit within `MAX_NN_INPUTS = 48`; no
  fallback is needed. The widest current layout is walled+species+multisight
  at 46 real slots → 48 padded.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/world/nn.rs → NnInputGroup::GrassBandsFar`,
  `NnInputLayout::for_settings`; `app/crates/evosim/src/world/proximity.rs →
  compute_grass_far_band_sectors`; `app/crates/evosim/src/constants.rs → GRASS_FAR_SIGHT_RADIUS`,
  `GRASS_FAR_MIP_LEVEL`.
- **Revisit when**: band count, radii, or mip level need retuning; or the
  MAX_NN_INPUTS ceiling becomes a bottleneck for future input groups.

### Fused RNG for scatter kernel (grass_hash_fused_4)

- **Decision**: The scatter kernel replaces 4 separate `grass_hash_u64` calls
  (salts 1–4) with `grass_hash_fused_4` (`app/crates/evosim/src/rng.rs`): 2 `grass_hash_u64`
  words, bit-sliced into non-overlapping windows for decay/spread/band/pick.
  The property "grass never perturbs `SimRng`" is preserved; distribution
  statistics match within tolerance (tested in `crates/evosim/src/grass_fused_rng_tests.rs`).
- **Why**: Attribution bench (S0) measured RNG as the dominant per-cell cost at
  dense fill (3.05 ns/cell, 47% of 6.42 ns/cell total). Fusing to 2 calls saves
  ~0.86 ns/cell at dense fill. Non-overlapping bit windows avoid cross-salt
  correlation while sharing the two-hash work.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/rng.rs → grass_hash_fused_4`; `app/crates/evosim/src/grass/mod.rs →
  compute_propagation_scatter` (the 2-hash dispatch replacing the 4-call path).

### Density-weighted ("fertility") spread: variant A, amount ∝ source

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
  `crates/evosim/src/grass_fertility_tests.rs`). The 0.04 decay default is **feel-tunable** —
  the lead should confirm it reads well in the browser.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/grass/mod.rs → compute_propagation_scatter`
  (the `(spread_byte * s + 127) / 255` add_byte line);
  `app/crates/evosim/src/constants.rs → GRASS_DECAY_PCT_DEFAULT`.

### grass_size slider: configurable cell size as perf lever

- **Decision**: `GRASS_CELL_SIZE` is no longer compile-time only — it is also
  a **construction slider** (`grass_size`, idx 62, range 5–20u, step 1, default
  5.0). The value flows via `WorldDims::from_world_size_with_cell_size`; changing
  it resizes `grass_dim`, `grass_cell_count`, `capacity[]`, the biome grid, and
  the snapshot slot. Restart/full-world-rebuild scoped.
- **Why**: Cell count scales ~1/size², so a larger cell is the bluntest available
  perf lever — reducing grass_step work without architectural change. It is the
  principled form of "use less grass" and pairs with the LOD/scale resolution theme.
- **Tradeoffs**: `LUT_RADIUS = 4` (in `app/crates/evosim/src/world/proximity.rs`) is pinned to the
  5u default — a safe over-approximation across the full 5–20u range. The grass
  `bilinear_sample` + circle-overlap uses the runtime `grass_cell_size`; so do
  `biome.rs` and the `render/gl.ts` UV transform (all
  previously hard-coded 5.0).
- **Applies to**: `architecture/simulation-core.md`, `architecture/render-pipeline.md`.
- **Code anchors**: `app/crates/evosim/src/constants.rs → WorldDims::from_world_size_with_cell_size`,
  `GRASS_CELL_SIZE_DEFAULT`; `app/web/src/render/gl.ts → renderWorldImpl`
  (`grass_cell_size` param from `boot_ready`).
- **Revisit when**: `LUT_RADIUS` needs retuning for cell sizes > 20u, or the
  sim's proximity semantics change to depend on absolute cell size.

### Geometric-skip (C4) dropped after attribution bench shows net harm

- **Decision**: C4 (geometric-skip spread sampler) and C5 (rare-event spread
  tuning) were **dropped** and will not ship. The S0 attribution bench
  (`crates/evosim/benches/grass_attribution.rs`, 512², 256 tiles) measured:
  - **RNG is dominant at dense fill**: 4-hash adds 3.05 ns/cell = 47% of
    6.42 ns/cell total; freeze floor 3.08 ns/cell (48%) is irreducible O(tile).
  - **Geometric-skip is net-harmful**: +0.49 ns/cell at 6.25% fill and
    **+18.7 ns/cell at ~90% fill**. The compact-list build is O(grassy) and
    the freeze+decay O(tile) floor is untouched; the cheap spread-gate-reducer
    form always loses.
  - The only path where skip pays is restructuring the freeze loop around an
    event list (the full event-sampling refactor), which is a deferred
    non-goal. See `decisions/perf.md` for the measured verdict.
  - **C3 (fused RNG)** was vindicated and shipped (see above).
- **Why**: The measured data contradicts the theoretical gain because the
  freeze+decay O(tile) floor is irreducible. Skip removes only the spread-gate
  hash; it cannot remove the freeze read of every cell.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/benches/grass_attribution.rs` (the attribution bench);
  see `decisions/perf.md` for bench findings and measured verdicts.
- **Revisit when**: the event-sampling refactor is in scope, OR a different
  architecture changes the freeze floor.

### Blur propagation path retained behind the `GrassPropagation` selector

- **Decision**: The `Blur` propagation path is **retained** behind the
  `GrassPropagation::{Scatter,Blur}` selector. Scatter is the live path;
  Blur is the default for low-level grid tests so existing blur tests keep
  passing without rewrite. It is not deleted. The blur uses a two-pass
  separable `[1,2,1]/4` kernel + 8-neighbour max; `GrassPropagation::Scatter`
  is set at `World` boot.
- **Why**: Blur deletion is a lead call, not a mechanical cleanup. The blur
  achieves isotropy (the 4-neighbour kernel grows diamonds; the 8-neighbour
  max rounds the front), and the equivalence tests serve as a regression
  baseline. Deleting it before the lead signs off would remove a
  benchmarking reference and break test constructors that rely on the Blur
  default.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/grass/mod.rs → compute_propagation_blur`,
  `blur_at`, `GrassPropagation`; `crates/evosim/src/grass_v201_tests.rs`.
- **Revisit when**: the lead decides to delete the blur path.

### Snapshot stays on the sim thread; off-thread snapshot worker rejected

- **Decision**: `WorldHandle::write_snapshot` runs on the sim worker thread.
  A separate snapshot worker was evaluated and rejected. Optimize-in-place
  is the chosen direction.
- **Why**: `write_snapshot` reads live `World` state by borrow — creature SoA
  columns (`x`, `y`, `radius`, `color_u32`, `packed_u32`, id halves), the grass
  `density` field (mutated every tick via atomic RMW), and the mip `pyramid`
  (fully rebuilt every tick). The next `step_n(1)` overwrites all of that in
  place. An off-thread split is therefore only feasible via one of:
  (a) double-buffering all hot `World` state — copies more bytes than it saves
  because the staging copy IS the dominant cost (not the write destination);
  (b) a per-tick barrier after every `step_n` — gives back the rayon parallelism
  gain and adds a sync tax;
  (c) staging a source copy on the sim thread — same dominant cost as (a).
  Additionally, restart becomes fragile: a boot-generation epoch must be added so
  main ignores a snapshot from a stale pre-restart worker.
  The bottleneck is the source read on the sim thread, not the write destination,
  so no destination choice (JS SAB, separate wasm module, etc.) rescues it.
- **Alternatives considered**: dedicated JS SharedArrayBuffer destination — rejected,
  moving the destination is irrelevant when the source read dominates and must
  stay on the sim thread.
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → WorldHandle::write_snapshot` (the source
  reads: creature SoA borrow, `grass.density`, `grass.pyramid`).
- **Revisit when**: a new dominant `write_snapshot` cost emerges that cannot
  be eliminated in-place, AND a double-buffer design is available that provably
  costs less than the source read. (The prior dominant cost — the biome-window
  recompute — was eliminated via `BiomePyramid` precomputation; snapshot write
  is now 2.35 ms.)

### Seeded grass clumps: initial-budget choice

- **Decision**: Boot grass with **40 clump centres** (`GRASS_CLUMP_COUNT_DEFAULT`),
  each a **radius-8-cell disc** (`GRASS_CLUMP_SIZE_DEFAULT`), centres derived
  deterministically from `world_seed` via the position-addressable hash RNG
  (independent of the sim RNG so creature draw order is unaffected).
- **Budget**: Disc area ≈ π × 8² ≈ 201 cells/clump × 40 clumps ≈ **8,040
  occupied cells** — matching the old uniform-scatter budget of
  `GRASS_INITIAL_SEED_COUNT_DEFAULT = 8000` (≤ 0.22% of a 1920² grid). Clumps
  may overlap; actual count is ≤ 8040. The value was chosen deliberately to
  preserve the old budget so existing perf/behaviour baselines are not silently
  disrupted.
- **Why**: Uniform single-cell scatter at density ~0.2% looks like dust — no
  visible patches, no spatial texture for creatures to navigate. Clumps of radius
  8 (≈16×16 cell bounding box, ≈11% of a 32-cell tile side) produce visually
  legible patches without changing the starting occupied-cell count.
- **Reproducibility**: Clump centres are derived from
  `grass_hash_u64(world_seed, clump_index, tick=0, salt=0/1)` — the same hash
  used by the scatter kernel but never during propagation. Same `world_seed`
  → same centres every boot; this is separate from the sim-string RNG so creature
  draws are unaffected.
- **Boot-seam constraint**: `grass_clump_count` and `grass_clump_size` are
  construction-only and ride the explicit `newWithFounderCount` boot params
  (same rule as `grass_cell_size`) — `initial_sliders` is applied AFTER
  world construction and cannot re-seed.
- **Fallback**: `grass_clump_count = 0` falls back to the old uniform-scatter path
  (`grass_initial_seed_count` cells uniformly at random). No other code path changed.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `app/crates/evosim/src/grass/mod.rs → GrassGrid::seed_clumps`;
  `app/crates/evosim/src/world/mod.rs → new_with_sliders_topology`;
  `app/crates/evosim/src/constants.rs → GRASS_CLUMP_COUNT_DEFAULT`, `GRASS_CLUMP_SIZE_DEFAULT`.
- **Alternatives considered**: sampling from Halton sequence instead of the
  stateless hash — rejected because the hash is already present and avoids adding
  a sequential dependency; Halton would need a per-clump sequential index that
  couples with the sim RNG. Poisson-disc spacing between clump centres — rejected
  as over-engineering for a first pass; visual patches are already legible at 40
  clumps on a 1920² grid.
- **Revisit when**: player feedback says the starting landscape needs more or less
  grass clustering; the budget knob can then be exposed as a DevPanel slider.

### BiomePyramid precomputed at construction; write_snapshot copies a window

- **Decision**: A `BiomePyramid` struct is built **once at world construction**
  (`BiomePyramid::build`) from the static `biome_grid` and stored on `WorldHandle`
  as `biome_pyramid`. Each call to `write_snapshot` copies a pre-downsampled biome
  window from the pyramid via `biome_pyramid.copy_window(...)`, replacing the
  old per-tick mode-downsample loop. The pyramid is invalidated only on world
  reconstruction (restart).
- **Why**: Before this change, `write_snapshot` recomputed the mode-downsampled
  biome window every tick from the static `biome_grid` via an unmemoized nested loop.
  Profiling (`write_output_sab.snapshot.biome` span) showed this was **83% of total
  snapshot cost** (11.4 ms / 13.7 ms at a 270-creature, 1920² scenario). Precomputing
  eliminates redundant work — the biome grid never changes after boot. After the
  optimization: snapshot write 13.74 ms → **2.35 ms** (5.9× speedup).
- **Tradeoffs**: `BiomePyramid` adds memory proportional to `O(grass_cell_count × 4/3)`
  (pyramid series). The build happens at world construction (already the most
  expensive phase). Window copy replaces a loop over `win_w × win_h` biome cells
  per tick — the copy is a bounded slice copy vs. the old per-cell mode calculation.
- **Applies to**: `architecture/simulation-core.md`, `architecture/profiler.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → BiomePyramid`, `BiomePyramid::build`,
  `BiomePyramid::copy_window`; `app/crates/evosim/src/wasm_api/mod.rs → WorldHandle::biome_pyramid`;
  `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot` (the `biome_pyramid.copy_window` call replacing
  the old loop); profiler span `write_output_sab.snapshot.biome`.
- **Revisit when**: biome grid becomes dynamic (live biome editing); at that point
  the pyramid must be rebuilt on mutation, or the per-tick recompute reinstated.

## See also

- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`cross-cutting.md`](cross-cutting.md)
