# Milestone C — body genome + mutation (sequential C.10→C.14)

**Spec anchors:** v5 §3.5 step 2, §4, §6, §10, §15 C.10–C.14, §3.6 (raycast budget).
v6 §B (visual rendering / rings), §C (camera), §D (sim defaults), §F (body mutation),
§G (action edge cases), §E (vision input layout for D).

**Status at handoff:** B is committed (`072e5ea`). C.10 / C.13 / C.14 are
implemented in B; C.11 is partial (eye+mouth rings); C.12 is the meat.

**Out of scope:** NN forward pass, real action selection, species speciation
distance — all deferred to D / E. Do not pull them forward.

---

## C.10 audit — Genome struct + per-trait mutation (DONE in B)

Re-audit only. The implementer should run `cargo test -p evosim genome` and
visually check `src/genome.rs::mutate_in_place` against v5 §6 + v6 §F. **Do not
edit** unless audit finds drift.

Checklist (cite line numbers in PR body if you find issues):
- [ ] `eye_count` mutates only to index-adjacent values in `EYE_VALID =
      [0,2,3,4,6,8,12,24]` (v6 §F). Already covered by `eye_count_only_mutates_to_adjacent`.
- [ ] `eye_offsets[i]` mutated only for `i < eye_count`, σ=0.1 rad, circular
      wrap (v5 §6, v6 §F). Confirm wrap formula `(x % τ + τ) % τ`.
- [ ] Continuous traits use `N(0, 0.1 × current)` (v5 §6). `mutate_in_place`
      precomputes σ with a floor (e.g. `self.move_speed.max(0.1)`) so a zeroed
      trait can re-enter the gene pool — confirm this floor is intentional
      and document if not (DECISIONS).
- [ ] Mutation rates themselves drift at 0.5%/birth, ±20% jitter (v5 §5+§6).
- [ ] Iteration order matches struct declaration order, single RNG (v6 §F).

If drift found, fix it minimally and add one regression test. **No struct
shape changes** — Brain/Persistence depend on the current layout.

---

## C.11 — pigment → color + feature rings (PARTIAL)

### State going in
`render.ts::drawCreatures` already draws filled circles using
`(pigment_r, pigment_g, pigment_b)` and two rings (white=eyes innermost,
red=mouth). v6 §B specifies **five** rings.

### v6 §B ring spec (verbatim)
> - `move_speed > 0`: yellow ring
> - `eat_efficiency > 0`: red ring
> - `scavenge_efficiency > 0`: brown ring
> - `eye_count > 0`: white ring (innermost)
> - `armor > 0`: silver/light-gray ring
> Render order: innermost (eye) outward, so visual layering is consistent.

### Interpretation (lock this in)
v6 §B fixes only `eye = innermost`. The other four are unordered in the spec.
Pin the order, **innermost → outermost**, as:

1. eye (white) — innermost
2. move (yellow)
3. scavenge (brown)
4. mouth (red)
5. armor (silver) — outermost

