# Profiler

The four-tree, no-rollup, per-row-real-measurement perf instrumentation.

## What it is

A runtime-toggleable hierarchical profiler with **six sibling top-level
trees** displayed as stacked tables in the perf panel (v1.10):

```
frame                ← outer RAF callback wall-clock (TS-side, main)
  frame.snapshot.read
  frame.render_world
    frame.render_world.grass
    frame.render_world.creatures

sim_worker           ← outer worker-loop iteration wall-clock (TS-side, worker)
  sim_worker.read_input_sab     ← drain CTRL_* SAB into worker locals
  sim_worker.tick               ← world.step_n(1)
  sim_worker.write_output_sab   ← snapshot + inspect/profile/NN-stats responses

tick                 ← per-tick sim wall-clock (Rust-side)
  tick.grid.rebuild
  tick.nn                       (LEAF — wall-clock wait for rayon)
  tick.movement
  tick.graze
  tick.eat_scavenge
  tick.grass_step               (LEAF — wall-clock wait for rayon)
  tick.energy_bookkeeping
  tick.collect_deaths
  tick.handle_births
  tick.color_ema
  tick.bookkeeping_tail

snapshot             ← write_snapshot_to wall-clock (Rust-side)

nn                   ← sum-busy across all rayon workers, per tick
  nn.build_input
    nn.build_input.proximity
      nn.build_input.proximity.creatures
      nn.build_input.proximity.grass
  nn.forward
    nn.forward.l1
    nn.forward.l2
    nn.forward.l3

grass_step           ← sum-busy across all rayon workers, per tick
  grass_step.dispatch           (par_chunks OR chunks_mut, mutually exclusive)
  grass_step.row_compute
    grass_step.row_compute.body
  grass_step.bitset_rebuild
```

v1.10 additions:
- The `sim_worker` tree is published from the worker via
  `WorldHandle::record_profile_sample(root, path, dur_us,
  call_count)`. The worker measures with `performance.now()`; the
  Rust profiler is the always-on shared instance that reaches main.
- The `snapshot` tree is a Rust-side external-timer entry recorded
  inside `WorldHandle::write_snapshot_to`. Single node, root-only path.
- The `nn.proximity*` rows moved under `nn.build_input` to reflect
  the actual call structure (`build_nn_input` invokes the proximity
  scans).

## What it owns

- The four-tree structure and the rule that every measured node is a
  real timer pair (NO rollups). Parent nodes have their own clock-now
  brackets; they may exceed the sum of their children (loop overhead,
  dispatch overhead, etc.) and that is the diagnostic value.
- The Rust profiler API (`Profiler::push`, `push_root`,
  `push_root_named`, `record_under_root`, `ensure_root`,
  `report_json`, `clear`). `WorldHandle::profile_enable(on)` toggles
  recording (v1.9.1: called once with `true` at boot, left on).
  `WorldHandle::profile_clear()` zeroes every per-node ring buffer
  and resets the epoch without touching the enabled flag — used by
  the panel's "Reset profiler + jank" button alongside
  `resetFrameTree()` on the TS side.
- The honest-call-count contract: every sample carries an underlying
  invocation count so `ms/call` reflects per-creature / per-row cost,
  not per-tick cost. RAII spans contribute 1; sum-of-workers drains
  contribute N (the real per-creature or per-row work count).
- The TS-side `frame` mirror in `web/src/perf.ts` (independent state,
  same shape, stitched in by the perf panel).
- The 1 Hz worker→main poll cadence for the profile report.
- The display rules: insertion-order stable, indent by dotted-prefix
  depth, `(no samples yet)` placeholder for an empty tree, a single
  `window: X.X s` header above the four trees.
- v1.9.1: the backend is **always-on**. The worker calls
  `world.profile_enable(true)` once at boot and never disables. The
  Settings "Show profiler" checkbox + the panel's ✕ button are
  visibility-only — they show/hide `#perf-box` and skip the per-poll
  tree render when hidden, but the Rust ring buffers keep accumulating
  so the panel has data the moment it reappears. Default panel
  visibility is `true` (`Settings.showProfiler = true`).

## What it does NOT own

- **What gets measured** — the span call sites live with the code being
  measured. This doc owns the tree shape and naming rule; the call
  sites' correctness is the owning subsystem's responsibility.
- **The wasm boot or the rayon pool init** — owned by
  [`worker-runtime.md`](worker-runtime.md).
- **The profile-toggle checkbox UI** — owned by
  `web/src/widgets/perf-panel.ts`; this doc owns the data the panel
  renders.

## Key invariants

- **No rollups.** Every row in every tree has its own measurement.
  Parent rows in the worker-sum trees (`nn`, `nn.build_input`,
  `nn.forward`, `grass_step`, `grass_step.row_compute`) get their own
  per-worker atomic accumulator bracketing the parent operation —
  separate from the child accumulators that sum the leaves.
