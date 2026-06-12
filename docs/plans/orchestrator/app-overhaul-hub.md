---
status:        draft
owner:         orchestrator
last_updated:  2026-06-12
okay_to_delete: false
long_lived:    false
owning_docs:
  - architecture/worker-runtime.md
  - architecture/render-pipeline.md
  - architecture/simulation-core.md
  - architecture/profiler.md
  - architecture/app-shell.md
  - architecture/testing.md
  - decisions/sim.md
  - decisions/perf.md
  - decisions/app-shell.md
  - decisions/cross-cutting.md
---

# App Overhaul Orchestration Hub

## Mission

Coordinate the overhaul prompted by the pasted external audit: make long
runs durable and analyzable, reduce wasted runtime work, simplify
configuration, improve failure handling, and add docs drift automation.
This hub is temporary; durable facts move into architecture/decision docs
when each plan ships.

## Intake Summary

The pasted audit identified seven concern groups:

- No world state survives a tab close.
- Observability is performance-heavy but lacks longitudinal sim data.
- The worker and renderer do expensive work the UI often does not consume.
- Runtime configuration is spread across slider lanes, TS defaults, and
  positional Rust constructor arguments.
- The threaded path is not reproducible enough for science/replay use.
- Worker panics or stalls can freeze the app without a user-facing recovery
  path.
- Docs drift exists and should be guarded by automation.

The user also requested an explicit app FPS setting, defaulting to 60 FPS
with options 15, 30, 60, and 120. Treat that as a product requirement for
the pacing/snapshot plan, not as a hidden implementation detail.

## Assumptions

- The full pasted audit is useful direction, but WebGPU compute is a later
  research/rewrite track, not part of the first implementation wave.
- The first implementation waves should prefer high-leverage, bounded
  changes: FPS-controlled snapshot production, static biome-upload caching,
  worker watchdog, docs drift fixes/lint, and telemetry/export foundations.
- Persistence and `WorldConfig` are major cross-language protocol changes and
  should be planned before they are implemented.
- Deterministic threaded grass is valuable but should follow config and
  persistence planning because it changes correctness expectations and test
  strategy.

## Stream Tracker

| Stream | Area | Status | Last observed fact | Next action | Blockers |
|---|---|---|---|---|---|
| Pacing/FPS snapshots | Worker runtime + app shell | Shipped | Commit `9fb9ea1` added app FPS choices, consumed-seq ack lane, snapshot decimation, and docs migration. | None for this wave; velocity lanes remain a later research item. | None. |
| Render hot path | WebGL renderer | Shipped | Commit `9fb9ea1` cached static biome uploads and added `trail_state` measurement. | Defer velocity lane as a protocol/layout research item. | Velocity lane likely changes stride/layout, not just one pad lane. |
| Crash resilience | Worker runtime + app shell | Active | Runtime ack/FPS semantics now exist; main can observe seq, consumed seq, pause, and restart state. | Implement `02-worker-watchdog-recovery.md`. | Need threshold semantics around low TPS, app FPS, hidden tab, and explicit pause. |
| Telemetry/history | Sim + protocol + UI/export | Draft plan | Existing cadence report carries profile/TPS/jank/grass; no per-N aggregate history/export surface yet. | See `03-telemetry-history-export.md`. | Needs control-SAB byte-buffer ownership if streamed through existing report family. |
| Persistence | Rust wasm API + IndexedDB | Draft plan | Only settings persist today; world save would touch `World`, `GrassGrid`, species, RNG, IDs, and selected `WorldHandle` layout state. | See `04-world-persistence-artifacts.md`. | Need format/versioning decision aligned with `WorldConfig`. |
| Config/schema/presets | Rust + generated TS + app shell | Draft plan | `DevSliders`, `SLIDER_NAMES`, TS settings, construction-only slider set, and positional `new_with_founder_count` all define config pieces. | See `05-world-config-schema-presets.md`. | Broad protocol migration; should not overlap other protocol edits. |
| Deterministic science mode | Sim core + grass | Draft plan | Threaded scatter uses relaxed cross-tile add/sub; docs already call out nondeterminism. | See `06-deterministic-science-mode.md`. | Needs config home, benchmark budget, and acceptance criteria. |
| Docs drift automation | Docs + tests/CI | Shipped | Commit `d09cac2` added `scripts/docs-lint.mjs`, `pnpm docs:lint`, CI integration, and drift fixes. | Keep docs-lint green as later waves land. | None. |

