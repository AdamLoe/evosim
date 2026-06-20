---
status:        shipped
owner:         unassigned
last_updated:  2026-06-17
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/worker-runtime.md
  - architecture/profiler.md
  - architecture/render-pipeline.md
  - decisions/perf.md
  - decisions/sim.md
---

# TPS stutter diagnosis

## Mission

Produce a measured diagnosis report for TPS volatility, apparent stutter, and
the report that changing target TPS now seems to require restart. This plan is
not a fix pass; done means the user has a concrete findings report with
reproduction steps, profiler/telemetry evidence, likely causes, and recommended
follow-up work for another implementation agent.

## Scope

In scope:

- Verify whether target TPS changes take effect instantly through the existing
  SAB path or whether a current UI/control regression requires restart.
- Measure TPS volatility and stutter across a small matrix of realistic cases:
  default boot, high population or long-running world if practical, profiler
  visible vs hidden, visual repeats on/off, and at least two target TPS values.
- Use existing profiler trees, status-line TPS, telemetry export, jank counters,
  and e2e/browser observations before adding any instrumentation.
- If existing data is insufficient, add only minimal diagnostic instrumentation
  needed to produce the report, and clearly mark it as temporary or report-only.
- Produce a written diagnosis with evidence, screenshots/log excerpts or saved
  telemetry artifacts where useful, and ranked suspected causes.

Out of scope:

- Shipping performance fixes, pacing refactors, or renderer changes.
- Retuning default TPS, App FPS, population, grass settings, or world settings.
- Replacing the worker pacing primitive without a follow-up implementation plan.
- Broad benchmarking unrelated to the observed TPS/stutter symptoms.

## Context Routes

- `docs/architecture/worker-runtime.md` for the synchronous worker loop,
  `Atomics.wait` pacing, target-TPS lane, and wake protocol.
- `docs/architecture/profiler.md` for the five profiler trees, jank summary,
  telemetry export, and which spans can explain worker vs render cost.
- `docs/architecture/render-pipeline.md` for App FPS, snapshot acking, and how
  status-line TPS/FPS are sampled.
- `docs/decisions/sim.md` for pacing and one-tick-per-loop decisions.
- `docs/decisions/perf.md` for snapshot backpressure, telemetry history, and
  parked performance decisions.
- Code routes: `app/web/src/sim/bridge.ts`, `app/web/src/sim/worker.ts`,
  `app/web/src/main.ts`, `app/web/src/widgets/perf-panel.ts`,
  `app/web/src/perf.ts`, and targeted e2e specs under
  `app/web/tests/e2e/`.

## Open Assumptions

- The target-TPS control is intended to apply instantly. Current code already
  writes `CTRL_TARGET_TPS_BITS` and wakes the futex, so any restart requirement
  should be treated as a regression or measurement/UI-state mismatch.
- The deliverable should be a measured report for later repair, not a fix.
- Existing profiler and telemetry surfaces should be preferred over new
  instrumentation unless they cannot answer the question.

## Approach

Start by making the symptom reproducible. Record the exact browser, world
settings, target TPS values, profiler visibility, visual repeat state, app FPS,
population range, and whether the worker is threaded. Then compare the surfaces
that can disagree:

- status-line `header.tps` from worker snapshots;
- profiler-panel TPS/FPS chart samples;
- worker-loop `sim_worker` and Rust `tick` profiler rows;
- jank count and worst-jank telemetry;
- snapshot publication cadence via `CTRL_SEQ`/paint behavior when relevant.

For the "TPS needs restart" report, specifically verify whether the SAB lane
changes, whether the worker observes it on the next loop, whether the futex wake
fires while parked, and whether the UI readout/chart/status line simply lags or
misreports the new value.

## Acceptance / Verification

- The report includes exact reproduction steps for at least one TPS volatility
  or stutter case, or states that the issue could not be reproduced under the
  tested matrix.
- The report separately answers whether target TPS live-apply is broken,
  delayed, or only misreported.
- The report includes measured evidence from profiler/telemetry/status data and
  identifies which subsystem appears dominant: worker pacing, sim tick cost,
  snapshot publication, main-thread render, browser scheduling/GC, or UI
  reporting.
- Existing worker-control e2e tests are reviewed for coverage gaps, and the
  report recommends any targeted regression test needed for a later fix.
- No performance fix is mixed into the diagnosis unless explicitly requested
  after the report.

## Handoff Notes

This is intentionally report-first. A later fix agent should be able to take the
ranked findings and choose a narrow implementation plan. If the diagnosis finds
that a trivial UI binding is the whole "TPS needs restart" symptom, record that
clearly but still keep code changes out unless the user redirects.

## Migration Notes

If the diagnosis discovers durable facts about pacing, profiler semantics, or
render/snapshot bottlenecks, migrate them into `architecture/worker-runtime.md`,
`architecture/profiler.md`, `architecture/render-pipeline.md`, or
`decisions/perf.md` before marking this plan shipped. If no durable system fact
is discovered, the report can remain a disposable plan artifact.

Shipped 2026-06-17 diagnosis:

- Target TPS live-apply is not broken in the verified path. The existing
  Playwright target-TPS smoke passed, and a pinned browser run changed a live
  world from 60 to 500 TPS without restart.
- Measurement evidence: 60 requested produced ~59.6 ticks/s over 2.5 s; after
  live-changing to 500, the same run produced ~173.6 ticks/s over 2.5 s.
- Profiler evidence at target 500 showed worker time dominated by
  `sim_worker.tick` (~98.9%), with `tick.nn` and `tick.grass_step` the largest
  leaves. The likely bottleneck is tick cost once the requested TPS slice is
  exceeded, not SAB delivery or UI restart semantics.
- Migrated durable findings to `architecture/worker-runtime.md` and
  `decisions/perf.md`.
