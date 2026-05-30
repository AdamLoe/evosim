# Architecture index

Current-state subsystem docs. These describe what the system is now, not
how it changed over time.

## How to use this folder

Load only the subsystem doc that matches the task. If you need rationale,
follow the doc's `Why is it shaped this way` link into `decisions/`.

## Subsystem map

| Need | Read |
|---|---|
| World / SoA / tick model / NN / grass mechanic | [`simulation-core.md`](simulation-core.md) |
| Web Worker lifecycle, boot handshake, pacing | [`worker-runtime.md`](worker-runtime.md) |
| Every main <-> worker message kind, SAB layout, snapshot stride | [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) |
| GL renderer, camera, frustum cull, snapshot read | [`render-pipeline.md`](render-pipeline.md) |
| Main-thread UI shell: DOM layout, right-rail tabs, Settings stage-then-apply | [`app-shell.md`](app-shell.md) |
| Profiler shape, no-rollup rule, four-tree layout | [`profiler.md`](profiler.md) |
| wasm-pack incantation, COOP/COEP, threaded-bundle invariants | [`build-and-deploy.md`](build-and-deploy.md) |
| Test suites and what each covers | [`testing.md`](testing.md) |

## See also

- [`../decisions/index.md`](../decisions/index.md)
- [`../ownership.md`](../ownership.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
