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
- **Code anchors**: `web/src/sim/worker.ts → simLoop`.

### Pacing uses synchronous `Atomics.wait` because steady-state control is SAB-only

- **Decision**: The worker loop is synchronous and parks with
  `Atomics.wait(ctrl, CTRL_FUTEX, before, timeoutMs)` while running, and
  `Infinity` while paused. It does not use `Atomics.waitAsync`.
- **Why**: The hot control path no longer depends on worker `onmessage`;
  sliders, pause, target TPS, inspector, profile, camera, and snapshot
  acknowledgement are all SAB lanes. With no steady-state macrotask queue to
  drain, synchronous wait gives zero-CPU parking without the old
  `waitAsync(..., 1ms)` throughput tax.
- **Tradeoffs**: Any future feature that reintroduces steady-state
  postMessage control must revisit the pacing primitive first.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/sim/worker.ts → simLoop`,
  `web/src/sim/bridge.ts → SimBridge`.

### Slider drain ordering: drain at the top of every iteration

- **Decision**: SAB control reads run at the very top of every
  `simLoop` iteration, before `step_n` and before
  `maybeWriteSnapshotToSAB`.
- **Why**: A slider sent at tick T takes effect for tick T+1
  deterministically. If the drain happened after `step_n`, the slider
  would skip a tick.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/sim/worker.ts → simLoop`,
  `web/src/sim/worker.ts → readControlSab`.

### `set_slider(name, value)` is the sole external mutation entry point

- **Decision**: One wasm-bindgen method dispatches every dev-panel
  slider by string name. There are no per-typed `set_max_age` /
  `set_split_threshold` / etc. exports. Bools such as `wrap_world`,
  `species_mode`, and `grass_multisight` ride the same path as `0|1`.
- **Why**: One small protocol surface, no per-slider message type, no
  carve-out for bools. Adding a slider is a 1-line
  `apply_X` + 1-line `try_set_slider` arm + a TS widget — no protocol
  change.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Tradeoffs**: Unknown slider names throw a `JsValue` error — typos
  surface as a `[sim] set_slider("X", v) rejected` warning rather than
  silently no-op'ing. (Considered a feature.)
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → WorldHandle::set_slider`,
  `crates/evosim/src/wasm_api/mod.rs → try_set_slider`.

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
- **Code anchors**: `crates/evosim/src/world/mod.rs → World::handle_births`,
  `crates/evosim/src/constants.rs → MAX_POP_FOR_SIM`,
  `crates/evosim/src/wasm_api/mod.rs → fill_creature_bytes` (debug_assert),
  `crates/evosim/src/world/mod.rs → World`.
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
  3. **Grass supply** — total food is bounded by the per-cell biome
     carrying-cap and the live Grass-tab scatter knobs (decay/spread/growth).
     The equilibrium slider names and stable indices are owned by
     `crates/evosim/src/wasm_api/mod.rs` → `SLIDER_NAMES`; their default values
     are owned by the `CROWDING_*` and `STARVATION_*` constants named below.
- **Why the random cull was the wrong mechanism**: random cull kills good and
  bad foragers with equal probability, so selection never compounds — a
  well-fed, long-lived forager has the same chance of dying as a starving one.
  Food-limited mortality makes death track foraging skill: a creature that
  cannot find food (high crowding, low energy) dies preferentially; a skilled
  forager at good energy level survives. This is the selection argument for
  replacing the cull.
- **Verified dynamics**: `cargo test --lib` runs
  `crates/evosim/src/world/equilibrium_tests.rs`, which gates the steady-state
  band, extinction/cap-pinning checks, backstop-cull absence, and death-cohort
  age-at-death trend.
  The startup transient dips to ~38 founders before recovery on some seeds —
  a UX artifact, not extinction. **Foraging learning signal confirmed**:
  death-cohort age-at-death up 3.7–5.4× from early to late cohorts (real
  selection, not monotone artifact). Energy-gathered-per-tick is flat (food is
  the constraint) — skill manifests as *living longer on the same budget*.
  Age-at-death is therefore the correct foraging proxy.
  The browser-sized default world also has a Playwright startup smoke in
  `web/tests/e2e/sim-bridge.spec.ts`: at high TPS after fresh boot, the
  page remains alive and below the default population cap.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/tick.rs → energy_bookkeeping` (crowding + starvation drain);
  `crates/evosim/src/world/mod.rs → DevSliders` (four equilibrium fields);
  `crates/evosim/src/constants.rs → CROWDING_STRENGTH_DEFAULT`, `CROWDING_RADIUS_DEFAULT`,
  `STARVATION_THRESHOLD_DEFAULT`, `STARVATION_DRAIN_RATE_DEFAULT`;
  `crates/evosim/src/world/equilibrium_tests.rs` (death-cohort + steady-state band gate).
