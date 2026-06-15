# Worker runtime

The Web Worker owns wasm initialization, the rayon pool, `controlSab`, the
`WorldHandle`, and the synchronous simulation loop.

## What it is

`web/src/main.ts` spawns one dedicated module worker per world lifetime. The
worker runs `web/src/sim/worker.ts`, handles the boot `postMessage`, allocates
`controlSab`, creates or loads a `WorldHandle`, writes the first snapshot into
wasm memory, posts `boot_ready`, and then enters `simLoop`.

After boot, the loop does not yield to the worker event loop. All steady-state
control is read from `controlSab`, and pacing uses synchronous `Atomics.wait`.
The high-risk pacing invariants are guarded by `cd app/web && pnpm docs:lint`:
no `Atomics.waitAsync`, paused waits use `Infinity`, running pacing waits on
`remainingMs` only past the threshold in code, and `simLoop` has no
`await`/`Promise.resolve`/`setTimeout` yield path.

The snapshot region is not allocated by the worker as a separate SAB. It is the
Rust `WorldHandle::snapshot_buf` in shared wasm linear memory, and main reads
it through the `wasm_memory` handle returned in `boot_ready`.

## What it owns

- Wasm module initialization and threaded rayon `initThreadPool` setup.
- The `crossOriginIsolated` check that determines whether threaded wasm can run.
- Worker-local construction of `WorldHandle` from fresh `WorldConfig` or a
  saved-world artifact.
- Boot seeding of pause, target TPS, and every slider lane in `controlSab`.
- The synchronous `simLoop` shape: read controls, serve paused requests or tick,
  write outputs, record worker profiler spans, then pace.
- Snapshot publication backpressure via `CTRL_CONSUMED_SEQ`.
- Main-side recovery integration in `web/src/main.ts`: boot timeout, worker
  `error`/`messageerror`, watchdog progress, restart, and artifact load swaps.

## What it does NOT own

- **SAB lane and byte-buffer layout** - see
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md).
- **World step semantics, NN, grass, species, and biome behavior** - see
  [`simulation-core.md`](simulation-core.md), [`species.md`](species.md), and
  [`biome.md`](biome.md).
- **Main-thread rendering and snapshot consumption** - see
  [`render-pipeline.md`](render-pipeline.md).
- **Build flags and headers required for shared memory** - see
  [`build-and-deploy.md`](build-and-deploy.md).

## Boot sequence

`handleBoot` awaits wasm init, checks isolation, initializes the rayon pool when
available, constructs the world, enables profiling, allocates `controlSab`, and
seeds the control lanes before the first loop iteration.

Fresh worlds call `WorldHandle::newWithConfigJson(JSON.stringify(boot.world_config))`
and then apply `boot.initial_sliders` by name. Artifact loads call
`WorldHandle::newFromArtifactJson` with the selected load mode and seed slider
lanes from the artifact state, falling back to Rust slider defaults where the
artifact has no value. Every slider lane must be seeded because the loop drains
the full generated slider table whenever the control epoch advances.

Before replying, the worker runs one tick and writes a snapshot so main's first
RAF has a populated slot. `boot_ready` returns the control SAB, shared wasm
memory, snapshot offset/length, resolved seeds, runtime world metadata, thread
status, Rust slider defaults JSON, and the Rust population cap used by main's
constant-drift check.

## Loop shape

`simLoop` is intentionally synchronous. Each iteration reads the control SAB,
serves paused requests or advances `world.step_n(1)`, writes outputs, records
`sim_worker.*` profiler samples through `WorldHandle::record_profile_sample`,
and parks with `Atomics.wait` when target TPS leaves time in the slice.

The read phase is `readControlSab`. It refreshes pause and target TPS, camera
lanes, profiler window, reset/profile-clear epochs, and slider values when
`CTRL_CONTROL_EPOCH` advances. Slider ordering is deterministic because the
read happens at the top of the iteration; a value written by main takes effect
for the next tick the worker starts after observing the epoch.

The paused branch does not tick, but it still serves inspector, telemetry, and
saved-world artifact requests before parking on the futex. That keeps paused UI
inspection/export paths responsive without reintroducing `postMessage` or an
event-loop yield.

The running write phase calls `maybeWriteSnapshotToSAB`, then serves inspector,
profile, NN stats, species table, telemetry, and world-artifact output paths.
Snapshot writes are ack-gated: if main has not stored the last painted
`CTRL_SEQ` into `CTRL_CONSUMED_SEQ`, the worker skips only the snapshot write
and continues ticking and serving SAB reports.

Pacing is a futex park on `CTRL_FUTEX` for the remaining target-TPS slice. When
the tick is over budget, the loop continues immediately; there is no
event-loop work to feed in steady state.

## Wake protocol

Main wakes the worker by bumping `CTRL_FUTEX` and notifying it after writes
that need prompt reaction: pause/unpause, target TPS, profiler window changes,
sliders, inspector requests, telemetry export requests, and saved-world
artifact requests. The same wake path breaks both paused parks and target-TPS
parks.

