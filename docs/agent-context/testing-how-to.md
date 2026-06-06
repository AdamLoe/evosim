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
```

### TypeScript

```bash
# From app/web/
cd app/web
pnpm typecheck            # tsc --noEmit
pnpm build                # also runs typecheck, then vite build
```

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
  TPS regime where the `Atomics.waitAsync(0)` regression class
  surfaces. A test at default TPS=60 passes on the buggy commit and
  misses the regression entirely.
- Read state back via the status bar (`#status` `textContent`:
  `seed: X · tick N · pop P`) or via a downstream observable
  (`#profiler-trees` populated, the dev-panel showing a new value).
- If you need worker-internal state that no UI exposes, plumb it
  through `request_profile_report` — it already round-trips JSON.

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
   `app/web/src/sim/worker.ts → simLoop`: the 1 ms `timeoutMs` floor,
   the macrotask yield on the not-equal branch, the
   `drainMessages()` at the top of the loop. The bug class has
   shipped twice; the test exists to fail in exactly this regime.
5. **Playwright `profile toggle` fails.** Four trees should populate
   within 4 s of toggling. If only some populate, a `record_under_root`
   call is missing or routed to the wrong tree. See
   [`../architecture/profiler.md`](../architecture/profiler.md).
6. **Playwright `restart 'r'`** fails. The worker isn't respawning or
   isn't accepting `boot` on the new instance. Look at `app/web/src/main.ts →
   restart` sequence and the worker's `handleBoot`.

## Per-commit gate suite (no code changes)

For doc-only commits, the heavy gates can stay skipped, but a sanity
pass still makes sense:

```bash
# From app/ workspace root
cargo fmt --all --check     # in case formatter rules drifted
cargo build                 # cheap; catches accidental compile-break
cd app/web && pnpm typecheck && cd -
```

## See also

- [`index.md`](index.md)
- [`../architecture/testing.md`](../architecture/testing.md)
- [`coding-style.md`](coding-style.md)
- [`repo-rules.md`](repo-rules.md)
- [`dev-loop.md`](dev-loop.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`maintaining-docs.md`](maintaining-docs.md)