- **Revisit when**: observed steady-state band or age-at-death trend differs
  materially in-browser (threaded scatter changes grass texture, headless is
  single-threaded); re-tune `crowding_strength` / `starvation_drain_rate` or the
  Grass-tab scatter knobs first.

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

### Worker recovery is restart-first and progress-observed

- **Decision**: Main detects boot timeout, worker `error` /
  `messageerror`, and missing unpaused progress, then recovers by spawning
  a fresh worker and terminating the old bridge. It does not restore the
  dead worker's exact world state.
- **Why**: The worker loop is intentionally synchronous and SAB-only, so a
  frozen worker cannot be asked to tear itself down. Main can still observe
  `CTRL_SEQ` plus cadence report epochs and can always kill the worker
  from outside.
- **Tradeoffs**: Recovery is a fresh world with current settings and the
  normal restart seed semantics. Exact state restoration waits for the
  persistence design.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/main.ts → checkWorkerWatchdog`,
  `restartWorker`, `recoverWorker`; `web/src/sim/bridge.ts →
  WorkerDebugFault`.
- **Revisit when**: world persistence ships, or a future protocol adds a
  live-worker health heartbeat that is cheaper or more precise than the
  snapshot/report epoch signature.

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
  `web/src/sim/worker.ts → handleBoot` (`(self as ...).crossOriginIsolated`).

### Deterministic chunking for the parallel NN pass

- **Decision**: Per-tick NN chunk count =
  `clamp(pop / 32, MIN_CHUNKS=4, min(MAX_CHUNKS=16, workers))`.
- **Why**: Locks parallel results to be bit-identical to sequential for
  any given `(pop, workers)` combination — chunk boundaries are a
  function of inputs, not of rayon's runtime scheduling.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/nn.rs → chunk_ranges`,
  `crates/evosim/src/world/nn.rs → dynamic_chunks`,
  `crates/evosim/src/constants.rs → MIN_CHUNKS`, `MAX_CHUNKS`.

### NN sector-search ranges are construction settings

- **Decision**: `CreatureSectors`, `GrassSectors`, and `GrassBandsFar` ranges
  live in `WorldConfig.nn_sensing` and apply on the next world, not through
  live `set_slider` updates.
- **Why**: the ranges define the constructed sensing contract and interact with
  cached lookup/pyramid behavior; applying them at boot keeps saved worlds,
  inspectors, and tick-time NN inputs aligned.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → WorldConfig`,
  `crates/evosim/src/world/nn.rs → build_nn_input`,
  `crates/evosim/src/world/proximity.rs → compute_grass_far_band_sectors`.

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
  `NnTopology`, a multiple of 8 capped by `MAX_NN_INPUTS`, fed to
  the first matmul as `fan_in`. `build_nn_input` is a composable,
  offset-computing `NnInputLayout` descriptor.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/brain/mod.rs → Brain`, `Brain::founder`,
  `Brain::forward`; `crates/evosim/src/constants.rs → NN_OUTPUTS`,
  `NN_WEIGHT_COUNT`, `NN_INIT_RANGE_L1`, `NN_INIT_RANGE_L2`, `NN_INIT_RANGE_L3`.

