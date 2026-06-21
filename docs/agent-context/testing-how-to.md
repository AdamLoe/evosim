# Testing how-to

How to run tests. How to add a new one. What to do when one fails.

## When does this apply

You're about to commit a code change, or you're triaging a flake, or
you're adding a new test. The catalogue of suites + what each covers
lives in [`../architecture/testing.md`](../architecture/testing.md);
this doc is the procedural side.

## Running the gates

### Rust unit tests

From the `app/` workspace root:

```bash
cargo test --lib                          # default features
cargo test --lib --features threads       # parallel paths
```

Both must pass. The threaded run exercises the rayon NN forward and the
parallel grass propagation; default-feature runs the sequential
fallback. If only one passes, you've broken cross-feature equivalence.

Standard clippy / fmt gates (also from `app/`):

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features threads -- -D warnings
cargo bench --no-run
```

`cargo bench --no-run` is a compile gate for Criterion benches; use it
instead of `cargo build --benches`, because the workspace's
`panic = "abort"` profiles conflict with Criterion's unwind requirement.

### TypeScript

```bash
# From app/web/
cd app/web
pnpm docs:lint           # mechanical docs drift gate
pnpm typecheck            # tsc --noEmit
pnpm build                # also runs typecheck, then vite build
```

`pnpm docs:lint` runs `scripts/docs-lint.mjs` from the repo root. It
checks local docs links/paths, ownership/index routes, generated Rust↔TS
constant mirrors, synchronous `Atomics.wait` worker pacing invariants,
and profiler tree/span drift. It is the first gate to run for docs-only
changes.

### Playwright e2e

```bash
# From app/web/
cd app/web
pnpm install              # one-time
pnpm test:e2e
```

The runner boots Vite via Playwright's `webServer` hook, so you do not
need a dev server already running. If one is on `:47821`, it is reused
(`reuseExistingServer: true`).

If chromium is missing:

```bash
npx playwright install chromium
```

Headed / debug / single-test:

```bash
pnpm test:e2e --headed
pnpm test:e2e --debug
pnpm test:e2e --project chromium --grep "pause"
```

## Adding a Rust unit test

Standard `#[cfg(test)] mod tests { ... }` at the bottom of the relevant
source file. Conventions:

- Use `World::new(seed_string)` or a `DevSliders` override for tight
  control.
- Use the seeded `SimRng` — never `getrandom` directly.
- For SAB layout tests, the native twin `write_snapshot_to_native`
  exists in `app/crates/evosim/src/wasm_api/mod.rs` and writes the same bytes into
  `Vec<u8>`s — use it instead of trying to construct a
  `js_sys::Uint8Array` off-wasm (the latter panics).
- Don't assert on `HashMap` iteration order. The clippy guard rejects
  `iter` calls there anyway; use `BTreeMap` if you need ordering.

## Adding a Playwright test

Add to `app/web/tests/e2e/sim-bridge.spec.ts` (or a new spec next to it).
Conventions:

- **Force `targetTPS = 1000` before interacting.** This is the only
  TPS regime where pacing overshoot, futex wake handling, and
  snapshot back-pressure failures reliably surface. A test at default
  TPS=60 can pass while high-throughput control is broken.
- Read state back via the status line (`#perf-status-line` `textContent`:
  `seed: X · tick N · pop P`) or via a downstream observable
  (`#profiler-trees` populated, the dev-panel showing a new value).
- If you need worker-internal state that no UI exposes, plumb it
  through `request_profile_report` — it already round-trips JSON.
- Settings rail and paused-canvas regressions have a focused smoke in
  `app/web/tests/e2e/settings-rail.spec.ts`; run it directly with
  `pnpm test:e2e -- settings-rail.spec.ts` when touching rail tabs, Escape
  handling, app badge placement, or paused repaint behavior.

## What to do when a test fails

1. **`cargo test --lib` default passes, `--features threads` fails.**
   You've broken cross-feature equivalence. Most often the parallel
   NN path now produces different output for the same seed; check
   `nn_forward_all_chunks` and any new mutations.
2. **Both clippy runs fail with `-D warnings`.** Read the lint. If it
   names a `HashMap::iter`, see
   [`coding-style.md`](coding-style.md) — the determinism guard is
   binding.
3. **`pnpm typecheck` fails after a Rust change.** The wasm-pack
   regen may have updated `app/web/wasm/evosim.d.ts` in a way TS
   doesn't like (a renamed export, a dropped method). Sync the TS
   consumer.
