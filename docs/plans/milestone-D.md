# Milestone D — brain (NN forward pass, input wiring, mutation audit, deterministic chunking)

**Spec anchors:** v5 §3.5 step 3, §5 (architecture + inference + mutation), §5.1
(input/output layout — but v6 §E overrides numbers), §15 D.15–D.19, §3.6
(tick budget).
v6 §1 (valid-fallthrough), §2 (last_action input), §3 (carrion_overlap
normalized), §E (brain inputs / outputs / activations / init / mutation),
§J (RNG determinism with rayon — fixed `N_CHUNKS` sub-RNGs, results identical
regardless of core count), §K (`nn_mutation_sigma` slider).

**Status at handoff:** Milestone C is committed. `Brain` stub in `src/brain.rs`
holds `weights: Vec<f32>` (length `NN_WEIGHT_COUNT = 3360`) and implements
`Brain::founder` (uniform `[-0.3, +0.3]`) + `Brain::child_from` (geometric-skip
mutation + rate-of-rate drift). The vision cache is pinned per C.12 (`VisionBuf
= [f32; 120]`, sectors × `[dist, size, r, g, b]`, raw world units). `world.rs`
calls `pick_action_c` (hardcoded vision-driven picker) as its action source.
`Action::ALL = [Rest, Photo, Eat, Scav, Split, Signal]`. `last_action` is
already promoted at the end of each tick.

**Out of scope (D does NOT touch):**
- Species detection / distance / naming — Milestone E.
- Right-rail overlays, toasts, highlight rings — Milestone E.
- Persistence / double-buffered SoA snapshot — Milestone F.26.
- Eulogy card 4-image grid — Milestone F.28.
- Adding new sliders / traits / actions / NN inputs beyond v6 §E's 136.
- WebGPU NN path (v1.1).

---

## Scope ceiling restatement

v5 §15 milestones run sequentially; D follows C. D ships:

- **D.15** Fixed-shape NN forward pass with `wide::f32x8` SIMD.
- **D.16** Wire 136-float inputs per v6 §E; decode 8 outputs to `(vx, vy,
  Action)` with v6 §1 valid-fallthrough.
- **D.17** Audit mutation (already implemented as `Brain::child_from`).
- **D.18** Deterministic chunking shape (`N_CHUNKS = 8`); rayon path behind
  `cfg(feature = "threads")`. Default off.
- **D.19** Smoke / behavior sanity check at ~1000 creatures × 1000 ticks.

