# Decisions — performance

Performance decisions: what we measured, what we shipped, and what we parked.
Each entry records a durable rationale — the measured finding or constraint that
drove the choice — so future optimization passes start from facts, not guesses.

---

### Spatial grid: 20u cells and one rebuild per tick

- **Decision**: Keep `HASH_CELL = 20` and rebuild the spatial grid once at the
  start of each tick.
- **Why**: The native tick profile showed three `hash_dim²` rebuilds dominating
  serial cost in the sparse default world. Coarsening the grid and removing the
  two movement-time rebuilds reduced combined grid plus movement cost from about
  2995µs to 404µs per tick (7.4×).
- **Tradeoffs**: Post-movement interaction queries use stale buckets and can
  miss an interaction for one tick; exact-distance checks prevent false hits.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/constants.rs → HASH_CELL`;
  `crates/evosim/src/world/tick.rs → World::apply_movement_and_repulsion`;
  `crates/evosim/benches/tick_profile.rs → make_handle`.
- **Revisit when**: density, movement range, or interaction fidelity makes the
  stale-bucket tradeoff unacceptable.

### Grass pyramid refresh is cadence-gated

- **Decision**: Refresh grass-pyramid L1+ every
  `GRASS_PYRAMID_REFRESH_PERIOD = 4` ticks.
- **Why**: Full pyramid refresh became a dominant serial cost after the grid
  work; cadence gating reduced its dense-regime amortized cost from about
  3675µs to 870µs (4.2×), while default zoom continues to read live L0.
- **Tradeoffs**: Zoomed-out render and far-grass NN sensing can lag by at most
  the refresh period.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/constants.rs → GRASS_PYRAMID_REFRESH_PERIOD`;
  `crates/evosim/src/world/mod.rs → World::step`;
  `crates/evosim/benches/tick_profile.rs → make_handle`.
- **Revisit when**: stale far-grass sensing is observable or a dirty-subtree
  refresh makes per-tick freshness cheap.

### Grass scatter kernel: RNG dominates at dense fill; freeze floor is irreducible

- **Decision**: At dense (~90%) fill the 4-hash RNG cost is 3.05 ns/cell (47% of
  6.42 ns/cell total), comparable to the 2.90 ns/cell RMW atomic cost. The freeze
  floor (reading every tile cell into a local buffer) is 3.08 ns/cell (48%) and is
  **irreducible** with the current tile-freeze design — it is O(tile) regardless of
  fill. The fused-RNG optimization (C3, `grass_hash_fused_4`) ships as a result:
  2 hash words bit-sliced into non-overlapping windows, saving ~0.86 ns/cell at
  dense fill. No further per-cell RNG reduction is available without restructuring
  the freeze loop.
- **Why**: Attribution bench (`crates/evosim/benches/grass_attribution.rs`) measured
  the incremental cost of each lever at 6.25% and ~90% fill on a 512² grid (256
  tiles all-active). The freeze floor at dense fill (3.08 ns/cell) equals the RNG
  cost — meaning even perfect RNG elimination would only halve the total. Fused RNG
  is the correct lever given the data; the freeze floor is the barrier to further
  progress without an architectural change (event-sampling, cadence gating).
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/rng.rs → grass_hash_fused_4`;
  `crates/evosim/src/grass/mod.rs → compute_propagation_scatter` (the 2-hash dispatch);
  `crates/evosim/benches/grass_attribution.rs → bench_attribution`.
- **Revisit when**: the event-sampling refactor (see deferred entry below) is in
  scope and the freeze loop is restructured around a compact live-cell list — only
  then does reducing the per-cell cost below the freeze floor become meaningful.

### Geometric-skip (C4) dropped: net harm at both fill levels

- **Decision**: The geometric-skip spread sampler (C4) is **not shipped** and will
  not be revisited unless the underlying architecture changes. At 6.25% fill it
  adds +0.49 ns/cell; at ~90% fill it adds +18.7 ns/cell (25.1 vs 6.42 total).
  The only path where skip pays is restructuring the entire freeze loop around an
  event list (event-sampling, perf-optimization-ideas.md idea #3), which requires
  a compact live-cell index — a separate, larger decision.
- **Why**: The compact-list build is O(grassy cells), and the freeze+decay O(tile)
  floor is untouched by skip — skip only removes the spread-gate hash, not the
  freeze read of every cell. At high fill the compact-list overhead dominates.
  The S0 attribution bench verdict is unambiguous.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/benches/grass_attribution.rs → variant_geom_skip`,
  `bench_attribution`.
