# evosim at a glance

evosim is a browser-deployed idle evolution sandbox. A **runtime-sized**
world (default 9600u per axis, optionally toroidal) is populated with small
creatures whose inheritance is the weights of a small neural network **plus a
6-trait body genome** (body_size, max_speed, metabolism, diet, water_affinity,
heat_tolerance). The world is split into 3 biomes (plains / water / desert)
generated from a `world_seed`; creatures graze on a runtime-sized grass density
field (1920² cells at 5u each by default), attack each other, reproduce, and die
when energy hits zero. An **opt-in species + sexual-mating mode** swaps the
single-pool asexual `Split` for N seeded species that reproduce by `Mate` and
refuse to eat their own kind. There is no save/load, no scripted scenario —
every page load starts a fresh world.

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
- **One SharedArrayBuffer plus two wasm-memory regions** bridge the two
  threads. The *control* SAB (≈30 KB) is the only real `SharedArrayBuffer`:
  it holds the double-buffer slot index, a sequence counter, a futex word, the
  slider lanes, and the length-prefixed request/response byte buffers. The
  *snapshot* region (two slots, each a stats header + stride-32 creature SoA +
  a u8 grass density region) and the static *biome* region (one u8 `Biome` tag
  per grass cell) both live in **wasm linear memory** and are viewed directly
  by main. All three are runtime-sized from `world_size` at boot — the grass +
  biome regions are `grass_dim²` bytes each (≈3.7 MB at default).
- **Pacing is a synchronous `Atomics.wait` on the futex word.** Since v1.10 the
  control surface is entirely SAB-backed, so the only surviving postMessage is
  the one-shot `boot` handshake; the tick loop parks the worker thread on
  `Atomics.wait` (with a target-TPS timeout, `Infinity` when paused) and main
  wakes it via `Atomics.add` + `Atomics.notify` on the futex.
- **One tick = one snapshot.** The worker calls `step_n(1)`, writes the
  inactive snapshot slot, atomically flips `CTRL_CURRENT_SLOT`, and bumps
  `CTRL_SEQ`. Main reads the live slot per RAF; the renderer always sees
  the freshest snapshot vsync allows.
- **Restart is `worker.terminate()` + `new Worker(...)`.** Old SAB views
  are GC'd with the previous bridge; the renderer keeps painting the
  last-good frame during the ~500 ms blip.

## Tick loop summary

Each tick rebuilds neighbour lookup state, runs the NN/action pass, applies
movement (biome-penalty-modulated) / graze / attack effects, advances grass,
settles energy/death/birth bookkeeping (asexual `Split` or, in species mode,
sexual `Mate` with crossover), and decays the action ring-flash. When the
population dies out the worker keeps ticking grass-only so the canvas fills in
the background while the UI shows a "world ended" popup.

The exact step order, phase names, and `tick.*` profiler spans are owned by
[`architecture/simulation-core.md`](architecture/simulation-core.md). The
parallel breakdown for NN and grass work is owned by
[`architecture/profiler.md`](architecture/profiler.md).

## Tech stack at a glance

- **Rust** (stable for native + tests; nightly for wasm w/ atomics) →
  wasm via `wasm-pack --target web --features threads`. Crate at
  `app/crates/evosim/`.
- **rayon** + `wasm-bindgen-rayon` for the parallel NN forward + grass
  propagation. Requires COOP/COEP for SharedArrayBuffer.
- **TypeScript + Vite + plain DOM** for the web shell (no framework).
  Renderer is WebGL2 with one instanced draw call for all creature
  bodies + highlights.
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
