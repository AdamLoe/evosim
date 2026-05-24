# evosim — PITCH v6 (delta + implementation defaults appendix)

> v6 supersedes v5. Most of v5 stands as written; v6 patches a handful of mechanics that came up during the underspec-closure pass and adds a comprehensive **Implementation Defaults Appendix** that pins every decision the orchestrator would otherwise have to invent. Read v5 for the full design; read v6 for the deltas + the appendix.

## What changed vs v5

**Mechanics:**
1. **Wasted-action policy rewritten (§7 / §3.5 step 6 of v5).** Instead of paying the action cost for an invalid action, **the creature picks the highest-scoring *valid* action** from its NN logits, with `rest` as universal fallback. Validity rules:
   - `rest`: always valid
   - `photosynth`: always valid
   - `eat`: valid iff `eat_efficiency > 0`
   - `scavenge`: valid iff `scavenge_efficiency > 0`
   - `split`: valid iff `energy ≥ 50`
   - `signal`: always valid (no-op in v1)
   Rationale: the NN inputs don't include the creature's own genome, so it can't learn to gate actions by capability. Valid-fallthrough lets new traits "come online" the moment they mutate in, with no NN catchup needed.

2. **`last_action` input added.** v5 spec'd 130 NN inputs; v6 adds a 6-input one-hot of the previous tick's action → **136 inputs total**. Lets the NN learn temporal patterns (don't keep retrying split right after one fires).

3. **`carrion_overlap` is a normalized float, not 0/1.** Now reports normalized count of overlapping corpses. Scavengers can target corpse-rich zones.

4. **Corpses are visually distinguishable in vision rays.** Carrion is rendered (and reported via vision) as a fixed gray `(0.4, 0.4, 0.4)` pigment, regardless of the dead creature's original color. Scavengers can NN-learn to seek gray.

5. **Feature display is concentric rings/stripes, not dots.** Each active trait family adds a ring on the creature's body (see appendix B).

**No other v5 mechanics changed.**

---

## Implementation Defaults Appendix

This appendix is the answer to every "the orchestrator will have to invent X" question raised in review. Treat as binding spec.

### A. Tooling
- **Rust toolchain:** stable, `wasm32-unknown-unknown` target via `wasm-pack`.
- **JS shell:** **plain TypeScript + DOM** (no framework). Aquarium is a `<canvas>`.
- **Bundler:** **Vite**.
- **Package manager:** **pnpm**.
- **Deploy target:** **Cloudflare Pages**. `_headers` file sets `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` to enable `SharedArrayBuffer` for `wasm-bindgen-rayon`.
- **Single-threaded fallback:** auto-detected at startup; sim runs without rayon if SAB unavailable.

### B. Visual rendering
- **Pigment → canvas RGB:** linear, `byte = round(p × 255)` per channel.
- **Creature body:** filled circle, radius = `size × 2.5` pixels at base zoom (creature size 1 → 2.5px radius; size 10 → 25px).
- **Active-trait rings:** drawn as concentric strokes inside the body outline, 1px each, in fixed colors:
  - `move_speed > 0`: yellow ring
  - `eat_efficiency > 0`: red ring
  - `scavenge_efficiency > 0`: brown ring
  - `eye_count > 0`: white ring (innermost)
  - `armor > 0`: silver/light-gray ring
  Render order: innermost (eye) outward, so visual layering is consistent.
- **Carrion:** filled circle, fixed gray `rgb(102, 102, 102)`, no rings.
- **Speciation/extinction toast:** white background, dark text, top-right, slide in/out, fade after **4 seconds**.
- **Event highlight ring:** golden `rgb(255, 200, 50)`, 2px stroked, applied to the creature in question for **1.5 seconds**, then fades.
- **Anti-aliasing:** browser default canvas AA. No custom rendering tricks.

### C. Camera
- **Initial position (new world):** centered on founder cell, **moderate zoom** = 4x (creature radius ≈ 10px on screen). Player sees a single cell and ~150u of surroundings.
- **Pan:** drag (mouse and touch).
- **Zoom:** wheel (mouse) and pinch (touch). Range **0.5x to 16x**. At 0.5x the entire 600×600 world fits with some margin.
- **Camera state IS persisted** in autosave (zoom level + pan center). On resume, restored. On New World, reset to default.

### D. Sim defaults
- **World coordinate range:** `[0, 600]` on both axes. Origin top-left. Y increases downward (canvas-native).
- **Movement integration:** Euler, dt = 1 tick. `position += velocity` per tick. Velocity clipped to `genome.move_speed` magnitude pre-application.
- **Soft-repulsion formula:** `F = clamp(2.0 × overlap_depth, 0, 5.0)`, applied symmetrically to both creatures along the separating direction.
- **Initial sun_current at world creation:** equal to each cell's `sun_capacity` (full).
- **Hotspot placement:** **Poisson-disk-style sampling**, 3 hotspots, min spacing **200u**, min **60u from walls**. Sample with deterministic RNG (world seed). σ=80u Gaussian shape, peak +4.0. Capacities sum with the east-west gradient with **no cap** — center of a hotspot near the bright (west) side can reach capacity ~7.
- **`is_at_wall` threshold:** true if any axis position is within `genome.size + 5` of a wall.

### E. Brain
- **Input count:** **136** total = 10 self + 120 vision + 6 last-action one-hot.
- **Self-state layout (10):** `[energy_frac, age_frac, size, vx, vy, is_at_wall, digestion_cooldown_frac, carrion_overlap_norm, 2 zeros reserved]`.
- **Vision layout (120):** `[24 sectors × (distance, target_size, target_r, target_g, target_b)]`.
- **Last action one-hot (6):** `[rest, photosynth, eat, scavenge, split, signal]`.
- **Hidden activation:** ReLU.
- **Output activation:** velocity outputs `tanh` then scaled by `genome.move_speed`. Discrete action logits raw (argmax with first-index tiebreak among ties; then valid-action fallthrough per §1).
- **Initial NN weights for founder cell:** uniform `[-0.3, +0.3]`.
- **Mutation sampling:** geometric-skip indices (`Geom(p)`) instead of per-weight Bernoulli; mutated weights perturbed by `N(0, σ)` via fast Ziggurat or polar Box–Muller.

### F. Body mutation
- **`eye_count` adjacency:** sorted divisors are `[0, 2, 3, 4, 6, 8, 12, 24]`. When mutating, pick uniformly from values *index-adjacent* in this list. From 6: choose 4 or 8. From 0: only 2. From 24: only 12.
- **Eye-offset mutation:** only `eye_offsets[i]` for `i < current_eye_count` are mutable; others held at default. Circular Gaussian, σ=0.1 rad.
- **Mutation order within a birth:** body traits iterated in struct-declaration order with a single RNG. Deterministic.

### G. Action edge cases
- **Choosing `eat` but no target in range:** consumed (`eat_attempt cost 0.3` paid), no damage, no digestion cooldown set.
- **Choosing `scavenge` with no corpse overlap:** consumed (`scavenge_attempt cost 0.1` paid), no energy gain.
- **Choosing `split` with `energy < 50`:** invalid per §1; fall through to next logit.
- **Choosing `eat` while in digestion cooldown:** invalid (treated like `eat_efficiency = 0` for validity purposes for that tick). Cooldown is a temporary capability gate.
- **Eat target tiebreak (closest):** if multiple targets are within `bite_reach × size` at exactly equal distance, lowest `creature_id` wins. Deterministic.

### H. Species detection
- **Per-trait σ_t scale constants** (used in §12 distance formula):
  ```
  size: 1.0          max_age: 2000          photosynth_efficiency: 0.3
  eat_efficiency: 0.3   scavenge_efficiency: 0.3   move_speed: 1.0
  vision_range: 20.0    armor: 0.2              bite_reach: 0.5
  pigment_r: 0.15       pigment_g: 0.15         pigment_b: 0.15
  eye_offsets[i]: 0.5 rad (only used indices counted)
  ```
- **Categorical jump cost** for `eye_count` mismatch: 1.5.
- **W_body = 3.0, W_brain = 1.0** (from v5 §12).
- **Threshold:** 4.0.
- **Species naming:** **letter-suffix lineage**. Founder = `Lineage A`. On first split: parent keeps `A`, child becomes `A1`. On `A1`'s first split: parent keeps `A1`, child becomes `A1a`. Counter increments per parent: `A1`, `A2`, `A3` are A's first three children-species; `A1a`, `A1b`, `A1c` are A1's first three. Alpha → numeric alternation continues `A1a1`, `A1a1a`, etc. Names get long; truncate to 16 chars in UI with ellipsis.

### I. Persistence
- **Save format:** JSON via `serde_json`.
- **Schema versioning:** integer `schema_version` field at the top of the JSON. v1 ships with `schema_version: 1`. On load, mismatched version → display a modal "This save is from a different build. Start a New World? [New World]" with no other option.
- **Autosave failure (IDB quota / permission):** small toast "Autosave failed — local storage may be full" in top bar, fades after 6s. Sim continues normally.
- **Event log retention:** UI ring buffer of last 200 events; **full history written to save file** (so download-save preserves the full event log for replay/sharing).

### J. RNG
- **Algorithm:** `xoshiro256++`.
- **String seed → u64:** **xxHash64** of the UTF-8 bytes of the seed string.
- **Determinism with rayon:** sim spawns `N_CHUNKS` sub-RNGs at tick start by stepping the master RNG; each chunk uses its sub-RNG. Number of chunks is fixed (e.g. 8), independent of actual thread count, so results are identical regardless of how many cores the machine has.

### K. Dev panel sliders — ranges
- `base_sun_rate`: `[0, 1.0]`, default 0.08, log-scale slider.
- `mutation_rate_multiplier`: `[0, 5.0]`, default 1.0, linear scale (multiplies all body and NN mutation rates).
- `sun_gradient_strength`: `[0, 3.0]`, default 1.0, linear (multiplies the gradient component of capacity, not hotspots).
- `mouth_tax`: `[0, 0.3]`, default 0.05, linear.
- `nn_mutation_sigma`: `[0, 0.2]`, default 0.02, linear.

### L. "First to" detection (for eulogy hall of fame)
- **First to move:** first creature born with `move_speed > 0` that actually traveled ≥ 5u in its lifetime.
- **Biggest:** creature that reached the largest `size` value during the run (snapshot stored when it died or via running max).
- **Weirdest:** per v5 §11.1 (max genome distance from day-0 founder among creatures that lived ≥ 500 ticks).
- **Last survivor:** the creature that died last (highest death tick).

### M. Snapshot hash for §16 acceptance test
- **Hash function:** xxHash64.
- **Hash inputs:** ordered concatenation of (a) tick number, (b) creature SoA: id, position, velocity, energy, age, genome floats serialized in struct order, NN weight slice, (c) sun map: every cell's `sun_current` and `sun_capacity`, (d) carrion list (id, position, pool_remaining, age), (e) species list (id, anchor genome hash), (f) RNG state.
- **Pinned golden value:** recorded once during initial build, committed to repo as `tests/golden_snapshot_t10000.txt`. Future runs check against it.

### N. Pan/zoom
- **Pan:** left-button drag on canvas; touch single-finger drag.
- **Zoom:** wheel (1.2× per notch) and two-finger pinch.
- **Zoom range:** 0.5× to 16×.
- **Pan limits:** clamped so world edge cannot leave the viewport center.

### O. Photosynth two-pass details (already in v5 §3.5 step 5, expanded)
- **Demand aggregation:** for each sun cell, sum `0.5 × photo_eff × size` across all creatures in that cell with `action == photosynth`. This is the "would-pay-if-unlimited" demand.
- **Payout factor per cell:** `factor = min(1.0, sun_current / demand)` (1.0 if demand=0, treated as no-op).
- **Each photosynther's actual gain:** `(0.5 × photo_eff × size) × factor`.
- **Drain:** `sun_current -= demand × factor` (== total paid out). Always ≤ pre-existing `sun_current`. Never negative.

---

## Orchestrator handoff

After v6 lands and is approved, the orchestrator should:
- Treat v5 as the **design source of truth**.
- Treat v6 as **patches + binding defaults** that supersede v5 on conflict.
- Implement v1 per v5 §15 milestones A–F (sequential where labeled, parallel only where v5/v6 explicitly says so).
- Record any decision NOT covered by v5+v6 in a file `DECISIONS.md` at the repo root with a one-line rationale.
- Not add features beyond what v5 §14 lists as included.
- Stop optimizing performance when the §16 headless test passes (< 8 wall-seconds for 10k ticks at 1500 creatures).