- **Revisit when**: the event-sampling refactor is in scope and the freeze loop is
  restructured so geometric-skip is applied to an already-sparse event list
  rather than a full tile scan.

### Deterministic science scatter stays opt-in despite a faster covered fixture

- **Decision**: `WorldConfig.science.deterministic` remains opt-in; the normal
  relaxed scatter path stays the default. `cargo bench --no-run` is part of the
  ship gate, and the focused threaded native bench command
  `cargo bench --features threads --bench grass_scatter -- grass --warm-up-time 1 --measurement-time 2`
  measured the covered 512², 6.25%-seeded fixture.
- **Measurement**: Criterion reported `grass_propagation_scatter` at
  2.2895 ms mean and `grass_propagation_science` at 1.6486 ms mean. The
  `grass_full_tick/scatter` mean was 2.1559 ms; `grass_full_tick/science` was
  1.5486 ms. On this fixture the deterministic reducer was faster, likely
  because it avoids contended relaxed atomics.
- **Why**: One fixture is not enough evidence to replace the default. Science
  mode still clears full-grid delta scratch and performs an ordered reduction,
  so large, dense, sparse-frontier, and browser-wasm regimes may have different
  costs. Keeping the warning honest is safer than generalizing from the native
  bench.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/profiler.md`.
- **Code anchors**: `crates/evosim/src/grass/mod.rs → compute_propagation_scatter_science`;
  `crates/evosim/benches/grass_scatter.rs → bench_grass_propagation`.

### Snapshot worker parked: staging copy is the dominant cost

- **Decision**: `WorldHandle::write_snapshot` runs on the sim worker thread.
  A separate dedicated snapshot worker is **parked** and may not be unparked
  unless the measured snapshot cost rises above ~2.35 ms AND is shown to be
  irreducible in place.
- **Why**: `write_snapshot` reads live `World` state by borrow — creature SoA
  columns, the grass `density` field (mutated every tick via atomic RMW), and the
  cadence-refreshed mip `pyramid`. Any off-thread split requires one of:
  (a) double-buffering all hot `World` state — copies more bytes than it saves
  because the staging copy IS the dominant cost; (b) a per-tick barrier after
  every `step_n` — gives back the rayon parallelism gain plus a sync tax;
  (c) staging a source copy on the sim thread — same dominant cost as (a). The
  `BiomePyramid` precompute eliminated the prior dominant cost (biome
  recompute was 83% of the 13.74 ms total); snapshot write is now **2.35 ms**,
  dominated by the grass-pyramid memcpy (~2.2 ms). That remaining cost is a
  bounded memcpy reducible in the existing worker (tighter LOD budget, async copy)
  without a second worker thread. Restart also becomes materially fragile under a
  split (boot-generation epoch, paired worker teardown, second error channel).
- **Alternatives considered**: dedicated JS SAB destination — rejected because
  moving the destination is irrelevant when the source read dominates and must
  stay on the sim thread; staged copy-out protocol — same dominant cost as (a).
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → write_snapshot`
  (source reads: creature SoA borrow, `grass.density`, `grass.pyramid`);
  `crates/evosim/benches/snapshot_write.rs → bench_snapshot_write_full` (the benchmark that measured the
  13.74 ms → 2.35 ms speedup).
- **Revisit when**: a new measured dominant cost in `write_snapshot` emerges that
  cannot be eliminated in place AND a double-buffer design is available that
  provably costs less than the source staging copy. The unpark bar is: snapshot
  write > ~2.35 ms irreducible in the existing worker.

### Snapshot output is back-pressured by consumed seq

- **Decision**: Main stores the last painted `CTRL_SEQ` in
  `CTRL_CONSUMED_SEQ`; the worker writes at most one fresh unconsumed snapshot
  while continuing to tick and serve SAB reports. App FPS caps expensive main
  paint work separately from target TPS.
- **Why**: High target TPS should advance the sim, not force the worker to
  copy grass/creature/biome windows that main cannot display. The e2e
  `app-fps.spec.ts` keeps the regression concrete: at target TPS 1000 and
  App FPS 15, snapshot seq growth stays near the render cap while reported
  TPS remains above the app FPS.
