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
  `set_split_threshold` / etc. exports. Bools (only `auto_curriculum`
  today) ride the same path as `0|1` via the `try_set_slider`
  `"auto_curriculum" => apply_auto_curriculum(value != 0.0)` arm.
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

### Creature color is per-creature action EMA, not NN-weight hash

- **Decision**: Three EMA channels track Graze argmax / successful
  bites / Split intent → green / red / blue. Display floor 0.15 per
  channel.
- **Why**: A glance shows behaviour, not identity. NN-weight hash
  colors were stable per lineage but gave no signal about what a
  creature was *doing*.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `src/world/mod.rs → color_ema_update`,
  `CreatureSoA::color_r / color_g / color_b`.

### No save/load, no species tracking, no events, no Hall of Fame

- **Decision**: Every page load is a fresh world. No persistence, no
  species registry, no event log, no eulogy.
- **Why**: All four cost surface area without delivering observable
  benefits. Removing them lets the rest of the system get smaller.
- **Applies to**: `architecture/simulation-core.md`.
- **Revisit when**: a use case appears that genuinely needs durable
  per-creature identity or cross-session continuity.

## See also

- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`cross-cutting.md`](cross-cutting.md)
