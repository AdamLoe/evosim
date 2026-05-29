# Profiler

The four-tree, no-rollup, per-row-real-measurement perf instrumentation.

## What it is

A runtime-toggleable hierarchical profiler with **four sibling top-level
trees** displayed as stacked tables in the perf panel:

```
frame                ← outer RAF callback wall-clock (TS-side)
  frame.snapshot.read
  frame.render_world
    frame.render_world.grass
    frame.render_world.creatures

tick                 ← main sim worker per-tick wall-clock (Rust-side)
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

nn                   ← sum-busy across all rayon workers, per tick
  nn.build_input
  nn.proximity
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

## What it owns

- The four-tree structure and the rule that every measured node is a
  real timer pair (NO rollups). Parent nodes have their own clock-now
  brackets; they may exceed the sum of their children (loop overhead,
  dispatch overhead, etc.) and that is the diagnostic value.
- The Rust profiler API (`Profiler::push`, `push_root`,
  `push_root_named`, `record_under_root`, `ensure_root`,
  `report_json`).
- The TS-side `frame` mirror in `web/src/perf.ts` (independent state,
  same shape, stitched in by the perf panel).
- The 1 Hz worker→main poll cadence for the profile report.
- The display rules: insertion-order stable, indent by dotted-prefix
  depth, `(no samples yet)` placeholder for an empty tree, default OFF.

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
- **`nn`** — `Profiler::record_under_root("nn", "<path>", dur_us)` from
  inside `World::step` after the parallel block returns. The
  per-worker atomic accumulators (`NnStats::tick_build_input_total_us`,
  `tick_proximity_total_us`, `tick_forward_l1_total_us`, etc.) are
  reset before dispatch and read out after.
- **`grass_step`** — same pattern with the atomic counters on
  `GrassGrid` (`par_chunks_us`, `chunks_mut_us`, `row_body_us`,
  `row_body_self_us`). `par_chunks_us` and `chunks_mut_us` are
  mutually exclusive at build time (only one is non-zero depending on
  `--features threads`), and `record_under_root` collapses them under
  the unified `grass_step.dispatch` name.
- **`frame`** — TS-side `span("frame")` at the top of the RAF callback
  in `web/src/main.ts`, with inner spans in `web/src/render-gl.ts`.

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

## Known limitation: the dead `worker.snapshot.write` span

`web/src/sim-worker.ts` opens a TS-side `worker.snapshot.write` span
inside the SAB write block, but the worker's TS perf module is a
separate instance from main's — and main never reads worker-side TS
spans. The span has been a silent no-op since SAB-snapshot landed.
Fixing it requires either a worker→main perf-message channel or
relocating the span to wrap the postMessage on the receiving side
(which would no longer measure the same thing). Known limitation;
flagged for a future pass.

## Code anchors

- `src/profiler.rs` → `Profiler`, `Profiler::push`,
  `Profiler::push_root`, `Profiler::push_root_named`,
  `Profiler::record_under_root`, `ProfilerInner::ensure_root`,
  `report_json`, `clock_now_us_threadsafe`, `SpanGuard`, `ROOT_TICK`,
  `ROOT_FRAME`, `WINDOW_MS`, `SAMPLES_PER_NODE`, `MAX_NODES`.
- `src/world/mod.rs` → the `record_under_root("grass_step", ...)` calls
  and the `tick.color_ema` sibling lift.
- `src/world/nn.rs` → `nn_forward_all_chunks`,
  `nn_stats.reset_tick`, the per-worker atomic-accumulator bracket
  ordering.
- `src/world/nn_stats.rs` → `NnStats`, `tick_build_input_total_us`,
  `tick_proximity_total_us`, the `tick.*` and `worker.*` fields.
- `src/grass.rs` → `par_chunks_us`, `chunks_mut_us`, `row_body_us`,
  `row_body_self_us` atomic counters.
- `web/src/perf.ts` → `span`, `setProfilerEnabled`,
  `isProfilerEnabled`, `reportJson`, the empty-stack `frame`
  special-case.
- `web/src/render-gl.ts` → the `frame.render_world*` span calls.
- `web/src/main.ts` → the outer `span("frame")` and
  `span("frame.snapshot.read")` brackets.
- `web/src/widgets/perf-panel.ts` → `TREE_ORDER`, the panel render.
- `web/src/sim-worker.ts` → `worker.snapshot.write` span (dead;
  see Known limitation).

## Update when

- A new span is added (it must use the full dotted prefix and nest
  under the correct top-level tree).
- A leaf moves between the `tick` tree (main-sim-worker wall clock)
  and one of the worker-sum trees (`nn` / `grass_step`).
- A new top-level tree is minted (add to `TREE_ORDER` in the panel and
  document it in this doc's diagram).
- The 1 Hz poll cadence changes.
- The Rust profiler API gains or loses an entry point.

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
