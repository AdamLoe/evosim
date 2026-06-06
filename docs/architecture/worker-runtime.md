# Worker runtime

The Web Worker that owns the wasm sim, the rayon pool, the control
SharedArrayBuffer, and the tight synchronous tick loop.

## What it is

A dedicated module worker (`app/web/src/sim/worker.ts`) spawned once per
world lifetime by `app/web/src/main.ts`. It holds the only `WorldHandle`,
the only rayon pool, the `controlSab` SharedArrayBuffer, and the per-tick
loop. The snapshot region lives in wasm linear memory (a `Vec<u8>` inside
`WorldHandle`), not in a separate `snapshotSab` SAB.

**All main↔worker control is on SAB.** The only surviving `postMessage`
path is the one-shot `boot` handshake and the one-shot `boot_ready` reply.
Every other control signal — sliders, paused, target TPS, inspector
requests, reset-jank, reset-profile, profile/NN report polls — is an
`Atomics.store` + epoch bump on the control SAB, read at the top of each
loop iteration. The loop body is synchronous; `Atomics.wait` (not
`Atomics.waitAsync`) is the pacing primitive.

## What it owns

- The wasm init + rayon `initThreadPool` boot sequence.
- The `crossOriginIsolated` re-check.
- Allocation of `controlSab` at boot: sized by `CONTROL_SAB_BYTES`
  from [`../../app/crates/evosim/src/control_sab.rs`](../../app/crates/evosim/src/control_sab.rs). The
  snapshot bytes live in wasm linear memory (`WorldHandle::snapshot_buf`,
  a `Vec<u8>`); no separate snapshot SAB is allocated.
- The boot handshake: apply persisted sliders, seed SAB slider /
  paused / target_tps values, run one tick, write one snapshot to slot
  0, post `boot_ready`. Main's first RAF then reads a populated slot.
- The tight synchronous `function simLoop()` body. No `async`, no
  `await`, no `setTimeout` — by design.
- Per-tick SAB read of paused / target_tps / sliders (gated on
  `CTRL_CONTROL_EPOCH`) / inspector request (gated on
  `CTRL_INSPECT_REQ_EPOCH`) / profile-clear + reset-jank requests.
- Per-tick SAB write of the snapshot (always) and the inspector
  response (when a request fires). Periodic SAB write of the profile
  report (every `PROFILE_REPORT_EVERY_N_TICKS` ticks) and the
  NN-worker stats (every `NN_STATS_EVERY_N_TICKS` ticks).
- The three `sim_worker.*` profile spans —
  `sim_worker.read_input_sab`, `sim_worker.tick`,
  `sim_worker.write_output_sab` — measured with
  `performance.now()` and published into the always-on Rust profiler
  via the `WorldHandle::record_profile_sample(root, path, dur_us,
  call_count)` wasm-bindgen export.

## What it does NOT own

- **The SAB byte layout** — owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md) and
  the canonical Rust source at
  [`../../app/crates/evosim/src/control_sab.rs`](../../app/crates/evosim/src/control_sab.rs). The TS
  mirror at [`app/web/src/generated/control-sab.ts`](../../app/web/src/generated/control-sab.ts)
  is code-generated; a Rust unit test (`bindings_in_sync`) fails CI
  on drift.
- **The slider name → index table** — canonical list in
  [`../../app/crates/evosim/src/wasm_api/mod.rs`](../../app/crates/evosim/src/wasm_api/mod.rs) `SLIDER_NAMES`;
  TS mirror in
  [`app/web/src/generated/slider-ids.ts`](../../app/web/src/generated/slider-ids.ts).
- **`World::step`, NN, grass mechanic** — owned by
  [`simulation-core.md`](simulation-core.md). The worker only calls
  `world.step_n(1)`.
- **The `SimBridge` runtime class** — defined in
  [`app/web/src/sim/bridge.ts`](../../app/web/src/sim/bridge.ts) and used by main; the
  worker only consumes the one `SimMessageBoot` payload + produces
  one `SimReplyBootReady`. After that, the worker reads main's writes
  through SAB.