- **`tick.nn` and `tick.grass_step` are LEAVES.** They each carry one
  number — the main sim worker's wall-clock wait for the rayon dispatch.
  The deeper breakdown lives only in the sibling `nn` / `grass_step`
  trees with sum-across-workers semantics. `nn.total > tick.nn.total`
  is expected at any non-trivial pop: 8 workers × 5 ms NN forward each
  gives `nn.forward = 40 ms` even though the sim worker only waited
  5 ms.
- **Every node uses its full dotted prefix.** A leaf under `nn.forward`
  is recorded as `nn.forward.l1`, not as a bare `l1` somewhere else
  that the panel has to guess at.
- **Honest per-work-unit call counts.** Every sample carries a
  `call_count` field naming the number of underlying invocations the
  sample represents. RAII spans (push/drop) record 1. Sum-of-workers
  drains record the real per-creature or per-row count: `nn.forward.l1`
  at pop 1500 records 1500 per tick, not 1. The panel's `calls` column
  and `ms/call` divisor both use the summed honest count, so a sub-µs
  per-creature cost reads as a sub-µs per-creature cost — not as a
  millisecond per-tick number that hides the real shape.
- **Outer `frame` is a real RAII span** around the whole RAF callback
  body. Unaccounted-for time inside RAF shows up as
  `frame.total - sum(frame.*.total)` — the diagnostic value the
  no-rollup rule is built for.
- **TS-side `span("frame")` at the empty stack attaches to the
  pre-seeded root** instead of opening a duplicate `frame.frame` child.

## How the trees are populated

- **`tick`** — `crate::profile_span!(&self.profile, "tick.<phase>")` at
  every numbered phase in `World::step` (see
  [`simulation-core.md`](simulation-core.md)).
- **`nn`** — `Profiler::record_under_root("nn", "<path>", dur_us,
  call_count)` from inside `World::step` after the parallel block
  returns. The per-worker atomic accumulators come in `_us` / `_calls`
  pairs (`tick_build_input_total_us` + `tick_build_input_total_calls`,
  `tick_proximity_total_us` + `tick_proximity_total_calls`,
  `tick_forward_l1_us` + `tick_forward_l1_calls`, …) — each worker
  `fetch_add`s into both atomics inside the chunk loop, and the
  post-tick drain passes both numbers to `record_under_root` so the
  panel's `ms/call` divides honest per-creature totals. Atomics are
  reset before dispatch and read out after.
- **`grass_step`** — same pattern with `GrassGrid`'s `_us` / `_calls`
  pairs (`par_chunks_us` + `dispatch_calls`, `chunks_mut_us` +
  `dispatch_calls`, `row_body_us` + `row_body_calls`,
  `row_body_self_us` + `row_body_calls`). `par_chunks_us` and
  `chunks_mut_us` are mutually exclusive at build time (only one is
  non-zero depending on `--features threads`); `record_under_root`
  collapses them under the unified `grass_step.dispatch` name and the
  shared `dispatch_calls` counter records 1 per tick.
- **`frame`** — TS-side `span("frame")` at the top of the RAF callback
  in `web/src/main.ts`, with inner spans in `web/src/render-gl.ts`.
  Every TS span is one RAII invocation → `call_count = 1`.

## JSON node shape

Every node in the report (Rust trees and the TS `frame` mirror) carries:

```text
{
  name: string,
  total_us: u64,             // sum of dur_us across ring samples
  call_count: u64,           // number of distinct ring-buffer samples
  total_call_count: u64,     // sum of per-sample call_count fields
  parent_call_count: u64 | null,  // parent's total_call_count, for ratios
  effective_window_ms: u32,  // now_ms - oldest_sample_ts, capped at WINDOW_MS
  children: Node[],
}
```

`call_count` keeps the legacy meaning (sample count in the ring) for
backward compatibility — older readers continue to render. New readers
divide `total_us / total_call_count` for honest `ms/call`. Stale wasm +
new TS panel still works (the panel falls back to `call_count` when
`total_call_count` is missing).

## Worker → main delivery

Polled at 1 Hz via `request_profile_report`. The worker bundles four
counters into a single reply alongside the JSON tree:

```text
{
  profile: { tree: [...], window_ms: ..., enabled: ..., now_ms: ... },
  tps: number,
  jank_count: number,
  live_grass_cell_count: number,
  total_grass_density: number,
}
```

The TS-side `frame` tree (from `web/src/perf.ts::reportJson()`) is
spliced in by `web/src/widgets/perf-panel.ts` before rendering. The
panel renders four `.profiler-tree-section` blocks in the order
`[frame, tick, nn, grass_step]`; any tree not in that whitelist
renders at the bottom in JSON-insertion order so a future fifth tree
is not dropped.

A single `window: X.X s` header sits above the four trees, computed
from `max(effective_window_ms)` across every populated node in the
merged report. Per-tick aggregation makes the window uniform across
nodes (every tree gets one drained sample per tick), so one number
describes the whole panel and `tick.total_ms = 40 s` is interpretable
without guessing what window backed it.

