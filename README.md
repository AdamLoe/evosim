# evosim

Browser-deployed idle evolution sandbox. Rust → wasm sim, plain-TS Vite shell,
Canvas 2D rendering. Spec lives in [docs/PITCH-v5.md](docs/PITCH-v5.md) and
[docs/PITCH-v6.md](docs/PITCH-v6.md); v6 supersedes v5 on conflict.

## Repo layout

```
/Cargo.toml            single Rust crate, builds to cdylib (wasm) + rlib
/src                   simulation engine
/web                   Vite + TypeScript shell
/web/wasm              wasm-pack output (gitignored, regenerated each build)
/docs                  PITCH-v1..v6, ORCHESTRATOR.md, original notes
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
cargo test           # native unit tests
cd web && pnpm typecheck
```

A headless acceptance test (PITCH v5 §16) lands in Milestone F.
