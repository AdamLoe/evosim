# Coding style — evosim

The universal principles and the generic per-language idioms live in
`~/agent-docs/v1/rules/coding-style.md` — read that first. This
file lists only this app's specific values, named patterns, and footguns.

## When does this apply

You're editing Rust in `app/crates/evosim/src/` or TypeScript in `app/web/`.

## Rust specifics

**Do:**

- Run `cargo fmt --all` before committing. CI checks it.
- Run `cargo clippy --all-targets -- -D warnings` AND
  `cargo clippy --all-targets --features threads -- -D warnings`. Both
  must be clean.
- Use SoA layout when adding per-creature state. New fields go on
  `CreatureSoA` (with parallel `Vec`s the tick code can iterate in a
  flat loop), not on a per-creature struct.
- Promote per-tick scratch buffers to long-lived `World` fields. The
  pattern is `scratch_*: Vec<T>` cleared at the top of each use and
  refilled. See existing `scratch_neighbors`, `scratch_damage`,
  `scratch_cull_pool` for the shape.
- Constant-assert layout invariants you care about:
  `const _: () = assert!(NN_WEIGHT_COUNT == 2808);`.
- Gate threaded-only helpers behind `#[cfg(feature = "threads")]`. Keep
  a sequential fallback path so the plain build compiles.
- Open profile spans via `profile_span!(&self.profile, "tick.X")` for
  RAII spans inside `World::step`, or
  `Profiler::record_under_root("nn", "forward.l1", dur_us)` for
  cross-worker atomic-accumulator paths.
- Use `self.rng` for any RNG draw — never `getrandom` directly inside
  a tick. The world's `SimRng` is the single deterministic source.

**Don't:**

- Use `HashMap` or `HashSet` iteration (`iter`, `iter_mut`,
  `into_iter`) in sim-critical files. `clippy.toml` rejects it. Use
  `BTreeMap` / `BTreeSet` / sorted `Vec` if iteration is needed.
- Bare-name a constant that's only used as the default seed for a
  live-tunable slider. Suffix it with `_DEFAULT` so the role is
  obvious at the use site and the drift-guard pairing is grep-stable.
  Example: `SPLIT_THRESHOLD_DEFAULT`, not `SPLIT_THRESHOLD`. Fixed
  constants that are *not* slider defaults (e.g. `UPKEEP_BASE`,
  `WORLD_SIZE`) stay bare.
- Add per-typed wasm-bindgen setters. `set_slider(name, value)` is the
  sole external mutation entry point — add an `apply_X` helper and a
  `try_set_slider` arm instead.
- Call `web_sys::window()` from anything that might run in the sim
  worker (e.g., from `wasm_now_ms`). It returns `None` in a Worker
  scope. Use `crate::profiler::clock_now_us_threadsafe()` (the
  scope-agnostic `performance.now()` binding).
- Add a Rust-side profile span for code that's called from JS, not
  from `World::step`. The Rust profiler stack assumes Rust callers;
  spans opened from JS-driven entry points orphan at the root level.
  Use a TS-side `span()` instead.
- Add a `cross_origin_isolated` Rust export. Both sides read it from
  their JS global directly.

## TypeScript specifics

**Do:**

- Type discriminated unions for any cross-thread message — extend
  `SimMessage` / `SimReply` in `app/web/src/sim/bridge.ts`. The
  discriminator field is `kind: "..."`. Handler switches must
  exhaustively cover every variant.
- Route every wasm-touching call through `SimBridge`. Main never holds
  a `WorldHandle`. The bridge handles the request_id correlation, the
  TTL, the per-name slider debouncer, and the futex wake.
- Open spans via `span("frame.X")` from `app/web/src/perf.ts`. Use the
  full dotted prefix that matches the tree position; the panel renders
  by name.
- Read `crossOriginIsolated` from the current global directly
  (`globalThis.crossOriginIsolated` on main,
  `(self as unknown as { crossOriginIsolated?: boolean }).crossOriginIsolated`
  in the worker). Don't round-trip through wasm.
- Run `pnpm typecheck` + `pnpm build` before committing TS changes. The
  build runs `tsc --noEmit && vite build`.
- For new persisted settings, add the key to `app/web/src/settings.ts →
  Settings` AND `DEFAULTS`. The loader picks only keys present in
  `DEFAULTS`, so a missing key in the type silently drops the value
  on load. The unknown-key filter at the same site is the migration
  path for removed keys — no further cleanup needed.

**Don't:**

- Replace synchronous `Atomics.wait` with `Atomics.waitAsync` in the
  worker loop.
  See [`../decisions/sim.md`](../decisions/sim.md) and the comment in
  `app/web/src/sim/worker.ts → simLoop`.
- Add `await`, `Promise.resolve()`, or `setTimeout` to `simLoop`.
  Steady-state control is SAB-only, so the loop is intentionally
  synchronous and does not feed a postMessage hot path.
- Source restart-time slider values from `getSettings()` (localStorage).
  Use `currentSliderState()` (in-memory widget values) so a mid-drag
  restart carries the dragged value.
- Build a new `Float32Array` over wasm linear memory and keep it
  across ticks — wasm memory can grow and the view detaches. The SAB
  views in `app/web/src/main.ts → frame` are intentionally rebuilt per RAF.
- Use `BigInt` for creature ids. They come out of wasm as `f64`;
  reassemble u32 pairs via `idHi * 4294967296 + idLo` if you need to
  decode the SAB stride directly.
- Use `text-transform: uppercase` or all-lowercase labels in user-
  facing UI. Section headers and slider labels are sentence-case
  ("Energy max", "Show profiler").

## See also

- [`index.md`](index.md)
- [`repo-rules.md`](repo-rules.md)
- [`dev-loop.md`](dev-loop.md)
- [`testing-how-to.md`](testing-how-to.md)
- [`maintaining-docs.md`](maintaining-docs.md)
- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
