# Worker runtime

The Web Worker that owns the wasm sim, the rayon pool, the message queue,
and the `Atomics.waitAsync` tick loop.

## What it is

A dedicated module worker (`web/src/sim-worker.ts`) spawned once per
world lifetime by `web/src/main.ts`. It holds the only `WorldHandle`, the
only rayon pool, both `SharedArrayBuffer`s, and the async tick loop.
Main holds no wasm; every sim mutation goes through a `SimMessage`
posted via `SimBridge`. The worker's loop is `async` and `await`s a
futex wait between ticks so `self.onmessage` callbacks can drain into a
JS-side message queue.

## What it owns

- The wasm init + rayon `initThreadPool` boot sequence.
- The `crossOriginIsolated` re-check (in addition to main's mirror check;
  both sides read the JS global directly — there is no Rust export).
- Allocation of the two SABs (`controlSab`, `snapshotSab`) at boot.
- The boot handshake: run one tick, write one snapshot to slot 0, then
  post `boot_ready` so main's first RAF reads a populated live slot.
- The `messageQueue` and `drainMessages()` — populated by `self.onmessage`,
  drained at the **top** of every loop iteration so a slider sent at
  tick T takes effect for tick T+1 deterministically.
- The `async function simLoop()` body, the pacing math, and the
  `Atomics.waitAsync` invocation including the 1 ms floor + the
  macrotask yield on the synchronous `not-equal` return path.
- Inspector message handling (`inspect_at` and `inspect_id`).
- Poll-reply bundling for `request_profile_report` (bundles `profile`,
  `tps`, `jank_count`, `live_grass_cell_count`, `total_grass_density`
  into one reply so main does not need four round-trips per second).
- Restart: implemented main-side as `worker.terminate() + new Worker(...)`.
  The worker does not have a re-boot path on a live instance; spurious
  duplicate `boot` messages are logged and ignored.

## What it does NOT own

- **Every message shape, SAB byte layout, snapshot stride** — owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md).
- **`World::step`, NN, grass mechanic** — owned by
  [`simulation-core.md`](simulation-core.md). The worker only calls
  `step_n(1)`.
- **wasm-pack build incantation, COOP/COEP headers** — owned by
  [`build-and-deploy.md`](build-and-deploy.md). The worker reads
  `crossOriginIsolated` to decide whether to spawn the rayon pool but
  does not own how those headers got there.
- **The `SimBridge` runtime class** — defined in `sim-bridge.ts` and
  used by main; the worker only consumes `SimMessage` / produces
  `SimReply`. See the protocol doc.

## Loop shape

```ts
async function simLoop(): Promise<void> {
  while (world !== null && ctrlI32 !== null) {
    drainMessages();                          // top-of-iter: slider drain ordering
    const iterStart = performance.now();

    if (!paused && !world.world_ended) {
      world.step_n(1);                        // exactly one tick per iteration
      writeSnapshotToSAB();                   // write inactive slot, atomic flip
    }

    const elapsed = performance.now() - iterStart;
    const timeoutMs = paused ? Infinity : Math.max(1, 1000/targetTPS - elapsed);
    //                                            ^^^^^^^^
    //                  1 ms FLOOR is load-bearing — `Atomics.waitAsync(0)` returns
    //                  synchronously with `{async: false}` and would spin the loop
    //                  without yielding, dark-holing every postMessage.

    const before = Atomics.load(ctrlI32, CTRL_FUTEX);
    const r = Atomics.waitAsync(ctrlI32, CTRL_FUTEX, before, timeoutMs);
    if (r.async) {
      await r.value;                          // standard path: park
    } else {
      // not-equal race: postMessage from main mutated the futex between our load
      // and the waitAsync. Macrotask yield needed — `onmessage` dispatches as a
      // task; `await Promise.resolve()` (microtask) is NOT sufficient.
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
  }
}
```

**Why this shape:**

- One tick per loop iteration. Batching (`step_n(floor(budget))`) sounds
  like an optimization but hurts SAB snapshot freshness — the renderer
  would repaint the same stale slot until the next batch landed. The
  worker doesn't share the render thread, so it doesn't need batching.
- `Atomics.waitAsync`, not `Atomics.wait`. Synchronous `Atomics.wait`
  inside the loop blocks the worker event loop and prevents
  `onmessage` from ever firing. The `await` on the wait promise is the
  load-bearing event-loop yield.
- 1 ms floor on `timeoutMs`. `Atomics.waitAsync(ta, idx, before, 0)`
  returns synchronously `{async: false, value: "timed-out"}` per the
  Web Atomics spec — no Promise, no microtask. Whenever per-tick cost
  exceeds the per-tick budget (high pop or high target-TPS) `timeoutMs`
  would bottom out at 0 without the floor, the loop would spin without
  yielding, and `onmessage` would dark-hole. This regression has shipped
  at least twice; the e2e suite covers it.