Heavy review per orchestrator brief — energy conservation + determinism + SIMD
correctness all matter (orchestrator section "Adapting review depth per
milestone": Milestone D NN inference is explicitly heavy).

---

## NN input layout (load-bearing — pin and DO NOT change)

This is the contract between C.12's vision cache and D's forward pass. D's
input vector for creature `i` is built once per tick in step 3 (between the
vision pass and the matmul). Layout:

| Range       | Count | Source                  | Notes / source-of-truth |
|-------------|-------|-------------------------|-------------------------|
| `0..10`     | 10    | self-state              | v6 §E "Self-state layout (10)" |
| `10..130`   | 120   | `world.vision[i]` (raw) | C.12 `VisionBuf` — copy byte-for-byte |
| `130..136`  | 6     | last_action one-hot     | v6 §E "Last action one-hot (6)" |

Total: **`NN_INPUTS = 136`**. Already asserted by
`world::tests::vision_layout_matches_nn_input_block`.

### Self-state block (offsets 0..10) — v6 §E

Index → field → normalization (pin):

| Off | Field                       | Formula                                                            | Spec |
|-----|-----------------------------|--------------------------------------------------------------------|------|
| 0   | `energy_frac`               | `(energy / 100.0).clamp(0.0, 1.0)`                                 | see DECISIONS punt: no canonical energy scale in v5/v6 |
| 1   | `age_frac`                  | `(age as f32 / max_age as f32).clamp(0.0, 1.0)` (1.0 if `max_age == 0`) | v5 §5.1 |
| 2   | `size`                      | `size / SIZE_MAX` (= `/10.0`)                                      | normalize so range matches other inputs |
| 3   | `vx`                        | `vx / MOVE_SPEED_MAX` (= `/5.0`)                                   | normalize to roughly `[-1, +1]` |
| 4   | `vy`                        | `vy / MOVE_SPEED_MAX`                                              | same |
| 5   | `is_at_wall`                | `1.0` if `min(x, y, WORLD_SIZE-x, WORLD_SIZE-y) < size + WALL_THRESHOLD_PAD` else `0.0` | v6 §D `is_at_wall` threshold |
| 6   | `digestion_cooldown_frac`   | `cooldown as f32 / DIGESTION_COOLDOWN_TICKS as f32` (= `/50.0`)    | v5 §5.1 |
| 7   | `carrion_overlap_norm`      | `(overlap_count as f32 / CARRION_OVERLAP_NORM_BASE).min(1.0)`      | v6 §3 (normalized count) |
| 8   | reserved                    | `0.0`                                                              | v5 §5.1 "2 reserved zeros" |
| 9   | reserved                    | `0.0`                                                              | v5 §5.1 |

#### Normalization rationale (cite in DECISIONS)

- `energy_frac`: v5/v6 give no canonical energy scale. SPLIT_THRESHOLD is 50,
  FOUNDER_ENERGY is 200, no spec'd cap. Pick `/100.0` (≈ "1.0 means
  comfortably-fed adult"). DECISIONS line:
  `nn-input energy_frac: clamp(energy/100,0,1) — v5/v6 unspecified; matches SPLIT_THRESHOLD×2`.
- `size`: normalize by `SIZE_MAX = 10`. Raw size up to 10 → input up to 1.0.
- `vx`/`vy`: normalize by `MOVE_SPEED_MAX = 5` so a fast mover hits ±1.0. (At
  this point velocities are *post-clip* from the previous tick's
  `apply_movement_and_repulsion`; the freshly-computed `vx/vy` for this tick
  doesn't exist yet because the NN *produces* them. We feed the **previous
  tick's** velocities.) Document in DECISIONS:
  `nn-input vx/vy: previous-tick post-clip velocities ÷ MOVE_SPEED_MAX — gives NN feedback on its own motion`.
- `is_at_wall`: uses the canonical `WALL_THRESHOLD_PAD = 5.0` constant
  already in `constants.rs`; matches v6 §D.
- `digestion_cooldown_frac`: cooldown is a u32 in `[0, 50]` post-decrement.
  Divide by `DIGESTION_COOLDOWN_TICKS = 50`.
- `carrion_overlap_norm`: per v6 §3, "normalized count of overlapping
  corpses." No canonical base in v6. Pin `CARRION_OVERLAP_NORM_BASE = 4.0`
  (4 corpses ≈ saturated zone). Capped at 1.0. DECISIONS line:
  `carrion_overlap_norm: count/4, clamp at 1 — v6 §3 unspecified; 4 ≈ "corpse-rich"`.

### Vision block (offsets 10..130) — C.12 contract

`input[10 + s*5 + f]` = `world.vision[i][s*5 + f]` for `s ∈ 0..24`, `f ∈ 0..5`.

**Raw passthrough.** Distances stay in world units (0 = empty). The
implementer **may** want to divide distance by `vision_range` for scale
parity with the other inputs; v6 §E says nothing about it. **Pin raw
distances** for v1 — the NN learns its own scale via weights, and inspecting
the cache stays trivial. If F.29 perf or behavior tests reveal a need for
normalization, revisit; document in DECISIONS at that point.

### Last-action block (offsets 130..136) — v6 §2 + §E

One-hot of `world.creatures.last_action[i]` per the
`Action::one_hot_index` mapping in `creature.rs`:
`[Rest, Photo, Eat, Scav, Split, Signal] → [130, 131, 132, 133, 134, 135]`.

For founders / freshly-spawned creatures, `last_action` is initialized to
`Action::Rest` in `CreatureSoA::push` → input[130] = 1.0, rest zero.
**Matches** v6 §2's "lets the NN learn temporal patterns."

### Output decode (8 outputs) — v6 §E + v6 §1

NN outputs `out[0..8]`:

| Out idx | Role                                 |
|---------|--------------------------------------|
| 0       | `vx_raw` (pre-tanh)                  |
| 1       | `vy_raw` (pre-tanh)                  |
| 2..8    | discrete action logits (6 entries)   |

**Velocity decode** (v6 §E "velocity outputs `tanh` then scaled by
`genome.move_speed`"):
```
let vx = tanh(out[0]) * genome.move_speed;
let vy = tanh(out[1]) * genome.move_speed;
```
The existing `apply_movement_and_repulsion` already clips by `move_speed`
magnitude in the next step; passing `tanh × move_speed` is correct (axis-wise
≤ `move_speed`, magnitude possibly up to `move_speed × √2`, then clipped).

**Action decode** — v6 §1 valid-fallthrough. Logits order MUST match
`Action::ALL` (see creature.rs): `[Rest, Photo, Eat, Scav, Split, Signal]`.

```
fn decode_action(logits: &[f32; 6], i, genome, energy, cooldown) -> Action {
    // 1. sort indices by logit DESC, first-index tiebreak among equal logits
    //    (v6 §E "argmax with first-index tiebreak among ties").
    let mut order = [0u8, 1, 2, 3, 4, 5];
    order.sort_by(|&a, &b| logits[b as usize].partial_cmp(&logits[a as usize])
                                 .unwrap_or(Ordering::Equal)
                                 .then(a.cmp(&b))); // stable: a < b breaks ties
    // 2. walk and return first valid action.
    for &k in &order {
        let act = Action::ALL[k as usize];
        if is_valid(act, genome, energy, cooldown) { return act; }
    }
    Action::Rest // fallback (always valid)
}

fn is_valid(act, genome, energy, cooldown) -> bool {
    match act {
        Action::Rest | Action::Photosynth | Action::Signal => true,
        Action::Eat       => genome.eat_efficiency > 0.0 && cooldown == 0,
        Action::Scavenge  => genome.scavenge_efficiency > 0.0,
        Action::Split     => energy >= SPLIT_THRESHOLD, // 50
    }
}
```

Validity table per v6 §1 + v6 §G:
- `rest`, `photosynth`, `signal`: always valid.
- `eat`: valid iff `eat_efficiency > 0` **AND** `digestion_cooldown == 0` (v6
  §G: cooldown treated as `eat_efficiency = 0` for validity that tick).
- `scavenge`: valid iff `scavenge_efficiency > 0`.
- `split`: valid iff `energy ≥ SPLIT_THRESHOLD` (50).

**Important:** `is_valid` runs against **post-NN, pre-action** state. Energy
at this point is what it was at the start of step 3 (no upkeep yet); cooldown
likewise. Same source-of-truth as `pick_action_c`.

**Tiebreak rationale.** `partial_cmp` with `Ordering::Equal` fallback +
stable sort by index keeps "first index wins on tie" — matches v6 §E. If
NaN slips into logits (shouldn't post-ReLU+linear, but defensively), treat as
"less than" everything so it never wins; `unwrap_or(Equal)` does this cleanly
for both NaN sides.

---

## File / function changes

### D.15 — NN forward pass

**File:** `src/brain.rs` (extend; do not break `Brain::founder` /
`Brain::child_from` / serde).

#### Weight layout

```
NN_WEIGHT_COUNT = NN_INPUTS * NN_HIDDEN + NN_HIDDEN * NN_OUTPUTS
                = 136 * 24 + 24 * 8
                = 3264 + 192 = 3456
```

(C-era assertion in `constants.rs`: `NN_WEIGHT_COUNT = NN_INPUTS * NN_HIDDEN
+ NN_HIDDEN * NN_OUTPUTS = 3456`. Confirm during audit — earlier brain.rs
test comment says "≈ 336" diffs at 10%, consistent with 3360 not 3456. Fix
the comment if 3456 is correct.)

`brain.weights` slices:
- `[0 .. 3264]`: input→hidden weights. **Row-major by hidden unit.**
  - Layer1 has 24 hidden units; each consumes 136 input weights.
  - `w_ih(h, i) = weights[h * NN_INPUTS + i]` for `h ∈ 0..24`, `i ∈ 0..136`.
- `[3264 .. 3456]`: hidden→output weights, row-major by output unit.
  - `w_ho(o, h) = weights[3264 + o * NN_HIDDEN + h]` for `o ∈ 0..8`, `h ∈ 0..24`.

**No biases.** v5 §5.1 / v6 §E don't spec biases. Spec'd weight count
matches "no biases" exactly. Don't add them. (Hidden ReLU with zero bias is
acceptable — founder weights are uniform `[-0.3, +0.3]` so initial activation
is non-degenerate.)

#### Row-major decision

Pin row-major (output-contiguous). Justification:

- Hidden layer matmul is `h_j = ReLU(Σ_i x[i] * w_ih(j, i))`. Iterating over
  `i` requires sequential reads of `weights[j*136 .. j*136+136]` (cache-hot)
  and `x[0..136]` (also cache-hot, reused 24×). That's the right access
  pattern.
- Column-major would interleave reads across all 24 hidden units, requiring
  a 24-wide accumulator vector and 136 strided loads per row — worse cache
  behavior and harder to SIMD per-hidden-unit.

#### SIMD plan (`wide::f32x8`)

**Hidden layer matmul (136 × 24).** Per hidden unit `j`:
```
acc = f32x8::ZERO
for chunk in 0..17 {                  // 17 × 8 = 136
    let xv = f32x8::from(&input[chunk*8 .. chunk*8+8]);
    let wv = f32x8::from(&weights[j*136 + chunk*8 .. j*136 + chunk*8 + 8]);
    acc += xv * wv;                   // FMA-equivalent
}
let sum: f32 = acc.reduce_add();      // wide provides this
hidden[j] = sum.max(0.0);             // ReLU
```
17 SIMD ops per hidden unit × 24 units = 408 f32x8 multiply-adds. With
`reduce_add` and ReLU, ~440 SIMD ops total for layer 1.

Hidden buffer: `let mut hidden = [0.0_f32; NN_HIDDEN];` on the stack (96
bytes — fine).

**Output layer matmul (24 × 8).** Since `NN_HIDDEN = 24 = 3 × 8`, accumulate
across all 8 outputs in parallel using a transpose of the natural pattern:

Approach A (per-output dot product):
```
for o in 0..8 {
    let mut acc = f32x8::ZERO;
    for chunk in 0..3 {
        let hv = f32x8::from(&hidden[chunk*8 .. chunk*8+8]);
        let wv = f32x8::from(&weights[3264 + o*24 + chunk*8 .. ...]);
        acc += hv * wv;
    }
    out[o] = acc.reduce_add();
}
```
24 SIMD multiply-adds, 8 reductions. Trivial cost.

Approach B (one SIMD pass across outputs): would require column-major output
weights and is not worth the code complexity for 192 weights. **Use Approach
A.**

No bias on output layer either. No activation on outputs 0..1 (tanh applied
post-forward); outputs 2..7 are raw logits. Done.

#### `Brain` extensions

```rust
impl Brain {
    /// Forward pass. `input.len() == NN_INPUTS`. Writes `output.len() == NN_OUTPUTS`.
    /// `hidden_scratch.len() == NN_HIDDEN`. Caller owns scratch to avoid alloc.
    pub fn forward(&self, input: &[f32; NN_INPUTS], output: &mut [f32; NN_OUTPUTS],
                   hidden_scratch: &mut [f32; NN_HIDDEN]);
}
```

Stack-allocated scratch is fine (24 × 4 = 96 bytes hidden, 8 × 4 = 32 bytes
output, 136 × 4 = 544 bytes input). All on the caller's stack. Per-tick
allocation budget: **zero** beyond the per-tick `input_buf` and per-creature
NN scratch (also stack).

The implementer may also expose a `forward_scalar` for testing — guaranteed
SIMD-free reference implementation. Use it in unit tests to assert
`forward_simd ≈ forward_scalar` to within 1e-5 (relative). Keep both.

#### Allocations

- `input: [f32; 136]` — stack, one per creature per tick. Built fresh each
  call inside the tick loop.
- `hidden: [f32; 24]` — stack scratch.
- `output: [f32; 8]` — stack scratch.

**Do NOT** keep per-creature persistent NN scratch buffers on `World`. The
inputs change every tick (depend on vision, energy, age, etc.) and the
hidden activations are dropped after action decode. Stack reuse is cheaper
and aliases nothing.

#### Tests (in `src/brain.rs::tests`)

1. `forward_pass_output_shape`: zero weights → output all zeros; non-zero
   weights → all 8 outputs finite. Shape check: 8 outputs.
2. `forward_pass_matches_scalar_reference`: random weights + random input;
   `forward` and `forward_scalar` agree within 1e-5 relative. (Critical for
   SIMD correctness review.)
3. `forward_pass_relu_clips_negative_hidden`: craft weights so hidden unit 0
   pre-activation is negative; confirm output unaffected by it (i.e. zero
   contribution).
4. `forward_pass_no_nan_on_extreme_inputs`: input all `1.0` and all `-1.0`;
   weights all `0.3`; confirm outputs finite (no overflow, no NaN). This
   stresses the worst-case accumulator magnitudes.
5. `forward_pass_deterministic`: same weights + same input → bit-identical
   output across two calls (asserts no internal RNG / global state).

### D.16 — input wiring + action plumbing

**File:** `src/world.rs`.

Add a new module-local function (in `world.rs` or split into `src/nn_io.rs`
if it grows past 200 LOC — orchestrator's call; default to inline in
`world.rs` to keep the integration site obvious):

```rust
/// Build the 136-float NN input vector for creature i.
fn build_nn_input(
    i: usize,
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    carrion_overlap_count: u32,
    is_at_wall_flag: f32,
) -> [f32; NN_INPUTS] {
    let mut buf = [0.0f32; NN_INPUTS];
    let g = &creatures.genomes[i];
    let energy = creatures.energy[i];
    let age = creatures.age[i];
    let cooldown = creatures.digestion_cooldown[i];

    // 0..10: self-state
    buf[0] = (energy / 100.0).clamp(0.0, 1.0);
    buf[1] = if g.max_age > 0 { (age as f32 / g.max_age as f32).clamp(0.0, 1.0) } else { 1.0 };
    buf[2] = g.size / SIZE_MAX;
    buf[3] = creatures.vx[i] / MOVE_SPEED_MAX;
    buf[4] = creatures.vy[i] / MOVE_SPEED_MAX;
    buf[5] = is_at_wall_flag;
    buf[6] = cooldown as f32 / DIGESTION_COOLDOWN_TICKS as f32;
    buf[7] = (carrion_overlap_count as f32 / CARRION_OVERLAP_NORM_BASE).min(1.0);
    // buf[8], buf[9] reserved zero.

    // 10..130: vision (raw passthrough)
    buf[10..130].copy_from_slice(vision);

    // 130..136: last_action one-hot
    let la = creatures.last_action[i].one_hot_index();
    buf[130 + la] = 1.0;

    buf
}
```

#### Where `is_at_wall_flag` comes from

Compute inline at the call site, do not store on SoA:
```rust
let r = g.size * BODY_RADIUS_PER_SIZE;
let near = creatures.x[i].min(creatures.y[i])
    .min(WORLD_SIZE - creatures.x[i])
    .min(WORLD_SIZE - creatures.y[i]);
let is_at_wall_flag = if near < r + WALL_THRESHOLD_PAD { 1.0 } else { 0.0 };
```
Cite v6 §D: "true if any axis position is within `genome.size + 5` of a
wall." Note v6 §D uses `genome.size`, not `size × radius_per_size`. We use
radius (`r`) here, matching what `apply_movement_and_repulsion` does for
wall-clamping. Document tiny deviation in DECISIONS:
`is_at_wall: uses creature radius (size × BODY_RADIUS_PER_SIZE), not raw size — keeps "creature touches wall" semantics consistent with physics step`.

(With current `BODY_RADIUS_PER_SIZE = 1.0`, the two are identical. Future
change to that constant would silently shift NN-vs-physics agreement. Worth
the one-line DECISION.)

#### Where `carrion_overlap_count` comes from

Compute per creature using existing `cell_to_carrion` index (already built
each tick in `run_vision_pass`). Same logic as `eat_and_scavenge` for
scavenge but counting all overlaps:
```rust
fn count_carrion_overlap(i, creatures, carrion, grid_cell_idx,
                          cell_to_carrion) -> u32 {
    let xi = creatures.x[i];
    let yi = creatures.y[i];
    let ri = creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
    let r2 = ri * ri;
    let mut count = 0u32;
    // walk this cell + immediate neighbors (3x3); carrion radius is tiny
    // so the creature's center-radius circle only overlaps neighboring cells
    // when ri exceeds HASH_CELL/2 (creature radius 2.5+ → check 3x3).
    for (cx, cy) in 3x3_around(grid_cell_idx) {
        for &ci in &cell_to_carrion[cell_idx(cx, cy)] {
            let dx = carrion[ci as usize].x - xi;
            let dy = carrion[ci as usize].y - yi;
            if dx*dx + dy*dy <= r2 { count += 1; }
        }
    }
    count
}
```

Helper lives in `world.rs` (private), reuses the per-tick `cell_to_carrion`
field. Cost: 9 cells × ~ε carrion typically; cheap.

Add a constant to `constants.rs`:
```
/// Normalization base for carrion_overlap_norm NN input (v6 §3, no canonical base).
pub const CARRION_OVERLAP_NORM_BASE: f32 = 4.0;
```

#### Replace `pick_action_c` with `pick_action_d`

`world.rs` currently calls `pick_action_c` in step 3. Replace with
`pick_action_d` that:

1. Builds the 136-input vector via `build_nn_input`.
2. Runs `creatures.brains[i].forward(&input, &mut out, &mut hidden)`.
3. Computes `vx = tanh(out[0]) * speed; vy = tanh(out[1]) * speed`.
4. Calls `decode_action(&out[2..8], i, genome, energy, cooldown)`.
5. Returns `(vx, vy, action)`.

Signature:
```rust
fn pick_action_d(
    i: usize,
    input_buf: &mut [f32; NN_INPUTS],   // scratch, reused per creature
    hidden_buf: &mut [f32; NN_HIDDEN],
    output_buf: &mut [f32; NN_OUTPUTS],
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    carrion_overlap_count: u32,
    is_at_wall_flag: f32,
) -> (f32, f32, Action);
```

**Why pass scratch in?** Lets the rayon path reuse one set of scratch
buffers per chunk (or per thread when threads are enabled). Single-threaded
sequential path declares them once outside the loop.

**Keep `pick_action_c` exported as a fallback ONLY if rayon-disabled-threads
test fails.** Default: delete `pick_action_c` and its tests once `pick_action_d`
lands and the world smoke tests pass. Don't leave dead code around.

#### Tick step 3 rewrite (in `World::step`)

```rust
// 3. NN forward pass + action decode (Milestone D).
let n = self.creatures.len();
let mut input_buf = [0.0f32; NN_INPUTS];
let mut hidden_buf = [0.0f32; NN_HIDDEN];
let mut output_buf = [0.0f32; NN_OUTPUTS];
for i in 0..n {
    let overlap = self.count_carrion_overlap(i);
    let is_at_wall = self.compute_is_at_wall(i);
    let (vx, vy, action) = pick_action_d(
        i,
        &mut input_buf,
        &mut hidden_buf,
        &mut output_buf,
        &self.creatures,
        &self.vision[i],
        overlap,
        is_at_wall,
    );
    self.creatures.vx[i] = vx;
    self.creatures.vy[i] = vy;
    self.creatures.action_this_tick[i] = action;
}
```

Helper methods on `World`:
- `fn count_carrion_overlap(&self, i: usize) -> u32`
- `fn compute_is_at_wall(&self, i: usize) -> f32`

#### Tests (in `src/world.rs::tests`)

8. `nn_input_layout_self_state_correct`: spawn one creature at known
   position with known genome / energy / age; assert `build_nn_input`
   returns the expected 10 self-state values at offsets 0..10.
9. `nn_input_layout_vision_passthrough`: set `vision[0]` to a known
   pattern; assert `input[10..130] == vision[0]` after build.
10. `nn_input_layout_last_action_onehot`: set `creatures.last_action[0] =
    Action::Eat`; assert `input[130 + Eat::one_hot_index()] == 1.0` and the
    other 5 last-action slots are zero.
11. `decode_action_valid_fallthrough_split_invalid`: build logits with
    `Split` highest but energy 10 (< 50); assert decoded action falls
    through to next-highest valid action.
12. `decode_action_first_index_tiebreak`: build logits with two tied at the
    max; assert lower index wins.
13. `decode_action_eat_invalid_in_cooldown`: logits favor `Eat`, cooldown >
    0; assert falls through.
14. `decode_action_scavenge_invalid_when_zero_eff`: logits favor
    `Scavenge`, `scavenge_efficiency = 0`; assert falls through.
15. `decode_action_rest_always_valid_as_fallback`: all five non-rest
    actions invalid (energy=0, eat_eff=0, scav_eff=0, cooldown>0); assert
    decoded == `Action::Rest` (or `Photosynth` if it's valid earlier in
    order — confirm: Photosynth is always valid).

  Important: since `Photosynth` is always valid and is in `Action::ALL` at
  index 1, the **true** fallback is always `Photosynth` when its logit is
  positive enough OR when all higher actions in the ordering fail. The
  "rest fallback" branch in `decode_action` only fires if all 6 sort to
  invalid — which is impossible given Photosynth + Signal + Rest are always
  valid. Acceptable; keep the explicit `Action::Rest` return as
  defensive-only.

### D.17 — NN mutation audit

**File:** `src/brain.rs` (audit only).

`Brain::child_from` (lines 37–55) already implements:
- Geometric-skip sampling via `rng.geom_skip(p)`.
- Per-mutation perturbation by `rng.normal() * sigma` (sigma from
  `DevSliders::nn_mutation_sigma` passed in).
- Rate-of-rate drift: 0.5% per birth, ±20% jitter.

#### Audit checklist (record findings in code-review section, not separate edits unless drift found)

- [ ] **Geometric step semantics.** Confirm `i = rng.geom_skip(p)` produces
  a 0-based index. `geom_skip(p)` returns "number of no-mutate trials before
  the next mutation index" — so `i = step` is the first index to mutate,
  then advance by `step + 1` per iteration. Current code does
  `i = i.saturating_add(step).saturating_add(1)` which advances past the
  current `i` then adds `step` skips. **Bug check:** for `p = 1.0`,
  `geom_skip` returns 0, so consecutive `i` values are 0, 1, 2, …, all
  weights mutate. ✓ For `p = 0.5`, expected ~50% mutation rate. ✓ For
  `p = 0.0`, returns `usize::MAX`, loop exits on first iter. ✓ Looks
  correct.
- [ ] **Sigma source.** `Brain::child_from` takes `sigma: f32` parameter;
  `world.rs::handle_births` passes `self.sliders.nn_mutation_sigma`. ✓
  Confirms v6 §K slider wiring.
- [ ] **Rate-of-rate drift.** `MUT_RATE_OF_RATES = 0.005` (0.5%),
  `MUT_RATE_JITTER = 0.20` (±20%). One roll per birth (not per weight).
  Matches v5 §5.3.
- [ ] **Founder weights.** `Brain::founder` uses
  `rng.uniform(-NN_INIT_RANGE, NN_INIT_RANGE)` with
  `NN_INIT_RANGE = 0.3`. ✓ Matches v6 §E.
- [ ] **Test comment drift.** `child_mutation_perturbs_some_weights` test
  says "≈ 0.1 × 3360 ≈ 336" but `NN_WEIGHT_COUNT = 3456` (per D.15
  recount). Fix comment + bounds: should be ≈ 346, range still 100..800
  passes.

No `Brain` code change expected; only the test comment fix.

### D.18 — deterministic chunking + threads feature

**Files:** `src/world.rs`, `src/rng.rs`, possibly `src/vision.rs`.

#### Plan

Per v6 §J: spawn `N_CHUNKS` sub-RNGs each tick, fixed regardless of actual
thread count. For the NN forward pass, **no per-tick RNG is needed** (pure
forward pass — no dropout, no stochastic activation, no Boltzmann sampling).
Mutation happens in `handle_births`, which is sequential and uses the
master RNG directly. So D.18 for the forward pass = pure parallelism
opportunity, no RNG plumbing required.

**Vision pass** also has no RNG. Both passes are candidates for chunking.

#### Pin chunking shape (single-threaded default)

Add to `constants.rs`:
```
/// Number of fixed chunks for deterministic rayon parallelism (v6 §J).
/// Results are identical regardless of actual core count.
pub const N_CHUNKS: usize = 8;
```

Add a small helper:
```rust
/// Compute chunk size so all chunks but the last are equal-sized;
/// last chunk holds the remainder. Returns the per-chunk slice ranges.
fn chunk_ranges(n: usize) -> [(usize, usize); N_CHUNKS] {
    let base = (n + N_CHUNKS - 1) / N_CHUNKS;  // ceil
    let mut out = [(0usize, 0usize); N_CHUNKS];
    for k in 0..N_CHUNKS {
        let lo = (k * base).min(n);
        let hi = ((k + 1) * base).min(n);
        out[k] = (lo, hi);
    }
    out
}
```

Single-threaded path iterates the chunks sequentially. Results identical
whether or not threads are enabled (deterministic chunking — that's the
whole point of v6 §J).

#### Sequential implementation (default)

```rust
// 3. NN forward pass + action decode.
let n = self.creatures.len();
let ranges = chunk_ranges(n);
for &(lo, hi) in &ranges {
    let mut input_buf = [0.0f32; NN_INPUTS];
    let mut hidden_buf = [0.0f32; NN_HIDDEN];
    let mut output_buf = [0.0f32; NN_OUTPUTS];
    for i in lo..hi {
        // ... per-creature pick_action_d ...
    }
}
```

Sequential code, but iteration ordering matches what the rayon path will
produce — so enabling threads later does not change tick results (only
their parallelism).

**Why default off?** Document in DECISIONS:
- Cloudflare Pages COOP/COEP headers are set, but enabling
  `wasm-bindgen-rayon` requires JS-side glue (spawn thread pool, await
  initialization) before the first sim tick. That's a Milestone F.30 / shell
  integration concern.
- Local `pnpm dev` + `cargo test` benefit from no thread pool init.
- §16 perf gate (10k ticks × 1500 creatures < 8s) is the trigger for
  flipping the feature flag on. Until F.29 shows we need threads, leave off.

DECISIONS:
`threads default off: feature flag `threads` wires rayon; v1 single-threaded by default; F.29 perf result decides whether to enable for §16 acceptance`.

#### Threaded path (behind `#[cfg(feature = "threads")]`)

```rust
#[cfg(feature = "threads")]
{
    use rayon::prelude::*;
    // par_chunks_mut wouldn't help here because we write to many SoA arrays.
    // Use rayon::scope or par_iter over the chunk ranges, writing into
    // per-chunk result Vecs that we drain back into SoA serially.
    //
    // Simplest deterministic pattern: split the output arrays at chunk
    // boundaries with split_at_mut, par_iter the ranges, scratch per chunk.
    let (vx_chunks, ...) = soa.vx.split_at_n(&ranges); // helper
    ranges.par_iter().enumerate().for_each(|(k, &(lo, hi))| {
        let mut input_buf = [0.0f32; NN_INPUTS];
        let mut hidden_buf = [0.0f32; NN_HIDDEN];
        let mut output_buf = [0.0f32; NN_OUTPUTS];
        for i in lo..hi {
            // exact same pick_action_d as the sequential path
        }
    });
}
```

**Determinism check:** for the NN forward pass with no RNG, the only
nondeterminism source is read-of-write hazards across chunks. Since each
chunk only writes to `vx[i]`, `vy[i]`, `action_this_tick[i]` for `i ∈ chunk`,
and reads from `genomes[i]`, `brains[i]`, `vision[i]`, `energy[i]`,
`age[i]`, `cooldown[i]`, `last_action[i]` (all index-aligned, no
cross-chunk reads), the parallel path is bit-identical to the sequential
path. ✓

**Carrion overlap count** reads `cell_to_carrion` (shared, read-only) and
`carrion[]` (read-only). No hazard. ✓

#### Vision pass (also chunkable)

Out-of-scope to refactor in D unless the implementer finds it trivial.
Vision pass writes to `self.vision[i]` only (no cross-creature dependencies
past the read-only grid + carrion index). Wrapping in the same
`chunk_ranges` pattern is a 5-LOC change. **Do it** while we're in the
neighborhood — keeps the chunking pattern consistent across both
chunkable passes.

#### Mutation (sequential — unchanged)

`handle_births` stays serial. Master RNG threads through each birth in
order. Per v6 §J, "results identical regardless of actual core count" —
births already satisfy this trivially because they're not parallelized.

#### Tests (in `src/world.rs::tests`)

16. `chunk_ranges_partition`: `n = 1000` → 8 chunks; assert sum of
    `(hi - lo)` == 1000, all ranges non-overlapping, first lo == 0, last hi
    == 1000.
17. `chunk_ranges_small_population`: `n = 3` → 8 chunks where most are
    empty; assert no panic, ranges valid.
18. `chunked_tick_matches_unchunked_tick` (only if implementer keeps both
    paths during D.18 development; can be deleted on landing): run 100
    ticks twice — once with sequential single-loop, once with chunked
    sequential. Assert identical creature SoA hashes. **This is the
    determinism gate** for D.18.

#### Cargo.toml

`features = { default = [], threads = ["wasm-bindgen-rayon", "rayon"] }` is
already present. Confirm during implementation; no edits expected.

### D.19 — behavior sanity check

**File:** `src/world.rs::tests` (or `tests/smoke.rs` if implementer prefers
integration test).

```rust
#[test]
fn d19_thousand_creatures_thousand_ticks_no_explode() {
    let mut w = World::new("d19-smoke");
    // Seed in 999 extra creatures with random genomes (+1 founder = 1000).
    // Use the same SimRng (forked) so test is deterministic.
    let mut seeder = SimRng::from_string("d19-seed");
    for k in 0..999 {
        let mut g = Genome::founder();
        // Randomize a subset of traits to get behavior diversity.
        g.move_speed = seeder.uniform(0.0, MOVE_SPEED_MAX);
        g.eye_count = EYE_VALID[seeder.index(EYE_VALID.len())];
        g.vision_range = seeder.uniform(0.0, VISION_RANGE_MAX);
        g.eat_efficiency = seeder.uniform(0.0, EAT_EFF_MAX);
        g.scavenge_efficiency = seeder.uniform(0.0, SCAVENGE_EFF_MAX);
        // Eye offsets: leave default
        let b = Brain::founder(&mut seeder);
        let x = seeder.uniform(10.0, WORLD_SIZE - 10.0);
        let y = seeder.uniform(10.0, WORLD_SIZE - 10.0);
        w.creatures.push(k + 1, x, y, FOUNDER_ENERGY, 0, 0, 0, g, b);
        w.vision.push([0.0f32; VISION_LEN]);
    }

    let total_energy_before = creatures_energy_sum(&w) + sun_total(&w);

    for _ in 0..1000 {
        w.tick_once();
    }

    // (a) no panic — implicit by reaching this point.
    // (b) total energy stayed bounded.
    let total_energy_after = creatures_energy_sum(&w) + sun_total(&w)
        + carrion_pool_sum(&w);
    assert!(total_energy_after.is_finite());
    // Energy can grow (sun regen) but should not explode > 10× start.
    // (Sun regen at 0.08/tick × 400 cells × 1000 ticks = 32000 energy max.)
    let max_expected = total_energy_before + 0.08 * 400.0 * 1000.0 + 1000.0;
    assert!(total_energy_after < max_expected * 1.5,
        "total energy {} exceeded sane bound {}", total_energy_after, max_expected);

    // (c) some creatures picked non-Photosynth actions during the run.
    // The NN is non-zero (uniform [-0.3, +0.3]) — argmax should hit varied
    // actions across creatures. Snapshot the action_this_tick distribution.
    let mut counts = [0usize; 6];
    for &a in &w.creatures.action_this_tick {
        counts[a as usize] += 1;
    }
    // Photosynth is everyone's safe choice in dead worlds, but in a 1k-creature
    // run with random brains some non-Photo action should fire.
    let non_photo: usize = counts.iter().enumerate()
        .filter(|(i, _)| *i != Action::Photosynth as usize)
        .map(|(_, c)| *c).sum();
    // Allow for the possibility that the population crashes to a small
    // photosynth-only remnant; just require at least 1 non-photo action
    // somewhere in the final tick OR a population > 100.
    assert!(non_photo > 0 || w.creatures.len() > 100,
        "all surviving creatures chose Photosynth (NN may be wired wrong)");
}
```

#### Notes on D.19 thresholds

- "Energy doesn't explode" is a loose bound — exact conservation isn't
  expected over 1000 ticks (sun refills, upkeep deducts, deaths convert to
  carrion which can refund to sun on decay). The bound `1.5 × max_expected`
  catches obvious bugs (NaN, infinite gain loops, double-counted
  photosynth).
- "Some non-Photosynth action" is the load-bearing behavior assertion. It
  catches: NN not wired in (would default to `Action::Rest` in fallback),
  validity logic always rejecting (would force `Photosynth`), tanh-output
  always saturated to ±1 (still would let actions vary).
- The test is **not** a perf test — that's F.29. Keep it under 5s on
  modern hardware.

---

## Integration points (touchpoints with prior milestones)

### From C.12 (vision)
- D reads `self.vision[i]: [f32; 120]` raw, never writes back. Layout
  pinned in `src/vision.rs` constants.

### From B (world tick orchestration)
- D replaces step 3 only. Steps 1, 2, 4–12 untouched.
- `creatures.action_this_tick[i]`, `creatures.vx[i]`, `creatures.vy[i]`
  must be set by end of step 3 — same contract as `pick_action_c`.
- `creatures.last_action[i]` already promoted at end of step 12 → no D
  change needed there.

### From B (energy economy)
- Energy conservation invariants in `photosynth_two_pass`,
  `eat_and_scavenge`, `decay_carrion` are unaffected by D — those steps
  read `action_this_tick` (set by D's pick_action_d) the same way they
  read it from `pick_action_b/c`.

### From B (DevSliders)
- `self.sliders.nn_mutation_sigma` already wired into `Brain::child_from`
  via `handle_births`. ✓ No D change.

### From C (mutation rate plumbing)
- `self.sliders.mutation_rate_multiplier` multiplies body mutation rates
  (genome.rs) and **does NOT scale NN mutation rate** today — v6 §K says
  it "multiplies all body and NN mutation rates." Audit: confirm
  `Brain::child_from` does or does not apply the multiplier. **Current
  code does not.** The multiplier hits `genome.mutate_in_place` via the
  `multiplier` parameter; `Brain::child_from` uses `nn_mutation_rate` as-is.
  Per v6 §K wording this is a v5/v6 alignment bug.
  - **Fix in D.17 audit:** pass `mutation_rate_multiplier` into
    `Brain::child_from` and multiply `p = (parent.nn_mutation_rate *
    multiplier).clamp(0.0, 1.0)` before the geom-skip loop.
  - DECISIONS: `nn-mutation-rate-multiplier: applied at child_from per v6 §K — was missing in B; D fixes`.

### From C.12 helpers
- `pick_action_c` deletion removes its tests (`world_runs_2000_ticks_with_movement`
  still works because D's picker also produces moving behavior — verify
  during implementation).
- `sector_to_angle` in `vision.rs` was used by `pick_action_c` only. After
  C's picker is deleted, audit whether `sector_to_angle` has remaining
  callers. If none, remove. If it's referenced by future E/F tooling, keep.

---

## Code-reviewer's checklist

### NN architecture / SIMD

- [ ] `NN_WEIGHT_COUNT == 3456` (= 136×24 + 24×8). Constant matches.
- [ ] Weight layout: input→hidden rows in `weights[0..3264]`, hidden→output
      rows in `weights[3264..3456]`. Row-major (output-contiguous).
- [ ] `forward_simd` and `forward_scalar` agree within 1e-5 on random
      inputs (test).
- [ ] Hidden layer applies ReLU **before** writing to scratch.
- [ ] Output layer applies **no** activation (tanh / argmax happens
      outside the forward pass).
- [ ] No biases (matches spec'd weight count exactly).
- [ ] No per-tick allocations inside `Brain::forward` (stack scratch only).

### NN input wiring

- [ ] All 10 self-state offsets present, in correct order (energy_frac,
      age_frac, size, vx, vy, is_at_wall, cooldown_frac, carrion_overlap,
      reserved×2).
- [ ] Vision passthrough: `input[10..130] == self.vision[i]` byte-equal.
- [ ] Last-action one-hot uses `Action::one_hot_index()` (already
      `as u8 == 0..5` per `repr(u8)` in `creature.rs`) and sets exactly one
      of input[130..136] to 1.0.
- [ ] `is_at_wall` uses `WALL_THRESHOLD_PAD = 5.0` and is computed against
      creature radius, not raw size (DECISIONS noted).
- [ ] `carrion_overlap_norm` normalized by `CARRION_OVERLAP_NORM_BASE = 4`
      and clamped to 1.0.
- [ ] `vx`/`vy` inputs are previous-tick post-clip velocities (the values
      stored in `self.creatures.vx[i]` at the moment of input-build).

### Action decode

- [ ] Output 2..8 logits map to `Action::ALL` order:
      `[Rest, Photo, Eat, Scav, Split, Signal]`.
- [ ] Argmax with first-index tiebreak (lower index wins on tie). NaN-safe.
- [ ] Valid-fallthrough: `Rest`, `Photo`, `Signal` always valid; `Eat`
      requires `eat_efficiency > 0 && cooldown == 0`; `Scavenge` requires
      `scavenge_efficiency > 0`; `Split` requires `energy >= 50`.
- [ ] No path returns an invalid action.

### Mutation audit

- [ ] `Brain::child_from` accepts `sigma` from `sliders.nn_mutation_sigma`.
- [ ] Geometric-skip loop advances by `step + 1` (correct).
- [ ] Rate-of-rate drift fires once per birth (not per weight).
- [ ] **D.17 fix:** `mutation_rate_multiplier` now applied to NN mutation
      rate per v6 §K.

### Determinism

- [ ] `chunk_ranges(n)` partitions correctly for n ∈ {0, 1, 7, 8, 9, 1000}.
- [ ] Single-threaded sequential pass iterates `chunk_ranges` in order;
      result identical regardless of `N_CHUNKS` value (vacuously true since
      no RNG in forward pass).
- [ ] Threaded path (when `--features threads`): per-chunk scratch
      buffers, no shared mutable state, identical results to sequential.
- [ ] `cargo test --features threads` passes if implementer enables; if
      threads off in CI for v1, document.

### Integration

- [ ] `pick_action_c` deleted (or kept ONLY behind cfg flag if rayon-off
      test fails — unlikely).
- [ ] `world::step` step 3 now uses NN forward; steps 1–2, 4–12 unchanged.
- [ ] Existing B/C tests still pass:
  - `lone_creature_eventually_splits` (within 5000 ticks)
  - `energy_conservation_in_photosynth_pass`
  - `carrion_returns_to_sun_on_decay`
  - `world_runs_many_ticks_without_panic`
  - `world_runs_2000_ticks_with_movement` (NN-driven, founder forced with
    move_speed=1, vision_range=30, eye_count=4)
  - all `vision::tests`
- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo test --lib` count grows by ≥ 13 (5 brain + 8 world).

### v6 spec compliance

- [ ] v6 §1 wasted-action policy: valid-fallthrough, not cost-and-abort. ✓
- [ ] v6 §2 last_action one-hot included in 136 inputs. ✓
- [ ] v6 §3 carrion_overlap is normalized float, not 0/1. ✓
- [ ] v6 §E init range `[-0.3, +0.3]`, hidden ReLU, velocity tanh ×
      move_speed, argmax with first-index tiebreak. ✓
- [ ] v6 §E geometric-skip mutation with Ziggurat / polar Box–Muller.
      Polar Box–Muller is what `rng.normal_pair` does. ✓
- [ ] v6 §J determinism: fixed `N_CHUNKS = 8`, results independent of
      core count. Forward pass needs no RNG so trivially satisfied. ✓

---

## Decision punts (to record in `DECISIONS.md`)

| Topic | Choice | Rationale |
|---|---|---|
| nn-input energy_frac | `clamp(energy/100,0,1)` | v5/v6 unspecified; matches `SPLIT_THRESHOLD × 2` |
| nn-input size norm | `/SIZE_MAX` | keeps in `[0, 1]` for activation parity |
| nn-input vx/vy norm | `/MOVE_SPEED_MAX` | keeps in roughly `[-1, +1]`; fast movers saturate at ±1 |
| nn-input vx/vy timing | previous-tick post-clip values | NN sees feedback on its own motion; current-tick values don't exist yet |
| is_at_wall radius vs size | uses `size × BODY_RADIUS_PER_SIZE` | physics step uses radius for wall clamp; consistency |
| carrion_overlap_norm base | 4.0 | v6 §3 unspecified; 4 corpses ≈ "corpse-rich zone" |
| vision distance norm | none (raw world units) | v6 §E silent; NN learns scale via weights |
| NN biases | none | spec'd weight count matches no-bias exactly |
| Weight layout | row-major, hidden-units / outputs contiguous | best cache behavior for SIMD matmul |
| Hidden / output scratch | stack per call | 96+32 bytes; no alloc; trivially thread-safe |
| threads default | off | v1 single-threaded; F.29 perf result triggers enable |
| `N_CHUNKS` value | 8 | v6 §J's example value; matches typical core counts |
| nn-mutation-rate-multiplier | applied in `Brain::child_from` | per v6 §K wording — fix vs B's missing wire |
| Output activation 0..1 | tanh post-forward (in pick_action_d), not in `Brain::forward` | keeps `Brain::forward` a pure linear-ReLU-linear graph; activation lives at decode site |
| `pick_action_c` lifetime | deleted on D land | dead code; D replaces it |

---

## Integration order (for the implementer)

1. **D.17 audit first** — read `Brain::child_from`, fix the
   `mutation_rate_multiplier` wire (1-line change in `world.rs::handle_births`
   call + adjust `Brain::child_from` signature to take multiplier). Update
   the test comment ("≈ 336" → "≈ 346"). Run `cargo test brain`. Commit
   alone: `M D.17: wire nn_mutation_rate_multiplier through child_from per v6 §K`.

2. **D.15 forward pass** — extend `Brain` with `forward` (SIMD) and
   `forward_scalar` (test reference). Add the 5 brain tests. Run `cargo
   test brain`. Commit: `M D.15: SIMD NN forward pass (136→24→8, wide::f32x8)`.

3. **D.16 input wiring + decode** — add `build_nn_input`,
   `count_carrion_overlap`, `compute_is_at_wall`, `decode_action`,
   `pick_action_d`. Add `CARRION_OVERLAP_NORM_BASE` to constants. Add the 8
   world tests. Replace `pick_action_c` call in `world::step`. Delete
   `pick_action_c` (and its tests if any specific to it). Run `cargo test
   --lib`. Commit: `M D.16: NN-driven action selection with valid-fallthrough`.

4. **D.18 chunking shape** — add `N_CHUNKS` + `chunk_ranges`. Refactor step
   3's loop into chunked form (single-threaded by default). Optionally
   chunk the vision pass too. Add 3 chunking tests. Commit: `M D.18:
   deterministic chunking (N_CHUNKS=8) for NN + vision passes`.

5. **D.18 rayon path** — gate `par_iter` block with `cfg(feature =
   "threads")`. Add a `cargo test --features threads` run to CI (or
   document as deferred to F.29). Commit: `M D.18: rayon parallelism behind
   `threads` feature flag (default off)`.

6. **D.19 smoke test** — add the 1000-creature × 1000-tick sanity test.
   Commit: `M D.19: 1000-creature behavior sanity check`.

7. Final pass: `cargo clippy --all-targets -- -D warnings`, `cargo test
   --lib`, `wasm-pack build --target web` (smoke). Manual: launch
   `pnpm dev`, watch 1000 ticks, eyeball that creatures pick varied
   actions and movement looks like real behavior, not the C placeholder
   "head to closest target" pattern.

---

## Out-of-scope reminders (v5 §15 guardrails)

- No species speciation distance (E).
- No right-rail overlays / Inspector / toasts / charts (E).
- No double-buffered persistence (F.26).
- No NN visualization (v1.1+).
- No NEAT / topology evolution (v1.1+).
- No WebGPU (out forever per v5 §2).
- No new inputs beyond the 136 v6 §E specifies.
- No new outputs beyond the 8 v6 §E specifies.
- No new actions beyond `Action::ALL`.
- No new dev sliders.

---

## Punts to F.29 / F.30 / v1.1

- Vision distance normalization: leave raw; revisit if behavior tests
  reveal NN-can't-learn-scale.
- NN biases: not in v1. If behavior is degenerate, v1.1 can add biases
  with a NN_WEIGHT_COUNT bump (and break-save schema).
- `pick_action_c` resurrection: dead unless rayon-off test fails.
- Per-creature persistent NN scratch: not needed for v1; stack is fine.
  v1.1 might revisit if profiling shows stack churn matters.
- Threads enable in production: F.29 decides.
- Carrion overlap normalization base: revisit if scavengers can't find
  corpse-rich zones (raise base) or always saturate (lower base).
- `is_at_wall` definition: if NN gets confused by "is_at_wall" flipping
  too eagerly, tighten threshold; or expose `WALL_THRESHOLD_PAD` as a
  hidden constant (not a slider; v1 keeps slider set fixed).

---

## Code review — APPROVED

All 49 lib tests pass (`cargo test --lib`); `cargo clippy --all-targets -- -D warnings` clean; `cargo check --features threads` builds.

Spec compliance verified: SIMD matmul layout (`brain.rs:76-101`) is row-major hidden-unit-contiguous, exactly matching `forward_scalar` (`brain.rs:113-130`); SIMD/scalar agreement test (`brain.rs:237-280`) uses random weights in [-0.3, 0.3] and random inputs in [-1, 1] — realistic, not identity. Weight count = 3456 = 136×24 + 24×8, no biases. Output decode (`world.rs:951-953`) applies `tanh(out[0..2]) * creatures.genomes[i].move_speed` live; logits `out[2..8]` map to `Action::ALL` order. Valid-fallthrough (`world.rs:913-930`) sorts by logit DESC with stable first-index tiebreak, walks "next-highest valid logit" — not a hardcoded preference. `is_valid_action` (`world.rs:902-909`) matches v6 §1/§G exactly. Self-state normalizations (`world.rs:877-888`) clamp energy/age/cooldown/carrion_overlap to [0,1], `vx/vy / MOVE_SPEED_MAX` bounded in [-1,1] because stored values are `tanh*move_speed`. `is_at_wall` (`world.rs:807-817`) uses `r + WALL_THRESHOLD_PAD` per v6 §D; documented radius-vs-raw-size deviation harmless at `BODY_RADIUS_PER_SIZE = 1.0`. `CARRION_OVERLAP_NORM_BASE = 4.0` (`constants.rs:120`) saturates at 4+ overlaps — reasonable. `chunk_ranges` (`world.rs:845-854`) partitions 0..n with last chunk holding remainder; handles n ∈ {0, 1, 3, 7, 1000} cleanly. Rayon path (`world.rs:241-314`) gated on `cfg(feature = "threads")`; sequential default. `Brain::child_from(parent, rng, sigma, multiplier)` signature change wired at `world.rs:708-713`. Stack-only scratch in `Brain::forward`; sequential path reuses one set of scratch per chunk. No per-tick allocations in default build. D.19 smoke test runs 1000 creatures × 1000 ticks with energy bound + non-photo action diversity check.
