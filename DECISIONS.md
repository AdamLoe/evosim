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
- founder starting energy: 200 — v5 doesn't pin this; at 20 the founder dies before reaching split with v5's actual balance numbers (steady-state photosynth 0.08/tick vs upkeep 0.15/tick). 200 gives a fat buffer so the placeholder behavior gets multiple split cycles; long-term sustainability is a brain/balance concern (F.30)
- past-lifespan penalty: interpreted as `4^(excess_ticks/1000)` compound multiplier on upkeep (capped at 1e6) — v5 §7's "multiplied by 4 per 1000-tick excess" is ambiguous; the compound reading makes age a hard cliff per design intent
- Milestone B placeholder action: split when energy >= 80 (not the spec's 50 floor). At 50, parent immediately starves post-split with no gift left for child; 80 leaves a 30-energy buffer for the child. Replaced wholesale by the NN in Milestone D
- internal body radius unit: world units, 1.0 per genome size unit — render layer scales 2.5 px per size at base zoom per v6 §B

