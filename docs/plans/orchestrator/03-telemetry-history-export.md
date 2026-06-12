---
status:        active
owner:         orchestrator
last_updated:  2026-06-12
okay_to_delete: false
long_lived:    false
owning_docs:
  - architecture/simulation-core.md
  - architecture/worker-runtime.md
  - architecture/shared-memory-and-protocol.md
  - architecture/profiler.md
  - architecture/app-shell.md
  - architecture/testing.md
  - decisions/profiler.md
  - decisions/perf.md
  - decisions/sim.md
---

# Longitudinal Telemetry, History, and Export

## Mission

Give long-running worlds durable, low-overhead observational data. Done means
the app records per-N-tick aggregate history, a bounded event log,
downloadable CSV/JSON exports, and actionable jank details that identify the
worst tick and phase rather than only incrementing `jank_count`.

## Scope

In scope:

- Sim-side aggregate history sampled every configurable or fixed N ticks.
- Bounded event log for major sim events such as world start/end, population
  extinction, restart, autosave milestones, large jank events, and future
  persistence/fork events.
- Export path from UI to CSV and JSON without requiring a server.
- `jank_count` expansion: preserve the existing count, but capture worst tick,
  worst duration, and the best available phase/profile attribution.
- Tests for aggregate cadence, event ordering, export shape, and reset behavior.

Out of scope:

- Do not store every creature every tick.
- Do not implement world save/load here.
- Do not turn the profiler into a long-term trace store. The profiler remains a
  short-window diagnostic tree; telemetry is aggregate history.
- Do not require IndexedDB unless this plan deliberately depends on the later
  persistence storage layer. Initial export can be in memory plus download.

## Dependencies

Implement after `01-runtime-fps-snapshot-render.md` so telemetry does not encode
the current every-tick snapshot write pattern. It may precede persistence, but
its schema should leave room for persistence metadata and world config IDs.

## Context Routes

Docs to load:

- `docs/architecture/simulation-core.md`
- `docs/architecture/worker-runtime.md`
- `docs/architecture/shared-memory-and-protocol.md`
- `docs/architecture/profiler.md`
- `docs/architecture/app-shell.md`
- `docs/architecture/testing.md`
- `docs/decisions/profiler.md`
- `docs/decisions/perf.md`
- `docs/decisions/sim.md`

Code routes:

- `app/crates/evosim/src/wasm_api/mod.rs` - `jank_count`, `profile_report_json`,
  report exports, snapshot header.
- `app/crates/evosim/src/world/mod.rs` and `world/tick.rs` - aggregate sources
  and event emission points.
- `app/crates/evosim/src/profiler.rs` - phase data available for jank
  attribution.
- `app/web/src/sim/worker.ts` - report cadence and SAB/JSON response writing.
- `app/web/src/sim/bridge.ts` - request/response API for telemetry reports.
- `app/web/src/widgets/perf-panel.ts` or a new rail widget - history/export UI.
- `app/web/tests/e2e/*` - export and report smoke tests.

## Workstreams

1. Aggregate schema.

   Define a compact per-sample record with tick range, wall-clock time, pop,
   TPS, jank count delta, grass density, optional species counts, and any other
   low-cardinality values already cheap to compute. The schema must be versioned
   so exports can be compared across app versions.

2. Event log.

   Add a bounded ring of structured events with tick, wall-clock timestamp,
   kind, and small payload. Keep payloads intentionally small and stable.
   Creature-level high-volume events are out of scope unless aggregated.

3. Actionable jank capture.

   Track the worst observed tick since reset and include duration, tick number,
   population, and phase attribution. Phase attribution can use profiler span
   data if reliable; if not, capture the highest-confidence phase available and
   label unknowns honestly.

4. Delivery and export.

   Expose history/event/jank reports through the existing worker report pattern
   or a new SAB buffer if the payload fits. Add UI download actions for CSV and
   JSON. CSV should target aggregate rows; JSON can include aggregate rows,
   event log, jank summary, and app/config metadata.

## Acceptance / Verification

- A long run produces bounded memory growth: history and events are capped or
  summarized.
- CSV export opens as tabular aggregate data with stable columns.
- JSON export includes schema version, app version if available, world seed,
  config/settings metadata, aggregate samples, event log, and jank summary.
- Reset profiler/jank behavior is clear: it resets jank summary and count
  without corrupting historical aggregate samples unless explicitly designed.
- Tests cover sample cadence, export shape, and worst-jank replacement logic.
- Expected gates:
  - `cargo test --lib`
  - `cargo test --lib --features threads`
  - `cd app/web && pnpm typecheck`
  - `cd app/web && pnpm test:e2e`

## Handoff Notes

- Keep telemetry distinct from profiler. The profiler answers "where is current
  time going"; telemetry answers "what happened over the run."
- Do not block on IndexedDB. Add persistence integration later if the storage
  plan creates a shared artifact/export layer.
- If payloads threaten current SAB report buffer caps, either keep reports
  paged/windowed or make a protocol plan for larger buffers. Do not silently
  truncate.

## Migration Notes

At ship time, update `profiler.md` for the jank/report relationship,
`worker-runtime.md` and `shared-memory-and-protocol.md` for any new report
transport, `app-shell.md` for export UI, and `decisions/perf.md` for sampling
cadence tradeoffs.
