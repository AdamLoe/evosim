# Decisions — profiler

---

### Four sibling top-level trees, not one tree rooted at `tick`

- **Decision**: `frame`, `tick`, `nn`, `grass_step` are independent
  trees displayed as stacked tables in the perf panel.
- **Why**: They mean different things. `frame` is RAF wall-clock on
  main; `tick` is per-tick wall-clock on the sim worker; `nn` and
  `grass_step` are *sum-busy across rayon workers* for the parallel
  phases. Mixing them under one root would force readers to compare
  apples to oranges and would hide the `nn.total > tick.nn.total`
  relationship that signals rayon is working.
- **Applies to**: `architecture/profiler.md`,
  `architecture/render-pipeline.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/profiler.rs → ProfilerInner::roots`,
  `ProfilerInner::root_order`, `ensure_root`;
  `app/web/src/widgets/perf-panel.ts → TREE_ORDER`.

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
- **Code anchors**: `crates/evosim/src/world/nn_stats.rs → NnStats`
  (the `tick_*_total_us` parent fields), `app/crates/evosim/src/grass/mod.rs → GrassGrid`
  (the `par_chunks_us` / `chunks_mut_us` / `row_body_us` /
  `row_body_self_us` fields).

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

- **Decision**: `app/web/src/perf.ts` maintains its own independent
  `frame` tree with the same ring-buffer + JSON shape as the Rust
  profiler. The perf panel concatenates the two reports before
  rendering.
- **Why**: `web_sys::window().performance` round-trips would add
  unnecessary cost per RAF, and the renderer doesn't need wasm. A TS
  mirror that obeys the same shape rules is the cheapest way to
  unify the four trees in display.
- **Applies to**: `architecture/profiler.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `app/web/src/perf.ts → span`, `reportJson`,
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
- **Code anchors**: `app/web/src/perf.ts → span` (the
  `stack.length === 0 && name === "frame"` branch).

### Worker-side `worker.snapshot.write` span is a known dead row

- **Decision**: The sim worker opens a TS-side
  `span("worker.snapshot.write")` inside `writeSnapshotToSAB`. The
  worker's TS perf module is a separate instance from main's; main
  never reads worker-side TS spans, so the row is a silent no-op.
- **Why**: Documented as a known limitation. Fixing it needs either a
  worker→main perf-message channel or moving the span to the
  receiving side (which would no longer measure the same thing).
- **Applies to**: `architecture/profiler.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/web/src/sim/worker.ts → writeSnapshotToSAB`
  (the `span("worker.snapshot.write")` call), `app/web/src/perf.ts`.
- **Revisit when**: a profile pass needs the SAB write cost visible
  somewhere; that's the time to plumb the worker's perf state to main.

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
  divisor. Without them, `nn.forward.l1` at pop 1500 reads as
  millisecond per-tick cost and hides the real microsecond per-creature
  shape.
- **Applies to**: `architecture/profiler.md`,
  `architecture/simulation-core.md`.
- **Alternatives considered**: A single `AtomicU128` carrying
  `(dur_us, call_count)` was rejected because the wasm32 atomics
  surface tops out at 64-bit CAS, which would force the worker hot
  path into a fallback. A new `record_drained` API alongside
  `record_under_root` was rejected because every drain caller is
  paired anyway — splitting the API doubles the surface for no win.
- **Code anchors**: `crates/evosim/src/world/nn_stats.rs → NnStats`,
  `record_chunk_subphases`, `record_chunk_lite`;
  `app/crates/evosim/src/grass/mod.rs → GrassGrid` (`dispatch_calls`, `row_body_calls`);
  `crates/evosim/src/world/mod.rs → step` (the drain into `record_under_root`).

### JSON adds `total_call_count`; legacy `call_count` retained as sample count

- **Decision**: `profile_report_json` and `app/web/src/perf.ts::reportJson`
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
- **Tradeoffs**: Sample tuple grows from `(u32, u32)` to
  `(u32, u32, u32)` — +33% per-sample memory. At
  `SAMPLES_PER_NODE = 4096` × `MAX_NODES = 256` that's ~4 MB worst
  case, acceptable for a runtime-toggleable dev tool.
- **Code anchors**: `crates/evosim/src/profiler.rs → serialize_node`, `Node.samples`
  (the 3-tuple shape); `app/web/src/perf.ts → recordSample`,
  `serializeNode`.

### Panel renders a single `window: X.X s` header above the four trees

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
- **Code anchors**: `app/web/src/widgets/perf-panel.ts → renderTreesStacked`.

### 1 Hz worker→main poll cadence; profiler default OFF

- **Decision**: `request_profile_report` polls at ~1 s; profile state
  is OFF on every page load (no localStorage persistence). The reply
  bundles `profile` + `tps` + `jank_count` + `live_grass_cell_count`
  + `total_grass_density` so main does not need four round-trips.
- **Why**: 1 Hz is enough for human-eye perf reading; per-frame would
  flood the protocol. Default-OFF avoids the per-tick clock-read cost
  for users not looking. Bundling collapses what would otherwise be
  four separate request types.
- **Applies to**: `architecture/profiler.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/web/src/widgets/perf-panel.ts → POLL_INTERVAL_MS`,
  `app/web/src/sim/worker.ts → handle` (the `request_profile_report`
  arm).

## See also

- [`../architecture/profiler.md`](../architecture/profiler.md)
- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/render-pipeline.md`](../architecture/render-pipeline.md)
- [`render.md`](render.md)