## Loop shape

See `app/web/src/sim/worker.ts` → `simLoop` for the body.

Each iteration has three phases — read, tick, write — followed by a
pacing park:

- **Read phase** (`readControlSab`): pause flag, target TPS, sliders
  (epoch-gated on `CTRL_CONTROL_EPOCH`), inspector request (epoch-gated
  on `CTRL_INSPECT_REQ_EPOCH`), profile-clear, reset-jank.
- **Pause park**: when paused, `Atomics.wait(..., Infinity)` blocks the OS
  thread and burns zero CPU. `world.world_ended` is intentionally not a
  park trigger — once population hits zero the sim drops into a thin
  grass-only tick path so the canvas keeps filling while main shows the
  world-end popup.
- **Tick phase**: `world.step_n(1)`. One tick per iteration; batching
  would hurt snapshot freshness since the renderer reads the latest
  snapshot on every RAF.
- **Write phase**: snapshot (always), inspect response (if epoch advanced),
  profile report and NN stats (every N ticks each).
- **Pacing**: `Atomics.wait` for `1000/targetTPS − elapsed` ms when
  `remainingMs > 0.25`. No floor — when the tick overshoots its slice the
  loop continues immediately (no event loop to feed). TPS is therefore
  effectively capped at `1000 / tick_ms`. See
  [`app/web/tests/e2e/sim-bridge.spec.ts`](../../app/web/tests/e2e/sim-bridge.spec.ts)
  for regression coverage of uncapped throughput.
- **Invariant — no `async`, no `await`**: with every control signal on SAB
  there is no `onmessage` macrotask to dispatch in steady state, so the
  loop never has to yield to the event loop.
- **Invariant — slider ordering**: SAB reads happen at the top of the
  iteration, so a slider value main wrote at wall-clock T takes effect for
  tick T+1 deterministically.

## Boot handshake

1. Worker receives the one `boot` message.
2. `await init()`. Mirror `crossOriginIsolated` log line.
3. `initThreadPool(min(TARGET_RAYON_WORKERS, hardwareConcurrency))`.
4. `rayon_current_num_threads()` sanity log (loud warn if `<= 1`).
5. Construct `WorldHandle` via `newWithFounderCount(...)`.
6. Apply every entry in `boot.initial_sliders` via
   `world.set_slider(name, value)` (still uses the name-keyed entry
   for forward compatibility with stale localStorage keys).
7. Allocate `controlSab` (`CONTROL_SAB_BYTES`). (The snapshot region is
   already resident in wasm linear memory — `WorldHandle::snapshot_buf` —
   no separate SAB is needed.)
8. Seed the control SAB: `CTRL_PAUSED`, `CTRL_TARGET_TPS_BITS`, every
   `CTRL_SLIDERS[i]` lane from `boot.initial_sliders`. Stamp the
   epoch counters so the first loop iteration is a no-op read.
9. Run one tick + write one snapshot to slot 0 (into wasm memory).
10. Post `boot_ready` with the `controlSab`, `wasm_memory` handle,
    `snapshot_buf_byte_offset`, `snapshot_buf_byte_len`, `max_pop_for_sim()`,
    and `world.sliders_defaults_json()`. Main builds a typed view directly
    over `wasm_memory.buffer` at the offset.
11. Enter `simLoop()`.

Main asserts `reply.max_pop_for_sim === MAX_POP_FOR_SIM` and throws on
mismatch (Rust/TS const drift → rebuild wasm).

## Wake protocol

Main wakes the worker by mutating the futex word + notifying:

```ts
Atomics.add(ctrlI32, CTRL_FUTEX, 1);
Atomics.notify(ctrlI32, CTRL_FUTEX, 1);
```

This pattern lives in every `SimBridge.setX(...)` writer that needs
the worker to react before the next pacing slice elapses (paused,
target TPS, inspector request). Sliders go through it too — the
worker might be parked when the user drags a slider, and we want
the new value to apply on the next tick rather than after a full
pacing-slice timeout.

