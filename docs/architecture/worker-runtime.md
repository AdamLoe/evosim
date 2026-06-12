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
Every other control signal — sliders, paused, target TPS, camera lanes,
snapshot-consumed acknowledgement, inspector requests, telemetry export
requests, reset-jank, reset-profile, profile/NN report polls — is an
`Atomics.store` + epoch bump or direct SAB lane write, read at the top of
each loop iteration. The loop body is synchronous; `Atomics.wait` (not
`Atomics.waitAsync`) is the pacing primitive.

## What it owns

- The wasm init + rayon `initThreadPool` boot sequence.
- The `crossOriginIsolated` re-check.
- Allocation of `controlSab` at boot: sized by `CONTROL_SAB_BYTES`
  from [`../../app/crates/evosim/src/control_sab.rs`](../../app/crates/evosim/src/control_sab.rs). The
  snapshot bytes live in wasm linear memory (`WorldHandle::snapshot_buf`,
  a `Vec<u8>`); no separate snapshot SAB is allocated.
- The boot handshake: construct from `boot.world_config`, apply persisted
  sliders, seed SAB slider / paused / target_tps values, run one tick, write
  one snapshot to slot 0, post `boot_ready`. Main's first RAF then reads a
  populated slot.
- Main-side worker health: boot timeout, worker `error` /
  `messageerror`, and missing snapshot/report progress while unpaused
  are detected in `main.ts`; recovery uses the same respawn path as
  restart.
- The tight synchronous `function simLoop()` body. No `async`, no
  `await`, no `setTimeout` — by design.
- Per-tick SAB read of paused / target_tps / sliders (gated on
  `CTRL_CONTROL_EPOCH`) / inspector request (gated on
  `CTRL_INSPECT_REQ_EPOCH`) / telemetry export request / profile-clear +
  reset-jank requests.
- Ack-gated snapshot publication: the worker writes a new snapshot only
  when main has stored the last painted `CTRL_SEQ` into `CTRL_CONSUMED_SEQ`.
  It continues to tick and write inspect/profile/NN/species reports even
  while snapshot writes are skipped.
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
  would change sim timing and make control writes take effect less
  predictably.
- **Write phase**: snapshot only if `CTRL_CONSUMED_SEQ` equals the last
  published `CTRL_SEQ`; inspect and telemetry responses (if their request
  epochs advanced), profile report and NN stats (every N ticks each) are
  served regardless.
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
- **Invariant — at most one unconsumed snapshot**: after boot, the worker
  compares main's consumed-seq ack with its last published seq before
  calling `write_snapshot`. A high target TPS can therefore continue to
  advance sim state while snapshot write cost is bounded by main's paint
  cadence.

## Boot handshake

1. Worker receives the one `boot` message.
2. `await init()`. Mirror `crossOriginIsolated` log line.
3. `initThreadPool(min(TARGET_RAYON_WORKERS, hardwareConcurrency))`.
4. `rayon_current_num_threads()` sanity log (loud warn if `<= 1`).
5. Construct `WorldHandle` via `newWithConfigJson(JSON.stringify(boot.world_config))`.
6. Apply every entry in `boot.initial_sliders` via
   `world.set_slider(name, value)` (still uses the name-keyed entry
   for forward compatibility with stale localStorage keys).
7. Allocate `controlSab` (`CONTROL_SAB_BYTES`). (The snapshot region is
   already resident in wasm linear memory — `WorldHandle::snapshot_buf` —
   no separate SAB is needed.)
8. Seed the control SAB: `CTRL_PAUSED`, `CTRL_TARGET_TPS_BITS`, and every
   `CTRL_SLIDERS[i]` lane from `boot.initial_sliders[name]` with
   `world.sliders_defaults_json()[name]` as the required fallback. The
   worker drains every slider lane whenever the control epoch advances, so
   leaving any lane at zero would reapply zero on the next Apply. Stamp the
   epoch counters so the first loop iteration is a no-op read.
9. Run one tick + write one snapshot to slot 0 (into wasm memory).
10. Post `boot_ready` with the resolved `master_seed`, derived `world_seed`,
    `controlSab`, `wasm_memory` handle,
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

## Restart and recovery

Implemented in `app/web/src/main.ts` → `restart`:

```ts
async function restart(): Promise<void> {
  await restartWorker(rail, false);
}
```

`restartWorker` keeps the old bridge alive while the replacement boots,
then calls `oldBridge.terminate()` and re-points the perf panel, inspector
selection, highlights, world-end overlay, and RAF seq gate at the new
bridge. Resetting the frame seq gate matters for early failures: a
replacement worker can start at the same `CTRL_SEQ` as the last painted
dead worker, and main must still consume that first replacement snapshot
so the worker is not left snapshot-back-pressured.

