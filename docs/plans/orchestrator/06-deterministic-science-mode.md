---
status:        shipped
owner:         orchestrator
last_updated:  2026-06-13
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/simulation-core.md
  - architecture/biome.md
  - architecture/profiler.md
  - architecture/testing.md
  - decisions/sim.md
  - decisions/perf.md
  - decisions/cross-cutting.md
---

# Deterministic Threaded Science Mode

## Mission

Add an opt-in mode where threaded runs can be replayed deterministically enough
for science, regression, and saved-state comparison. Done means the threaded
grass scatter path no longer depends on lossy relaxed cross-tile write races
when science mode is enabled, and tests prove cross-feature reproducibility for
the selected acceptance fixtures.

## Scope

In scope:

- Opt-in science/deterministic mode, default off.
- Deterministic threaded grass scatter strategy, likely tile ownership plus
  border exchange or another explicit conflict-resolution design.
- Clear performance budget and user-facing warning if deterministic mode costs
  throughput.
- Tests comparing default-feature and threaded-feature outcomes for configured
  deterministic fixtures.
- Documentation of which systems are covered by deterministic mode and which are
  not.

Out of scope:

- Do not make normal/default threaded mode slower unless measurement justifies
  replacing it globally.
- Do not require WebGPU.
- Do not promise browser/hardware independent floating-point identity beyond
  what the code can actually guarantee.
- Do not include persistence implementation, though saved-state fixtures should
  eventually use this mode.

## Dependencies

Implement after `05-world-config-schema-presets.md` so the opt-in mode has a
stable config home and after at least the design portion of
`04-world-persistence-artifacts.md` so saved-state metadata can record
determinism expectations.

## Context Routes

Docs to load:

- `docs/architecture/simulation-core.md`
- `docs/architecture/biome.md`
- `docs/architecture/profiler.md`
- `docs/architecture/testing.md`
- `docs/decisions/sim.md`
- `docs/decisions/perf.md`
- `docs/decisions/cross-cutting.md`

Code routes:

- `app/crates/evosim/src/grass/mod.rs` - scatter kernel, active tiles,
  relaxed RMW writes.
- `app/crates/evosim/src/grass/tests/*` - scatter, fertility, fused RNG,
  pyramid, size tests.
- `app/crates/evosim/src/world/mod.rs`, `world/tick.rs` - grass tick cadence
  and deterministic run tests.
- `app/crates/evosim/src/world/mating_tests.rs`,
  `world/seeding_tests.rs` - existing determinism expectations.
- `app/crates/evosim/src/rng.rs` - positional hash RNG.
- `clippy.toml` and `docs/architecture/testing.md` - determinism guardrails.

## Workstreams

1. Acceptance definition.

   Define what deterministic mode promises: same seed/config/saved state plus
   same app version should produce the same aggregate and state hashes across
   default and threaded feature runs for the covered fixtures. Decide whether
   exact creature/grass byte equality or bounded aggregate equality is required.

2. Scatter redesign.

   Replace race-dependent cross-tile writes in science mode with deterministic
   ownership and merge ordering. A likely shape is source-tile computation into
   owned output buffers plus deterministic border exchange/reduction, then a
   single ordered commit.

3. Configuration and UI.

   Add the opt-in mode to `WorldConfig` and expose it as a construction-only
   setting or preset. Label it as science/replay mode with expected performance
   tradeoff.

4. Bench and profiler evidence.

   Measure normal mode and science mode on representative grass sizes and active
   tile densities. Add profiler spans only if they help explain the new cost.

5. Tests.

   Add tests that fail under the current relaxed threaded scatter behavior and
   pass with science mode enabled. Keep normal mode tests from becoming brittle.

## Acceptance / Verification

- Deterministic mode default is off and normal mode behavior/perf does not
  regress materially.
- With deterministic mode on, chosen fixtures produce reproducible results in
  both `cargo test --lib` and `cargo test --lib --features threads`.
- Tests cover tile boundaries and border exchange, including wrapped worlds.
- Benchmark notes quantify the cost and are migrated to `decisions/perf.md`.
- Expected gates:
  - `cargo test --lib`
  - `cargo test --lib --features threads`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo clippy --all-targets --features threads -- -D warnings`
  - focused bench compile/run evidence as agreed for this plan

## Handoff Notes

- The current docs explicitly say threaded scatter is intentionally
  non-reproducible because of relaxed cross-tile RMW. Treat this as a mode
  addition, not a bug fix, unless measurements support replacing the default.
- Keep the test oracle small. Full-world long-run bit equality can be expensive
  and fragile.
- Coordinate with persistence so saved artifacts record whether future replay is
  expected to be exact.

## Migration Notes

Shipped 2026-06-13.

- Added `WorldConfig.science.deterministic` as an additive, default-off config
  block and generated the TypeScript mirror. The Settings panel exposes it as a
  next-world-only science/replay toggle with a throughput warning.
- Added deterministic science scatter: it reads the same tick-start
  `scatter_prev` source as normal scatter, accumulates decay/add deltas into
  owned per-cell buffers, and commits in ascending cell order. Normal threaded
  scatter remains the default relaxed-RMW path.
- Saved artifacts now record mode-appropriate compatibility flags and validate
  the embedded `WorldConfig.science.deterministic` value against runtime state.
- Added exact deterministic fixtures: a world hash pinned at
  `0xda586f7d1ea01dbc` across `cargo test --lib` and
  `cargo test --lib --features threads`, plus a grass boundary/wrap-seam
  scatter repeatability fixture.
- Migrated durable facts to `architecture/simulation-core.md`,
  `architecture/profiler.md`, `architecture/testing.md`,
  `decisions/sim.md`, `decisions/perf.md`, `decisions/cross-cutting.md`, and
  `decisions/app-shell.md`. `architecture/biome.md` needed no content change
  because the biome-capacity cap semantics did not change.
- Bench evidence: `cargo bench --no-run` compiled, and the focused threaded
  native `grass_scatter` run measured the covered 512², 6.25%-seeded fixture at
  2.2895 ms mean for normal propagation scatter and 1.6486 ms mean for science
  propagation. Science was faster on that fixture, but remains opt-in because
  dense/large/browser regimes were not proven globally faster.