### Founder NN is pure uniform-random — no hardwired priors

- **Decision**: `Brain::founder` initializes weights from `rng.uniform`
  per layer with the He ranges above. No "energy → split" wiring, no
  grass-detector hot slot, no Move baseline.
- **Why**: Hardwired founder priors are brittle to topology / slot-layout
  changes. Uniform-random with a reasonable multi-founder count
  (`STARTING_POP_DEFAULT` via Halton placement)
  gives the sim enough lineages that at least one survives
  generation 0.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/brain/mod.rs → Brain::founder`,
  `crates/evosim/src/constants.rs → STARTING_POP_DEFAULT`.
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
- **Code anchors**: `crates/evosim/src/creature.rs → Action`, `Action::ALL`;
  `crates/evosim/src/world/nn.rs → ActionGate`, `decode_action`, `is_valid_action`.

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
- **Code anchors**: `crates/evosim/src/creature.rs → CreatureSoA` (`hue` column),
  `crates/evosim/src/world/tick.rs → flash_decay`,
  `crates/evosim/src/wasm_api/mod.rs → lineage_color_u32, pack_render_u32`.

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
- **Code anchors**: `crates/evosim/src/creature.rs → CreatureSoA` (`hue` column replaces `genome`),
  `crates/evosim/src/constants.rs → BODY_RADIUS_PER_SIZE`,
  `crates/evosim/src/world/tick.rs → graze`, `attack`, `energy_bookkeeping`.

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
  `crates/evosim/src/world/tick.rs → attack`, `AttackPick`.

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
- **Code anchors**: `crates/evosim/src/world/proximity.rs → proximity_starburst`,
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

### Saved worlds are versioned Rust-owned artifacts; no Hall of Fame

- **Decision**: Durable worlds use a versioned `evosim.world` JSON artifact
  produced and validated at the Rust wasm boundary. The artifact embeds
  resolved `WorldConfig`, identity/lineage metadata, runtime world state,
  compatibility flags, and telemetry policy metadata. It does not serialize
  every telemetry sample/event by default, and it does not create a Hall of
  Fame or cross-run lineage archive.
- **Why**: Resume/fork/import/export need the actual sim state, not a replay of
  app settings. Rust owns the fields that must round-trip and can validate
  schema, config/state compatibility, dimensions, topology, seeds, SoA column
  lengths, grass/biome byte lengths, species registry state, and population
  caps before a worker starts using the artifact.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `crates/evosim/src/world/mod.rs →
  WorldRuntimeStateV1`; `crates/evosim/src/wasm_api/mod.rs →
  WorldArtifactV1`, `WorldHandle::world_artifact_json`,
  `WorldHandle::new_from_artifact_json`.
- **Tradeoffs**: Brain weights, grass density, and biome bytes are compact
  base64 blobs inside JSON rather than separate binary sections. This keeps
  import/export simple and inspectable enough for fixtures, with a fixed worker
  transport cap instead of unbounded messages.
- **Revisit when**: deterministic science mode needs bit-for-bit cross-version
  replay, or telemetry history should be deliberately bundled with saved worlds.

### No-grass zones are generated from `world_seed` via a dedicated PRNG

- **Decision**: The static grass/no-grass-zone grid is generated from a
  **dedicated SplitMix64 PRNG seeded by `world_seed`**, independent of the
  string/XxHash64 sim RNG. The generator scatters a few large low-capacity
  blobs over a Plains background; distance is toroidal when `wrap_world`.
  The internal byte tags remain for saved-world and capacity compatibility, but
  the product model is binary: normal grass capacity vs no-grass/dead-grass
  zones.
- **Why**: A separate stream lets the map pin to `world_seed` alone (so
  it is reproducible / shareable) while the live run still varies on the
  sim RNG. Large blobs (not noise speckle) give creatures a navigable,
  legible landscape with distinct food-rich and food-poor regions. SplitMix64
  is trivial, fast, and never touches the sim RNG draw order.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/biome.rs → generate_biome_grid`;
  `crates/evosim/src/world/mod.rs`.
