# Profiler

The five-tree, no-rollup, per-row-real-measurement perf instrumentation.

## What it is

A runtime-toggleable hierarchical profiler with **5 sibling top-level
trees** displayed as stacked tables in the perf panel:

```
frame                ← outer RAF callback wall-clock (TS-side, main)
  frame.snapshot.read
  frame.render_world
    frame.render_world.grass
    frame.render_world.creatures
      frame.render_world.creatures.trail_state

sim_worker           ← outer worker-loop iteration wall-clock (TS-side, worker)
  sim_worker.read_input_sab     ← drain CTRL_* SAB into worker locals
  sim_worker.tick               ← world.step_n(1)
  sim_worker.write_output_sab   ← ack-gated snapshot + inspect/profile/NN-stats responses
    sim_worker.write_output_sab.snapshot          ← total write_snapshot wall-clock
      sim_worker.write_output_sab.snapshot.creatures  ← creature SoA pack per-pop
      sim_worker.write_output_sab.snapshot.grass_copy ← pyramid window copy
      sim_worker.write_output_sab.snapshot.biome      ← BiomePyramid window copy

tick                 ← per-tick sim wall-clock (Rust-side)
  tick.grid.rebuild
  tick.nn                       (LEAF — wall-clock wait for rayon)
  tick.movement
    tick.movement.integrate
    tick.movement.repulsion
    tick.movement.apply
  tick.graze
  tick.attack
  tick.grass_step               (LEAF — wall-clock wait for rayon)
  tick.pyramid_refresh          (cadence-amortized mip-pyramid rebuild)
  tick.energy_bookkeeping
  tick.collect_deaths
  tick.handle_births
  tick.color_ema
  tick.bookkeeping_tail

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
  grass_step.dispatch           (normal par_chunks/chunks_mut, or science reduction)
  grass_step.row_compute
    grass_step.row_compute.body
  grass_step.bitset_rebuild
```

## What it owns

- The five-tree structure and the rule that every measured node is a
  real timer pair (NO rollups). Parent nodes have their own clock-now
  brackets; they may exceed the sum of their children (loop overhead,
  dispatch overhead, etc.) and that is the diagnostic value.
- The Rust profiler API (`Profiler::push`, `push_root`,
  `push_root_named`, `record_under_root`, `ensure_root`,
  `report_json`, `clear`). `WorldHandle::profile_enable(on)` toggles
  recording (called once with `true` at boot, left on).
  `WorldHandle::profile_clear()` zeroes every per-node ring buffer
  and resets the epoch without touching the enabled flag — used by
  the panel's "Reset profiler + jank" button alongside `reset_jank`
  and `resetFrameTree()` on the TS side.
- The honest-call-count contract: every sample carries an underlying
  invocation count so `ms/call` reflects per-creature / per-row cost,
  not per-tick cost. RAII spans contribute 1; sum-of-workers drains
  contribute N (the real per-creature or per-row work count).
- The TS-side `frame` mirror in `web/src/perf.ts` (independent state,
  same shape, stitched in by the perf panel).
- The 1 Hz worker→main poll cadence for the profile report.
- The display rules: insertion-order stable, indent by dotted-prefix
  depth, `(no samples yet)` placeholder for an empty tree, a single
  `window: X.X s` header above the five trees.
- The backend is **always-on**. The worker calls
  `world.profile_enable(true)` once at boot and never disables. The
  `showProfiler` setting is the source of truth for panel visibility and
  poll/render activity, not for backend recording. Rust ring buffers and
  TS-side frame samples keep accumulating, so the panel has data the moment it
  reappears.
  **Panel location**: the profiler content is in `#settings-profiler-pane`
  (the Settings panel's "Profiler" sub-nav category), not the old
  `#perf-box` in the left column. `#perf-box` remains in the DOM as an
  empty hidden placeholder.
  **Activation**: selecting the Profiler sub-nav category calls
  `setSetting("showProfiler", true)` + `setProfilerVisible(true)`, which
  starts polling/rendering the panel. Navigating to any other category calls
  both with `false`. Default `Settings.showProfiler = false`; the profiler is
  hidden by default, while recording stays warm.

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
  `row_body_self_us` + `row_body_calls`). Normal scatter uses
  `par_chunks_us` in threaded builds and `chunks_mut_us` in non-threaded
  builds. Deterministic science scatter records its ordered reduction
  dispatch in `chunks_mut_us` even under `--features threads`, because it is
  intentionally sequential at the commit boundary. `record_under_root`
  collapses the dispatch counters under the unified `grass_step.dispatch`
  name and the shared `dispatch_calls` counter records 1 per tick.
- **`frame`** — TS-side `span("frame")` at the top of the RAF callback
  in `web/src/main.ts`, with inner spans in `web/src/render/gl.ts`.
  The `trail_state` child measures the id-keyed previous/current map rebuild
  used by velocity trails. Every TS span is one RAII invocation →
  `call_count = 1`.

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
panel renders five `.profiler-tree-section` blocks in the order
`[frame, sim_worker, tick, nn, grass_step]`; any tree not in that whitelist
renders at the bottom in JSON-insertion order so a future sixth tree
is not dropped.

