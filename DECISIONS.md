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
- ring order: eye→move→scav→mouth→armor (innermost→outermost) — v6 §B fixes only eye=innermost; rest are aesthetic (armor as outermost = shell metaphor)
- eye_offsets semantics: per-active-slot additive offset off slot center — v5 §10 ambiguous; treats offsets as fine-grain, matches σ=0.1 rad scale in v6 §F
- CARRION_RADIUS_FOR_VISION: 1.5 world units — not in spec; small enough not to dominate but large enough a ray reliably hits a corpse blob
- eat-while-cooldown cost-charge: B charges attempt cost even in cooldown; D's valid-fallthrough will avoid issuing Eat in cooldown — punt fix to D
- carrion vision color: fixed gray (0.4, 0.4, 0.4) per v6 §4 "What changed vs v5", regardless of dead creature's original pigment
- cell-to-carrion index: rebuilt every tick (O(N+C)); simpler than incremental maintenance; trivial at v1 sizes
- hardcoded picker energy threshold: 80 (matches B's SPLIT_PLACEHOLDER) — same starvation reason as B's DECISIONS entry

# Milestone D decisions

- nn-input energy_frac: clamp(energy/100,0,1) — v5/v6 unspecified; matches SPLIT_THRESHOLD×2
- nn-input size norm: /SIZE_MAX — keeps in [0,1] for activation parity
- nn-input vx/vy norm: /MOVE_SPEED_MAX — keeps in roughly [-1,+1]; fast movers saturate at ±1
- nn-input vx/vy timing: previous-tick post-clip velocities ÷ MOVE_SPEED_MAX — gives NN feedback on its own motion
- is_at_wall: uses creature radius (size × BODY_RADIUS_PER_SIZE), not raw size — keeps "creature touches wall" semantics consistent with physics step
- carrion_overlap_norm: count/4, clamp at 1 — v6 §3 unspecified; 4 ≈ "corpse-rich zone"
- vision distance norm: none (raw world units) — v6 §E silent; NN learns scale via weights
- NN biases: none — spec'd weight count (3456 = 136×24 + 24×8) matches no-bias exactly
- weight layout: row-major, hidden-units / outputs contiguous — best cache behavior for SIMD matmul
- hidden and output scratch: stack per call (96+32 bytes) — no alloc; trivially thread-safe
- threads default off: feature flag `threads` wires rayon; v1 single-threaded by default; F.29 perf result decides whether to enable for §16 acceptance
- N_CHUNKS value: 8 — v6 §J's example value; matches typical core counts
- nn-mutation-rate-multiplier: applied at child_from per v6 §K — was missing in B; D fixes
- output activation 0..1: tanh post-forward (in pick_action_d), not in Brain::forward — keeps Brain::forward a pure linear-ReLU-linear graph; activation lives at decode site
- pick_action_c lifetime: deleted on D land — dead code; D replaces it