- **Tradeoffs**: Blob count/radius are balance knobs (constants in
  `biome.rs`); proportions are statistical, not exact per seed. The grid
  is `grass_cell_count` u8 — same size as the snapshot grass region.
- **Revisit when**: no-grass zones need finer structure, gradients, or more
  than one user-visible no-grass behavior.

### No-grass zones are food-only and construction-disableable

- **Decision**: No-grass zones carry **no movement, cost, or upkeep
  penalties**. The only surviving zone effect is the grass-capacity cap on
  scatter writes. The next-world setting `WorldConfig.grass.no_grass_zones`
  can disable the effect; disabled worlds construct an all-plains capacity and
  visual grid, so grass can grow normally everywhere and opacity cannot reveal
  a dead-zone pattern.
- **Why**: Movement tax + genome modulation coupled biome selection and body
  evolution, muddying the learning signal. With the genome gone and no penalty,
  zones do exactly one thing — absence of food — which is the cleanest possible
  selection pressure. Avoidance remains learnable through grass-sector and
  current-grass inputs. Disabling the zones is construction-only because
  capacity arrays, zone bytes, and snapshot pyramid state are built from world
  dimensions at construction.
- **Applies to**: `architecture/biome.md`, `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/biome.rs → capacity_factor_from_u8`;
  `crates/evosim/src/grass/mod.rs → compute_propagation_scatter` (the cap write);
  `crates/evosim/src/constants.rs → GRASS_CAPACITY_PLAINS`,
  `GRASS_CAPACITY_WATER`, `GRASS_CAPACITY_DESERT`.

### No-grass zones have no direct NN type input

- **Decision**: The active NN layout has no direct zone-type input. Zones
  influence the brain only through grass-sector, far-grass, and current-grass
  density inputs.
- **Why**: Zones now only change grass carrying capacity. A raw zone id is a
  redundant proxy for the food signal and makes the learning surface wider
  without adding an effect the creature can act on directly.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/nn.rs → NnInputLayout::for_settings`,
  `build_nn_input`; `crates/evosim/src/world/proximity.rs →
  compute_grass_density_sectors`, `compute_grass_far_band_sectors`.
- **Revisit when**: a new zone effect other than grass capacity is added.

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
- **Code anchors**: `crates/evosim/src/world/mod.rs`, `crates/evosim/src/world/nn.rs`,
  `crates/evosim/src/world/tick.rs`, `crates/evosim/src/world/species.rs`,
  `crates/evosim/src/wasm_api/mod.rs`.
- **Code anchors**: `crates/evosim/src/world/mod.rs → DevSliders`, `World`;
  `crates/evosim/src/world/species.rs → SpeciesRegistry`;
  `crates/evosim/src/creature.rs → CreatureSoA`.

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
- **Code anchors**: `crates/evosim/src/world/mod.rs → World::handle_mating`,
  `crates/evosim/src/world/nn.rs → ActionGate`, `decode_action`, `is_valid_action`,
  `crates/evosim/src/world/tick.rs → energy_bookkeeping` (cooldown decrement).
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
- **Code anchors**: `crates/evosim/src/brain/mod.rs → Brain::child_from_crossover_with_sigma`,
  `crates/evosim/src/world/mod.rs → World::handle_mating`.
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
- **Code anchors**: `crates/evosim/src/world/tick.rs → attack`.
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
- **Code anchors**: `crates/evosim/src/world/species.rs → SpeciesRegistry`, `SPECIES_PALETTE`;
  `crates/evosim/src/brain/mod.rs → Brain::founder_spread_with_sigma`;
  `crates/evosim/src/world/mod.rs → pick_anchors`, `World::new_with_sliders_topology`
  (seeding branch); `crates/evosim/src/wasm_api/mod.rs → fill_creature_bytes` (species color lane).
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
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → species_table_json`;
  `crates/evosim/src/control_sab.rs` (slots + byte window);
  `web/src/sim/worker.ts → maybeWriteSpeciesTable`;
  `web/src/generated/control-sab.ts` (regenerated).

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
- **Code anchors**: `crates/evosim/src/constants.rs → MATING_COOLDOWN_TICKS_DEFAULT`,
  `SPLIT_GIFT_MAX_DEFAULT`, `STARTING_SPECIES_MEMBER_COUNT_DEFAULT`,
  `STARTING_SPECIES_COUNT_DEFAULT`.
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
  parents' positions, even if that cell is a food-poor no-grass zone. No retry,
  no nearest-habitable search.
