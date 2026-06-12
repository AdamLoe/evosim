---
status:        shipped
owner:         orchestrator
last_updated:  2026-06-12
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/worker-runtime.md
  - architecture/shared-memory-and-protocol.md
  - architecture/render-pipeline.md
  - architecture/app-shell.md
  - architecture/profiler.md
  - decisions/sim.md
  - decisions/render.md
  - decisions/perf.md
  - decisions/app-shell.md
---

# Runtime FPS, Snapshot Decimation, and Render Hot Path

## Mission

Reduce wasted main <-> worker output work without changing sim semantics.
Done means users can choose an app FPS cap of 15, 30, 60, or 120 FPS
(default 60), the worker no longer writes snapshots that the main thread
cannot consume, static biome uploads are avoided when the camera window and
LOD have not changed, and the renderer's interpolation/velocity path is
measured with a bounded follow-up decision.

## Scope

In scope:

- Add a persisted app FPS setting with choices `15`, `30`, `60`, `120`; default
  `60`.
- Use that setting to cap paint cadence and limit unnecessary state/snapshot
  writes where appropriate.
- Add a main-to-worker consumed-sequence acknowledgement lane so the worker can
  decimate snapshot writes while continuing to tick at target TPS.
- Preserve snapshot freshness for the latest consumed frame: main should paint
  the newest available snapshot, not queue historical snapshots.
- Cache biome texture uploads because the biome grid is static and the uploaded
  window only changes when the snapshot window, mip level, texture dimensions,
  or renderer reset changes.
- Measure the current interpolation/trail map path enough to decide whether a
  snapshot velocity lane is worth a later protocol change.

Out of scope:

- Do not add WebGPU compute. Treat it as rewrite-scale research.
- Do not add velocity lanes in this wave unless the implementer proves the
  current path remains dominant after decimation and biome caching.
- Do not change sim tick semantics or target TPS choices.
- Do not add persistence, telemetry history, or `WorldConfig` migration here.

## Dependencies

This is the preferred first implementation wave. It lowers write/upload volume
before telemetry and persistence make more data durable. It can run in parallel
with the worker watchdog plan only if one implementer owns `app/web/src/main.ts`
coordination and merges carefully.

## Context Routes

Docs to load:

- `docs/architecture/worker-runtime.md`
- `docs/architecture/shared-memory-and-protocol.md`
- `docs/architecture/render-pipeline.md`
- `docs/architecture/app-shell.md`
- `docs/architecture/profiler.md`
- `docs/architecture/testing.md`
- `docs/decisions/sim.md`
- `docs/decisions/render.md`
- `docs/decisions/perf.md`
- `docs/decisions/app-shell.md`

Code routes:

- `app/web/src/main.ts` - RAF loop, restart, settings binding, camera lanes.
- `app/web/src/sim/worker.ts` - `simLoop`, snapshot write phase, profile spans.
- `app/web/src/sim/bridge.ts` - `SimBridge`, control SAB constants, slot layout.
- `app/crates/evosim/src/control_sab.rs` - canonical SAB layout.
- `app/crates/evosim/src/bin/gen_bindings.rs` and generated TS mirrors if a new
  control lane is needed.
- `app/web/src/settings.ts` and `app/web/src/widgets/devpanel.ts` - persisted
  app FPS setting and Display category UI.
- `app/web/src/render/gl.ts` - texture upload cache and interpolation/trail state.
- `app/web/src/perf.ts`, `app/web/src/widgets/perf-panel.ts` - frame and upload
  evidence.

## Workstreams

1. FPS setting and main paint cap.

   Add an app/render FPS setting in the Display category. It should be
   live-apply and persisted through the existing settings schema. The RAF
   callback may still run at browser cadence, but expensive snapshot
   read/render/upload work should be skipped until the configured frame
   interval elapses, except for required repaint triggers such as camera
   movement while paused.

2. Snapshot write decimation.

   Add a protocol-level consumed-sequence lane in the control SAB. Main writes
   the last painted/consumed `CTRL_SEQ`; the worker compares that ack with the
   last published snapshot sequence and writes at most one fresh unconsumed
   snapshot. The worker should keep ticking at target TPS and serving
   control/inspect/profile work even while snapshot writes are skipped. The
   design must preserve the existing double-buffer atomic flip ordering when a
   snapshot is written.

3. Static biome upload caching.

   Split the grass and biome upload invalidation decisions in the renderer.
   Grass remains dynamic and may upload each painted frame. Biome upload should
   occur only when the biome window bytes that matter to sampling may have
   changed: boot/restart, mip/window metadata change, texture allocation change,
   or explicit renderer reset. The cache key should be derived from window
   metadata and slot layout, not from tick count alone.

4. Interpolation and velocity-lane triage.

   Add measurement around the current map-based interpolation/trail state path,
   then document the result. If it remains a scaling cliff, write a follow-up
   protocol plan for velocity lanes or packed previous-position lanes. Do not
   smuggle that protocol expansion into this wave.

## Acceptance / Verification

- UI exposes the app FPS setting with exactly 15, 30, 60, 120 and default 60.
- At high target TPS, snapshot write count over a fixed wall-clock interval is
  bounded near the configured app FPS while tick count continues to follow target
  TPS.
- Paused camera pan/zoom still repaints without requiring a new sim tick.
- Grass continues to animate/evolve; biome stays visually aligned after zoom,
  pan, LOD changes, wrap seams, restart, and `grass_size` restart.
- Profiler evidence shows `sim_worker.write_output_sab.snapshot` and biome
  upload work drop under high TPS / low FPS settings.
- Existing gates expected for this surface:
  - `cargo test --lib`
  - `cargo test --lib --features threads`
  - `cd app/web && pnpm typecheck`
  - `cd app/web && pnpm test:e2e`
- Add or update Playwright coverage for FPS selection and high-TPS snapshot
  decimation. Keep the suite's `targetTPS = 1000` convention.

## Handoff Notes

- Any new SAB lane must originate in Rust `control_sab.rs`, be generated into
  TypeScript, and update `shared-memory-and-protocol.md` at ship time.
- Worker-local timer decimation is not the primary contract. Use it only as a
  fallback if the consumed-seq lane proves infeasible, and document the reason.
- The app FPS setting is an app/render control, not sim target TPS. Keep the
  names distinct in UI, settings, tests, and docs.
- Coordinate `main.ts` edits with `02-worker-watchdog-recovery.md`; both plans
  touch restart/RAF/control paths.
- If velocity lanes become necessary, create a separate plan because they alter
  snapshot layout and require Rust/TS layout tests.

## Migration Notes

Shipped 2026-06-12. Migrated the app FPS setting, consumed-sequence snapshot
acknowledgement protocol, render paint cap, static biome upload cache, and
trail-state measurement into the owning architecture and decision docs listed
above.

Velocity lanes remain deferred. This wave added
`frame.render_world.creatures.trail_state` measurement for the current
map-based trail path instead of expanding the snapshot layout.
