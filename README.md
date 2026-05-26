# evosim

Browser-deployed idle evolution sandbox. Rust → wasm sim, plain-TS Vite shell,
Canvas 2D rendering. See [docs/README.md](docs/README.md) for the full
documentation index.

v1.3 state: no save/resume, no carrion, no genome (only NN-weight evolution),
no species tracking, no events or Hall of Fame. Walled world. 3-action enum
(`Graze / Eat / Split`). Grass cells 5 world-units. Creature color derived from
NN-weight xxhash64.

## Repo layout

```
/Cargo.toml            single Rust crate, builds to cdylib (wasm) + rlib
/src                   simulation engine
/web                   Vite + TypeScript shell
/web/wasm              wasm-pack output (gitignored, regenerated each build)
/docs                  README, architecture, development, contributing guides
/docs/plans            v1.3 planning docs (canonical record of this pass)
/DECISIONS.md          running log of orchestrator decisions
```

## Local development

Prereqs: Rust stable + nightly (for wasm atomics) with `wasm32-unknown-unknown`,
`wasm-pack`, Node 20+, pnpm.

```bash
# 1. build the Rust → wasm package (requires nightly for atomics/build-std)
rustup toolchain install nightly --component rust-src
rustup target add wasm32-unknown-unknown --toolchain nightly
rustup run nightly wasm-pack build --target web --out-dir web/wasm --dev --features threads

# 2. run the dev server (serves with COOP/COEP set for SharedArrayBuffer)
cd web
pnpm install
pnpm dev
```

The dev server prints a local URL; opening it shows a walled world with grass
cells and evolving creatures.

## Deployment

Static build via `pnpm build` in `web/`. Deploy `web/dist/` to Cloudflare
Pages (or any static host that respects `_headers`). The shipped `_headers`
file sets:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

These are required so `SharedArrayBuffer` (and therefore
`wasm-bindgen-rayon`) is available. The sim degrades gracefully to a
single-threaded path if isolation is unavailable.

## Tests

```bash
cargo test --lib                          # unit tests (default build)
cargo test --lib --features threads       # unit tests (threaded build)
cd web && pnpm typecheck
```

See [docs/development.md](docs/development.md) for full details.