## Worker-side spans (v1.10 escape hatch)

The worker's TS perf module is still a separate instance from main's
— main never reads worker-side TS spans directly. v1.10 added
`WorldHandle::record_profile_sample(root, path, dur_us, call_count)`
so the worker can publish JS-measured durations into the always-on
Rust profile tree, which *is* what main reads through
`profile_report_json`.

Pattern:

```ts
const t0 = performance.now();
doWork();
world.record_profile_sample("sim_worker", "tick",
  Math.round((performance.now() - t0) * 1000), 1);
```

The three `sim_worker.*` spans use this pattern. Any new worker-side
span that wants to reach the perf panel should too.

(The historical TS-side `worker.snapshot.write` span has been deleted;
the `snapshot` Rust-side tree replaces it.)

## Code anchors

- `src/profiler.rs` → `Profiler`, `Profiler::push`,
  `Profiler::push_root`, `Profiler::push_root_named`,
  `Profiler::record_under_root` (signature
  `(root_name, path, dur_us, call_count)`),
  `Profiler::record_external`, `Profiler::clear`,
  `ProfilerInner::ensure_root`,
  `report_json`, `serialize_node` (writes `total_call_count`),
  `clock_now_us_threadsafe`, `SpanGuard`, `ROOT_TICK`,
  `ROOT_FRAME`, `WINDOW_MS`, `SAMPLES_PER_NODE`, `MAX_NODES`.
- `src/wasm_api.rs` → `WorldHandle::profile_enable`,
  `WorldHandle::profile_clear`, `WorldHandle::profile_report_json`.
- `src/world/mod.rs` → the `record_under_root("grass_step", ...)` calls
  (passing paired `_us` + `_calls`) and the `tick.color_ema` sibling
  lift.
- `src/world/nn.rs` → `nn_forward_all_chunks`,
  `nn_stats.reset_tick`, the per-worker atomic-accumulator bracket
  ordering, the `creatures_in_chunk` argument threaded into
  `record_chunk_subphases`.
- `src/world/nn_stats.rs` → `NnStats`, `tick_build_input_total_us` +
  `tick_build_input_total_calls`, `tick_proximity_total_us` +
  `tick_proximity_total_calls`, `tick_forward_calls`,
  `tick_forward_l1_calls` / `_l2_calls` / `_l3_calls`,
  `tick_chunk_wall_calls`, `record_chunk_subphases`, `record_chunk_lite`.
- `src/grass.rs` → `par_chunks_us`, `chunks_mut_us`, `row_body_us`,
  `row_body_self_us`, `dispatch_calls`, `row_body_calls` atomic
  counters.
- `web/src/perf.ts` → `span`, `setProfilerEnabled`,
  `isProfilerEnabled`, `reportJson`, `resetFrameTree` (v1.9.1
  panel-reset helper), `recordSample` (3-tuple
  `[ts, dur, call_count]`), the empty-stack `frame` special-case.
- `web/src/render-gl.ts` → the `frame.render_world*` span calls.
- `web/src/main.ts` → the outer `span("frame")` and
  `span("frame.snapshot.read")` brackets.
- `web/src/widgets/perf-panel.ts` → `TREE_ORDER`, the
  `total_call_count` divisor in the `ms/call` formula, the
  `window: X.X s` header render.
- `web/src/sim-worker.ts` → `worker.snapshot.write` span (dead;
  see Known limitation).

## Update when

- A new span is added (it must use the full dotted prefix and nest
  under the correct top-level tree).
- A leaf moves between the `tick` tree (main-sim-worker wall clock)
  and one of the worker-sum trees (`nn` / `grass_step`).
- A new top-level tree is minted (add to `TREE_ORDER` in the panel and
  document it in this doc's diagram).
- A new per-worker `_us` accumulator is added without a paired `_calls`
  counter (every drain into `record_under_root` owes an honest
  invocation count — RAII spans record 1, sum-of-workers samples
  record the real per-creature / per-row count).
- The JSON node shape gains or loses a field (`total_call_count` is
  load-bearing for honest `ms/call`; `call_count` is retained as a
  back-compat alias for sample count).
- The window header derivation changes (currently
  `max(effective_window_ms)` across populated nodes).
- The 1 Hz poll cadence changes.
- The Rust profiler API gains or loses an entry point, or
  `record_under_root`'s signature changes.

## Why is it shaped this way

See [`decisions/profiler.md`](../decisions/profiler.md) — no-rollup
rule, the four-tree layout, the per-worker accumulator pattern, the
TS-side `frame` mirror.

## See also

- [`simulation-core.md`](simulation-core.md) — where `tick.*` spans live.
- [`worker-runtime.md`](worker-runtime.md) — the 1 Hz poll path.
- [`render-pipeline.md`](render-pipeline.md) — the `frame.*` spans.
- [`../decisions/profiler.md`](../decisions/profiler.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
