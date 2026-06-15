# Decisions — profiler

---

### Five sibling top-level trees, not one tree rooted at `tick`

- **Decision**: `frame`, `sim_worker`, `tick`, `nn`, `grass_step` are independent
  trees displayed as stacked tables in the perf panel.
- **Why**: They mean different things. `frame` is RAF wall-clock on main;
  `sim_worker` is worker-loop wall-clock in TypeScript; `tick` is Rust
  per-tick wall-clock; `nn` and `grass_step` are *sum-busy across rayon
  workers* for the parallel phases. Mixing them under one root would force
  readers to compare apples to oranges and would hide the
  `nn.total > tick.nn.total` relationship that signals rayon is working.
- **Applies to**: `architecture/profiler.md`,
  `architecture/render-pipeline.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/profiler.rs → ProfilerInner`,
  `ProfilerInner::ensure_root`, `Profiler::record_under_root`;
  `web/src/widgets/perf-panel.ts → TREE_ORDER`.

### No rollups — every row is a real timer pair

- **Decision**: No row's `total_us` is computed from its children.
  Every node (parents included) has its own `clock_now_us` bracket
  around the work it claims to represent.
- **Why**: A parent whose total comes from summing children can never
  surface loop overhead, dispatch overhead, or skipped phases. Real
  parent measurements give `parent_total - sum(children_total)` as a
  positive "overhead" number you can act on.
