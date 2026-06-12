# Decisions index

Declarative rationale for current design choices.

## How to use this folder

Each entry states `Decision`, `Why`, and `Applies to`, with optional
`Alternatives considered`, `Tradeoffs`, `Code anchors`, and
`Revisit when`. Decisions are grouped by architecture domain, not by
date. Superseded rationale stays in git.

## Domain map

| Need | Read |
|---|---|
| Sim core, worker loop, protocol choices | [`sim.md`](sim.md) |
| Rendering, camera, GL, snapshot read choices | [`render.md`](render.md) |
| Profiler tree shape and measurement semantics | [`profiler.md`](profiler.md) |
| wasm build, headers, CI/build choices | [`build.md`](build.md) |
| App shell: settings panel, localStorage schema, SAB-view binding | [`app-shell.md`](app-shell.md) |
| Performance: measured verdicts, perf-critical choices, deferred levers | [`perf.md`](perf.md) |
| Decisions that bind more than one subsystem | [`cross-cutting.md`](cross-cutting.md) |

## Decision routing

Use this as a quick table of contents. Open the named file, then jump to
the matching heading.

| Need | Read |
|---|---|
| Single wasm owner, worker-only `WorldHandle` | [`sim.md`](sim.md) → `Single wasm instance, sim worker holds it` |
| Worker pacing, synchronous `Atomics.wait`, SAB-only control | [`sim.md`](sim.md) → `Pacing uses synchronous Atomics.wait because steady-state control is SAB-only` |
| One tick per worker loop iteration | [`sim.md`](sim.md) → `Sim runs step_n(1) per loop iteration` |
| Slider protocol and mutation surface | [`sim.md`](sim.md) → `set_slider(name, value) is the sole external mutation entry point` |
| Population cap and random cull | [`sim.md`](sim.md) → `MAX_POP_FOR_SIM is a hard sim invariant, enforced by random cull` |
| Restart semantics and slider state at restart | [`sim.md`](sim.md) → `Sim worker handles restart by terminate + respawn` + `Restart sources sliders from in-memory widget state, not localStorage` |
| NN topology, founder init, chunking, action set | [`sim.md`](sim.md) |
| World wrap toggle (toroidal vs walled), world_size as runtime setting | [`sim.md`](sim.md) → `World wrap is a construction toggle (default toroidal)` |
| Biome map generation, blob seeding from world_seed | [`sim.md`](sim.md) → `Biome map: a few large blobs from world_seed via a dedicated PRNG` |
| Species + sexual mating opt-in mode | [`sim.md`](sim.md) → `Species + sexual mating is an opt-in mode, not a replacement` |
| Grass scatter kernel, stochastic u8 propagation, live path | [`sim.md`](sim.md) → `Stochastic u8 scatter kernel is the live propagation path; blur retained behind a selector` |
| Grass LOD step: discrete mip-level stepper (`grass_lod_step`) | [`render.md`](render.md) → `grass_lod_step` |
| Seeded grass clumps at boot, budget, reproducibility | [`sim.md`](sim.md) → `Seeded grass clumps: initial-budget choice` |
| BiomePyramid precomputed at construction, write_snapshot window copy | [`sim.md`](sim.md) → `BiomePyramid precomputed at construction; write_snapshot copies a window` |
| grass_size slider: configurable cell size as perf lever | [`sim.md`](sim.md) → `grass_size slider: configurable cell size as perf lever` |
| SAB layout, byte alignment, cross-language constants, id encoding | [`cross-cutting.md`](cross-cutting.md) |
| Settings tab stage-then-apply, construction-only sliders, settings schema | [`app-shell.md`](app-shell.md) |
| Runtime world_size → SAB view binding (app-shell side) | [`app-shell.md`](app-shell.md) → `Runtime world_size ⇒ computed-dims-equality SAB view binding` |
| Grass scatter bench: ns/cell attribution, fused RNG, geom-skip verdict | [`perf.md`](perf.md) |
| Snapshot worker (v2.0.7) NO-GO and unpark bar | [`perf.md`](perf.md) → `Snapshot worker (v2.0.7) parked` |
| grass_step cadence/visibility gating deferred | [`perf.md`](perf.md) → `Grass grass_step cadence/visibility gating` |
| `MAX_POP_FOR_SIM` Rust/TS duplication and boot assert | [`cross-cutting.md`](cross-cutting.md) → `MAX_POP_FOR_SIM is duplicated in Rust + TS and asserted at boot` |
| Playwright `targetTPS = 1000` rule | [`cross-cutting.md`](cross-cutting.md) → `Worker-control e2e tests force targetTPS = 1000 before interacting` |
| Rendering, camera, GL, direct SAB reads | [`render.md`](render.md) |
| R8 grass texture, JS-side cull, highlight id decode, status bar | [`render.md`](render.md) |
| Profiler four-tree shape, no-rollup rule, honest call counts | [`profiler.md`](profiler.md) |
| Worker-sum profiler trees and `_calls` atomics | [`profiler.md`](profiler.md) |
| Telemetry sampling/export and worst-jank attribution | [`perf.md`](perf.md) + [`profiler.md`](profiler.md) |
| Threaded wasm default, `panic = "abort"`, link args, COOP/COEP | [`build.md`](build.md) |
| Gitignored `web/wasm/`, Vite worker format, CI threaded gates, dev port | [`build.md`](build.md) |

## See also

- [`../architecture/index.md`](../architecture/index.md)
- [`../ownership.md`](../ownership.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