- **Why**: Simple, and honest selection — a child spawning in a food-poor zone
  is exposed to the same selection pressure as any other creature. A
  habitable-cell search would add a scan and quietly mask cross-zone mating
  as a failure mode.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/mod.rs → World::handle_mating`.

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
- **Code anchors**: `crates/evosim/src/world/mod.rs → World`;
  `crates/evosim/src/world/species.rs → SpeciesRegistry`;
  `crates/evosim/src/wasm_api/mod.rs → creature_inspect_json`.
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
- **Code anchors**: `crates/evosim/src/creature.rs → Action`;
  `crates/evosim/src/world/nn.rs → decode_action`.

### NN input layout is settings-derived; no vestigial slots; one bias slot

- **Decision**: The input vector composition is computed at world init from the
  active construction settings (`NnInputLayout::for_settings`) and SIMD-padded
  to a multiple of 8. No slot is preserved for compatibility — inputs constant
  for a brain's whole run (the dropped "action-set / mode indicator" slot, the
  always-zero wall inputs in wrap mode) collapse to a per-output bias and are
  removed. One slot stays pinned to `1.0` as the classical bias-learning
  constant (distinct from the dropped mode flag). Artifact load validates the
  saved topology against the embedded construction config instead of migrating
  arbitrary settings changes onto old brains.
- **Why**: A constant input wastes weights and trains against a dishonest
  surface. Mode information that matters (8 vs 16 creature sectors, present vs
  absent wall inputs) is carried *implicitly* by the layout, so a separate
  "are we in species mode" input buys nothing. The bias-learning constant earns
  its slot (every hidden unit gets a learnable threshold) and is standard
  practice. (The full 8-way width table across wrap×species×multisight —
  32/32/40/40/40/40/48/48 — lives in `architecture/simulation-core.md`; the
  cross-language safety implication is in `decisions/cross-cutting.md`.)
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/nn.rs → NnInputLayout`,
  `NnInputGroup`; `crates/evosim/src/brain/mod.rs → NnTopology`.
- **Alternatives**: keep a constant action-set-indicator input for
  "portability" (rejected — a brain selected against one `action[2]` semantic
  doesn't transfer just because one constant signal says which mode it's in);
  directional zone/penalty sectors (rejected — penalties are gone and grass
  density carries the remaining zone effect).

### Body radius and repulsion: constants; sprite/repulsion not collision

- **Decision**: Body radius, repulsion strength, and all creature mechanics
  previously modulated by genome traits are now constants. `body_size` no
  longer exists as a trait; the constant radius scales sprite + repulsion
  strength, **not** collision (there is no collision).
