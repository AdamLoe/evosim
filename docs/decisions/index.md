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
| Decisions that bind more than one subsystem | [`cross-cutting.md`](cross-cutting.md) |

## Decision routing

Use this as a quick table of contents. Open the named file, then jump to
the matching heading.

| Need | Read |
|---|---|
| Single wasm owner, worker-only `WorldHandle` | [`sim.md`](sim.md) → `Single wasm instance, sim worker holds it` |
| Worker pacing, `Atomics.waitAsync`, 1 ms floor, macrotask yield | [`sim.md`](sim.md) → `Pacing uses Atomics.waitAsync with a 1 ms floor on timeoutMs` + `Macrotask yield on the Atomics.waitAsync not-equal path` |
| One tick per worker loop iteration | [`sim.md`](sim.md) → `Sim runs step_n(1) per loop iteration` |
| Slider protocol and mutation surface | [`sim.md`](sim.md) → `set_slider(name, value) is the sole external mutation entry point` |
| Population cap and random cull | [`sim.md`](sim.md) → `MAX_POP_FOR_SIM is a hard sim invariant, enforced by random cull` |
| Restart semantics and slider state at restart | [`sim.md`](sim.md) → `Sim worker handles restart by terminate + respawn` + `Restart sources sliders from in-memory widget state, not localStorage` |
| NN topology, founder init, chunking, action set, world shape | [`sim.md`](sim.md) |
| SAB layout, byte alignment, cross-language constants, id encoding | [`cross-cutting.md`](cross-cutting.md) |
| `MAX_POP_FOR_SIM` Rust/TS duplication and boot assert | [`cross-cutting.md`](cross-cutting.md) → `MAX_POP_FOR_SIM is duplicated in Rust + TS and asserted at boot` |
| Playwright `targetTPS = 1000` rule | [`cross-cutting.md`](cross-cutting.md) → `Every e2e Playwright test forces targetTPS = 1000 before interacting` |
| Rendering, camera, GL, direct SAB reads | [`render.md`](render.md) |
| R8 grass texture, JS-side cull, highlight id decode, status bar | [`render.md`](render.md) |
| Profiler four-tree shape, no-rollup rule, honest call counts | [`profiler.md`](profiler.md) |
| Worker-sum profiler trees and `_calls` atomics | [`profiler.md`](profiler.md) |
| Threaded wasm default, `panic = "abort"`, link args, COOP/COEP | [`build.md`](build.md) |
| Gitignored `web/wasm/`, Vite worker format, CI threaded gates, dev port | [`build.md`](build.md) |

## See also

- [`../architecture/index.md`](../architecture/index.md)
- [`../ownership.md`](../ownership.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
