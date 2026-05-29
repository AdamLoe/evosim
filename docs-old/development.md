# Development

## Prerequisites

- **Rust stable** with the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)
- **wasm-pack** (`curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh`)
- **Node 20+**
- **pnpm** — on this machine it is installed at `~/.local/bin/pnpm`; ensure `~/.local/bin` is on your `PATH`

## Build commands

```bash
# Rust native (for tests only — no wasm output)
cargo build

# Debug wasm build — ALWAYS use --features threads. The plain command
# (without --features threads) silently produces a single-threaded
# bundle that runs the NN pass on the main thread. See
# docs/dev-server-prompt.md §3 for details.
rustup run nightly wasm-pack build --target web --out-dir web/wasm --dev --features threads

# Release wasm build (used by CI and pnpm build)
rustup run nightly wasm-pack build --target web --out-dir web/wasm --release --features threads

# Install JS dependencies (run once; re-run after pnpm-lock.yaml changes)
cd web && pnpm install

# Type-check + bundle (output to web/dist/)
cd web && pnpm build

# Dev server with COOP/COEP headers
cd web && pnpm dev
```

After every Rust change, verify the bundle is threaded:

```bash
grep -c initThreadPool web/wasm/evosim.js   # → 2 (threaded) or 0 (broken)
grep -F 'shared:true'  web/wasm/evosim.js   # → one hit
```

`web/wasm/` is gitignored. Rebuild it after any Rust change — the JS will silently use the old wasm otherwise.

## Test commands

```bash
# Unit tests (native, fast)
cargo test --lib

# §16 acceptance test (release build, checks golden hash)
cargo test --release --test acceptance

# TypeScript type-check
cd web && pnpm typecheck

# Clippy (must be clean)
cargo clippy --all-targets -- -D warnings

# Formatter check
cargo fmt --all -- --check
```

## The acceptance test

`tests/acceptance.rs` runs the sim for 10,000 ticks on seed `evosim-test-001` and checks four criteria (v5 §16): population > 0, >= 2 species, < 8s wall-time, and world hash matches the golden value in `tests/golden_snapshot_t10000.txt`.

To re-bootstrap the golden after a legitimate sim-output change:

```bash
EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance
```

This overwrites `tests/golden_snapshot_t10000.txt`. Commit the new golden alongside the change that required it. Do not re-bootstrap to silence a test failure caused by a bug.

## CI workflow

`.github/workflows/ci.yml` runs two jobs:

1. **rust** — `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --lib`, `cargo test --release --test acceptance`, `wasm-pack build --release`
2. **web** (needs rust) — `pnpm install --frozen-lockfile`, `pnpm build` (which runs typecheck internally)

Both jobs run on `ubuntu-latest`. Rust cache via `Swatinem/rust-cache`.

## Deploy target

Cloudflare Pages serving `web/dist/`. The file `web/public/_headers` sets `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` — required for `SharedArrayBuffer` (and therefore the rayon threads path). The sim degrades to single-threaded if these headers are absent.

## Common pitfalls

- **Forgot to rebuild wasm.** If JS behavior looks wrong after a Rust change, run `rustup run nightly wasm-pack build --target web --out-dir web/wasm --dev --features threads` and hard-refresh. (Always include `--features threads` — see §"Build commands" above.)
- **pnpm not found.** On this machine: `export PATH="$HOME/.local/bin:$PATH"` or use the full path `~/.local/bin/pnpm`.
- **Clippy failure blocks CI.** Fix before pushing; `cargo clippy --all-targets -- -D warnings` locally.
- **Golden mismatch after balance change.** If you intentionally changed a constant that affects sim output, re-bootstrap the golden (see above). If unexpected, investigate before re-bootstrapping.
- **`pnpm install --frozen-lockfile` fails on CI after adding a package.** Commit the updated `pnpm-lock.yaml`.

## Dev-panel sliders

The five dev sliders (v5 §11 / v6 §K) are wired in Rust (`WorldHandle::set_slider`) but have no DOM UI in v1 (see [../BUILD-REPORT.md](../BUILD-REPORT.md) Known Issue #4). Use the JS console:

```js
world.set_slider("base_sun_rate", 0.5)
world.set_slider("mutation_rate_multiplier", 2.0)
world.set_slider("sun_gradient_strength", 1.5)
world.set_slider("mouth_tax", 0.05)
world.set_slider("nn_mutation_sigma", 0.02)
```

`world` is exposed on `window` by `main.ts`. Slider ranges and default values: `web/src/main.ts` (ranges) and `src/constants.rs` / `world.rs::DevSliders::default()` (defaults).

## Where decisions are documented

[../DECISIONS.md](../DECISIONS.md) — running log of every non-spec implementation choice (about 50 entries for v1). If you're puzzled by why something is the way it is, check there first before reading the milestone plan docs in `docs/archive/plans/`.