- **Applies to**: `architecture/profiler.md`.
- **Code anchors**: `crates/evosim/src/profiler.rs → SpanGuard` (RAII drop records
  to the node's own ring), `Profiler::record_under_root`.

### `tick.nn` and `tick.grass_step` are LEAVES in the `tick` tree

- **Decision**: These two `tick.*` rows carry only the main sim
  worker's wall-clock wait for the rayon dispatch — nothing deeper.
  Their breakdown lives only in the sibling top-level `nn` and
  `grass_step` trees with sum-across-workers semantics.
- **Why**: Putting the breakdown inside `tick` would imply that the
  child totals sum to the parent — which is false for parallel work.
  Two separate trees make the semantic split visible.
- **Applies to**: `architecture/profiler.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/mod.rs → step` (the
  `tick.grass_step` and `tick.nn` profile_span! call sites + the
  `record_under_root("grass_step", ...)` calls).

### Per-worker atomic accumulators for sum-busy parents AND leaves

- **Decision**: In the worker-sum trees, both parent rows and leaf
  rows have their own `AtomicU64` accumulators. The parent atomically
  brackets the entire parent operation across workers; the leaves
  atomically bracket the sub-operations inside.
- **Why**: Without parent accumulators, the no-rollup rule can't hold
  for parallel phases — there's no Rust-side RAII span to use because
  the parent runs in worker threads, not the main sim worker. Atomic
  brackets work because the accumulator is summable (commutative).
- **Applies to**: `architecture/profiler.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/nn_stats.rs → NnStats`,
  `NnStats::record_chunk_subphases`, `NnStats::record_chunk_lite`;
  `crates/evosim/src/grass/mod.rs → GrassGrid`.

### `grass_step.dispatch` collapses `par_chunks` vs `chunks_mut`

- **Decision**: `par_chunks_us` (threaded build) and `chunks_mut_us`
  (sequential build) are mutually exclusive — only one is non-zero per
  tick. They sum into a single `grass_step.dispatch` row in the panel
  regardless of the active build.
- **Why**: Different build configurations measuring the same conceptual
  span should show the same row name. Forcing the reader to know which
  one is active would be a footgun.
- **Applies to**: `architecture/profiler.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/world/mod.rs → step` (the
  `record_under_root("grass_step", "dispatch", par + chunks_mut)`
  call).

### TS-side `frame` mirror; stitched in by the panel

- **Decision**: `web/src/perf.ts` maintains its own independent
  `frame` tree with the same ring-buffer + JSON shape as the Rust
  profiler. The perf panel concatenates the two reports before
  rendering.
- **Why**: `web_sys::window().performance` round-trips would add
  unnecessary cost per RAF, and the renderer doesn't need wasm. A TS
  mirror that obeys the same shape rules is the cheapest way to
  unify main-thread frame timings with the Rust-backed trees in display.
- **Applies to**: `architecture/profiler.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/perf.ts → span`, `reportJson`,
  `setProfilerEnabled`.

### `span("frame")` at the empty stack attaches to the root

- **Decision**: When the TS stack is empty and the caller opens
  `span("frame")`, the sample attaches to node 0 (the pre-seeded
  `frame` root) rather than minting a `frame.frame` child.
- **Why**: The outer RAF callback span is what every other `frame.*`
  child nests under. Without the special-case, the panel would show a
  duplicate `frame.frame` row whose total equals the outer span.
- **Applies to**: `architecture/profiler.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/perf.ts → span` (the
  `stack.length === 0 && name === "frame"` branch).

### Worker snapshot write spans live under `sim_worker`

- **Decision**: Snapshot write timing is recorded into the always-on Rust
  profiler as `sim_worker.write_output_sab.snapshot` and its children, not
  through worker-local TS `span()` state.
- **Why**: The worker's TS perf module is separate from main's. Publishing
  the JS-measured wall time through `WorldHandle::record_profile_sample` and
  Rust-side `record_under_root` keeps the row visible in the same report as
  the Rust tick, NN, and grass trees without adding a separate worker-to-main
  perf channel.
- **Applies to**: `architecture/profiler.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/sim/worker.ts → simLoop`;
  `crates/evosim/src/wasm_api/mod.rs → WorldHandle::record_profile_sample`,
  `WorldHandle::write_snapshot`.

### Per-worker `_calls` atomics paired with every `_us` accumulator

- **Decision**: Every per-tick sum-of-workers `_us` accumulator in
  `NnStats` and `GrassGrid` has a paired `_calls: AtomicU64` that
  workers `fetch_add` in lockstep with the timing. The post-tick drain
  reads both and passes them to `record_under_root`.
- **Why**: The drain records one sample per tick into the ring, so the
  legacy sample-count `call_count` measured "ticks where the phase
  fired" not the underlying per-creature / per-row invocation count.
  Paired atomics are the smallest change that carries the truthful
  count from the worker that owns it through to the panel's `ms/call`
  divisor. Without them, per-creature rows such as `nn.forward.l1` read as
  millisecond per-tick cost and hide the real microsecond per-creature shape.
- **Applies to**: `architecture/profiler.md`,
  `architecture/simulation-core.md`.
- **Alternatives considered**: A single `AtomicU128` carrying
  `(dur_us, call_count)` was rejected because the wasm32 atomics
  surface tops out at 64-bit CAS, which would force the worker hot
  path into a fallback. A new `record_drained` API alongside
  `record_under_root` was rejected because every drain caller is
  paired anyway — splitting the API doubles the surface for no win.
- **Code anchors**: `crates/evosim/src/world/nn_stats.rs → NnStats`,
  `NnStats::record_chunk_subphases`, `NnStats::record_chunk_lite`;
  `crates/evosim/src/grass/mod.rs → GrassGrid`;
  `crates/evosim/src/world/mod.rs → step` (the drain into `record_under_root`).

### JSON adds `total_call_count`; legacy `call_count` retained as sample count

- **Decision**: `profile_report_json` and `web/src/perf.ts::reportJson`
  emit a `total_call_count` field on every node (sum of per-sample
  call counts). The existing `call_count` field keeps its meaning —
  sample count in the ring buffer — so any reader on the old shape
  keeps rendering. New readers divide `total_us / total_call_count`
  for honest `ms/call`.
- **Why**: An in-place semantic change to `call_count` would silently
  break the perf panel and any external consumer. Adding a new field
  with a clear name keeps the shape forward-compatible, and the panel
  falls back to `call_count` when `total_call_count` is missing so a
  stale wasm bundle still renders.
- **Applies to**: `architecture/profiler.md`.
- **Tradeoffs**: Samples carry one additional call-count lane, increasing profiler
  ring memory while preserving backward-compatible reader semantics.
- **Code anchors**: `crates/evosim/src/profiler.rs → Node`,
  `serialize_node`; `web/src/perf.ts → recordSample`, `serializeNode`.

### Panel renders a single `window: X.X s` header above the five trees

- **Decision**: The perf panel computes
  `windowSec = max(effective_window_ms) / 1000` across every populated
  node in the merged report and renders one header line above the
  stacked trees.
- **Why**: Per-tick aggregation makes the window uniform across nodes
  — every tree gets one drained sample per tick — so one number
  describes the whole panel. Without it, `tick.total_ms = 40 s` is
  uninterpretable: the reader has to guess whether the window is
  17 s, 40 s, or 60 s. Reading from `effective_window_ms` reuses the
  per-node field already in the JSON instead of plumbing a new
  top-level value.
- **Applies to**: `architecture/profiler.md`.
- **Revisit when**: A tree adopts a non-per-tick sampling rate (the
  single header becomes a lie at that point — switch to per-tree
  headers).
- **Code anchors**: `web/src/widgets/perf-panel.ts → renderTreesStacked`.

### 1 Hz worker→main poll cadence; profiler backend always on

- **Decision**: `request_profile_report` polls at ~1 s; profile state
  is enabled once at worker boot and remains on. `showProfiler` controls
  whether the Settings profiler category polls/renders the report. The reply
  bundles `profile` + `tps` + `jank_count` + `live_grass_cell_count`
  + `total_grass_density` so main does not need separate round-trips.
- **Why**: 1 Hz is enough for human-eye perf reading; per-frame would
  flood the protocol. Keeping backend recording warm means the panel has a
  useful rolling window immediately after navigation. Bundling collapses what
  would otherwise be several separate request types.
- **Applies to**: `architecture/profiler.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/widgets/perf-panel.ts → POLL_INTERVAL_MS`,
  `web/src/widgets/perf-panel.ts → setProfilerVisible`;
  `web/src/sim/worker.ts → maybeWriteProfileReport`.

### Worst-jank phase attribution stays honest until per-tick traces exist

- **Decision**: Telemetry records worst jank by tick, duration, and population,
  but reports phase attribution as `unknown` with an explicit reason.
- **Why**: `jank_count` is measured around a whole `WorldHandle::step_n` call.
  The profiler trees are rolling aggregate windows, so mining them after the
  fact would produce a plausible-looking but unreliable phase for a specific
  tick.
- **Applies to**: `architecture/profiler.md`.
- **Tradeoffs**: The summary is actionable enough to identify when and at what
  population the worst spike happened; exact phase attribution waits for a real
  per-tick trace or a jank-triggered one-shot profiler sample.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → observe_completed_tick`,
  `telemetry_report_json`.

## See also

- [`../architecture/profiler.md`](../architecture/profiler.md)
- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/render-pipeline.md`](../architecture/render-pipeline.md)
- [`render.md`](render.md)