- **Why**: Removing per-creature body-size variance eliminates one axis of
  non-brain variation from the learning signal. Repulsion still approximates
  "creatures own space" without requiring per-creature spatial tracking.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/constants.rs` (body radius constant);
  `crates/evosim/src/world/tick.rs` (repulsion use site).

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
- **Code anchors**: `crates/evosim/src/grass/mod.rs → GrassGrid`,
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
- **Code anchors**: `crates/evosim/src/grass/mod.rs → compute_propagation_scatter`,
  `compute_propagation_blur`, `GrassPropagation`, `DiscTable`,
  `ScatterParams`, `GrassGrid::scatter_params`;
  `crates/evosim/src/constants.rs → GRASS_DECAY_PCT_DEFAULT`, `GRASS_DECAY_AMOUNT_DEFAULT`,
  `GRASS_SPREAD_PCT_DEFAULT`, `GRASS_SPREAD_AMOUNT_DEFAULT`,
  `GRASS_SPREAD_RING1_PCT_DEFAULT`, `GRASS_SPREAD_RING2_PCT_DEFAULT`,
  `GRASS_SPREAD_RING3_PCT_DEFAULT`, `GRASS_SPREAD_RADIUS`.
- **Revisit when**: the blur deletion decision is made by the lead (the
  bench is the trigger).

### Threaded scatter stays nondeterministic by default; science mode is exact for covered fixtures

- **Decision**: Normal scatter keeps the existing relaxed cross-tile RMW path
  and remains the default. Under `--features threads` and the live wasm sim,
  normal-mode grass density is intentionally non-reproducible run-to-run.
  `WorldConfig.science.deterministic = true` selects an opt-in science/replay
  path that reads the same tick-start `scatter_prev`, accumulates decay/add
  deltas into owned per-cell scratch buffers, and commits cells in ascending
  order. The covered contract is exact state/hash equality for the same
  seed/config/artifact and same app version across `cargo test --lib` and
  `cargo test --lib --features threads`; the fixture hash is owned by
  `crates/evosim/src/world/science_mode_tests.rs`.
- **Why**: The live app values throughput and statistical grass behavior more
  than bit-exact replay, while science/regression workflows need a stable
  comparison target. Keeping the mode opt-in preserves the normal hot path and
  lets saved artifacts advertise whether future replay is expected to be exact.
- **Tradeoffs**: Science mode adds full-grid delta scratch and an ordered
  reduction/commit. The measured 512², 6.25%-seeded threaded native fixture was
  faster than normal relaxed scatter, but that does not prove every large or
  dense workload is faster. The UI labels it as science/replay mode and warns
  that it can reduce throughput. The promise excludes normal mode,
  profiler/telemetry timing, browser event-loop timing, future app versions,
  and arbitrary hardware/browser floating-point identity beyond the covered
  native fixtures.
- **Alternatives considered**: tile-local spread only (rejected because it
  shrinks the effective disc radius), per-tile inbox merge (viable but more
  complex than needed for the local scratch reducer), and CAS-add loops
  (rejected because dense-frontier contention would make the write path heavy).
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/testing.md`.
- **Code anchors**: `crates/evosim/src/grass/mod.rs → compute_propagation_scatter_science`,
  `GrassGrid::set_deterministic_science_mode`;
  `crates/evosim/src/world/mod.rs → DevSliders`;
  `crates/evosim/src/world/science_mode_tests.rs → deterministic_science_mode_is_default_off`;
  `crates/evosim/src/wasm_api/mod.rs → WorldConfig`,
  `world_artifact_json`;
  `web/src/widgets/devpanel.ts → currentWorldConfig`.

### GRASS_EQ_EPS = 1/255 (one u8 quantum)

- **Decision**: `GRASS_EQ_EPS` (the epsilon below which a cell is
  classified EMPTY and above `cap − EPS` is classified SATURATED) is
  `1.0 / 255.0` — exactly one u8 quantization step.
- **Why**: The original value of `1e-4` was 39× smaller than one u8
  quantum (1/255 ≈ 3.92e-3). Low-capacity zone cells could round-trip below
  their f32 cap without classifying SATURATED, leaving those tiles permanently
  MIXED and preventing the frontier skip. Raising to exactly one u8 step means
  any cell whose f32 value round-trips to within one quantum of `cap − EPS` is
  correctly classified saturated, restoring the frontier skip for
  zone-capped cells.
- **Applies to**: `architecture/simulation-core.md`,
  `crates/evosim/src/grass/mod.rs → GRASS_EQ_EPS`.

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
- **Code anchors**: `crates/evosim/src/constants.rs → GRASS_BITES_PER_BLOCK_DEFAULT`;
  `crates/evosim/src/wasm_api/mod.rs → SLIDER_NAMES` (indices 54–59), `try_set_slider`;
  `crates/evosim/src/world/tick.rs → graze`;
  `crates/evosim/src/grass/mod.rs → GrassGrid::consume`.

