# Build and deploy

The wasm-pack incantation, the COOP/COEP headers, and the threaded-bundle
invariants.

## What it is

A Cargo workspace (`Cargo.toml` at `app/`) with a single member crate
(`crates/evosim/`) compiled to wasm via `wasm-pack`, with a `threads`
feature flag that pulls in `rayon` + `wasm-bindgen-rayon`. The web shell
(Vite + TS) imports the wasm output from `app/web/wasm/`. The dev server pins
port 47821 and sets COOP/COEP so `SharedArrayBuffer` is available.
Production deploys static `app/web/dist/`. Hosts that support `_headers`
read `app/web/public/_headers`; the GitHub Pages workflow builds with
`VITE_BASE=/evosim/` and relies on the checked-in COI service-worker shim
for cross-origin isolation.

## What it owns

- The canonical wasm-pack incantation:

  ```bash
  # Run from app/ (the workspace root)
  rustup run nightly wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --dev --features threads
  ```

- The `.cargo/config.toml` link args. They are required and live there
  by design — overriding via `RUSTFLAGS` env vars silently clobbers them.
- The threaded-bundle invariants — two `grep` checks that must both
  succeed after every build:

  ```bash
  grep -c initThreadPool app/web/wasm/evosim.js   # → 2 (threaded), 0 (plain)
  grep -F 'shared:true'  app/web/wasm/evosim.js   # → 1 hit
  ```

- The COOP/COEP headers, set in two places that must stay in sync:
  `app/web/vite.config.ts` for dev + preview, `app/web/public/_headers` for
  Cloudflare Pages and any other static host.
- The `panic = "abort"` setting on both `dev` and `release` profiles —
  required when building `--features threads` for wasm32. Without it,
  the linker silently emits non-shared `WebAssembly.Memory` and
  `wasm-bindgen-rayon`'s `postMessage(memory)` fails with
  `DataCloneError` at boot. These profiles live in the workspace manifest
  (`Cargo.toml` at `app/`).
- The Vite `worker: { format: "es" }` config — `wasm-bindgen-rayon`
  spawns workers via `new URL('./workerHelpers.js', import.meta.url)`;
  Vite must bundle them as ES modules, not the default IIFE.
- The CSP header in `app/web/public/_headers` (allows `'wasm-unsafe-eval'`
  for wasm instantiation).

## What it does NOT own

- **How the wasm code uses threads** — owned by
  [`worker-runtime.md`](worker-runtime.md). This doc owns "the bundle
  is threaded"; the runtime doc owns "the worker calls
  `initThreadPool`".
- **Test commands** — owned by [`testing.md`](testing.md). The build
  steps in CI live here; the test commands themselves live there.
- **What the dev loop feels like** — owned by
  [`../agent-context/dev-loop.md`](../agent-context/dev-loop.md). The
  facts (which command, which port) live here; the agent's procedural
  steps live there.

## Canonical build incantations

**Dev wasm build (always use this for running the sim):**

```bash
# Run from app/ (the workspace root)
rustup run nightly wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --dev --features threads
```

**Release wasm build (only for perf measurements or shipping):**

```bash
# Run from app/ (the workspace root)
rustup run nightly wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --release --features threads
```

**Plain (non-threaded) wasm build — `NOT FOR RUNNING THE SIM`:**

```bash
# Run from app/ (the workspace root)
wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --dev
```