4. **Playwright `pause + resume` or `target TPS` or `slider change`
   fails.** This is the smoke for the worker control path. Check
   `app/web/src/sim/worker.ts → simLoop`: synchronous `Atomics.wait`,
   futex wake handling, `readControlSab()` at the top of the loop, and
   ack-gated snapshot publication. The test exists to fail in the
   high-throughput regime where those regressions surface.
5. **Playwright `profile toggle` fails.** Five trees should populate
   within 4 s of toggling. If only some populate, a `record_under_root`
   call is missing or routed to the wrong tree. See
   [`../architecture/profiler.md`](../architecture/profiler.md).
6. **Playwright `restart 'r'`** fails. The worker isn't respawning or
   isn't accepting `boot` on the new instance. Look at `app/web/src/main.ts →
   restart` sequence and the worker's `handleBoot`.

## Perf bench procedure (before/after comparison)

Use this when measuring the impact of a sim performance change. The
canonical fixed scenario: `world_size=9600`, `grass_cell_size=5.0`,
`world_seed=42`, `?seed=bench-v2.0.5`, all other sliders at
defaults (see `crates/evosim/src/constants.rs`). This scenario yields
`grass_dim=1920` cells/axis, `hash_dim=480` cells/axis.

### Seed policy

| Field | Value |
|---|---|
| `world_seed` | **42** — pins biome and grass-clump layout (via SplitMix64 in `world/biome.rs`). |
| URL string seed | **`?seed=bench-v2.0.5`** — pins founder brains/genomes and the sim RNG stream. |

**Threading note:** the threaded scatter kernel uses lossy relaxed
cross-tile writes; trajectories can still diverge after tick 0 even with
both seeds pinned. Use warmup + windowed averaging rather than
exact-output matching.

### TPS methodology

Set `targetTPS = 9999` (uncapped) before measuring. The default 180 TPS
cap causes the worker to sleep for the remainder of each time slice — a
fast tick still pads to 5.6 ms, making improvements invisible in the
profiler. Uncapped, the profiler reports raw compute cost; the achieved
TPS counter is the primary perf number.

### Run procedure (copy-pasteable)

```
BEFORE EACH RUN:
  1. Open /?seed=bench-v2.0.5, then DevTools → Application → Storage →
     Clear site data (or press "Reset settings"). Reload the same seeded
     URL after clearing — this pins the string RNG seed while resetting
     localStorage to defaults.
  2. Set world_seed = 42 in the dev panel. All other construction-only
     sliders should be at defaults (world_size=9600, grass_size=5.0,
     wrap_world=true, founder_count=32, species_mode=false, etc.).
  3. Click Apply, then hard-reload /?seed=bench-v2.0.5.
     Do NOT click Restart: Restart clears the URL string seed.
  4. Set targetTPS = 9999 (uncapped) via the TPS slider.
  5. Fix the browser viewport to 1920 × 1080 CSS px (DevTools device
     emulation). This pins the snapshot LOD path to mip level 0.

WARMUP:
  6. Wait until the population counter is no longer in rapid exponential
     growth (typically tick 1000–3000 at these settings).
  7. Click "Reset profiler + jank" to clear accumulated samples.
  8. Wait for ≥ 500 more ticks within the 10 s profiler window.

RECORD:
  9. Capture achieved TPS, current population, current tick, and the
     mean durations for the tick and sim_worker span subtrees listed in
     architecture/profiler.md.
  10. In DevTools console: copy(window.__lastProfilerReport) to capture
      the full Rust profile tree.
```

### Limitations

- Per-tick trajectory is not pinnable under threaded scatter.
- Rayon thread count is not directly pinnable from the UI; note the
  machine's core count in any before/after table.
- Canvas physical pixel count depends on `devicePixelRatio`; the
  `write_snapshot` LOD path uses CSS viewport dimensions, so DPR does
  not affect snapshot cost.

See [`../architecture/profiler.md`](../architecture/profiler.md) for the
full profiler span tree and what each span measures.

## Per-commit gate suite (no code changes)

For doc-only commits, the heavy gates can stay skipped, but a sanity
pass still makes sense:

```bash
# From app/ workspace root
cargo fmt --all --check     # in case formatter rules drifted
cargo build                 # cheap; catches accidental compile-break
cd app/web && pnpm docs:lint && pnpm typecheck && cd -
```

## See also

- [`index.md`](index.md)
- [`../architecture/testing.md`](../architecture/testing.md)
- [`coding-style.md`](coding-style.md)
- [`repo-rules.md`](repo-rules.md)
- [`dev-loop.md`](dev-loop.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`maintaining-docs.md`](maintaining-docs.md)
