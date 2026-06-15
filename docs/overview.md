# evosim at a glance

evosim is a browser-deployed idle evolution sandbox. A runtime-sized world
of small creatures combines neural-network inheritance with a body genome.
Biomes are generated from `world_seed`, creatures graze on a runtime-sized
grass density field, attack each other, reproduce, and die when energy runs
out. An opt-in species + sexual-mating mode swaps the asexual split loop for
seeded species that reproduce by `Mate` and refuse to eat their own kind.
Worlds can be autosaved, named-saved, resumed, forked,
exported, and imported as versioned `evosim.world` artifacts; scripted
scenarios are still out of scope.

## Shape of the system

```
                main thread                          sim worker
              ┌───────────────┐                  ┌────────────────┐
              │ render loop   │  ── boot ──>     │ wasm + rayon   │
              │ WebGL2        │  <-- snapshots-- │ WorldHandle    │
              │ DevPanel UI   │  -- control  --> │ step loop      │
              └───────────────┘                  └────────────────┘
                      │                                   │
                      └──── shared bridge ─── snapshot + ctrl ┘
```

- The wasm instance lives in the sim worker. The main thread holds no wasm
  handle. All sim mutation funnels through `WorldHandle::set_slider(name,
  value)` and read-only inspect / report calls.
- A `SharedArrayBuffer` and a wasm-memory snapshot region bridge the main
  thread and worker. The control SAB holds sequencing, pacing, and slider lanes; the
  snapshot region lives in wasm linear memory and main reads it directly.
- Pacing uses synchronous `Atomics.wait` on the futex word. The control
  surface is SAB-backed, so the remaining `postMessage` is the boot
  handshake; the worker parks on `Atomics.wait` and main wakes it via
  `Atomics.add` + `Atomics.notify`.
- The worker advances the sim each loop iteration and gates snapshot writes
  on main-thread acknowledgement, which keeps paint-rate traffic bounded
  while the sim keeps ticking.
- Restart is `worker.terminate()` + `new Worker(...)`. Old bridge views are
  discarded and the renderer keeps the last good frame during the swap.

## Tick loop summary

Each tick rebuilds neighbour lookup state, runs the NN/action pass, applies
movement / graze / attack effects, advances grass, settles energy /
death / birth bookkeeping, and decays the action ring flash. When the
population dies out the worker keeps ticking grass-only so the canvas fills
in the background while the UI shows a world-ended popup.

The exact step order, phase names, and `tick.*` profiler spans are owned by
[`architecture/simulation-core.md`](architecture/simulation-core.md). The
parallel breakdown for NN and grass work is owned by
[`architecture/profiler.md`](architecture/profiler.md).

## Tech stack at a glance

- **Rust** (stable for native + tests; nightly for wasm with atomics) via
  `wasm-pack --target web --features threads`. Crate at
  `app/crates/evosim/`.
- **rayon** + `wasm-bindgen-rayon` for the parallel NN forward pass and
  grass propagation. Requires COOP/COEP for `SharedArrayBuffer`.
- **TypeScript + Vite + plain DOM** for the web shell. Renderer is WebGL2
  with an instanced draw call for all creature bodies and highlights.
- **Playwright** for the e2e suite covering the main↔worker control path.

## Where to go next

- For global routing: [`index.md`](index.md).
- For current subsystem facts: [`architecture/index.md`](architecture/index.md).
- For "what file does X live in": [`repository-layout.md`](repository-layout.md).
- For procedural work (code edits, tests, dev loop, commits, docs):
  [`agent-context/index.md`](agent-context/index.md).

## See also

- [`architecture/index.md`](architecture/index.md)
- [`decisions/index.md`](decisions/index.md)
- [`agent-context/index.md`](agent-context/index.md)
- [`ownership.md`](ownership.md)