- Macrotask yield on the `not-equal` race. A microtask yield
  (`await Promise.resolve()`) is not enough — `onmessage` dispatches as a
  macrotask. `setTimeout(0)` gives a real macrotask boundary.

## Boot handshake

1. Worker receives the first `boot` message (only one allowed).
2. `await init()` — wasm module init.
3. Read `crossOriginIsolated` from `self`. Log `[sim] crossOriginIsolated=…`.
4. If isolated and `initThreadPool` is exported: `await initThreadPool(N)`
   where `N = min(TARGET_RAYON_WORKERS, navigator.hardwareConcurrency)`.
5. Call `rayon_current_num_threads()`; if `<= 1`, log a loud warning
   (silent single-thread mode would otherwise look identical to a
   correct threaded boot — making it observable is a hard requirement).
6. Construct the `WorldHandle` via `newWithFounderCount(seed, ...)`.
7. Apply every entry in `boot.initial_sliders` via `world.set_slider(name,
   value)` — bools encoded as `0|1`.
8. Allocate `controlSab` + `snapshotSab`.
9. **Run one tick + write one snapshot to slot 0.** This guarantees main's
   first RAF reads a populated live slot.
10. Post `boot_ready` carrying both SAB handles + `max_pop_for_sim()`.
11. `void simLoop()`.

Main asserts `reply.max_pop_for_sim === MAX_POP_FOR_SIM` from
`sim-bridge.ts` and throws if they disagree — Rust/TS constant drift is
fatal and means rebuild wasm.

## Restart

Implemented in `main.ts`:

```ts
async function restart(): Promise<void> {
  const oldBridge = simBridge;
  simBridge = await spawnSimWorker("");      // new worker + new SABs
  oldBridge.terminate();                      // tear down the old worker
  // Reset per-world UI state.
}
```

`worker.terminate()` is unconditional; an in-progress wasm call is
dropped. SAB views on main remain valid (the SAB stays alive in the
previous bridge's closures); rayon child workers are GC'd with the
parent. Hammer-restart (5× `r` in 5 s) is tested for the orphaned-thread
scenario; it does not produce console errors today.

`initial_sliders` is sourced from `devpanel.ts::currentSliderState()`
(in-memory widget state), **not** `getSettings()` (localStorage), so a
mid-drag restart carries the dragged value rather than the
last-persisted one.

## Pacing

The user-facing TPS dropdown is a fixed set: `[10, 30, 60, 180, 500,
1000]`. The worker honours `set_target_tps` immediately; the next loop
iteration uses the new value. The `1000` setting has historically been a
regression vector (it's where the wait-async-zero bug surfaces); the e2e
suite runs every test at TPS=1000.

When paused, `timeoutMs = Infinity`. The worker parks on the futex until
main sends `set_paused(false)` and notifies via `Atomics.notify(..., 1)`.
No 60 Hz idle wake, no snapshot churn nobody's reading. Main keeps
painting the last snapshot main read while paused.

## Code anchors

- `web/src/sim-worker.ts` → `handleBoot`, `handle`, `drainMessages`,
  `writeSnapshotToSAB`, `simLoop`, `TARGET_RAYON_WORKERS`.
- `web/src/main.ts` → `main`, `spawnSimWorker`, `restart`,
  `installPacingControls`, `installRestartButton`, `setSettingsOpen`.
- `web/src/sim-bridge.ts` → `SimBridge`, `SimBridge.postMessage`,
  `SimBridge.attachControlSab`, `SimBridge.debouncedSetSlider`,
  `SimBridge.terminate`.
- `web/src/widgets/devpanel.ts` → `currentSliderState`,
  `getInitialGrassSeedCount`, `getEnergyMax`, `getFounderCount`.
- `src/wasm_api.rs` → `WorldHandle`, `max_pop_for_sim`,
  `rayon_current_num_threads`.

## Update when

- The pacing math changes (especially the `timeoutMs` floor — that
  number has a footgun behind it; the comment on it is mandatory).
- A new message kind is added or removed.
- The boot handshake gains or loses a step.
- The rayon thread-count policy changes (currently
  `min(TARGET_RAYON_WORKERS, hardwareConcurrency)`).
- The restart sequence changes shape (e.g., reusing the same worker
  instead of respawning).
- `worker.snapshot.write` is moved or renamed.

## Why is it shaped this way

See [`decisions/sim.md`](../decisions/sim.md) for the
`Atomics.waitAsync` / 1 ms-floor / one-tick-per-iter rationale and the
slider drain ordering decision. See
[`decisions/cross-cutting.md`](../decisions/cross-cutting.md) for the
boot handshake's first-snapshot guarantee and the `MAX_POP_FOR_SIM`
parity assert.

## See also

- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md)
- [`simulation-core.md`](simulation-core.md)
- [`build-and-deploy.md`](build-and-deploy.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/dev-loop.md`](../agent-context/dev-loop.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