The plain build runs the sim ~1/N× speed because the rayon pool never
initializes, `rayon::current_num_threads()` returns 1, and the whole NN
forward pass runs on a single thread with no error or warning. Only use
this if you're specifically exercising the `cfg(not(feature =
"threads"))` code path.

**Verify the bundle is threaded after every Rust change:**

```bash
grep -c initThreadPool app/web/wasm/evosim.js   # → 2 = threaded, 0 = plain
grep -F 'shared:true'  app/web/wasm/evosim.js   # → one hit = shared memory wired
```

**Runtime confirmation (DevTools console):**

```
[threads] main thread: SharedArrayBuffer=true crossOriginIsolated=true
[sim] crossOriginIsolated=true
[sim] rayon workers: N (hardware: M)
```

If `[sim] rayon collapsed to 1 thread — sim will run single-threaded`
appears, the wasm bundle is threaded but rayon didn't pool — almost
always a COOP/COEP miss or a stale wasm bundle.

## Toolchain

- **Native + tests + non-threaded wasm**: stable, pinned to `1.95.0` in
  `rust-toolchain.toml`. Includes clippy + rustfmt.
- **Threaded wasm**: nightly, invoked explicitly via `rustup run nightly
  wasm-pack ...`. Required for the atomics + bulk-memory target
  features and `-Z build-std=panic_abort,std`.
- **Node**: 20+; not pinned. `pnpm` is the only supported package
  manager. Installed at `~/.local/bin/pnpm` on the dev box.

## Link args (`.cargo/config.toml`)

`[target.wasm32-unknown-unknown]` rustflags include:

- `-C target-feature=+atomics,+bulk-memory,+mutable-globals,+simd128`
- `-C link-arg=--shared-memory`
- `-C link-arg=--max-memory=4294967296`
- `-C link-arg=--import-memory`
- `-C link-arg=--export=__heap_base` (so wasm-bindgen finds the TLS
  injection point)
- `-C link-arg=--export=__wasm_init_tls` / `__tls_size` / `__tls_align`
  / `__tls_base`

`[unstable] build-std = ["panic_abort", "std"]` is required because
atomics are not stabilized in the wasm target.

**`+atomics` does not imply `--shared-memory`** — wasm-ld emits
non-shared memory unless told otherwise, and `wasm-bindgen-rayon`'s
`postMessage(memory)` then fails with `DataCloneError` at boot. The
explicit `--shared-memory` link arg is load-bearing.

Native (`x86_64`) builds are unaffected; the `[target.*]` block scopes
the flags.

## COOP/COEP

The required headers everywhere wasm runs:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

Set in `app/web/vite.config.ts` for `server` and `preview` blocks, and in
`app/web/public/_headers` for static deploy. If either copy drifts, the
sim "looks fine" but the rayon pool silently collapses to 1 thread.

`app/web/public/_headers` also pins a CSP that allows `'wasm-unsafe-eval'`
for wasm instantiation and the standard caching rules for
`/assets/*` and `/wasm/*`.

## Vite + dev server

- Port: `47821` (`strictPort: true` — fails hard if held).
- Host: `0.0.0.0` so WSL2→Windows works.
- HMR: TS hot-reloads; the `.wasm` is re-fetched on full reload.
- `app/web/wasm/` is gitignored and **not** auto-rebuilt by `pnpm dev` —
  every Rust change requires manual `wasm-pack build ...`.

## Production deploy

`pnpm build` in `app/web/` runs `tsc --noEmit && vite build` and emits
`app/web/dist/`. The GitHub Pages workflow sets `VITE_BASE=/evosim/` before
that build and uploads `app/web/dist/`; Cloudflare Pages and other hosts that
respect `_headers` read `app/web/public/_headers` directly.

The release wasm goes through `wasm-opt -O4 --enable-bulk-memory
--enable-mutable-globals` via
`package.metadata.wasm-pack.profile.release` in `crates/evosim/Cargo.toml`.

## CI

`.github/workflows/ci.yml` runs:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo clippy --all-targets --features threads -- -D warnings`
- `cargo test --lib`
- `cargo test --lib --features threads`
- `rustup run nightly wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --release --features threads`
- (Web job, depends on Rust job) `pnpm docs:lint` + `pnpm build`

## Code anchors

- `Cargo.toml` (workspace, at `app/`) → `[profile.dev] panic = "abort"`,
  `[profile.release] panic = "abort"`.
- `crates/evosim/Cargo.toml` → `[features] threads`,
  `[package.metadata.wasm-pack.profile.release]`.
- `.cargo/config.toml` → `[target.wasm32-unknown-unknown]` rustflags,
  `[unstable] build-std`.
- `rust-toolchain.toml` → pinned `1.95.0` stable.
- `web/vite.config.ts` → `crossOriginIsolationHeaders`, port `47821`,
  `worker: { format: "es" }`.
- `web/public/_headers` → COOP/COEP, CSP, caching rules.
- `.github/workflows/ci.yml` → gate matrix.
- `.github/workflows/deploy-pages.yml` → GitHub Pages `VITE_BASE` deploy.

## Update when

- The wasm-pack invocation changes shape (new feature flag, different
  output dir, different target).
- A link arg is added or removed.
- The nightly toolchain stops needing one of the `+atomics`-related
  flags (or starts needing a new one).
- The COOP/COEP requirements change browser-side.
- The pinned stable toolchain version moves.
- A new gate is added or removed in CI.

## Why is it shaped this way

See [`decisions/build.md`](../decisions/build.md) — the
`--features threads` default rule, the `panic = "abort"` requirement,
the two-place COOP/COEP duplication, the gitignored `app/web/wasm/`.

## See also

- [`worker-runtime.md`](worker-runtime.md)
- [`testing.md`](testing.md)
- [`../decisions/build.md`](../decisions/build.md)
- [`../agent-context/dev-loop.md`](../agent-context/dev-loop.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- [Doc authoring rules](~/agent-docs/v1/rules/authoring-rules.md)
