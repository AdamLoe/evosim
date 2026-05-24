# DECISIONS

Running log of decisions made by the orchestrator for things v5+v6 didn't pin.
Format: `<topic>: <choice> — <one-line why>`.

- cargo layout: single crate at repo root (not workspace) — only one wasm artifact, simpler for v1 size
- web app location: `/web` directory, Vite + TS — keeps Rust and JS shell clearly separated, matches v6 §A
- wasm-pack target: `web` — direct ES module import from Vite, no extra bundler shim
- pkg output location: `web/wasm` — wasm-pack output is consumed only by the web app; co-locate so Vite's import graph and CI step are obvious
- pnpm install path: `~/.local/bin` via user-prefixed npm — `npm -g` blocked by /usr perms on this WSL2 box; corepack also blocked
- web framework: none (plain TS + DOM) — per v6 §A
- node version pin: not pinned in v1 — repo runs on whatever Node 20+ the user/CI has
- CI provider: GitHub Actions — only one set up; v5 says "GitHub Actions or similar"