## Restart

Implemented in `app/web/src/main.ts` → `restart`:

```ts
async function restart(): Promise<void> {
  const oldBridge = simBridge;
  simBridge = await spawnSimWorker("");
  oldBridge.terminate();
}
```

`worker.terminate()` is unconditional; any in-progress wasm call is
dropped, and the synchronous `Atomics.wait` parked thread is killed
along with the worker. SAB views on main remain valid until the old
bridge is collected. Restart was the easiest path to leave outside
the SAB transport because it's already external to the worker loop —
no in-loop tear-down logic needed.

## Pacing

The user-facing TPS dropdown is `[10, 30, 60, 180, 500, 1000]`. The
worker reads `CTRL_TARGET_TPS_BITS` at the top of every tick, so a
new dropdown selection takes effect immediately (the SAB write also
notifies the futex to wake any in-progress pacing park).

When paused, the worker calls `Atomics.wait(..., Infinity)` and burns
zero CPU. Main wakes it via `Atomics.notify` on the unpause SAB write.

## Code anchors

- [`app/web/src/sim/worker.ts`](../../app/web/src/sim/worker.ts) →
  `handleBoot`, `readControlSab`, `serveInspectRequest`,
  `maybeWriteProfileReport`, `maybeWriteNnStats`,
  `maybeWriteSpeciesTable`, `writeSnapshotToSAB`, `simLoop`,
  `TARGET_RAYON_WORKERS`, `PROFILE_REPORT_EVERY_N_TICKS`,
  `NN_STATS_EVERY_N_TICKS`.
- [`app/web/src/sim/bridge.ts`](../../app/web/src/sim/bridge.ts) →
  `SimBridge`, `SimBridge.sendBoot`, `SimBridge.attachControlSab`,
  `SimBridge.debouncedSetSlider`, `SimBridge.setPaused`,
  `SimBridge.setTargetTps`, `SimBridge.resetJank`,
  `SimBridge.resetProfile`, `SimBridge.requestInspectAt`,
  `SimBridge.requestInspectId`, `SimBridge.requestProfileReport`,
  `SimBridge.requestNnStats`, `SimBridge.terminate`.
- [`app/crates/evosim/src/wasm_api/mod.rs`](../../app/crates/evosim/src/wasm_api/mod.rs) → `WorldHandle`,
  `set_slider`, `set_slider_by_index`, `record_profile_sample`,
  `creature_at`, `creature_idx_by_id`, `creature_inspect_json`,
  `profile_report_json`, `nn_worker_stats_json`, `profile_clear`,
  `reset_jank`, `max_pop_for_sim`, `rayon_current_num_threads`.
- [`app/crates/evosim/src/control_sab.rs`](../../app/crates/evosim/src/control_sab.rs) → canonical SAB
  byte layout. Mirror in
  [`app/web/src/generated/control-sab.ts`](../../app/web/src/generated/control-sab.ts).
- [`app/web/tests/e2e/sab-control.spec.ts`](../../app/web/tests/e2e/sab-control.spec.ts) —
  regression coverage for the all-SAB transport (sim_worker tree,
  snapshot tree, nn.build_input.proximity nesting, SAB inspector
  round-trip).

## Update when

- The pacing primitive changes (currently sync `Atomics.wait`).
- A new SAB control epoch / response slot is added.
- A new top-level profiler tree is added inside the worker.
- The boot handshake gains or loses a step.
- The rayon thread-count policy changes.
- The restart sequence changes shape (e.g. SAB-based tear-down).

## Why is it shaped this way

See [`../decisions/sim.md`](../decisions/sim.md) for the all-SAB
control decision and the locked design choices (pacing primitive,
restart strategy, slider table source-of-truth, response buffer
sizes).

## See also

- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md)
- [`simulation-core.md`](simulation-core.md)
- [`profiler.md`](profiler.md)
- [`build-and-deploy.md`](build-and-deploy.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/dev-loop.md`](../agent-context/dev-loop.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
