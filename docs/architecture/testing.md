# Testing

What test suites exist, what each covers, where they live.

## What it is

Three gate surfaces:

1. **Rust unit tests** — `cargo test --lib`, run both with the default
   feature set and with `--features threads`. Live next to the code as
   `#[cfg(test)] mod tests` blocks. Cover sim invariants, wasm-bindgen
   surface behaviour, NN forward + decode, grass propagation, slider
   dispatch, snapshot byte layout.
2. **Docs lint** — `cd app/web && pnpm docs:lint`. Dependency-free Node
   gate for mechanical docs drift: local links/paths, architecture/decision
   routing, ownership paths, Rust↔TS generated constants, worker pacing
   invariants, and profiler tree/span drift.
3. **Playwright e2e** — `cd app/web && pnpm test:e2e`. Specs under
  `app/web/tests/e2e/` cover the main↔worker control path, generated
  Rust↔TS config/defaults drift, settings persistence, world persistence,
  grass LOD/window metadata, restart-time grass sizing, and worker watchdog
  recovery. Playwright
   boots Vite via its `webServer` hook so the suite is one command.

There is no Rust integration-test crate, no goldens, and no snapshot-hash
acceptance. Saved-world artifacts have focused Rust round-trip tests and a
Playwright save/resume/fork/export/import smoke path.

## What it owns

- The list of test suites and what each one covers.
- The convention that the threaded clippy run + the threaded test run
  are first-class gates, not opt-in.
- The "worker-control Playwright tests force `targetTPS = 1000` before
  interacting" rule, because that is the regime where worker pacing,
  futex wake handling, and snapshot back-pressure regressions surface.
- The `pnpm docs:lint` command name and what mechanical drift it covers.
- The `pnpm test:e2e` command name and how Playwright finds Vite.

## What it does NOT own

- **How to run the tests as an agent** — owned by
  [`../agent-context/testing-how-to.md`](../agent-context/testing-how-to.md).
- **The CI gate matrix** — owned by
  [`build-and-deploy.md`](build-and-deploy.md).
- **What individual tests assert** — that lives in the test files; this
  doc points at them.

## Rust unit tests

Run:

```bash
cargo test --lib                          # default feature set
cargo test --lib --features threads       # parallel paths
```

Both must pass. The threaded run exercises the rayon NN forward, the
parallel grass propagation, and any other `#[cfg(feature = "threads")]`
branches.

Notable coverage by file:

| File | What it covers |
|---|---|
| `app/crates/evosim/src/wasm_api/mod.rs` | `write_snapshot_to_native` layout matches the documented byte stride; `max_pop_for_sim()` mirrors the constant; `creature_at` returns stable ids; `set_slider` dispatch round-trip; telemetry sample cadence/export shape/worst-jank/reset behavior; saved-world artifact round-trip and fork identity semantics. |
| `crates/evosim/src/world/mod.rs` | Tick step body, slider effects on world construction, multi-founder spawn placement. |
| `crates/evosim/src/world/tick.rs` | Per-phase invariants — graze energy conservation, eat per-bite math, repulsion clamping, death/birth bookkeeping. |
| `crates/evosim/src/world/nn.rs` | NN input layout, slot offsets, threaded NN matches sequential NN bit-for-bit (when seeded), chunk-range partition invariants. |
| `crates/evosim/src/world/proximity.rs` | Sector LUT correctness, wall proximity edges, grass density bilinear seam wrap. |
| `app/crates/evosim/src/brain/mod.rs` | Forward-pass shape, Leaky ReLU sign behaviour, mutation produces finite values. |
| `app/crates/evosim/src/grass/mod.rs` | Density init, in-cell growth, scatter/blur propagation coverage, active-tile path equivalence to full-grid reference, wrapped `viewport_window` extraction, bilinear sample, row-has-density bitset rebuild. Tests live in the `grass/tests/` subdir. |
| `crates/evosim/src/grid.rs` | `cell_of` boundary clamping, `for_each_in_radius` enumeration. |
| `crates/evosim/src/profiler.rs` | Ring buffer pruning, five-tree minting via `ensure_root`, RAII span correctness. |

## Docs lint

Run:

```bash
cd app/web
pnpm docs:lint
```

The script lives at `scripts/docs-lint.mjs` and has no third-party
dependencies. It checks mechanical drift only: relative markdown links,
high-confidence inline code paths, architecture/decision index routes,
ownership owner/reference paths, generated Rust→TS constants
(`control-sab.ts`, `slider-ids.ts`, `lod-constants.ts`), the duplicated
snapshot constants in `sim/bridge.ts`, the synchronous `Atomics.wait`
worker pacing invariants, and the profiler doc's tree/span list against
current producers. It intentionally skips non-current plan files.

Determinism gates: `clippy.toml` forbids `HashMap` / `HashSet`
`iter*` in sim-critical files (non-deterministic order would silently
make tests flaky); use `BTreeMap` / sorted `Vec` if iteration is needed.

## Playwright e2e (`app/web/tests/e2e/`)

Run:

```bash
# From app/ workspace root
cd app/web
pnpm install            # one-time
pnpm test:e2e
```

Playwright boots Vite itself via its `webServer` hook (see
`app/web/playwright.config.ts`); a running dev server on `:47821` is
reused (`reuseExistingServer: true`).

Key coverage:

- `app-fps.spec.ts` verifies the Display App FPS selector exposes exactly
  15/30/60/120, persists selection, and at high target TPS keeps snapshot
  publication near the configured app FPS while worker TPS stays above the
  render cap.
- `sim-bridge.spec.ts` **fresh default world** — boots the default browser-sized
  world, runs it at high TPS for startup progress, and asserts it remains alive
  below the default population cap.
- `sim-bridge.spec.ts` **pause + resume** — clicks `#playpause-btn`, asserts the tick counter
  stops within ~300 ms and resumes on the second click.
- `sim-bridge.spec.ts` **target TPS** — selects values in `#target-tps-input` and asserts the
  observed tick rate tracks the dropdown.
- `sim-bridge.spec.ts` **slider change** — opens the dev panel, edits `basic upkeep`, asserts
  no `set_slider … rejected` warning lands on the console.
- `sim-bridge.spec.ts` **profile toggle** — toggles `show profiler` + `#profiler-enable`,
  asserts all five stacked profiler tables populate within 4 s.
- `sim-bridge.spec.ts` **restart `r`** — presses the `r` hotkey, asserts the tick counter
  resets (drops below the pre-restart value).
- `sab-control.spec.ts` verifies the all-SAB transport still populates
  worker/profile trees, nested NN proximity rows, and SAB inspector
  round-trips.
- `inspector-nn.spec.ts` verifies paused NN inspection through the SAB
  inspector path.
- `defaults-drift.spec.ts` compares generated `DEFAULT_LIVE_SLIDER_VALUES`
  against Rust `sliders_defaults_json()`, checks Settings defaults derive from
  generated config/default mirrors, and asserts generated presets are complete
  `WorldConfig` payloads.
- `settings-persistence.spec.ts` seeds localStorage, verifies every
  `Settings` key loads/saves/reloads, asserts `currentSliderState()` emits
  every persisted Rust slider setting needed to seed worker boot, and asserts
  persisted construction settings build a complete boot `WorldConfig`.
- `grass-size-restart.spec.ts` verifies `grass_size` changes `grass_dim` only
  after restart.
- `grass-lod-smoke.spec.ts` verifies the default grass window/LOD metadata and
  grass evolution path.
- `worker-watchdog.spec.ts` uses the test-only worker fault hook to verify
  crash recovery, frozen-worker recovery while unpaused, and no stall false
  positive while paused.
- `world-persistence.spec.ts` verifies named save, resume, fork identity,
  export, and import through the production top-bar controls and IndexedDB
  save store.

**Every worker-control test forces `targetTPS = 1000` before interacting.**
That is the regime where pacing overshoot, futex wake handling, and
snapshot back-pressure are most stressed. A test at default TPS=60 can
pass while high-throughput control is broken.

This suite is the strongest safety net for the worker's pacing math.
If you touch `simLoop()` in `app/web/src/sim/worker.ts`, run it.

## Code anchors

- `crates/evosim/Cargo.toml` → `[features] threads`.
- `app/web/package.json` → `"test:e2e": "playwright test"`.
- `app/web/package.json` → `"docs:lint": "node ../../scripts/docs-lint.mjs"`.
- `scripts/docs-lint.mjs` → mechanical docs drift gate.
- `app/web/playwright.config.ts` → `webServer`, `reuseExistingServer`.
- `app/web/tests/e2e/app-fps.spec.ts` → App FPS choices, persistence, and
  snapshot back-pressure under high target TPS.
- `app/web/tests/e2e/sim-bridge.spec.ts` → worker control-path smoke tests.
- `app/web/tests/e2e/worker-watchdog.spec.ts` → worker crash/freeze recovery
  and paused no-false-positive coverage.
- `app/web/tests/e2e/world-persistence.spec.ts` → saved-world Save/Resume/Fork/
  Export/Import coverage.
- `app/web/tests/e2e/defaults-drift.spec.ts` → generated Rust↔TS config/default drift guard.
- `app/web/tests/e2e/settings-persistence.spec.ts` → Settings localStorage and
  boot slider-state / WorldConfig persistence guard.
- `app/web/tests/README.md` → onboarding pointer; the authoritative
  command/coverage list lives here and in
  [`../agent-context/testing-how-to.md`](../agent-context/testing-how-to.md).
- `clippy.toml` → `disallowed-methods` for the HashMap/HashSet ban.
- `.github/workflows/ci.yml` → which gates run on every push.

## Update when

- A new Rust test suite or integration crate is added.
- The Playwright suite gains, loses, or renames a spec file.
- The docs-lint command or coverage changes.
- The `pnpm test:e2e` command name changes (must stay in sync with
  `app/web/tests/README.md` and `app/web/package.json`).
- The worker-control `targetTPS = 1000` rule is relaxed (would need a
  separate doc + reason, because the rule exists to catch high-throughput
  pacing and back-pressure regressions).
- The determinism guard in `clippy.toml` changes scope.

## Why is it shaped this way

See [`decisions/cross-cutting.md`](../decisions/cross-cutting.md) —
why the threaded-feature gate is a first-class CI run, why Playwright
became necessary, why the `targetTPS = 1000` rule is mandatory.

## See also

- [`build-and-deploy.md`](build-and-deploy.md) — the CI gate matrix.
- [`worker-runtime.md`](worker-runtime.md) — the pacing math the e2e
  suite exists to defend.
- [`../agent-context/testing-how-to.md`](../agent-context/testing-how-to.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