## Phase Tracker

- [x] Bootstrap orchestrator context.
- [x] Capture pasted audit and user FPS requirement.
- [x] Planning agent creates implementation-ready plan docs.
- [x] High-level plan review agent reviews the plan split and sequencing.
- [x] Apply plan-review corrections.
- [ ] Implement wave 1 with disjoint ownership.
- [ ] Review wave 1 work.
- [ ] Continue later waves until shipped or blocked.

## Candidate Wave Order

1. Wave 1a: app FPS setting, consumed-seq snapshot ack/decimation, static
   biome upload caching, and render interpolation/velocity-lane triage.
2. Wave 1b: concrete docs drift fixes and a first Node-based docs-lint gate.
3. Wave 1c: worker watchdog, boot timeout, crash/freeze detection, and restart
   recovery UX after the runtime ack/FPS contract lands.
4. Wave 2: longitudinal telemetry ring/report/export and actionable jank
   capture.
5. Wave 3: `WorldConfig` schema, generated TS defaults, presets, and master
   seed derivation.
6. Wave 4: persistence/autosave/forking and regression/shareable artifacts.
7. Wave 5: deterministic threaded science mode.
8. Research track: velocity-lane interpolation beyond triage and WebGPU compute.

## Child Plans

- `01-runtime-fps-snapshot-render.md`
- `02-worker-watchdog-recovery.md`
- `03-telemetry-history-export.md`
- `04-world-persistence-artifacts.md`
- `05-world-config-schema-presets.md`
- `06-deterministic-science-mode.md`
- `07-docs-drift-lint.md`

## Plan Review Notes

- Runtime explorer verified the pacing/watchdog/render surfaces:
  `app/web/src/sim/worker.ts`, `app/web/src/main.ts`,
  `app/web/src/render/gl.ts`, `app/crates/evosim/src/control_sab.rs`,
  and generated `app/web/src/generated/control-sab.ts`.
- Stale docs about `waitAsync` are real: code currently uses synchronous
  `Atomics.wait`; docs need correction as part of the docs drift or runtime
  plan.
- Data/config explorer verified the broad protocol surfaces for later waves:
  `app/crates/evosim/src/world/mod.rs`, `app/crates/evosim/src/wasm_api/mod.rs`,
  `app/web/src/settings.ts`, `app/web/src/widgets/devpanel.ts`,
  `app/web/src/sim/bridge.ts`, and generated binding files.
- Persistence is not a small IndexedDB-only feature: it needs a versioned
  Rust world snapshot format and explicit treatment of RNG, grass scatter,
  species, creature IDs, topology/layout, and derived `WorldHandle` caches.
- High-level plan review accepted the split and required one concrete runtime
  contract before implementation: main writes the last painted/consumed
  `CTRL_SEQ` into a control-SAB ack lane, and the worker writes at most one
  fresh unconsumed snapshot while continuing to tick and serve reports.
- Wave 1 ownership is serialized around runtime state: `01` owns `main.ts`,
  `worker.ts`, `bridge.ts`, control-SAB layout/bindings, settings/devpanel,
  render cache work, and runtime docs. `07` may run in parallel only across
  tooling, CI, and known stale docs; it should not migrate runtime plan facts.

## Implementation Evidence

- `9fb9ea1` shipped `01-runtime-fps-snapshot-render.md`.
  Verification reported by implementer: generated bindings, Rust lib tests
  with and without `threads`, web typecheck, focused app-FPS e2e, full e2e,
  and diff check all passed.
- `d09cac2` shipped `07-docs-drift-lint.md`.
  Verification reported by implementer: `cd app/web && pnpm docs:lint`,
  `cd app/web && pnpm typecheck`, and `git diff --check` passed.

## Migration Notes

At ship time, migrate durable facts and decisions into the owning docs in
frontmatter, then mark this hub `shipped` and `okay_to_delete: true` only if
all temporary context has moved.
