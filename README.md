# evosim

Browser-deployed idle evolution sandbox. Rust → wasm sim, plain-TS Vite shell,
Canvas 2D rendering. Spec lives in [docs/archive/PITCH-v5.md](docs/archive/PITCH-v5.md) and
[docs/archive/PITCH-v6.md](docs/archive/PITCH-v6.md); v6 supersedes v5 on conflict.
See [docs/README.md](docs/README.md) for the full documentation index.

## Repo layout

```
/Cargo.toml            single Rust crate, builds to cdylib (wasm) + rlib
/src                   simulation engine
/web                   Vite + TypeScript shell
/web/wasm              wasm-pack output (gitignored, regenerated each build)
/docs                  README, architecture, development, contributing guides
/docs/archive          PITCH-v1..v6, ORCHESTRATOR.md, original notes, milestone plans
/DECISIONS.md          running log of orchestrator decisions outside v5+v6
```

## Local development

Prereqs: Rust stable with `wasm32-unknown-unknown`, `wasm-pack`, Node 20+, pnpm.

```bash
# 1. build the Rust → wasm package
wasm-pack build --target web --out-dir web/wasm --dev

# 2. run the dev server (serves with COOP/COEP set for SharedArrayBuffer)
cd web
pnpm install
pnpm dev
```

The dev server prints a local URL; opening it should show a bouncing blue
circle (Milestone A walking skeleton).

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
cargo test --lib                          # 77 unit tests
cargo test --release --test acceptance    # §16 acceptance gate (golden hash)
cd web && pnpm typecheck
```

See [docs/development.md](docs/development.md) for full details including how to re-bootstrap the golden snapshot.