The watchdog in `main.ts` tracks a progress signature made from
`CTRL_SEQ`, `CTRL_PROFILE_REPORT_EPOCH`, and `CTRL_NN_STATS_EPOCH`.
While unpaused, no change for `WORKER_STALL_TIMEOUT_MS` means the worker
is stalled/frozen and automatic recovery starts. Paused worlds are
excluded because pause intentionally freezes new snapshots. World-ended
grass-only ticking is not special-cased as a crash; as long as snapshots
or reports continue, the worker is healthy.

Boot is also bounded by `WORKER_BOOT_TIMEOUT_MS`, and worker `error` /
`messageerror` events after boot enter the same automatic recovery path.
Automatic recovery is restart-first: it does not serialize or restore the
dead worker's exact world state. After repeated automatic failures
(`MAX_AUTO_RECOVERY_ATTEMPTS`), main switches to `failed` and exposes a
manual Retry control in the top bar.

The e2e-only `debug_fault` boot field can simulate crash, freeze, or boot
timeout via `window.__evosimE2E`; no production UI exposes it.

## Pacing

The user-facing TPS dropdown is `[10, 30, 60, 180, 500, 1000]`. The
worker reads `CTRL_TARGET_TPS_BITS` at the top of every tick, so a
new dropdown selection takes effect immediately (the SAB write also
notifies the futex to wake any in-progress pacing park).

When paused, the worker calls `Atomics.wait(..., Infinity)` and burns
zero CPU. Main wakes it via `Atomics.notify` on the unpause SAB write.

## Snapshot publication

Boot still writes one initial snapshot before `boot_ready` so main has a
valid first frame. In steady state, `app/web/src/sim/worker.ts →
maybeWriteSnapshotToSAB` is the gate: if `CTRL_CONSUMED_SEQ` is still
behind the worker's `lastPublishedSeq`, the worker skips only
`world.write_snapshot(...)`. It still serves SAB requests and cadence
reports in the same write phase.

Main advances `CTRL_CONSUMED_SEQ` only after it renders a snapshot. The
render loop may run at browser RAF cadence, but expensive snapshot read
and GL work is capped by the persisted App FPS setting.

## Code anchors

- [`app/web/src/sim/worker.ts`](../../app/web/src/sim/worker.ts) →
  `handleBoot`, `readControlSab`, `serveInspectRequest`,
  `serveTelemetryRequest`,
  `maybeWriteProfileReport`, `maybeWriteNnStats`,
  `maybeWriteSpeciesTable`, `writeSnapshotToSAB`, `freezeForE2E`, `simLoop`,
  `TARGET_RAYON_WORKERS`, `PROFILE_REPORT_EVERY_N_TICKS`,
  `NN_STATS_EVERY_N_TICKS`.
- [`app/web/src/main.ts`](../../app/web/src/main.ts) →
  `spawnSimWorker`, `checkWorkerWatchdog`, `restartWorker`,
  `recoverWorker`, `installWorkerStatusUi`.
- [`app/web/src/sim/bridge.ts`](../../app/web/src/sim/bridge.ts) →
  `SimBridge`, `SimBridge.sendBoot`, `SimBridge.attachControlSab`,
  `SimBridge.debouncedSetSlider`, `SimBridge.setPaused`,
  `SimBridge.setTargetTps`, `SimBridge.resetJank`,
  `SimBridge.resetProfile`, `SimBridge.requestInspectAt`,
  `SimBridge.requestInspectId`, `SimBridge.requestProfileReport`,
  `SimBridge.requestNnStats`, `SimBridge.terminate`.
- [`app/crates/evosim/src/wasm_api/mod.rs`](../../app/crates/evosim/src/wasm_api/mod.rs) → `WorldHandle`,
  `newWithConfigJson`, `set_slider`, `set_slider_by_index`, `record_profile_sample`,
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
- [`app/web/tests/e2e/worker-watchdog.spec.ts`](../../app/web/tests/e2e/worker-watchdog.spec.ts) —
  crash/freeze recovery and paused no-false-positive coverage.

## Update when

- The pacing primitive changes (currently sync `Atomics.wait`).
- A new SAB control epoch / response slot is added.
- A new top-level profiler tree is added inside the worker.
- The boot handshake gains or loses a step.
- The rayon thread-count policy changes.
- The restart/recovery sequence changes shape (e.g. SAB-based tear-down,
  different watchdog progress signals, or stateful world restoration).

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
