# Testing

What test suites exist, what each covers, where they live.

## What it is

Two suites:

1. **Rust unit tests** — `cargo test --lib`, run both with the default
   feature set and with `--features threads`. Live next to the code as
   `#[cfg(test)] mod tests` blocks. Cover sim invariants, wasm-bindgen
   surface behaviour, NN forward + decode, grass propagation, slider
   dispatch, snapshot byte layout.
2. **Playwright e2e** — `cd web && pnpm test:e2e`. One spec file at
   `web/tests/e2e/sim-bridge.spec.ts` covering the main↔worker control
   path: pause, target TPS, slider change, profile toggle, restart.
   Boots Vite via Playwright's `webServer` hook so the suite is one
   command.

There is no Rust integration-test crate, no goldens, no snapshot-hash
acceptance, no save-load round-trip. The sim has no persistence layer
to round-trip.

## What it owns

- The list of test suites and what each one covers.
- The convention that the threaded clippy run + the threaded test run
  are first-class gates, not opt-in.
- The "every Playwright test forces `targetTPS = 1000` before
  interacting" rule, because that is the regime where the worker's
  pacing math degenerates and the message-path regressions surface.
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
| `src/wasm_api.rs` | `write_snapshot_to_native` layout matches the documented byte stride; `max_pop_for_sim()` mirrors the constant; `creature_at` returns stable ids; `set_slider` dispatch round-trip. |
| `src/world/mod.rs` | Tick step body, slider effects on world construction, multi-founder spawn placement. |
| `src/world/tick.rs` | Per-phase invariants — graze energy conservation, eat per-bite math, repulsion clamping, death/birth bookkeeping. |
| `src/world/nn.rs` | NN input layout, slot offsets, threaded NN matches sequential NN bit-for-bit (when seeded), chunk-range partition invariants. |
| `src/world/proximity.rs` | Sector LUT correctness, wall proximity edges, grass density bilinear seam wrap. |
| `src/brain.rs` | Forward-pass shape, Leaky ReLU sign behaviour, mutation produces finite values. |
| `src/grass.rs` | Density init, in-cell growth, two-pass separable Gaussian propagation (active-tile path equivalence to full-grid reference), dirty-tile quantize correctness, bilinear sample, row-has-density bitset rebuild. See also `src/grass_v201_tests.rs` for the v2.0.1 active-tile + dirty-tile test module. |
| `src/grid.rs` | `cell_of` boundary clamping, `for_each_in_radius` enumeration. |
| `src/profiler.rs` | Ring buffer pruning, four-tree minting via `ensure_root`, RAII span correctness. |

Determinism gates: `clippy.toml` forbids `HashMap` / `HashSet`
`iter*` in sim-critical files (non-deterministic order would silently
make tests flaky); use `BTreeMap` / sorted `Vec` if iteration is needed.

## Playwright e2e (`web/tests/e2e/sim-bridge.spec.ts`)

Run:

```bash
cd web
pnpm install            # one-time
pnpm test:e2e
```

Playwright boots Vite itself via its `webServer` hook (see
`web/playwright.config.ts`); a running dev server on `:47821` is
reused (`reuseExistingServer: true`).

Tests:

- **pause + resume** — clicks `#playpause-btn`, asserts the tick counter
  stops within ~300 ms and resumes on the second click.
- **target TPS** — selects values in `#target-tps-input` and asserts the
  observed tick rate tracks the dropdown.
- **slider change** — opens the dev panel, edits `basic upkeep`, asserts
  no `set_slider … rejected` warning lands on the console.
- **profile toggle** — toggles `show profiler` + `#profiler-enable`,
  asserts all four stacked profiler tables populate within 4 s.
- **restart `r`** — presses the `r` hotkey, asserts the tick counter
  resets (drops below the pre-restart value).

**Every test forces `targetTPS = 1000` before interacting.** That is
the regime where `1000/targetTPS - elapsed` clamps to 0 and
`Atomics.waitAsync(.., 0)` returns synchronously without yielding to
the event loop — the regression class the suite exists to catch. A
test at default TPS=60 passes on a buggy commit; the suite exists
because that bug class has shipped twice.

This suite is the strongest safety net for the worker's pacing math.
If you touch `simLoop()` in `web/src/sim-worker.ts`, run it.

## Code anchors

- `Cargo.toml` → `[features] threads`.
- `web/package.json` → `"test:e2e": "playwright test"`.
- `web/playwright.config.ts` → `webServer`, `reuseExistingServer`.
- `web/tests/e2e/sim-bridge.spec.ts` → the five smoke tests.
- `web/tests/README.md` → onboarding instructions (kept in sync with this
  doc).
- `clippy.toml` → `disallowed-methods` for the HashMap/HashSet ban.
- `.github/workflows/ci.yml` → which gates run on every push.

## Update when

- A new Rust test suite or integration crate is added.
- The Playwright suite gains, loses, or renames a spec file.
- The `pnpm test:e2e` command name changes (must stay in sync with
  `web/tests/README.md` and `web/package.json`).
- The "every test at TPS=1000" rule is relaxed (would need a separate
  doc + reason, because the rule exists to catch a specific bug class).
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