Rationale: rings draw inward from the body edge; armor as outermost makes
visual sense (it's literally a shell). Cite this choice as a DECISIONS line:
`ring order: eye→move→scav→mouth→armor (innermost→outermost) — v6 §B fixes only eye=innermost; rest are aesthetic`.

### Wasm API change — extend the creature buffer

`src/wasm_api.rs::creatures_buffer` currently emits stride **10**. The render
needs 5 ring flags. Two options:

- **(A) Extend stride to 12:** add `has_move`, `has_scavenge`, `has_armor` (also
  rename existing `has_eyes`/`has_mouth` slots for clarity). Pro: one
  contiguous buffer, identical to existing memory pattern. Con: every consumer
  must be updated.
- **(B) Add a separate `creature_flags_buffer()` returning 1 byte (or 1 f32)
  per creature with bitpacked flags.** Pro: keeps the 10-float core lean.
  Con: two buffers per render frame, two `Float32Array.view` calls.

**Pick (A).** Existing renderer already reads `data[i+8]/data[i+9]` flags from
the same buffer; pattern is established; D will add more state (`last_action`
display, current action) and we'd otherwise be juggling 3+ buffers.

#### New stride layout (v1.0 — bump `creature_stride()` to 12)

| Offset | Field | Notes |
|---|---|---|
| 0 | x | world units |
| 1 | y | world units |
| 2 | radius_world | `size × BODY_RADIUS_PER_SIZE` |
| 3 | pigment_r | 0..1 |
| 4 | pigment_g | 0..1 |
| 5 | pigment_b | 0..1 |
| 6 | energy_frac | clamped to [0,1] for UI |
| 7 | age_frac | clamped to [0,1] |
| 8 | flag_eye | `eye_count > 0 ? 1.0 : 0.0` |
| 9 | flag_move | `move_speed > 0 ? 1.0 : 0.0` |
| 10 | flag_scav | `scavenge_efficiency > 0 ? 1.0 : 0.0` (new) |
| 11 | flag_mouth | `eat_efficiency > 0 ? 1.0 : 0.0` (renamed slot) |
| 12 | flag_armor | `armor > 0 ? 1.0 : 0.0` (new) |

Stride becomes **13** (not 12 — I miscounted: 0..12 inclusive). Update
`creature_stride() -> 13` + the buffer-fill loop + render reader.

### Render-side change (`web/src/render.ts::drawCreatures`)

Pseudocode:
```
let ringR = radiusPx - 1;
const RING_STEP = 1.5;
function drawRing(color: string) {
  if (ringR < 1) return;
  ctx.strokeStyle = color;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.arc(sx, sy, ringR, 0, Math.PI * 2);
  ctx.stroke();
  ringR -= RING_STEP;
}
if (flagEye)   drawRing("rgba(255,255,255,0.85)"); // innermost
if (flagMove)  drawRing("rgba(240,220, 80,0.85)"); // yellow
if (flagScav)  drawRing("rgba(140, 90, 40,0.85)"); // brown
if (flagMouth) drawRing("rgba(255, 80, 80,0.85)"); // red
if (flagArmor) drawRing("rgba(200,200,210,0.85)"); // silver
```

Tiny creatures: if `radiusPx < 3` the inner rings clip; that's acceptable —
zooming reveals them. Don't add a min-size override.

### C.11 tests
- Audit only — no Rust logic change. Add a 1-line assertion in
  `creature_stride` doctest or a `wasm_api` smoke test that the buffer length
  matches `creatures.len() * 13`.
- Manual: launch the app, watch a 1k-tick run, confirm 5 ring colors appear
  on differently-traited creatures. (Hardcoded behavior in C.12 will spawn
  enough trait diversity within a few hundred ticks; if not, bump
  `mutation_rate_multiplier` slider to 5 in dev panel.)

---

## C.12 — vision + movement + hardcoded behavior (the meat)

### Goal
Wire per-creature raycasts into a 120-float vision cache and pin the cache
layout so Milestone D's NN forward pass slots in with zero refactor. Movement
gets driven by a temporary hardcoded picker that consumes the cache.

### v5 §10 (verbatim)
> 24 fixed angular sector slots. `eye_count` ∈ {0,2,3,4,6,8,12,24} selects how
> many physical eyes; they tile the 24 slots evenly starting from
> `eye_offsets[0]`. Each active sector raycasts one ray of length
> `vision_range` against the shared spatial grid. Returns first hit's
> `(distance, target_size, target_r, target_g, target_b)`. Inactive sectors:
> zeros.

### Sector layout — decision (pin now, D inherits)

**Active sector predicate.** For `eye_count = k > 0`, the active sectors are
`s ∈ {0, 24/k, 2·24/k, ..., (k-1)·24/k}` (i.e. `s mod (24/k) == 0`). For
`k = 0`, no sectors active. Use `EYE_STRIDE[k_index] = 24 / k` lookup.

**Slot-center angle.** Sector `s` has nominal center
`theta_center(s) = 2π × s / 24`. Active-sector indices into `eye_offsets` are
`active_index = s / (24 / k)` (so active sector `s` reads `eye_offsets[active_index]`).

**Effective ray angle.** `theta_ray(s) = theta_center(s) + eye_offsets[active_index]`.
The offset is an additive perturbation off slot center — small (σ=0.1 rad per
v6 §F). This matches the v5 §10 wording "tile the 24 slots evenly starting
from `eye_offsets[0]`" with the natural reading that `eye_offsets[0]` is the
*per-slot offset* of the first active eye, not the global rotation. DECISIONS
line: `eye_offsets semantics: per-active-slot additive offset off slot center — v5 §10 ambiguous; treats offsets as fine-grain, matches σ=0.1 rad scale in v6 §F`.

Output buffer offset for sector `s` is always `s * 5`; **inactive sectors
write zeros, never get skipped.** This is critical for D — the NN sees a
fixed 120-input layout regardless of `eye_count`.

### Files & function signatures

**New module:** `src/vision.rs`

```rust
//! Per-creature vision: 24 sector slots × 5 features.
//! Filled in tick step 2 (v5 §3.5). Reused by D as NN inputs (v6 §E).

use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grid::SpatialGrid;

pub const SECTORS: usize = 24;
pub const FEATURES_PER_SECTOR: usize = 5; // dist, size, r, g, b
pub const VISION_LEN: usize = SECTORS * FEATURES_PER_SECTOR; // 120

/// Fixed per-creature vision buffer. World owns Vec<[f32; 120]>.
pub type VisionBuf = [f32; VISION_LEN];

/// Carrion is rendered/seen as fixed gray per v6 §4 of "What changed vs v5".
pub const CARRION_R: f32 = 0.4;
pub const CARRION_G: f32 = 0.4;
pub const CARRION_B: f32 = 0.4;

/// Cap on grid cells walked per ray. Bounds worst-case raycast cost.
/// vision_range_max = 80u; HASH_CELL = 5u → ~16 cells along a ray axis.
/// Worst diagonal traversal ≈ 32 cells; double for slack.
pub const MAX_CELLS_PER_RAY: usize = 64;

/// Lookup: index = position of eye_count in EYE_VALID; value = sector stride.
/// EYE_VALID = [0, 2, 3, 4, 6, 8, 12, 24] → strides [-, 12, 8, 6, 4, 3, 2, 1].
/// Position 0 (eye_count == 0) is unused (no active sectors).
pub const EYE_STRIDE: [u8; 8] = [0, 12, 8, 6, 4, 3, 2, 1];

pub struct VisionPass<'a> {
    pub creatures: &'a CreatureSoA,
    pub carrion: &'a [Carrion],
    pub grid: &'a SpatialGrid,
}

impl<'a> VisionPass<'a> {
    /// Populate `out[i]` for every creature with the post-grid-rebuild state.
    /// `out.len() == creatures.len()` precondition.
    pub fn run(&self, out: &mut [VisionBuf]);

    /// Fill `buf` for creature index `i`.
    fn fill_one(&self, i: usize, buf: &mut VisionBuf);

    /// DDA along the spatial grid. Returns (distance, target_idx, target_kind).
    /// target_kind = Creature(usize) | Carrion(usize) | None.
    fn raycast(&self, self_idx: usize, ox: f32, oy: f32, dx: f32, dy: f32, max_dist: f32) -> Option<RayHit>;
}

enum RayHit { Creature(usize, f32), Carrion(usize, f32) }
```

### Raycast algorithm — grid DDA

Standard 2D-DDA (Amanatides & Woo 1987), single shared spatial grid (v5 §3.3).
Per-ray:

1. **Setup:** origin `(ox, oy)` = self position. Direction `(dx, dy) =
   (cos θ_ray, sin θ_ray)`. `max_dist = genome.vision_range`. If
   `max_dist <= 0.0` skip (return None).
2. **Start cell:** `(cx, cy) = (floor(ox / HASH_CELL), floor(oy / HASH_CELL))`,
   clamped to `[0, HASH_DIM-1]`.
3. **Step direction:** `step_x = sign(dx)`, `step_y = sign(dy)`. Handle
   `dx == 0` / `dy == 0` by setting `t_delta = ∞` for that axis (use `f32::MAX`).
4. **Initial `t_max`:** distance from origin to first cell boundary along
   each axis, divided by ray direction component. `t_delta_x = HASH_CELL / |dx|`,
   `t_delta_y = HASH_CELL / |dy|`.
5. **Walk:**
   ```
   cells_walked = 0;
   loop {
     // Test all creatures in this cell (skip self).
     let start = grid.starts[c]; let end = grid.starts[c+1];
     for k in start..end {
       let j = grid.indices[k] as usize;
       if j == self_idx { continue; }
       // circle-ray intersection
       let t = ray_circle_hit(ox, oy, dx, dy, creatures.x[j], creatures.y[j], radius_j);
       if t.is_some() && t? <= max_dist && t? < best_dist {
         best = Some(Creature(j, t?));
       }
     }
     // Also test overlapping carrion in this cell (carrion list is short — linear scan acceptable for v1; we can index-by-cell in v1.1 if needed)
     // Carrion is rendered same radius as a tiny creature; use a small fixed radius (1.0 world unit) for ray collision per CARRION_RADIUS.

     if best.is_some() && best_t < min(t_max_x, t_max_y) { return best; }
     // advance to next cell
     if t_max_x < t_max_y { cx += step_x; t_max_x += t_delta_x; }
     else                 { cy += step_y; t_max_y += t_delta_y; }
     if cx out of [0, HASH_DIM) || cy out of [0, HASH_DIM) { break; }
     if min(t_max_x, t_max_y) > max_dist { break; }
     cells_walked += 1;
     if cells_walked >= MAX_CELLS_PER_RAY { break; }
   }
   return best;
   ```

**Ray-circle intersection (closed form):**
```
fn ray_circle_hit(ox, oy, dx, dy, cx, cy, r) -> Option<f32> {
    let mx = ox - cx; let my = oy - cy;
    let b = mx * dx + my * dy;
    let c = mx*mx + my*my - r*r;
    if c > 0.0 && b > 0.0 { return None; }  // origin outside, ray points away
    let disc = b*b - c;
    if disc < 0.0 { return None; }
    let t = -b - disc.sqrt();
    if t < 0.0 { return None; }              // origin inside — treat as no hit (don't see things touching us)
    Some(t)
}
```

**Carrion-in-cell lookup:** v1 keeps `World.carrion: Vec<Carrion>` flat. For
each ray, build a per-tick HashMap<cell_index, Vec<usize>> of carrion indices
**once** at the start of the vision pass (or pre-sort by `sun_cell` then
binary-search). Simplest: pre-build `cell_to_carrion: Vec<Vec<usize>>` keyed
by **HASH grid cell** (not sun cell). One pass over `self.carrion`, O(N+C).
Document: `CARRION_RADIUS_FOR_VISION = 1.5` world units (DECISIONS).

**Why DDA over per-cell brute force:** v5 §3.6 calls raycasts "the dominant
cost." Brute force iterates *every* creature in the bounding box of the ray
(quadratic-ish at density). DDA visits only cells along the ray line — O(L/cell)
where L is ray length. At `vision_range = 80` and `HASH_CELL = 5`, that's ≤ 16
cells per ray, vs ~ 256 cells in the bounding box.

### Hardcoded behavior (C.12 only; D replaces wholesale)

In `World::step`, after the vision pass, replace `pick_action_b` with a
`pick_action_c` that uses the vision cache to set `vx`, `vy` AND choose action.

**Algorithm (deterministic, no RNG):**
```
fn pick_action_c(i, vision_buf_i, genome, energy, cooldown, rng) -> (vx, vy, Action):
    let speed = genome.move_speed;

    // 1. Find closest visible target (any non-empty sector).
    // Walk all 24 sectors, pick smallest non-zero distance.
    let mut best: Option<(usize, f32, [f32;5])> = None;
    for s in 0..24 {
        let off = s * 5;
        let d = vision_buf_i[off];
        if d <= 0.0 { continue; }
        let features = [vision_buf_i[off], ..off+5];
        if best.map_or(true, |(_, bd, _)| d < bd) {
            best = Some((s, d, features));
        }
    }

    // 2. Action selection (v6 §1 valid-fallthrough — but no NN logits here).
    //    Hardcoded preference order:
    //    - if energy >= 80, Split
    //    - else if best is carrion-colored AND scavenge_efficiency > 0, Scavenge
    //    - else if best is non-carrion AND eat_efficiency > 0 AND cooldown==0, Eat
    //    - else Photosynth
    let action = if energy >= 80.0 { Split }
                 else if best.map(is_carrion).unwrap_or(false) && genome.scavenge_efficiency > 0.0 { Scavenge }
                 else if best.is_some() && !best.map(is_carrion).unwrap_or(false)
                         && genome.eat_efficiency > 0.0 && cooldown == 0 { Eat }
                 else { Photosynth };

    // 3. Velocity: if we have a target and speed > 0, head toward it; otherwise tiny drift.
    let (vx, vy) = if let Some((s, _, _)) = best && speed > 0.0 {
        let theta = sector_to_angle(s, &genome.eye_offsets, genome.eye_count);
        (speed * theta.cos(), speed * theta.sin())
    } else if speed > 0.0 {
        // tiny drift so distance-travelled accumulates ("first to move" event fires)
        // deterministic per-creature: hash creature id → angle
        let theta = (creatures.id[i] as f32 * 1.61803).rem_euclid(TAU);
        (0.3 * speed * theta.cos(), 0.3 * speed * theta.sin())
    } else { (0.0, 0.0) };

    (vx, vy, action)

fn is_carrion(features: [f32; 5]) -> bool {
    // r, g, b at indices 2, 3, 4 — carrion is exactly (0.4, 0.4, 0.4)
    (features[2] - 0.4).abs() < 1e-4 && (features[3] - 0.4).abs() < 1e-4 && (features[4] - 0.4).abs() < 1e-4
}
```

This is intentionally dumb and replaced in D. It exists to (a) exercise the
vision cache end-to-end, (b) let movement-cost accounting, eat, scavenge, and
the "first to move" event fire under real-ish conditions in C.

### World integration

Add to `World`:
```rust
pub vision: Vec<VisionBuf>,  // one per creature, index-aligned
```

In `CreatureSoA::push`, also push a zeroed `VisionBuf`. In `remove_indices`,
also `swap_remove` from vision. (Alternative: keep `vision` separate on World
and resize per tick — but index-aligning with the other Vecs is the safer
choice.)

In `World::step`, replace the stub at step 2:
```rust
// 2. Vision pass.
self.run_vision_pass();

// 3. Choose action (Milestone C hardcoded picker).
for i in 0..self.creatures.len() {
    let (vx, vy, action) = pick_action_c(
        i,
        &self.vision[i],
        &self.creatures.genomes[i],
        self.creatures.energy[i],
        self.creatures.digestion_cooldown[i],
        self.creatures.id[i],
    );
    self.creatures.vx[i] = vx;
    self.creatures.vy[i] = vy;
    self.creatures.action_this_tick[i] = action;
}
```

`run_vision_pass` impl:
```rust
fn run_vision_pass(&mut self) {
    let n = self.creatures.len();
    if self.vision.len() < n { self.vision.resize(n, [0.0; 120]); }
    let pass = VisionPass {
        creatures: &self.creatures,
        carrion: &self.carrion,
        grid: &self.grid,
    };
    // build per-cell carrion index here, pass into VisionPass via field
    pass.run(&mut self.vision[..n]);
}
```

If `genome.vision_range == 0.0` OR `genome.eye_count == 0`: zero the whole
buffer and return early (saves 100% of the per-creature work for the founder
and any blind lineages).

### Performance notes (within v5 §3.6 budget)

- **Budget:** ~3–5 ms total for 1500 creatures × ~12 active rays × ~10 cells/ray.
- **Hot loop:** `raycast` inner. Avoid bounds checks via slice access of
  `grid.starts` / `grid.indices` (already `Vec<u32>`). No allocation per ray.
- **Per-tick allocation:** the cell→carrion index. Cache as a `World` field
  (`Vec<Vec<u32>>` of size HASH_DIM²); `.clear()` each entry per tick, no
  realloc.
- **DO NOT** parallelize with rayon yet — D will set the parallelization
  pattern. Sequential here keeps determinism trivial.
- B's perf gate isn't here. F.29 is. Don't over-optimize; keep the code
  readable and reach for SIMD only if F.29 fails.

### C.12 tests (minimum 5)

Add to `src/vision.rs::tests`:

1. **`eye_count_zero_yields_all_zero_vision`** — create one creature with
   `eye_count=0, vision_range=80`, place another in front, run vision pass,
   assert `vision[0] == [0.0; 120]`.

2. **`ray_hits_self_ignored`** — single creature at origin with
   `eye_count=24, vision_range=80`; assert vision is all zeros (no self-hit).

3. **`ray_hits_creature_returns_color_and_distance`** — creature A at
   `(100, 100)`, `eye_count=24, vision_range=80`. Creature B at `(110, 100)`
   with `pigment = (0.9, 0.1, 0.2)`, `size = 1.0`. Active sector pointing
   east (0 rad) should return `dist ≈ 9` (10 - radii), `size = 1.0`, `r=0.9
   g=0.1 b=0.2`. Other sectors zero.

4. **`carrion_returns_gray`** — single creature with eyes; carrion in front;
   the responsible sector returns `(d, ~0.4, 0.4, 0.4)` and the size feature
   matches `CARRION_RADIUS_FOR_VISION`.

5. **`out_of_range_returns_zero`** — target placed at distance 90 > vision_range
   80 → that sector is zero.

Add to `src/world.rs::tests`:

6. **`world_runs_2000_ticks_with_movement`** — like the existing smoke test
   but force the founder's `move_speed = 1.0` and `vision_range = 30,
   eye_count = 4` via test helper; assert no panic, tick > 0 (population may
   end at 0 — that's fine; we're stressing the vision+movement loop).

7. **`vision_layout_matches_nn_input_block`** — assert
   `VISION_LEN == 120 && NN_INPUTS == 10 + VISION_LEN + 6`. Prevents D from
   silently drifting.

---

## C.13 audit — eat + scavenge (DONE in B)

`src/world.rs::eat_and_scavenge` already implements:
- bite_reach × size range check
- closest-target tiebreak by lowest id (v6 §G)
- mouth/gut taxes (in `energy_bookkeeping`)
- digestion cooldown (50 ticks)
- attempt costs whether or not a target was found (v6 §G)
- armor damage reduction `(1 - armor)`

Audit checklist:
- [ ] Eat in cooldown short-circuits *before* consuming attempt cost? **No** —
      v6 §G says cooldown still treats action as invalid; v5 §7 says attempt
      cost is paid for invalid action. v6 §1 says invalid actions fall through
      to next-logit. **In C the hardcoded picker won't issue Eat while in
      cooldown**, so this discrepancy is latent. **Leave as-is in C**; flag
      for D (where the NN's logit fallthrough machinery lives). DECISIONS:
      `eat-while-cooldown cost-charge: B charges attempt cost; D's fallthrough will avoid issuing Eat in cooldown — punt fix to D`.
- [ ] Carrion drain — currently picks first overlapping corpse and breaks.
      That's deterministic since carrion order is stable per tick. Acceptable.
- [ ] No re-entrancy bug: damage[] / gain[] applied after iteration. Good.

No code change expected.

---

## C.14 audit — soft-repulsion + wall clamp (DONE in B)

`src/world.rs::apply_movement_and_repulsion` implements:
- velocity clipping to `move_speed` magnitude
- `F = clamp(2.0 × overlap, 0, 5.0)` symmetric (v6 §D)
- wall clamp + zero wall-ward velocity (v5 §3.3, §9)
- co-located deterministic nudge for newborn siblings
- post-repulsion grid rebuild

Audit checklist:
- [ ] Per v6 §D "Velocity clipped to `genome.move_speed` magnitude
      pre-application." — confirm clip happens **before** integration. Code
      reads vx/vy, computes `(cvx, cvy)`, uses those to integrate position.
      Good.
- [ ] Movement cost `COST_MOVE_PER_DIST × distance` debited per v5 §7. ✓
- [ ] Wall-clamp uses `genome.size × BODY_RADIUS_PER_SIZE` (not raw size).
      Confirm: yes (line ~281).
- [ ] `is_at_wall` threshold (v6 §D, `size + 5`) used by the **NN input**,
      not by the physics step. That's a brain.rs/D concern, not C.14. Skip.

No code change expected.

---

## Integration order (for the implementer)

1. **C.10 audit** — run `cargo test`, eyeball `genome.rs`. Likely zero changes.
2. **C.13/C.14 audit** — run tests, eyeball `world.rs`. Likely zero changes.
3. **C.11 wasm side** — bump `creature_stride()` 10 → 13; extend
   `creatures_buffer` fill; update render reader. One commit.
4. **C.11 render side** — add 3 ring colors + render order. Same commit as
   above OR separate. Verify visually with `pnpm dev`.
5. **C.12 vision module** — new `src/vision.rs`, add `pub mod vision` to
   `lib.rs`. Add `vision: Vec<VisionBuf>` to `World`, init in `World::new`,
   resize/zero on push/remove. Add `cell_to_carrion: Vec<Vec<u32>>` field
   (reused per tick). Implement DDA raycast. Tests first (TDD: write the 5
   unit tests, watch them fail with the stub, then make them pass).
6. **C.12 hardcoded picker** — replace `pick_action_b` with `pick_action_c`.
   Remove `pick_action_b` (it's dead). Rerun world smoke + the new movement
   smoke. Bump `cargo test`.
7. **Commit each step separately** (so a regression bisects cleanly).
   Suggested messages:
   - `M C.11: add 3 missing feature rings + bump creature stride to 13`
   - `M C.12: vision pass (24-sector DDA raycast) + 120-float cache`
   - `M C.12: hardcoded vision-driven action picker (placeholder for D)`

---

## Vision layout (load-bearing for D)

**Do not change after this commit lands.** D's NN forward pass reads the
136-input vector in this exact order:

| Range | Count | Source | Notes |
|---|---|---|---|
| 0..10 | 10 | self-state | v6 §E |
| 10..130 | 120 | `vision_buf[i]` directly | this is C.12's output |
| 130..136 | 6 | last_action one-hot | v6 §E |

Inside the 120-float vision block:

```
sector 0:  [dist, size, r, g, b]   floats 0..5
sector 1:  [dist, size, r, g, b]   floats 5..10
...
sector 23: [dist, size, r, g, b]   floats 115..120
```

`distance` is **raw world units** (0 = empty sector or hit at zero distance —
disambiguate by reading `size`; size==0 means empty). If we want strict
"distance==0 means empty," ensure the raycast never reports `dist < 1e-4`
(use a tiny epsilon).

D may want normalized inputs (`dist / vision_range`); that normalization
**happens in D's input-prep**, not here. Vision cache stays in raw units so
the cache stays inspectable for debugging.

---

## Feature-ring rendering order (lock-in)

```
innermost → outermost:
  1. eye    (white)
  2. move   (yellow)
  3. scav   (brown)
  4. mouth  (red)
  5. armor  (silver)
```

DECISIONS line required.

---

## Code-reviewer's checklist

- [ ] `vision_buf` resized exactly when `creatures.push` / `remove_indices`
      runs — no stale data after a death.
- [ ] DDA handles `dx == 0` and `dy == 0` without NaN (divide-by-zero
      guard).
- [ ] `ray_circle_hit` does the "origin inside circle" check — otherwise a
      raycaster overlapping a target self-hits at t = 0.
- [ ] Carrion vision uses fixed gray `(0.4, 0.4, 0.4)` **regardless** of
      the dead creature's original pigment (v6 §4 of "What changed vs v5").
- [ ] `eye_count == 0` OR `vision_range == 0` short-circuits the per-creature
      vision fill to a zero buffer.
- [ ] Inactive sectors are zero, NOT missing (the 120-float buffer is always
      120 floats, even with `eye_count = 2`).
- [ ] `pick_action_c` uses `creatures.id` (not loop index `i`) for the
      "no target" deterministic drift — `i` shuffles on swap_remove.
- [ ] `creature_stride()` bumped to **13** and matches the fill loop and the
      render reader. (3 separate places — easy to drift.)
- [ ] Existing `world_runs_many_ticks_without_panic` and
      `energy_conservation_in_photosynth_pass` still pass.
- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `pnpm build` clean.
- [ ] Manual: load app, watch 1k ticks, confirm 5 ring colors appear and
      creatures with `move_speed > 0` actually move.

---

## Punt list (file as DECISIONS lines if accepted, else leave for D/E/F)

- Eat-while-in-cooldown attempt-cost: leave B's behavior (charge cost); D's
  valid-fallthrough makes this moot.
- Carrion ray-collision radius: 1.5 world units (made up; not in spec). v1.1
  could expose this as a constant if scavengers struggle to find carrion.
- Per-cell carrion index rebuilt every tick: simpler than maintaining
  incrementally. O(N+C) per tick; trivial at v1 sizes.
- Hardcoded picker uses energy threshold 80 (matches B's `SPLIT_PLACEHOLDER`),
  not the v5 spec's 50, for the same starvation reason documented in DECISIONS.

---

## Out-of-scope reminders (v5 §15 guardrails)

- No NN forward pass, no real action selection by logits. (D)
- No species speciation distance check. Children still inherit parent species_id. (E)
- No double-buffered transferable persistence. (F.26)
- No new traits, sliders, or UI affordances beyond v5 §14's Included list.

---

## Code review — APPROVED

All 31 lib tests pass; `cargo clippy --all-targets -- -D warnings` is clean.
Spec compliance and the priority-check items below all verified:

- **Vision layout & determinism** (`src/vision.rs:53-125`): 120-float buffer
  is fully zeroed at the top of `fill_one`; inactive sectors stay zero. Active
  sector predicate `s % stride == 0` and `active_index = s / stride` match
  spec. `EYE_STRIDE` lookup table correctly maps `eye_count ∈ EYE_VALID`.
- **DDA raycast** (`src/vision.rs:128-254`):
  - `dx == 0` / `dy == 0` handled via `t_delta/t_max = f32::MAX` (lines 147-178).
  - `ray_circle_hit` (line 260) returns `None` when origin inside circle
    (`t < 0`), preventing self-hits through overlap.
  - Self-creature skipped explicitly (line 196).
  - `MAX_CELLS_PER_RAY = 64` enforced (line 247).
  - Per-cell "test all then compare to next boundary" logic is correct: hit
    is only returned when `best_dist < next_t` (line 225), so a closer hit
    in a later cell can still win until the boundary is crossed.
- **Vision Vec lifecycle**: founder init at `world.rs:123`; `swap_remove`
  mirrored before `creatures.remove_indices` at `world.rs:181-188`; births
  rely on next-tick `run_vision_pass` resize at `world.rs:683-685` (safe —
  vision is consumed in step 2, births occur in step 11).
- **Carrion-as-gray** (`vision.rs:114-121`): fixed `(0.4, 0.4, 0.4)`
  regardless of dead creature's original pigment. Carrion struct doesn't
  retain pigment so drift is impossible.
- **Short-circuit** (`vision.rs:64`): `eye_count == 0 || vision_range <= 0.0`
  zeros the buffer and returns.
- **`pick_action_c` determinism** (`world.rs:711-778`): no RNG, drift uses
  `creature_id` (line 769) not loop index.
- **Movement wiring**: `vx`/`vy` set in step 3 (`world.rs:158-159`) flow
  into `apply_movement_and_repulsion` step 4; speed_cap > 0 gating preserved.
- **Render order** (`render.ts:144-148`): eye(white) → move(yellow) →
  scav(brown) → mouth(red) → armor(silver), innermost outward — matches
  plan and DECISIONS pin.
- **Stride 13 consistency**: `creature_stride() = 13`
  (`wasm_api.rs:193`); fill loop writes offsets 0..12 (`wasm_api.rs:107-126`);
  render reads offsets 0..12 (`render.ts:103-117`). All three sites aligned.
- **Tests present**: 5 vision tests (`eye_count_zero_yields_all_zero_vision`,
  `ray_hits_self_ignored`, `ray_hits_creature_returns_color_and_distance`,
  `carrion_returns_gray`, `out_of_range_returns_zero`) + 2 world tests
  (`world_runs_2000_ticks_with_movement`, `vision_layout_matches_nn_input_block`).

Minor non-blockers (do not gate D):

1. `vision.rs:317` re-exports `BODY_RADIUS_PER_SIZE` from `world::` to keep
   the module use-line stable; could be sourced from `world::` directly in
   D's cleanup pass. Cosmetic.
2. `cell_to_carrion: &'a Vec<Vec<u32>>` (vision.rs:47) could be `&'a [Vec<u32>]`
   for idiomatic Rust; clippy didn't flag it. Cosmetic.
3. Carrion's `size` feature in the vision cache is the world-unit radius
   (`CARRION_RADIUS_FOR_VISION = 1.5`), whereas creatures' `size` is the
   unitless genome size (1..10). D's NN-input prep should be aware that the
   "size" channel has mixed semantics. Already documented as a punt-list item.
4. When `energy >= SPLIT_PLACEHOLDER`, action is Split but velocity is still
   set toward the visible target — harmless (parent moves a tick toward
   target before splitting), and not specified by plan.
5. C.10/C.13/C.14 audit-only: no code touched. ✓