- **Tradeoffs**: Intermediate sim states are intentionally skipped by the
  renderer. The renderer consumes the newest published snapshot; historical
  snapshots are never queued.
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/control_sab.rs → CTRL_CONSUMED_SEQ`;
  `web/src/sim/worker.ts → maybeWriteSnapshotToSAB`;
  `web/src/main.ts → frame`.

### Telemetry history is fixed-cadence and exported on request

- **Decision**: Keep longitudinal telemetry in bounded in-memory rings:
  aggregate samples every fixed tick period, a capped low-cardinality event
  log, and a worst-jank summary. Serialize the full telemetry payload only when
  main requests export through the telemetry SAB response buffer.
- **Why**: The profiler already handles short-window diagnostics; per-frame or
  cadence-written full-history JSON would add avoidable worker cost and
  pressure the report buffers. Fixed-cadence samples bound memory and
  CPU, while request-only export keeps steady-state cost to cheap aggregate
  sampling.
- **Tradeoffs**: CSV/JSON export can be a few seconds stale only until the
  request is served on the next worker loop iteration. Phase attribution for
  worst jank is deliberately `unknown` because the available profiler data is a
  rolling aggregate, not a per-jank-tick trace.
- **Applies to**: `architecture/profiler.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/worker-runtime.md`,
  `architecture/app-shell.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → telemetry_report_json`;
  `crates/evosim/src/control_sab.rs → TELEMETRY_REPORT_CAP`;
  `web/src/sim/worker.ts → serveTelemetryRequest`;
  `web/src/sim/bridge.ts → requestTelemetryReport`;
  `web/src/widgets/perf-panel.ts → exportTelemetry`.

### Static biome texture uploads are metadata-cached

- **Decision**: The renderer skips biome `texSubImage2D` when the published
  window/LOD/texture metadata and wasm-memory buffer identity match the last
  upload. Grass still uploads each painted frame.
- **Why**: Biome bytes are static after boot, but grass density changes as the
  sim runs. Caching by window metadata removes repeated static uploads without
  risking biome/grass UV misalignment after pan, zoom, LOD, wrap, or restart.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render/gl.ts → renderWorldImpl`.

### Velocity lanes remain deferred

- **Decision**: Do not add velocity or previous-position lanes to the snapshot
  layout. Measure the current id-keyed trail map path under
  `frame.render_world.creatures.trail_state` first.
- **Why**: Snapshot decimation and biome-upload caching reduce the larger
  repeated work without widening the hot protocol. The trail map now has a
  named profiler row, so a later change can justify a layout expansion from
  measured dominance rather than assumption.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/profiler.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render/gl.ts → renderWorldImpl`.

### Grass `grass_step` cadence/visibility gating: deferred as the next 10× lever

- **Decision**: Cadence/visibility-gated `grass_step` (idea #1 in
  `perf-optimization-ideas.md`) is explicitly **deferred**. It is the identified
  next 10× lever for grass propagation at scale and is not yet built.
- **Why**: At 14.7M fully-grassed cells, `grass_step` is memory-bandwidth-bound
  touching every occupied cell; the per-cell path is already optimal (fused RNG,
  stack freeze, active-tile frontier). The active-tile frontier gives little relief
  once decay keeps every grass-bearing tile permanently active. Cadence gating
  (process a tile every Nth tick, batch N decay/spread rolls into one
  rate-preserving roll) is the only behavior-neutral lever that beats the O(N)
  floor without a fidelity trade. The current decision is to keep pyramid
  cadence separate from `grass_step` cadence and revisit event-sampling only
  when a larger grass-step reduction is needed.
- **Applies to**: `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/grass/mod.rs → compute_propagation_scatter`
  (the current per-tick full-pass path).
- **Revisit when**: grass-step wall time becomes the dominant TPS constraint at
  the user's target world size — the first lever to reach for is cadence/throttle
  (batch N ticks of decay/spread into one rate-preserving roll, keyed off tile
  frontier + creature proximity + camera visibility).

## See also

- [`../architecture/simulation-core.md`](../architecture/simulation-core.md) — sim architecture this constrains.
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md) — worker loop and snapshot write path.
- [`sim.md`](sim.md) — sim-domain decisions (grass scatter, BiomePyramid, fused RNG entries).
- [`index.md`](index.md) — decisions index and domain map.
- [`../plans/perf-optimization-ideas.md`](../plans/perf-optimization-ideas.md) — the long-lived perf backlog (parked ideas, measured verdicts).
- [`~/agent-docs/v1/rules/authoring-rules.md`](~/agent-docs/v1/rules/authoring-rules.md) — doc maintenance rules.