## Restart and recovery

`web/src/main.ts` keeps recovery on the main thread. `spawnSimWorker` bounds
boot with `WORKER_BOOT_TIMEOUT_MS` and rejects if `boot_ready` never arrives,
if worker boot errors, or if the reply is missing shared-memory handles.

After boot, `checkWorkerWatchdog` tracks progress from `CTRL_SEQ`,
`CTRL_PROFILE_REPORT_EPOCH`, and `CTRL_NN_STATS_EPOCH` while unpaused. If the
signature stops changing for the stall window, `recoverWorker` starts the same
replacement-worker flow used by restart. Paused worlds are excluded because
pause intentionally stops snapshots.

`restartWorker` and artifact resume/fork both boot a replacement before
terminating the previous bridge. That preserves the old world if replacement
boot fails. After a successful swap, main repoints UI integrations and resets
the frame seq gate so the first replacement snapshot is consumed even when its
`CTRL_SEQ` matches the last painted dead worker.

Automatic recovery is restart-first; it does not serialize the failed worker's
exact state. Repeated automatic failures put the UI into a failed state with a
manual retry control.

## Snapshot publication

The worker's publication gate is `maybeWriteSnapshotToSAB`. A publish writes
the inactive slot via `WorldHandle::write_snapshot`, stores
`CTRL_CURRENT_SLOT`, then bumps `CTRL_SEQ`. Main stores `CTRL_CONSUMED_SEQ`
only after rendering the snapshot. This bounds worker snapshot write cost to
main's paint cadence while allowing simulation ticks and response reports to
continue at target TPS.

## Code anchors

- [`web/src/sim/worker.ts`](../../app/web/src/sim/worker.ts) -> `handleBoot`,
  `readControlSab`, `serveInspectRequest`, `serveTelemetryRequest`,
  `serveWorldArtifactRequest`, `maybeWriteProfileReport`,
  `maybeWriteNnStats`, `maybeWriteSpeciesTable`, `writeSnapshotToSAB`,
  `maybeWriteSnapshotToSAB`, `freezeForE2E`, `simLoop`,
  `TARGET_RAYON_WORKERS`, `PROFILE_REPORT_EVERY_N_TICKS`,
  `NN_STATS_EVERY_N_TICKS`, `SPECIES_TABLE_EVERY_N_TICKS`.
- [`web/src/main.ts`](../../app/web/src/main.ts) -> `spawnSimWorker`,
  `checkWorkerWatchdog`, `restartWorker`, `recoverWorker`,
  `installWorkerStatusUi`, `frame`.
- [`web/src/sim/bridge.ts`](../../app/web/src/sim/bridge.ts) -> `SimBridge`,
  `SimBridge.sendBoot`, `SimBridge.attachControlSab`,
  `SimBridge.debouncedSetSlider`, `SimBridge.setPaused`,
  `SimBridge.setTargetTps`, `SimBridge.resetJank`,
  `SimBridge.resetProfile`, `SimBridge.requestInspectAt`,
  `SimBridge.requestInspectId`, `SimBridge.requestNnInspectId`,
  `SimBridge.requestTelemetryReport`, `SimBridge.requestWorldArtifact`,
  `SimBridge.terminate`.
- [`crates/evosim/src/wasm_api/mod.rs`](../../app/crates/evosim/src/wasm_api/mod.rs)
  -> `WorldHandle`, `newWithConfigJson`, `newFromArtifactJson`,
  `set_slider`, `set_slider_by_index`, `record_profile_sample`,
  `creature_at`, `creature_idx_by_id`, `creature_inspect_json`,
  `creature_nn_inspect_json`, `profile_report_json`,
  `nn_worker_stats_json`, `profile_clear`, `reset_jank`,
  `profile_set_window_ms`, `world_artifact_json`, `max_pop_for_sim`.
- [`crates/evosim/src/control_sab.rs`](../../app/crates/evosim/src/control_sab.rs)
  -> `CONTROL_SAB_BYTES`, `CTRL_FUTEX`, `CTRL_CONSUMED_SEQ`, and the
  generated control constants consumed by the worker.

## Update when

- `simLoop` changes phase order, pacing, pause behavior, or yield behavior.
- Boot starts using a new construction or artifact-load path.
- Slider seeding, default fallback, or control-epoch drain behavior changes.
- Snapshot publication, consumed-seq acking, or first-snapshot boot behavior
  changes.
- Main watchdog, restart, recovery, or artifact replacement semantics change.
- Worker profiler span names or cadence-written reports change.

## See also

- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md)
- [`simulation-core.md`](simulation-core.md)
- [`render-pipeline.md`](render-pipeline.md)
- [`profiler.md`](profiler.md)
- [`build-and-deploy.md`](build-and-deploy.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- [Doc authoring rules](~/agent-docs/v1/rules/authoring-rules.md)
