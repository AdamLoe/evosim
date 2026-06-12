---
status:        draft
owner:         orchestrator
last_updated:  2026-06-12
okay_to_delete: false
long_lived:    false
owning_docs:
  - architecture/worker-runtime.md
  - architecture/shared-memory-and-protocol.md
  - architecture/app-shell.md
  - architecture/testing.md
  - decisions/sim.md
  - decisions/app-shell.md
---

# Worker Watchdog and Recovery

## Mission

Make sim-worker failure visible and recoverable from the main thread. Done
means a crashed, terminated, or frozen worker is detected, the UI shows a clear
recovering/failed state, and the app can restart the world through the same
safe terminate-and-respawn path used by manual restart.

## Scope

In scope:

- Main-side watchdog keyed on snapshot sequence/tick progress, worker error
  events, and boot readiness.
- Recovery states for booting, running, paused, stalled, crashed, restarting,
  and unrecoverable failure.
- A restart path that preserves the current settings and existing
  construction-only boot semantics.
- Tests or debug hooks that can simulate worker crash/freeze without relying on
  random panics.

Out of scope:

- Do not serialize or restore the dead worker's exact world state in this wave.
  That belongs to the persistence plan.
- Do not introduce in-worker teardown machinery unless required; current restart
  semantics are `worker.terminate()` plus `new Worker(...)`.
- Do not change target TPS or FPS behavior except to observe it correctly.

## Dependencies

Can be Wave 1 alongside runtime FPS work, but both touch `main.ts` restart and
RAF state. Pick one merge owner for the main-thread state machine.

## Context Routes

Docs to load:

- `docs/architecture/worker-runtime.md`
- `docs/architecture/shared-memory-and-protocol.md`
- `docs/architecture/app-shell.md`
- `docs/architecture/testing.md`
- `docs/decisions/sim.md`
- `docs/decisions/app-shell.md`

Code routes:

- `app/web/src/main.ts` - `spawnSimWorker`, `restart`, RAF loop, top-bar state.
- `app/web/src/sim/worker.ts` - boot, loop, possible test-only failure hook.
- `app/web/src/sim/bridge.ts` - `SimBridge.terminate`, control SAB access.
- `app/web/src/widgets/devpanel.ts`, `app/web/src/toast.ts`, `app/web/index.html`
  if a visible recovery state needs UI.
- `app/web/tests/e2e/sim-bridge.spec.ts` or a new e2e spec.

## Workstreams

1. Failure model and state machine.

   Define how main distinguishes expected no-progress states from failure.
   Paused worlds and world-ended grass-only ticks must not trigger false
   recovery. Boot timeout, worker `error`/`messageerror`, missing snapshot
   progress while unpaused, and explicit test-triggered freeze/crash should
   become distinct observable states.

2. Recovery path.

   Reuse the existing restart mechanism wherever possible. The recovery path
   should keep settings, reroll/seed behavior, rail state, and renderer reset
   behavior consistent with manual restart. It should avoid accumulating stale
   RAF state or SAB views after repeated failures.

3. User-facing surface.

   Add a small status/toast/top-bar indication that the worker is restarting or
   failed. Avoid modal-only recovery. If automatic restart fails repeatedly,
   expose a manual retry path.

4. Test hook and coverage.

   Add a controlled way for e2e tests to trigger crash or freeze in development
   builds. Keep it out of normal user controls. Assert that the UI recovers or
   reports failure within bounded wall-clock time.

## Acceptance / Verification

- A simulated worker crash is detected and causes a clean respawn or a visible
  failed state with retry.
- A simulated worker freeze while unpaused is detected without confusing a
  paused worker for a failure.
- Repeated recovery does not leave stale `SimBridge`, snapshot views, or
  interpolation state active.
- Existing manual restart still works.
- Expected gates:
  - `cd app/web && pnpm typecheck`
  - `cd app/web && pnpm test:e2e`
  - Rust tests only if the worker failure hook touches wasm/Rust.

## Handoff Notes

- Coordinate with `01-runtime-fps-snapshot-render.md`; snapshot decimation means
  lack of new snapshots is no longer automatically a stall. The watchdog should
  consider configured FPS, target TPS, pause, and last acknowledged/consumed
  state.
- If a test-only worker command is added, document why it cannot be reached by
  production UI.
- This plan should not promise state recovery. Recovery is restart-first until
  the persistence plan ships.

## Migration Notes

At ship time, update `worker-runtime.md` restart/watchdog sections,
`app-shell.md` for visible recovery UI, and `decisions/sim.md` if the failure
model becomes a durable design choice.