### Grass pyramid: u8 box-filter mip clipmap over the density field

- **Decision**: `GrassPyramid` is a u8 box-filter mip pyramid over
  `GrassGrid`. L0 aliases the live `density` field (not stored in the
  pyramid). L1+ are owned, MEAN-downsampled, edge-clamped u8 buffers.
  Each Lk cell is the arithmetic mean of its (up to) 4 in-bounds Lk-1
  parent cells: `(sum + count/2) / count` (round-before-truncate). At the
  default runtime `grass_dim = 480` this yields enough levels before reaching 1×1
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
  `TODO(stage3-pyramid-perf)` anchor in `crates/evosim/src/grass/mod.rs`.
- **Code anchors**: `crates/evosim/src/grass/mod.rs → GrassPyramid`,
  `GrassGrid::refresh_pyramid`;
  `crates/evosim/src/constants.rs → GRASS_PYRAMID_MAX_LEVELS`,
  `GRASS_PYRAMID_REFRESH_PERIOD`;
  `crates/evosim/src/world/mod.rs → World::step`.

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
- **Constraint**: without direct zone-type inputs, all
  wrap×species×multisight combinations fit within `MAX_NN_INPUTS`; no
  fallback is needed. The widest current layout is walled+species+multisight
  at 46 real slots → 48 padded.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/nn.rs → NnInputGroup::GrassBandsFar`,
  `NnInputLayout::for_settings`; `crates/evosim/src/world/proximity.rs →
  compute_grass_far_band_sectors`; `crates/evosim/src/constants.rs → GRASS_FAR_SIGHT_RADIUS`,
  `GRASS_FAR_MIP_LEVEL`.
- **Revisit when**: band count, radii, or mip level need retuning; or the
  MAX_NN_INPUTS ceiling becomes a bottleneck for future input groups.

### Fused RNG for scatter kernel (grass_hash_fused_4)

- **Decision**: The scatter kernel replaces 4 separate `grass_hash_u64` calls
  (salts 1–4) with `grass_hash_fused_4` (`crates/evosim/src/rng.rs`): 2 `grass_hash_u64`
  words, bit-sliced into non-overlapping windows for decay/spread/band/pick.
  The property "grass never perturbs `SimRng`" is preserved; distribution
  statistics match within tolerance (tested in `crates/evosim/src/grass/tests/fused_rng.rs`).
- **Why**: Attribution bench (S0) measured RNG as the dominant per-cell cost at
  dense fill (3.05 ns/cell, 47% of 6.42 ns/cell total). Fusing to 2 calls saves
  ~0.86 ns/cell at dense fill. Non-overlapping bit windows avoid cross-salt
  correlation while sharing the two-hash work.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/rng.rs → grass_hash_fused_4`; `crates/evosim/src/grass/mod.rs →
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
  tighter, more organic-looking clumps. The persistence invariant (normal zones
  super-critical, low-capacity zones sub-critical at edges) is preserved (tested in
  `crates/evosim/src/grass/tests/fertility.rs`). The 0.04 decay default is **feel-tunable** —
  the lead should confirm it reads well in the browser.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/grass/mod.rs → compute_propagation_scatter`
  (the `(spread_byte * s + 127) / 255` add_byte line);
  `crates/evosim/src/constants.rs → GRASS_DECAY_PCT_DEFAULT`.

### grass_size slider: configurable cell size as perf lever

- **Decision**: `GRASS_CELL_SIZE` is no longer compile-time only — it is also
  a **construction slider** (`grass_size`, idx 62, range 5–20u, step 1, default
  5.0). The value flows via `WorldDims::from_world_size_with_cell_size`; changing
  it resizes `grass_dim`, `grass_cell_count`, `capacity[]`, the biome grid, and
  the snapshot slot. Restart/full-world-rebuild scoped.
