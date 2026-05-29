# evosim docs

evosim is a browser-deployed idle evolution sandbox: creatures live on a 600x600 canvas, photosynthesize, eat each other, and evolve via a fixed-shape neural network. The simulation is written in Rust and compiled to wasm; the shell is plain TypeScript + Vite with Canvas 2D rendering and no framework.

## Status

v1 shipped. §16 headless acceptance: 4/4 pass (population > 0 at tick 10,000; >= 2 species; < 8s wall-time; hash matches golden). 77 unit tests + 1 acceptance test, all green. CI clean. See [../BUILD-REPORT.md](../BUILD-REPORT.md) for full details and known issues.

## Quick start

Prerequisites: Rust stable + `wasm32-unknown-unknown` target, `wasm-pack`, Node 20+, pnpm. On this box pnpm is at `~/.local/bin/pnpm`.

```bash
# Build wasm
wasm-pack build --target web --out-dir web/wasm --dev

# Install JS deps and start dev server (from repo root)
cd web && pnpm install && pnpm dev

# Run tests
cargo test --lib                          # 77 unit tests
cargo test --release --test acceptance    # §16 acceptance gate
cd web && pnpm typecheck                  # TypeScript check
```

## Where to read next

| Goal | Doc |
|---|---|
| Understand the system shape and every module | [architecture.md](architecture.md) |
| Set up a working dev environment | [development.md](development.md) |
| Make a code change or add a feature | [contributing.md](contributing.md) |
| Understand the full game mechanics spec | [archive/PITCH-v5.md](archive/PITCH-v5.md) |
| See what changed between v5 and v6 | [archive/PITCH-v6.md](archive/PITCH-v6.md) |

## Where the design lives

[archive/PITCH-v5.md](archive/PITCH-v5.md) is the primary spec (tick ordering, genome, NN, energy economy, species detection, persistence, UI, acceptance criteria). [archive/PITCH-v6.md](archive/PITCH-v6.md) patches it (stack, render, NN details, slider defaults, HoF definitions, snapshot hash). v6 overrides v5 on any conflict. Both are binding for v1.