A single `window: X.X s` header sits above the five trees, computed
from `max(effective_window_ms)` across every populated node in the
merged report. Per-tick aggregation makes the window uniform across
nodes (every tree gets one drained sample per tick), so one number
describes the whole panel and `tick.total_ms = 40 s` is interpretable
without guessing what window backed it.

## Telemetry History And Jank Summary

Profiler data stays a short-window diagnostic tree. Longitudinal run history
is a separate bounded telemetry buffer owned by `WorldHandle` and exported
only on request through `SimBridge.requestTelemetryReport`. Samples are compact
aggregate rows, not creature snapshots: tick range, wall elapsed time,
population, TPS, jank-count delta/total, live grass cell count, and total grass
density. The event log is also capped and low-cardinality: world start/end,
jank-worst replacement, large-jank observations, and jank reset.

`jank_count` remains the existing cumulative counter in the snapshot/profile
surfaces. The telemetry jank summary adds the worst observed tick since the
last jank reset: duration, tick, population, and phase attribution. Phase is
reported as `unknown` with an explicit reason because the whole-tick timer is
measured around `WorldHandle::step_n`, while profiler spans are rolling
aggregates rather than a per-jank-tick trace. The reset button clears the
counter and worst-jank summary but preserves historical aggregate samples.

## Worker-side spans

The worker's TS perf module is a separate instance from main's —
main never reads worker-side TS spans directly.
`WorldHandle::record_profile_sample(root, path, dur_us, call_count)`
lets the worker publish JS-measured durations into the always-on
Rust profile tree, which is what main reads through
`profile_report_json`.

Worker-side spans that need to reach the perf panel should use
`web/src/sim/worker.ts` → `simLoop` and
`crates/evosim/src/wasm_api/mod.rs` → `WorldHandle::record_profile_sample`
as the template.

## Code anchors

- `crates/evosim/src/profiler.rs` → `Profiler`, `Profiler::push`,
  `Profiler::push_root`, `Profiler::push_root_named`,
  `Profiler::record_under_root` (signature
  `(root_name, path, dur_us, call_count)`),
  `Profiler::record_external`, `Profiler::clear`,
  `ProfilerInner::ensure_root`, `Profiler::report_json`,
  `serialize_node` (writes `total_call_count`),
  `clock_now_us_threadsafe`, `SpanGuard`.
- `crates/evosim/src/wasm_api/mod.rs` → `WorldHandle::profile_enable`,
  `WorldHandle::profile_clear`, `WorldHandle::profile_report_json`,
  `WorldHandle::telemetry_report_json`,
  `WorldHandle::record_profile_sample`, `WorldHandle::write_snapshot`.
- `crates/evosim/src/world/mod.rs` → `World::step`.
- `crates/evosim/src/world/nn.rs` → `nn_forward_all_chunks`,
  `build_nn_input`.
- `crates/evosim/src/world/nn_stats.rs` → `NnStats`,
  `NnStats::record_chunk_subphases`, `NnStats::record_chunk_lite`.
- `crates/evosim/src/grass/mod.rs` → `GrassGrid`,
  `GrassGrid::compute_propagation`, `GrassGrid::rebuild_row_bitset`.
- `web/src/perf.ts` → `span`, `setProfilerEnabled`,
  `isProfilerEnabled`, `reportJson`, `resetFrameTree`,
  `recordSample`,
  the empty-stack `frame` special-case.
- `web/src/render/gl.ts` → the `frame.render_world*` span calls.
- `web/src/main.ts` → the outer `span("frame")` and
  `span("frame.snapshot.read")` brackets.
- `web/src/widgets/perf-panel.ts` → `installProfilerPanel` (mounts
  into `#settings-profiler-pane`), `setProfilerVisible` (called by
  devpanel sub-nav wiring on Profiler category select/deselect),
  `TREE_ORDER`, telemetry CSV/JSON export actions, the `total_call_count`
  divisor in the `ms/call` formula, the `window: X.X s` header render.
- `web/src/sim/worker.ts` → the `sim_worker.*` span calls and the
  `WorldHandle::record_profile_sample` invocations.
- `crates/evosim/src/wasm_api/mod.rs` → `record_under_root("sim_worker", "write_output_sab.snapshot*", ...)`
  calls.

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
rule, the five-tree layout, the per-worker accumulator pattern, the
TS-side `frame` mirror.

## See also

- [`simulation-core.md`](simulation-core.md) — where `tick.*` spans live.
- [`worker-runtime.md`](worker-runtime.md) — the 1 Hz poll path.
- [`render-pipeline.md`](render-pipeline.md) — the `frame.*` spans.
- [`app-shell.md`](app-shell.md) — the `#settings-profiler-pane` mount and category-select activation wiring.
- [`../decisions/profiler.md`](../decisions/profiler.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- [Agent-docs authoring rules](~/agent-docs/v1/rules/authoring-rules.md)