- **Why**: Cell count scales ~1/size², so a larger cell is the bluntest available
  perf lever — reducing grass_step work without architectural change. It is the
  principled form of "use less grass" and pairs with the LOD/scale resolution theme.
- **Tradeoffs**: `LUT_RADIUS = 4` (in `crates/evosim/src/world/proximity.rs`) is pinned to the
  5u default — a safe over-approximation across the full 5–20u range. The grass
  `bilinear_sample` + circle-overlap uses the runtime `grass_cell_size`; so do
  `biome.rs` and the `render/gl.ts` UV transform (all
  previously hard-coded 5.0).
- **Applies to**: `architecture/simulation-core.md`, `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/constants.rs → WorldDims::from_world_size_with_cell_size`,
  `GRASS_CELL_SIZE_DEFAULT`; `web/src/render/gl.ts → renderWorldImpl`
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
- **Code anchors**: `crates/evosim/src/grass/mod.rs → compute_propagation_blur`,
  `blur_at`, `GrassPropagation`; `crates/evosim/src/grass/tests/v201.rs`.
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
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → WorldHandle::write_snapshot` (the source
  reads: creature SoA borrow, `grass.density`, `grass.pyramid`).
- **Revisit when**: a new dominant `write_snapshot` cost emerges that cannot
  be eliminated in-place, AND a double-buffer design is available that provably
  costs less than the source read. (The prior dominant cost — the biome-window
  recompute — was eliminated via `BiomePyramid` precomputation; snapshot write
  is now 2.35 ms.)

### Seeded grass clumps: initial-budget choice

- **Decision**: Boot grass with dense seeded clumps using
  `GRASS_CLUMP_COUNT_DEFAULT` centres and `GRASS_CLUMP_SIZE_DEFAULT` radius,
  with centres derived deterministically from `world_seed` via the
  position-addressable hash RNG (independent of the sim RNG so creature draw
  order is unaffected).
- **Budget**: The current defaults are intended to cover most of the default
  runtime grass grid. Clumps may overlap and are capped by the runtime grid; the
  old `GRASS_INITIAL_SEED_COUNT_DEFAULT = 8000` path remains the fallback when
  `grass_clump_count = 0`.
- **Why**: Uniform single-cell scatter looks like dust — no visible patches, no
  spatial texture for creatures to navigate. Dense clumps start the default world
  with visible contiguous grass patches.
- **Reproducibility**: Clump centres are derived from
  `grass_hash_u64(world_seed, clump_index, tick=0, salt=0/1)` — the same hash
  used by the scatter kernel but never during propagation. Same `world_seed`
  → same centres every boot; this is separate from the sim-string RNG so creature
  draws are unaffected.
- **Boot-seam constraint**: `grass_clump_count` and `grass_clump_size` are
  construction-only and ride the boot `WorldConfig` (same rule as
  `grass_cell_size`) — `initial_sliders` is applied AFTER world construction
  and cannot re-seed.
- **Fallback**: `grass_clump_count = 0` falls back to the old uniform-scatter path
  (`grass_initial_seed_count` cells uniformly at random). No other code path changed.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/grass/mod.rs → GrassGrid::seed_clumps`;
  `crates/evosim/src/world/mod.rs → new_with_sliders_topology`;
  `crates/evosim/src/constants.rs → GRASS_CLUMP_COUNT_DEFAULT`, `GRASS_CLUMP_SIZE_DEFAULT`.
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
  (`BiomePyramid::build`) from the static `biome_grid`. Each call to
  `write_snapshot` copies a pre-downsampled biome window from the pyramid via
  `BiomePyramid::copy_window`. The pyramid is invalidated only on world
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
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → BiomePyramid`,
  `BiomePyramid::build`, `BiomePyramid::copy_window`, `WorldHandle::write_snapshot`;
  profiler span `write_output_sab.snapshot.biome`.
- **Revisit when**: biome grid becomes dynamic (live biome editing); at that point
  the pyramid must be rebuilt on mutation, or the per-tick recompute reinstated.

## See also

- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`cross-cutting.md`](cross-cutting.md)
