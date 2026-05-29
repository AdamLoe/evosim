# evosim at a glance

evosim is a browser-deployed idle evolution sandbox. A walled 1200×1200
world is populated with small creatures whose only inheritance is the
weights of a small neural network. Creatures graze on a 960×960 grass
density field, occasionally eat each other, split when energy is high,
and die when energy hits zero. There is no save/load, no species
tracking, no scripted scenario — every page load starts a fresh world.

## Shape of the system

```
                main thread                          sim worker
              ┌───────────────┐                  ┌────────────────┐
              │ render loop   │  ── boot ──>     │ wasm + rayon   │
              │ (RAF)         │                  │ WorldHandle    │
              │ WebGL2        │  <-- snapshots-- │ step_n(1) loop │
              │ DevPanel UI   │  -- messages --> │ message queue  │
              └───────────────┘                  └────────────────┘
                      │                                   │
                      └──── shared SAB ─── snapshot + ctrl ┘
```

- **One wasm instance**, in the sim worker. The main thread holds no wasm
  handle. All sim mutation funnels through `WorldHandle::set_slider(name,
  value)` and a small set of read-only inspect / report calls.
- **Two SharedArrayBuffers** bridge the two threads. The *control* SAB
  (16 bytes) holds a double-buffer slot index, a sequence counter, and a
  futex word. The *snapshot* SAB (~3.9 MB) holds two slots, each with a
  stats header, a stride-32 creature SoA, and a `GRASS_CELL_COUNT`-byte
  quantized grass density region.
- **Pacing is `Atomics.waitAsync` on the futex word.** The worker's loop
  is `async` and `await`s the wait promise between ticks so `onmessage`
  callbacks (sliders, pause, restart, inspect) can drain. `Atomics.wait`
  is forbidden — it blocks the worker event loop and dark-holes every
  main→worker message.
- **One tick = one snapshot.** The worker calls `step_n(1)`, writes the
  inactive snapshot slot, atomically flips `CTRL_CURRENT_SLOT`, and bumps
  `CTRL_SEQ`. Main reads the live slot per RAF; the renderer always sees
  the freshest snapshot vsync allows.
- **Restart is `worker.terminate()` + `new Worker(...)`.** Old SAB views
  are GC'd with the previous bridge; the renderer keeps painting the
  last-good frame during the ~500 ms blip.

## Tick step order (`World::step`)

1. Curriculum factor recompute (smoothstep over population).
2. Spatial grid rebuild.
3. NN forward pass (chunked, optionally rayon-parallel).
4. Movement + soft repulsion + grid rebuild.
5. Graze (grass density consume; sequential).
6. Eat (per-bite predator/prey energy transfer).
7. Grass propagation (logistic growth + cross-kernel; parallel-able).
8. Energy bookkeeping (upkeep, costs, past-lifespan penalty).
9. Collect deaths (zero energy or world-edge wander).
10. Handle births (split → new creature; random cull back to `MAX_POP_FOR_SIM`).
11. Color EMA update (per-creature action-driven RGB).
12. Bookkeeping tail (last_action promote, tick bump, world-end check).

Every numbered phase is bracketed by a `tick.<phase>` profile span. The
`tick.nn` and `tick.grass_step` rows are **leaves** in the tick tree —
their breakdown lives in the sibling top-level `nn` and `grass_step` trees
(sum-busy across rayon workers).

## Tech stack at a glance

- **Rust** (stable for native + tests; nightly for wasm w/ atomics) →
  wasm via `wasm-pack --target web --features threads`. Single crate at
  repo root.
- **rayon** + `wasm-bindgen-rayon` for the parallel NN forward + grass
  propagation. Requires COOP/COEP for SharedArrayBuffer.
- **TypeScript + Vite + plain DOM** for the web shell (no framework).
  Renderer is WebGL2 with one instanced draw call for all creature
  bodies + highlights.
- **Playwright** for the e2e suite covering the main↔worker control path.

## Where to go next

- For a routing pointer to every subsystem: [`index.md`](index.md).
- For "what file does X live in": [`repository-layout.md`](repository-layout.md).
- For the wasm-pack incantation:
  [`architecture/build-and-deploy.md`](architecture/build-and-deploy.md).
- For how to run / add tests:
  [`agent-context/testing-how-to.md`](agent-context/testing-how-to.md).

## See also

- [`architecture/simulation-core.md`](architecture/simulation-core.md)
- [`architecture/worker-runtime.md`](architecture/worker-runtime.md)
- [`architecture/shared-memory-and-protocol.md`](architecture/shared-memory-and-protocol.md)
- [`architecture/render-pipeline.md`](architecture/render-pipeline.md)
- [`architecture/profiler.md`](architecture/profiler.md)
- [`agent-context/maintaining-docs.md`](agent-context/maintaining-docs.md)
