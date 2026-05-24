# Plan — perf-5: genome hot-field SoA split (7 scalars, mirror)

**Status:** plan only. No code lands until this is signed off.
**Scope:** Add seven parallel `Vec<f32>` / `Vec<u8>` mirror arrays on
`CreatureSoA` for the seven per-tick-hot `Genome` scalars (`size`,
`photosynth_efficiency`, `eat_efficiency`, `scavenge_efficiency`,
`move_speed`, `vision_range`, `eye_count`). Rewrite the six hot tick
phases to read from the mirrors. Keep `genomes: Vec<Genome>` exactly as
today for cold paths (mutation, speciation, inspector, save, hash, HoF
clones). Mirrors are derived state: same f32 bytes, different layout.
Single commit. Golden-safe (the hash inputs do not change).

**Spec / research anchors (read before touching code):**

- `docs/plans/perf+ui-master.md` §"perf-5 — genome SoA split (7 fields,
  mirror)", §3 dependency graph (no upstream deps; lands as natural
  mid-PR review checkpoint per §6 commit 5), §6 R1 (do NOT remove
  `genomes`), §6 R7 (sync completeness — three write sites).
- `docs/research/perf-final-report.md` §3 item #7 (the 41 KB → in-L2
  win, six benefiting phases enumerated), §5a (golden-safe rationale —
  "values are identical; only storage layout differs"), §6 commit 5
  (~250 LOC budget, natural review checkpoint).
- `docs/research/perf-layout.md` §1 — source the final report cites.
  Confirms the 7 hot fields total ≈29 bytes/creature and the AoS
  `Genome` is 204 bytes (only ~14% of bytes loaded are actually read on
  any given pass).
- `src/genome.rs:53–70` — `Genome` struct; field types are the contract
  (six `f32`, plus `eye_count: u8`).
- `src/creature.rs:43–63` — `CreatureSoA`; new fields appended here.
- `src/creature.rs:97–155` — `push` / `remove_indices`; the only two
  sync points inside the SoA module.
- `src/world.rs` — the six hot read sites + the three creature-insert
  paths (founder push line 119; birth push line 945; save-restore push
  line 1026).
- `src/vision.rs:61–125, 191–210` — reads `size`, `vision_range`,
  `eye_count` for the self-creature; reads `size` for hit-targets.
- `src/save.rs:64–85, 207–239` — `CreatureSoASnapshot`; mirrors are
  derived and NOT added.
- `src/snapshot_hash.rs:20–80, 82–117` — `snapshot_hash` reads only
  `genomes[i]` via `hash_genome`; mirrors are NOT added.
- `src/wasm_api.rs:103–133` (`creatures_buffer`), `:226–238`
  (`creature_at`), `:243–280` (`creature_inspect_json`) — cold paths
  that continue to read `genomes[i]`.

**What is intentionally NOT in this plan:**

- No removal of `genomes: Vec<Genome>` (R1). Mirrors are **additive**;
  the AoS struct stays the source of truth.
- No change to the `Genome` struct or its `mutate_in_place` body.
- No SIMD over the new arrays — that's perf-commit 7 in
  `perf-final-report.md` §6 and is intentionally deferred. This plan
  only unblocks it.
- No change to `wasm_api.rs::creature_inspect_json` /
  `creatures_buffer` / `creature_at` — they continue to read the
  full AoS `Genome` so the inspector keeps seeing all 14 trait fields
  (R1).
- No change to `species_distance` (cold; reads whole anchor genome).
- No change to `save.rs::CreatureSoASnapshot` or
  `snapshot_hash::hash_genome` — mirrors are derived and would
  double-count.
- No change to the `World::new` / `handle_births` / `from_save_v1`
  insertion paths beyond what's required to feed the new sync helper
  through `CreatureSoA::push` (which already runs at all three sites).

---

## High-level decisions (pinned)

These eight decisions are the contract; downstream review keys off them.

**D1. The seven mirror fields and their exact names.** Pinned to mirror
`Genome` field names with a `g_` prefix (`g_` = "genome scalar, hot
mirror"). Prefix makes grep trivial and prevents collision with the
existing SoA scalar names (e.g. `size` is not on `CreatureSoA` today, but
a future refactor might add `max_size_reached → size` — the prefix keeps
the namespace clean).

```rust
// src/creature.rs — appended to CreatureSoA:
pub g_size:          Vec<f32>,  // genomes[i].size
pub g_photo_eff:     Vec<f32>,  // genomes[i].photosynth_efficiency
pub g_eat_eff:       Vec<f32>,  // genomes[i].eat_efficiency
pub g_scav_eff:      Vec<f32>,  // genomes[i].scavenge_efficiency
pub g_move_speed:    Vec<f32>,  // genomes[i].move_speed
pub g_vision_range:  Vec<f32>,  // genomes[i].vision_range
pub g_eye_count:     Vec<u8>,   // genomes[i].eye_count
```

  Rationale on types: `g_eye_count` is `u8` to match the `Genome` field
  exactly (`genome.rs:61`). The final report's per-piece briefing
  suggests `u32` — but matching the source-of-truth type avoids any
  widening/narrowing surprise and is what `perf-layout.md §1` actually
  shows. Read sites that need `as usize` / `as f32` do the cast at the
  use site, just as they do today against `genome.eye_count`.

  Visibility: `pub(crate)` to match the rest of `CreatureSoA`. The
  fields are accessed from `world.rs`, `vision.rs`, and the new tests in
  `creature.rs`; no external (wasm) caller touches them, so they do
  NOT need to be `pub` to wasm-bindgen.

**D2. Mirror invariant (write-once semantics).** Mirrors must satisfy
`g_*[i] == genomes[i].<field>` for every `i ∈ 0..len()` at every
function-boundary observable from outside `CreatureSoA`. The only
functions that write the mirrors are:

1. `CreatureSoA::push` — pushes the seven mirror values from the
   provided `&Genome`, immediately after pushing `genomes`.
2. `CreatureSoA::remove_indices` — `swap_remove`s each of the seven
   mirror Vecs at the same `k` index alongside every existing
   `swap_remove` call. Same loop body, same iteration direction.
3. `CreatureSoA::with_capacity` — also reserves the seven mirror Vecs.

  All three insertion sites in `world.rs` (founder, birth, save-load)
  funnel through `push`, so this is **the single right place** to seed
  the mirrors. No code outside `CreatureSoA` is permitted to write
  `g_*`. Document this as a doc-comment on each mirror field:
  `/// Mirror of genomes[i].<field>. Written ONLY by CreatureSoA::push
  /// and remove_indices. Read by hot tick paths.`

  See R1 below for the debug-only sanity check.

**D3. No mid-tick mutation of `Genome` exists.** Confirmed by grep
across `src/`:

- `mutate_in_place` is invoked exactly once in non-test code, at
  `src/world.rs:896`: `child_genome.mutate_in_place(&mut self.rng,
  self.sliders.mutation_rate_multiplier);`. This mutates a **local
  `child_genome`** before the subsequent `self.creatures.push(...)` at
  line 945. The mutated value reaches the mirror through `push`. No
  separate sync call is needed.
- All other `genome.<field> = …` write sites in `src/` are inside
  `#[cfg(test)]` modules (see `world.rs:1389, 1418–1419, 1520, 1533,
  1633–1637, 1708–1709, 1785, 1827, 1840, 1855–1856, 1874–1875, 1968,
  2016`, all under `mod tests`). Tests that mutate a `Genome` in-place
  after `push` do NOT go through any hot tick path that this plan
  rewrites — the acceptance / golden hash test does not hit those
  paths. The implementer audits this list during step 1 of the
  sequencing section and, for any test that DOES subsequently call a
  hot tick phase, adds a sync helper invocation (see R3).

  This is the linchpin assumption of the entire design. If the grep
  audit at implementation time finds a non-test in-place mutation,
  STOP and re-plan — the implementer must route it through `push`,
  add a sync helper, OR refactor it away.

**D4. Single-source-of-truth sync helper on `CreatureSoA`.** Define one
private helper to keep `push` clean and document the invariant:

```rust
/// Push the seven hot mirror scalars from `g` onto the parallel Vecs.
/// MUST be called exactly once per `genomes.push(...)`.
fn push_hot_mirrors(&mut self, g: &Genome) {
    self.g_size.push(g.size);
    self.g_photo_eff.push(g.photosynth_efficiency);
    self.g_eat_eff.push(g.eat_efficiency);
    self.g_scav_eff.push(g.scavenge_efficiency);
    self.g_move_speed.push(g.move_speed);
    self.g_vision_range.push(g.vision_range);
    self.g_eye_count.push(g.eye_count);
}
```

  Called from `push` immediately after `self.genomes.push(genome);` (it
  reads `&genome` before the move into the Vec — or we accept the
  borrow shape by reading into locals before the `push(genome)` and
  pushing the seven scalars afterward; the implementer's call,
  bit-identical either way).

**D5. `remove_indices` mirrors `swap_remove` on the seven Vecs.** Use
the existing `dead.iter().rev()` loop, append seven `swap_remove(k)`
calls — one per mirror — alongside the existing 18. No helper needed;
this matches the existing pattern verbatim.

**D6. Mirrors are EXCLUDED from save and from snapshot_hash.** Same
rationale as perf-1: mirrors are bit-identically reconstructible from
`genomes[i]` (which IS in both the snapshot and the hash). Including
them would double-count and would also create a save-load divergence
risk where a hand-edited save with mismatched mirror values silently
behaves wrong.

  - `src/save.rs:65–85` (`CreatureSoASnapshot`): **no new fields.**
  - `src/save.rs:207–239` (`validate_soa_lengths`): **no new length
    check.** (The mirror lengths are re-derived from the rebuilt
    `genomes` via `push`.)
  - `src/save.rs::SaveV1::from_world` / `to_save_v1`: **no new
    serialize lines.**
  - `src/snapshot_hash.rs:20–80, 82–117`: **no changes.** Hash input
    order stays fixed at v6 §M.

  On the load path (`world.rs:1026`), every restored creature flows
  through `creatures.push(save.creatures.genomes[i].clone(), ...)`,
  which calls `push_hot_mirrors(&g)` — mirrors are rebuilt automatically
  with zero extra code in `from_save_v1`.

**D7. Read-site rewrites are **pure scalar substitutions**.** Every
rewrite has the shape:

```rust
// before
let g = &self.creatures.genomes[i];           // or .clone(), or by ref
let want = PHOTO_GAIN_COEFF * g.photosynth_efficiency * g.size;

// after
let want = PHOTO_GAIN_COEFF * self.creatures.g_photo_eff[i] * self.creatures.g_size[i];
```

  Bit-identical: same f32 inputs, same multiplication order, same
  product. The only constraint is preserving the **left-to-right
  product order** in mixed expressions — copy expression structure
  verbatim from the existing line.

  Where a site reads multiple hot fields AND multiple cold fields
  (e.g. `energy_bookkeeping` reads `g.size`, `g.move_speed`,
  `g.eye_count`, `g.vision_range`, `g.eat_efficiency`,
  `g.scavenge_efficiency` — six of seven hot — AND `g.armor`,
  `g.max_age`), the implementer rewrites the six hot reads to mirror
  accessors and keeps a `let g = &self.creatures.genomes[i];` binding
  for the residual cold reads (`g.armor`, `g.max_age`). Net effect:
  the same `Genome` is loaded by the cold reads, but the hot SoA
  arrays are warm in L2 — which is the actual win, because the cold
  reads in `energy_bookkeeping` are now a *smaller* fraction of the
  loop body's memory traffic.

**D8. The threaded NN forward (`#[cfg(feature="threads")]` branch at
`src/world.rs:351–424`) is rewritten too.** It currently reads
`creatures_ref.genomes[i].size` at line 374 to compute `ri` for the
inline carrion-overlap test. That single read must switch to
`creatures_ref.g_size[i]` so the threaded codepath sees the same
mirror values. `pick_action_d` (called from inside the rayon closure
at line 404) ALSO reads hot fields — see the `build_nn_input` and the
`creatures.genomes[i].move_speed` read at line 1295. Those two sites
also switch to mirrors. The mirrors are `Sync` (a `Vec<f32>` shared by
ref into a `par_iter_mut` closure is fine) — same shape as `vision_ref`
and `creatures_ref` already passed in today.

---

## Files & function signatures (concrete diffs)

### `src/creature.rs`

Add the seven fields, extend `with_capacity` / `push` / `remove_indices`,
add `push_hot_mirrors`.

```rust
pub struct CreatureSoA {
    // … existing 18 fields, unchanged …

    // Hot-field mirrors (perf-5). Each entry at index i is bit-identically
    // equal to genomes[i].<field>. Written only by `push` and
    // `remove_indices`. Read by every per-tick hot phase
    // (photosynth_two_pass, energy_bookkeeping, apply_movement_and_repulsion,
    // build_nn_input, count_carrion_overlap, compute_is_at_wall,
    // eat_and_scavenge, VisionPass::fill_one).
    /// Mirror of genomes[i].size.
    pub(crate) g_size:          Vec<f32>,
    /// Mirror of genomes[i].photosynth_efficiency.
    pub(crate) g_photo_eff:     Vec<f32>,
    /// Mirror of genomes[i].eat_efficiency.
    pub(crate) g_eat_eff:       Vec<f32>,
    /// Mirror of genomes[i].scavenge_efficiency.
    pub(crate) g_scav_eff:      Vec<f32>,
    /// Mirror of genomes[i].move_speed.
    pub(crate) g_move_speed:    Vec<f32>,
    /// Mirror of genomes[i].vision_range.
    pub(crate) g_vision_range:  Vec<f32>,
    /// Mirror of genomes[i].eye_count.
    pub(crate) g_eye_count:     Vec<u8>,
}

impl CreatureSoA {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            // … existing 18 Vec::with_capacity(cap) …
            g_size:         Vec::with_capacity(cap),
            g_photo_eff:    Vec::with_capacity(cap),
            g_eat_eff:      Vec::with_capacity(cap),
            g_scav_eff:     Vec::with_capacity(cap),
            g_move_speed:   Vec::with_capacity(cap),
            g_vision_range: Vec::with_capacity(cap),
            g_eye_count:    Vec::with_capacity(cap),
        }
    }

    pub fn push(&mut self, …, genome: Genome, brain: Brain) -> usize {
        // … existing 17 pushes unchanged, ending with self.brains.push(brain); …
        // NEW: seed the seven mirrors BEFORE moving `genome` into self.genomes.
        self.push_hot_mirrors(&genome);
        self.genomes.push(genome);
        self.brains.push(brain);
        self.x.len() - 1
    }

    pub fn remove_indices(&mut self, dead: &[usize]) {
        for &k in dead.iter().rev() {
            // … existing 18 swap_removes …
            self.g_size.swap_remove(k);
            self.g_photo_eff.swap_remove(k);
            self.g_eat_eff.swap_remove(k);
            self.g_scav_eff.swap_remove(k);
            self.g_move_speed.swap_remove(k);
            self.g_vision_range.swap_remove(k);
            self.g_eye_count.swap_remove(k);
        }
    }

    fn push_hot_mirrors(&mut self, g: &Genome) {
        self.g_size.push(g.size);
        self.g_photo_eff.push(g.photosynth_efficiency);
        self.g_eat_eff.push(g.eat_efficiency);
        self.g_scav_eff.push(g.scavenge_efficiency);
        self.g_move_speed.push(g.move_speed);
        self.g_vision_range.push(g.vision_range);
        self.g_eye_count.push(g.eye_count);
    }
}
```

  Note the ordering in `push`: call `push_hot_mirrors(&genome)` BEFORE
  `self.genomes.push(genome)` to avoid the move-then-borrow tangle. The
  reads use `&Genome`, so this is a simple inversion.

### `src/world.rs` — six hot read-site rewrites

The implementer rewrites these specific lines. Each is a 1-line scalar
substitution (some lines have multiple hot reads).

#### Site A — `photosynth_two_pass` (`world.rs:561–604`)

```rust
// Line 568: let g = &self.creatures.genomes[i];
// Line 569: let want = PHOTO_GAIN_COEFF * g.photosynth_efficiency * g.size;
// Line 581: let g = &self.creatures.genomes[i];
// Line 582: let want = PHOTO_GAIN_COEFF * g.photosynth_efficiency * g.size;

// Rewrite both passes (5a demand + 5b payout):
let want = PHOTO_GAIN_COEFF
    * self.creatures.g_photo_eff[i]
    * self.creatures.g_size[i];
// (delete the `let g = ...` binding entirely — no other field needed in
// this scope.)
```

#### Site B — `apply_movement_and_repulsion` (`world.rs:427–559`)

Five hot reads:

```rust
// Line 433: let speed_cap = self.creatures.genomes[i].move_speed;
let speed_cap = self.creatures.g_move_speed[i];

// Line 452: && self.creatures.genomes[i].move_speed > 0.0
&& self.creatures.g_move_speed[i] > 0.0

// Line 456: let g = self.creatures.genomes[i].clone(); … (HoF capture)
// KEEP AS-IS — `g` is used at lines 461 (g.clone()), 464 (g.size).
// This is a cold path (fires once when first_move_fired flips). Leaving
// the clone() avoids inventing a separate cold/hot read pattern for the
// HoF capture; the cost is amortized.

// Line 485: let ri = self.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
let ri = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;

// Line 496: let rj = self.creatures.genomes[j].size * BODY_RADIUS_PER_SIZE;
let rj = self.creatures.g_size[j] * BODY_RADIUS_PER_SIZE;

// Line 531: let r = self.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
let r = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
```

#### Site C — `eat_and_scavenge` (`world.rs:606–719`)

Two `let g_i = self.creatures.genomes[i].clone()` bindings (lines 625
and 671) get split: the hot scalar reads move to mirrors; the residual
cold fields (`g_i.bite_reach`) stay on a tightened binding. One
`genomes[j].size` and one `genomes[j].armor` per body.

```rust
// Line 625–667 (Action::Eat arm):
// Replace:
//   let g_i = self.creatures.genomes[i].clone();
//   if g_i.eat_efficiency <= 0.0 { continue; }
//   let radius_i = g_i.size * BODY_RADIUS_PER_SIZE;
//   let reach = g_i.bite_reach * g_i.size;
//   …
//   let dmg = EAT_DAMAGE_COEFF * g_i.size;
//   …
//   gain[i] += EAT_GAIN_COEFF * g_i.eat_efficiency;
// With:
let eat_eff_i = self.creatures.g_eat_eff[i];
if eat_eff_i <= 0.0 { continue; }
let size_i = self.creatures.g_size[i];
let bite_reach_i = self.creatures.genomes[i].bite_reach; // COLD field; stays AoS
let radius_i = size_i * BODY_RADIUS_PER_SIZE;
let reach = bite_reach_i * size_i;
// …
let dmg = EAT_DAMAGE_COEFF * size_i;
// …
gain[i] += EAT_GAIN_COEFF * eat_eff_i;

// Line 645: let rj = self.creatures.genomes[j].size * BODY_RADIUS_PER_SIZE;
let rj = self.creatures.g_size[j] * BODY_RADIUS_PER_SIZE;

// Line 662: let armor = self.creatures.genomes[j].armor.clamp(0.0, 1.0);
// STAYS AS-IS — armor is NOT in the hot-7. Cold AoS read.

// Line 671–688 (Action::Scavenge arm):
// Replace:
//   let g_i = self.creatures.genomes[i].clone();
//   if g_i.scavenge_efficiency <= 0.0 { continue; }
//   let r_i = g_i.size * BODY_RADIUS_PER_SIZE;
//   let want = SCAVENGE_GAIN_COEFF * g_i.scavenge_efficiency;
// With:
let scav_eff_i = self.creatures.g_scav_eff[i];
if scav_eff_i <= 0.0 { continue; }
let r_i = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
let want = SCAVENGE_GAIN_COEFF * scav_eff_i;
```

  This also kills two `.clone()` calls per-creature-per-tick — a free
  win (item #4 of `perf-final-report.md` §3 already nibbles at this;
  perf-5 finishes it for the eat path).

#### Site D — `energy_bookkeeping` (`world.rs:721–781`)

Six of the seven hot fields appear; armor and max_age stay AoS:

```rust
// Line 724: let g = &self.creatures.genomes[i];
// REWRITE the binding to read mirrors directly and keep `g` for cold fields:
let size_i        = self.creatures.g_size[i];
let move_speed_i  = self.creatures.g_move_speed[i];
let eye_count_i   = self.creatures.g_eye_count[i];
let vision_range_i= self.creatures.g_vision_range[i];
let eat_eff_i     = self.creatures.g_eat_eff[i];
let scav_eff_i    = self.creatures.g_scav_eff[i];
let g = &self.creatures.genomes[i]; // for g.armor, g.max_age, g.clone() (HoF)

let mut up = UPKEEP_BASE;
if size_i > 1.0 {
    up += UPKEEP_SIZE_PER_UNIT * (size_i - 1.0);
}
if move_speed_i > 0.0 {
    up += UPKEEP_MOBILITY_FLAG;
    up += UPKEEP_MOVE_SPEED_PER_UNIT * move_speed_i;
}
up += UPKEEP_PER_EYE * eye_count_i as f32;
up += UPKEEP_VISION_COEFF * vision_range_i * vision_range_i;
if eat_eff_i > 0.0 {
    up += mouth_tax;
}
if scav_eff_i > 0.0 {
    up += UPKEEP_GUT;
}
if g.armor > 0.0 {
    up += UPKEEP_ARMOR_PER_UNIT * g.armor;
}
up += UPKEEP_LIFESPAN_PER_1K * (g.max_age as f32 / 1000.0);
up += UPKEEP_NN_FIXED;

let age = self.creatures.age[i];
if age > g.max_age {
    let excess = (age - g.max_age) as f32;
    let mult = PAST_LIFESPAN_MULT.powf(excess / 1000.0);
    up *= mult.min(1e6);
}

if size_i > self.creatures.max_size_reached[i] {
    self.creatures.max_size_reached[i] = size_i;
}
// Biggest-ever HoF (cold; still uses g.clone() / g.size). Keep AoS.
{
    let current_best = self.biggest_ever.as_ref().map_or(0.0, |h| h.captured_size);
    if size_i > current_best {
        let species_name = self.species.get(self.creatures.species_id[i]).name.clone();
        self.biggest_ever = Some(HallOfFame {
            creature_id: self.creatures.id[i],
            genome: g.clone(),
            species_name,
            captured_tick: self.tick,
            captured_size: size_i,
            captured_age: self.creatures.age[i],
        });
    }
}
// … rest unchanged …
```

  Critical: `mult.min(1e6)` is the existing pattern; do NOT reshuffle
  operator order or it'll perturb the lower bits. Copy verbatim.

#### Site E — `count_carrion_overlap` / `compute_is_at_wall` (`world.rs:1117–1161`)

```rust
// Line 1120: let ri = self.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
let ri = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;

// Line 1154: let r = self.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
let r = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
```

#### Site F — `build_nn_input` + `pick_action_d` (`world.rs:1207–1306`)

```rust
// Line 1215: let g = &creatures.genomes[i];    ← binding
// Line 1227: buf[2] = g.size / SIZE_MAX;        ← actual size read
// REWRITE: read mirror for `size` at line 1227; keep `g` for cold `max_age`
// at lines 1222–1226.
let size_i = creatures.g_size[i];
let g = &creatures.genomes[i]; // for g.max_age (cold, lines 1222–1226)
// …
buf[1] = if g.max_age > 0 {
    (age as f32 / g.max_age as f32).clamp(0.0, 1.0)
} else { 1.0 };
buf[2] = size_i / SIZE_MAX;

// Line 1295 (inside pick_action_d):
// let speed = creatures.genomes[i].move_speed;
let speed = creatures.g_move_speed[i];

// Line 1303: decode_action(logits, &creatures.genomes[i], energy, cooldown)
// STAYS AS-IS — decode_action reads g.eat_efficiency and g.scavenge_efficiency
// via `is_valid_action`. Both are in the hot-7, BUT decode_action takes
// `&Genome` not `&CreatureSoA`. The cleanest rewrite is to leave the
// `&Genome` API and accept one AoS read here (called once per creature
// per tick, identical cost as today). The hot win is in the OTHER five
// rewrites; this single AoS read is on the action-decode path and would
// require an API churn (changing decode_action to take SoA + index) for
// near-zero marginal benefit. KEEP AS-IS.
```

#### Site G — threaded NN forward path (`world.rs:351–424`)

```rust
// Line 374 (inside the par_iter_mut closure):
// let ri = creatures_ref.genomes[i].size * BODY_RADIUS_PER_SIZE;
let ri = creatures_ref.g_size[i] * BODY_RADIUS_PER_SIZE;
```

  `pick_action_d` (called at line 404 from inside the closure) already
  goes through the Site F rewrite above — no separate change needed
  for the threaded path beyond the inline carrion-overlap read at 374.

### `src/vision.rs` — hot self-reads and target-size reads

```rust
// Line 62: let g = &self.creatures.genomes[i];
// Line 64: if g.eye_count == 0 || g.vision_range <= 0.0 { … }
// Line 70: let k = g.eye_count as usize;
// Line 80: let max_dist = g.vision_range;
// Line 91: g.eye_offsets[active_index]  // COLD; eye_offsets is NOT in hot-7
// (kept on AoS read)

// REWRITE:
let eye_count_i = self.creatures.g_eye_count[i];
let vision_range_i = self.creatures.g_vision_range[i];
if eye_count_i == 0 || vision_range_i <= 0.0 {
    *buf = [0.0; VISION_LEN];
    return;
}
let k = eye_count_i as usize;
// … (k_idx computation unchanged) …
let max_dist = vision_range_i;
// `g.eye_offsets` still needed at line 91 — keep a binding:
let g = &self.creatures.genomes[i];
// … unchanged from here …

// Line 109: buf[slot + 1] = gj.size;  (and line 105 binding `let gj = …`)
// REWRITE:
buf[slot + 1] = self.creatures.g_size[j];
// (delete `let gj = &self.creatures.genomes[j];` line 105 — it was only
// used at 109–112; lines 110–112 read pigment_r/g/b which are COLD, so
// keep the AoS binding for those three reads. Either keep `gj` for the
// pigment trio and read `g_size[j]` directly, or split.)
//
// Cleanest:
let pigment = &self.creatures.genomes[j]; // for pigment_r/g/b only
buf[slot + 1] = self.creatures.g_size[j];
buf[slot + 2] = pigment.pigment_r;
buf[slot + 3] = pigment.pigment_g;
buf[slot + 4] = pigment.pigment_b;

// Line 199: let rj = self.creatures.genomes[j].size * BODY_RADIUS_PER_SIZE;
let rj = self.creatures.g_size[j] * BODY_RADIUS_PER_SIZE;
```

### `src/save.rs`

**No changes.** Mirrors are not in `CreatureSoASnapshot`, not in
`validate_soa_lengths`, not in `SaveV1::from_world`. Rebuilt via `push`
in `from_save_v1`.

### `src/snapshot_hash.rs`

**No changes.** Cite lines:

- `snapshot_hash.rs:38` — `hash_genome(&mut h, &w.creatures.genomes[i]);`
  reads the AoS source of truth.
- `snapshot_hash.rs:82–117` — `hash_genome` body. All seven hot fields
  are already in the hash via this function. Adding mirrors would
  double-count.

### `src/wasm_api.rs`

**No changes.** Inspector / creatures_buffer / creature_at all read
`self.inner.creatures.genomes[i]` directly. The inspector continues to
see ALL 14 genome fields (mutation_rates included). Mirrors are
internal-only.

### `src/constants.rs`

**No new constants.** All field accesses use scalar arithmetic with
existing constants.

---

## Integration points (implementer checklist)

The implementer works through this list mechanically.

### 1. Pre-flight grep audit

```sh
git grep -nE 'genome\.size|genome\.photosynth_efficiency|genome\.eat_efficiency|genome\.scavenge_efficiency|genome\.move_speed|genome\.vision_range|genome\.eye_count' src/
git grep -nE 'genomes\[[a-z0-9_]+\]\.(size|photosynth_efficiency|eat_efficiency|scavenge_efficiency|move_speed|vision_range|eye_count)' src/
git grep -nE 'g_i\.(size|eat_efficiency|scavenge_efficiency)|g\.(size|photosynth_efficiency|eat_efficiency|scavenge_efficiency|move_speed|vision_range|eye_count)|gj\.size' src/
```

  Confirm:
  - `mutate_in_place` appears only at `world.rs:896` outside tests.
  - All `genomes[i].<hot>` reads are in the seven enumerated sites
    (A–G above) PLUS the cold sites explicitly preserved (inspector,
    `creatures_buffer`, `creature_at`, `save.rs::tests`, HoF clone in
    `apply_movement_and_repulsion` lines 456–466 and `collect_deaths`
    805/815, `from_save_v1` reads at 1018, and the in-test
    `world.rs:1389+` patches).
  - If any NEW non-test write to `genomes[i].<hot>` has appeared
    since this plan: STOP, audit, and add a `push_hot_mirrors` /
    explicit single-field sync call.

### 2. Order of edits

1. Add the seven fields, `with_capacity` / `push` / `remove_indices`
   / `push_hot_mirrors` in `src/creature.rs`. Run `cargo check`.
2. Rewrite site A (`photosynth_two_pass`). Run `cargo test --lib world`
   — `energy_conservation_in_photosynth_pass` is the canary.
3. Rewrite site B (`apply_movement_and_repulsion`). Run
   `cargo test --lib world`.
4. Rewrite site C (`eat_and_scavenge`). Run `cargo test --lib world`.
5. Rewrite site D (`energy_bookkeeping`). Run `cargo test --lib world`.
6. Rewrite site E (`count_carrion_overlap` / `compute_is_at_wall`).
7. Rewrite site F (`build_nn_input` + `pick_action_d` `speed` read).
8. Rewrite site G (threaded NN forward inline carrion-overlap).
9. Rewrite the four reads in `src/vision.rs`. Run `cargo test --lib
   vision` — all 5 existing vision tests must pass unchanged.
10. Run `cargo test --release --test acceptance` — **golden hash must
    match**. This is the determinism gate.
11. (Conditional on the threaded golden existing per master plan §7)
    `cargo test --release --features threads --test acceptance` —
    threaded golden must also match.
12. Add the three new unit tests (T1, T2, T3 below).
13. `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
    `cargo test --lib`.

### 3. Reviewer grep (after the diff is staged)

```sh
rg -n 'genome\.(size|photosynth_efficiency|eat_efficiency|scavenge_efficiency|move_speed|vision_range|eye_count)|genomes\[[a-z0-9_]+\]\.(size|photosynth_efficiency|eat_efficiency|scavenge_efficiency|move_speed|vision_range|eye_count)' src/world.rs src/vision.rs
```

  Expected output: ONLY cold-path lines remain:
  - `world.rs:456` (HoF clone in `apply_movement_and_repulsion`,
    deliberately AoS).
  - `world.rs:805`, `world.rs:815`, `world.rs:847` (HoF clone in
    `collect_deaths`, deliberately AoS).
  - `world.rs:895`, `world.rs:905` (`handle_births`: parent genome
    clone + radius for spawn-position clamp; cold, fires once per
    birth).
  - `world.rs:1018` (`from_save_v1` parent-genome read on restore).
  - Any reads inside `#[cfg(test)]` modules (lines 1389+).

  If the grep returns a line inside `photosynth_two_pass`,
  `energy_bookkeeping`, `apply_movement_and_repulsion` (excluding 456),
  `eat_and_scavenge`, `count_carrion_overlap`, `compute_is_at_wall`,
  `build_nn_input`, or `pick_action_d` — the rewrite is incomplete.
  The diff still passes tests (golden unchanged, since the AoS read
  returns identical values) but the perf win is incomplete.

### 4. Field-naming convention

`g_` prefix. Do NOT use `hot_*`, `cache_*`, or `scratch_*` — those
imply different lifetimes than authoritative-mirror semantics. The
`g_` prefix groups the new fields together when listed in struct order
and matches the convention pinned in this plan (D1).

### 5. Threading & Sync

Mirrors are `Vec<f32>` / `Vec<u8>` — both `Send + Sync` by-ref. The
threaded NN-forward closure at `world.rs:363–417` already captures
`creatures_ref: &CreatureSoA`; the new mirror fields are reachable
through that same shared reference. No additional `Arc` / lock needed.

---

## Tests (four minimum)

### T1. Unit — mirrors mirror genomes after a sequence of pushes + mutations

In `src/creature.rs::tests` (add the module if absent):

```rust
#[test]
fn hot_mirrors_match_genomes_after_births_and_mutations() {
    use crate::rng::SimRng;
    use crate::brain::Brain;
    use crate::genome::{Genome, TraitMutationRates};

    let mut soa = CreatureSoA::with_capacity(8);
    let mut rng = SimRng::from_u64(7);
    // Push 100 creatures, each with a different mutated genome.
    for k in 0..100 {
        let mut g = Genome::founder();
        g.mutation_rates = TraitMutationRates::uniform(1.0);
        for _ in 0..(k % 5) { g.mutate_in_place(&mut rng, 1.0); }
        let brain = Brain::founder(&mut rng);
        soa.push(k as u64, 0.0, 0.0, 1.0, 0, 0, 0, g, brain);
    }
    // Invariant check.
    for i in 0..soa.len() {
        let g = &soa.genomes[i];
        assert_eq!(soa.g_size[i],          g.size,                  "size[{i}]");
        assert_eq!(soa.g_photo_eff[i],     g.photosynth_efficiency, "photo[{i}]");
        assert_eq!(soa.g_eat_eff[i],       g.eat_efficiency,        "eat[{i}]");
        assert_eq!(soa.g_scav_eff[i],      g.scavenge_efficiency,   "scav[{i}]");
        assert_eq!(soa.g_move_speed[i],    g.move_speed,            "move[{i}]");
        assert_eq!(soa.g_vision_range[i],  g.vision_range,          "vision[{i}]");
        assert_eq!(soa.g_eye_count[i],     g.eye_count,             "eye_count[{i}]");
    }
}
```

  `assert_eq!` on f32 is intentional — mirror values come from `push`
  copying the source f32; bit-identical.

### T2. Unit — `remove_indices` keeps mirrors index-aligned

```rust
#[test]
fn remove_indices_keeps_mirrors_in_step() {
    // Build 10 creatures with distinct sizes 1.0..10.0, then kill
    // indices [1, 3, 5, 7] and confirm mirror arrays mirror the
    // post-swap_remove order of `genomes`.
    let mut soa = CreatureSoA::with_capacity(16);
    let mut rng = SimRng::from_u64(11);
    for k in 0..10u8 {
        let mut g = Genome::founder();
        g.size = (k as f32) + 1.0;
        g.eye_count = if k % 2 == 0 { 4 } else { 6 };
        soa.push(k as u64, 0.0, 0.0, 1.0, 0, 0, 0, g, Brain::founder(&mut rng));
    }
    soa.remove_indices(&[1, 3, 5, 7]);
    assert_eq!(soa.len(), 6);
    for i in 0..soa.len() {
        assert_eq!(soa.g_size[i],      soa.genomes[i].size);
        assert_eq!(soa.g_eye_count[i], soa.genomes[i].eye_count);
    }
}
```

### T3. Unit — save round-trip rebuilds mirrors bit-identically

```rust
#[test]
fn save_round_trip_rebuilds_hot_mirrors() {
    let mut w = World::new("perf5-save-round-trip");
    for _ in 0..200 { w.tick_once(); }
    let save = w.to_save_v1();
    let w2 = World::from_save_v1(save).expect("load");
    assert_eq!(w.creatures.len(), w2.creatures.len());
    for i in 0..w.creatures.len() {
        assert_eq!(w2.creatures.g_size[i],         w.creatures.genomes[i].size);
        assert_eq!(w2.creatures.g_photo_eff[i],    w.creatures.genomes[i].photosynth_efficiency);
        assert_eq!(w2.creatures.g_eat_eff[i],      w.creatures.genomes[i].eat_efficiency);
        assert_eq!(w2.creatures.g_scav_eff[i],     w.creatures.genomes[i].scavenge_efficiency);
        assert_eq!(w2.creatures.g_move_speed[i],   w.creatures.genomes[i].move_speed);
        assert_eq!(w2.creatures.g_vision_range[i], w.creatures.genomes[i].vision_range);
        assert_eq!(w2.creatures.g_eye_count[i],    w.creatures.genomes[i].eye_count);
    }
}
```

### T4. Acceptance — golden hash unchanged

`cargo test --release --test acceptance` must pass with
`tests/golden_snapshot_t10000.txt` UNCHANGED. **This is the determinism
gate.** If the hash differs after this commit, the implementer has
introduced a non-bit-identical computation in one of the six read sites
— diagnose, do NOT regen the golden. The most likely culprit is
operator reordering in `energy_bookkeeping` (Site D) where the upkeep
sum chain is sensitive to floating-point order. Copy expression shape
verbatim.

Also run `cargo test --release --features threads --test acceptance`
if the threaded golden has landed; the threaded golden must also match
(Site G ensures the threaded codepath sees identical mirror values).

### Bonus (recommended, not required)

T5. `cargo test --lib` covers existing `vision.rs::tests` (5 tests) and
`world.rs::tests` (the energy-conservation canary). All must pass
unchanged.

---

## Risks & mitigations

| # | Risk | Mitigation |
|---|---|---|
| R1 | Operator-order drift in one of the six rewrites perturbs lower-bit f32 → golden break. | Copy expression structure verbatim. Optional debug-only sanity check at end of `World::step`: `#[cfg(debug_assertions)] for i in 0..self.creatures.len() { debug_assert_eq!(self.creatures.g_size[i], self.creatures.genomes[i].size); /* …7× */ }`. Excluded from release builds, catches drift in any test that touches a hot tick path. |
| R2 | Implementer adds the seven fields but forgets to wire `push_hot_mirrors` into `push`. | `cargo build` fails at the first read-site rewrite (the mirror Vec is empty; indexing panics in any unit test that calls `world.tick_once()`). Detected immediately. |
| R3 | A test that pokes `genomes[i].<hot>` after `push` (without going through `push` again) leaves the mirror stale. | The existing in-tree test sites (world.rs:1389+, 1418, 1419, 1520, 1533, 1633–1637, 1708–1709, 1785, 1827, 1840, 1855–1856, 1874–1875, 1968, 2016) all run their assertions on `creatures.energy[0]` or similar — they don't read mirrors. The implementer's grep audit (step 1) flags any that subsequently calls a hot tick phase. For those, add `w.creatures.g_<field>[idx] = new_value;` line after the genome poke, OR call a new `pub(crate) fn resync_hot_mirrors_at(&mut self, i: usize)` helper. Document the new helper in the same commit. |
| R4 | The threaded NN-forward closure (`world.rs:351–424`) sees a stale mirror when a creature is mutated mid-flight. | Impossible by construction: the closure captures `creatures_ref: &CreatureSoA` (immutable borrow) — no mutation can occur inside `nn_forward_all_chunks`. Mutations only happen at birth (after this phase) and at `from_save_v1` (different call entirely). |
| R5 | A reviewer/future contributor forgets one of the six hot read sites; the rewrite is incomplete. Result: still correct (mirror matches AoS), just slower. | Mandate the reviewer grep in §"Integration points #3". Add a comment block at the top of each rewritten function: `// perf-5: hot reads use g_* mirrors; cold reads (armor, max_age, pigment_*, bite_reach, mutation_rates, eye_offsets) stay on &self.creatures.genomes[i].` Makes the next reader's job a one-second visual scan. |
| R6 | Diff bloat: 250 LOC across three files makes review hard. | The diff structure is mechanical (1 struct edit, 1 `push` edit, 1 `remove_indices` edit, ~12 read-site edits, 3 new tests). Reviewer reads top-to-bottom: creature.rs first (the contract), then world.rs hot sites in tick order (Site A → G), then vision.rs (smallest), then tests. Cite this plan's §"Files & function signatures" subheadings in the PR body. |
| R7 | Save load order: `save.creatures.genomes[i]` is read BEFORE `creatures.push(g.clone(), ...)` runs. If we accidentally call `push_hot_mirrors` from outside `push`, we might race the order. | The plan keeps `push_hot_mirrors` private and called only from `push`. The `from_save_v1` path (world.rs:1026) goes through `creatures.push(..., g.clone(), b.clone())` — mirrors are seeded as part of that push. No special load-path code. T3 catches divergence. |
| R8 | `decode_action` continues reading AoS (Site F kept-as-is). A future maintainer assumes "all hot reads are mirror" and is confused. | Add a one-line comment above the `decode_action` call at world.rs:1303: `// decode_action takes &Genome (reads eat_eff/scav_eff). Hot, but kept on AoS to avoid API churn; one read/creature/tick.` |

---

## Sequencing (implementer steps)

1. **Grep audit.** Run the three greps in §"Integration points #1".
   Capture output as a sanity baseline. Confirm `mutate_in_place` is
   single-site in non-test code.
2. **Field + sync helper.** Add the seven fields to `CreatureSoA`,
   extend `with_capacity` / `push` / `remove_indices`, add
   `push_hot_mirrors`. Run `cargo check`.
3. **Read-site rewrites in tick order.** Sites A → G in `world.rs`,
   then the four reads in `vision.rs`. After each site, run `cargo
   test --lib world` (or `vision` for the vision site). Catches an
   operator-order slip the moment it appears.
4. **Acceptance gate.** `cargo test --release --test acceptance` —
   pinned `tests/golden_snapshot_t10000.txt` must match. If not,
   STOP and diagnose R1.
4a. **Threaded clippy.** Run `cargo clippy --all-targets --features
    threads -- -D warnings` to validate the Site G rewrite inside the
    `#[cfg(feature="threads")]` block at `world.rs:374`. This step runs
    even before perf-4 lands — the `threads` feature flag is already
    wired; no threaded golden is required.
5. **Threaded acceptance.** (Only if the threaded golden from perf-4
   exists yet.) `cargo test --release --features threads --test
   acceptance`.
6. **Add T1, T2, T3.** Run `cargo test --lib`.
7. **Format + clippy.** `cargo fmt`, `cargo clippy --all-targets --
   -D warnings`.
8. **Reviewer grep** (§"Integration points #3"). Expected output is
   only the cold-path lines enumerated there.
9. **Commit.** Message: `perf(layout): genome hot-field SoA split (7
   scalars, mirror)`. Body: cite `docs/research/perf-final-report.md`
   §3 item 7 and this plan. No `DECISIONS.md` entry required — the
   v6 §M hash contract is unchanged; this is purely a layout change.

---

## Citations

- `src/genome.rs:53–70` — `Genome` struct; the seven hot fields and
  their types (six `f32`, `eye_count: u8`).
- `src/genome.rs:97–259` — `mutate_in_place`; sole non-test mutation
  invocation site.
- `src/creature.rs:43–63` — `CreatureSoA` struct; new fields appended.
- `src/creature.rs:97–129` — `push`; `push_hot_mirrors` call added.
- `src/creature.rs:133–155` — `remove_indices`; seven new
  `swap_remove` calls.
- `src/world.rs:119` — founder push (flows through `CreatureSoA::push`,
  no per-site edit needed).
- `src/world.rs:351–424` — threaded NN forward; Site G rewrite of one
  inline `genomes[i].size` read at line 374.
- `src/world.rs:427–559` — `apply_movement_and_repulsion`; Site B (5
  hot reads at 433, 452, 485, 496, 531).
- `src/world.rs:561–604` — `photosynth_two_pass`; Site A (two `g.size *
  g.photosynth_efficiency` products at 569, 582).
- `src/world.rs:606–719` — `eat_and_scavenge`; Site C (two `g_i` clone
  bindings split into mirror reads + cold `bite_reach` reads).
- `src/world.rs:721–781` — `energy_bookkeeping`; Site D (six of seven
  hot reads, with cold `armor` + `max_age` left on the AoS binding).
- `src/world.rs:878–957` — `handle_births`; child genome mutation at
  line 896, push at line 945. Mirror seeding happens inside `push`.
- `src/world.rs:999–1046` — `from_save_v1`; restore push at line 1026.
  Mirror seeding happens inside `push`.
- `src/world.rs:1117–1145` — `count_carrion_overlap`; Site E (`size`
  read at 1120).
- `src/world.rs:1147–1161` — `compute_is_at_wall`; Site E (`size`
  read at 1154).
- `src/world.rs:1207–1243` — `build_nn_input`; Site F (binding at line
  1215; `g.size` read at line 1227; `max_age` stays AoS).
- `src/world.rs:1281–1306` — `pick_action_d`; Site F (`move_speed`
  read at 1295). `decode_action` kept on AoS (R8).
- `src/vision.rs:61–125` — `fill_one`; four hot reads (self
  `eye_count` + `vision_range`; target `size` × 2).
- `src/vision.rs:191–210` — DDA inner cell loop; target `size` read
  at 199.
- `src/save.rs:64–85` — `CreatureSoASnapshot`; mirrors deliberately
  omitted.
- `src/save.rs:207–239` — `validate_soa_lengths`; mirrors deliberately
  omitted.
- `src/snapshot_hash.rs:38` — `hash_genome(&w.creatures.genomes[i])`;
  AoS source of truth, unchanged.
- `src/snapshot_hash.rs:82–117` — `hash_genome` body; all seven hot
  fields already hashed via AoS read.
- `src/wasm_api.rs:103–133` — `creatures_buffer`; cold, AoS read,
  unchanged.
- `src/wasm_api.rs:226–238` — `creature_at`; cold, AoS read, unchanged.
- `src/wasm_api.rs:243–280` — `creature_inspect_json`; cold, AoS read,
  unchanged (inspector sees all 14 genome fields per R1).
- `tests/golden_snapshot_t10000.txt` — pinned hash; must remain
  unchanged.
- `docs/plans/perf+ui-master.md` §5 perf-5, §3 R1, §6 R7.
- `docs/research/perf-final-report.md` §3 item 7, §5a, §6 commit 5.
- `docs/research/perf-layout.md` §1 — savings rationale (299 KB → 41 KB,
  six benefiting passes).

---

## Merge notes

### Merge notes vs perf-1

perf-5 lands **after** perf-1 per the master commit ordering
(perf+ui-master.md §4). perf-1 already adds `eye_trig: Vec<f32>` and a
`recompute_eye_trig_at` helper to `CreatureSoA`, and extends `push`,
`with_capacity`, and `remove_indices`. When the perf-5 implementer opens
`src/creature.rs`, those edits are already present.

**Ordering convention inside `CreatureSoA::push` (binding agreement):**

- `self.push_hot_mirrors(&genome)` (perf-5) MUST run **BEFORE**
  `self.genomes.push(genome)` — it borrows `&genome`, which is moved
  into the Vec on the next line.
- perf-1's `recompute_eye_trig_at(i)` stays at the **END** of `push`,
  after `self.genomes.push(genome)` and `self.brains.push(brain)` — it
  needs the index `i = self.x.len() - 1` which is only valid after the
  push.

The resulting merged shape after both patches are applied:

```rust
pub fn push(..., genome: Genome, brain: Brain) -> usize {
    // ... existing primitive pushes ...
    self.push_hot_mirrors(&genome);        // perf-5: BEFORE genome move
    self.genomes.push(genome);
    self.brains.push(brain);
    let new_len = self.eye_trig.len() + SECTORS * 2;  // perf-1
    self.eye_trig.resize(new_len, 0.0);
    let i = self.x.len() - 1;
    self.recompute_eye_trig_at(i);         // perf-1: AFTER push
    i
}
```

For `with_capacity`: both plans append `Vec::with_capacity(cap)` lines —
concatenate them; no ordering concern.

For `remove_indices`: perf-1 appends `swap_remove_chunk(&mut self.eye_trig, k, SECTORS*2)`;
perf-5 appends seven `swap_remove(k)` calls. All append to the same
`dead.iter().rev()` loop; relative order between the two sets is
immaterial since each operates on its own Vec(s).

Do NOT remove perf-1's `eye_trig` field, `recompute_eye_trig_at`, or
`swap_remove_chunk` when writing the perf-5 diff.

### Merge notes vs perf-2

perf-5 lands **after** perf-2 per the master commit ordering. perf-2
promotes six local scratch `Vec`s in `apply_movement_and_repulsion` and
`eat_and_scavenge` to `self.scratch_*` fields on `World`. When the
perf-5 implementer opens `src/world.rs`, those functions already use
`self.scratch_fx[i]`, `self.scratch_gain[i]`, etc. instead of the
local `vec![]` allocations.

**Two specifics the implementer must keep in mind:**

1. **Line numbers in this plan are pre-perf-2 numbers.** After perf-2
   lands the affected functions gain or lose lines. Re-locate each
   read site by searching for the expression, not by jumping to a
   hard-coded line number. The `g.size`/`g.eat_eff`/etc. reads that
   perf-5 rewrites are still present and still at the conceptual
   locations described in Sites B and C — only the surrounding
   `fx[i]`/`gain[i]` writes now read `self.scratch_*` instead.

2. **Repulsion neighbor loop structure after perf-2.** In
   `apply_movement_and_repulsion`, the repulsion neighbor loop header
   becomes `for &j in &neighbors` (where `neighbors` is a
   locally-`mem::take`'d field — perf-2's recipe). perf-5's read-site
   rewrites inside that loop (`genomes[j].size → g_size[j]`) must keep
   this loop header intact; only the inner `self.creatures.genomes[j].size`
   read changes to `self.creatures.g_size[j]`.

---

## Revision history

- **v1** — Initial plan authored against `main` (pre-perf-1, pre-perf-2).
- **v2** — Incorporated plan-review feedback: fixed Site F line citation
  (binding line 1215, `g.size` read line 1227); added Merge notes vs
  perf-1 and perf-2; inserted `cargo clippy --features threads` step
  between sequencing steps 4 and 5; removed verbose review commentary
  and replaced with this revision note.

*End of perf-5 plan.*

---

## Code review

**Verdict: APPROVED.** Diff faithfully implements the plan; no blocking issues.

### Verified correct

- **Seven mirror fields, exact names and types** (`creature.rs:76–97`):
  `g_size/g_photo_eff/g_eat_eff/g_scav_eff/g_move_speed/g_vision_range:
  Vec<f32>`, `g_eye_count: Vec<u8>`. `pub(crate)` visibility as planned.
  Doc-comments document the write-only invariant on each field.
- **`with_capacity` extends all seven** (`creature.rs:123–129`).
- **`push` ordering correct** (`creature.rs:170`): `push_hot_mirrors(&genome)`
  fires BEFORE `self.genomes.push(genome)`; perf-1's `recompute_eye_trig_at(i)`
  stays at end of `push` (line 181). Merge note honored.
- **`remove_indices` swap_removes all 7 mirrors** in the same `dead.iter().rev()`
  loop (`creature.rs:209–215`); ordering immaterial since each Vec is
  independent.
- **`from_save_v1` rebuilds mirrors automatically** via the existing
  `creatures.push(g.clone(), ...)` call path — confirmed by T3 round-trip
  test passing.
- **All 7 hot sites rewritten** (spot-checked Site A photosynth two-pass at
  `world.rs:596,614`, Site D energy_bookkeeping at `world.rs:763–779`, Site
  G threaded NN forward at `world.rs:396`, Site F build_nn_input at
  `world.rs:1271,1286`, vision.rs `fill_one` at `vision.rs:60–96` and DDA
  inner loop at `vision.rs:190`). Expression structure preserved
  bit-identically — operator order in `energy_bookkeeping` matches the
  pre-diff line verbatim.
- **Cold paths preserved.** Confirmed `wasm_api.rs:113–129,231,267–274`
  (creatures_buffer, creature_at, creature_inspect_json), `species.rs:155,159`
  (species_distance), `snapshot_hash.rs:85–111` (hash_genome reads from
  AoS Genome only), `save.rs` (no mirror fields in CreatureSoASnapshot),
  HoF clone in `energy_bookkeeping` (`world.rs:889`), birth radius
  (`world.rs:953`), `decode_action`/`is_valid_action` AoS read per R8.
- **`resync_hot_mirrors_at` added for test fixtures** (`creature.rs:236`)
  and applied at 5 test sites in `world.rs` that mutate `genomes[i]`
  in-place before a hot tick phase. Test sites that mutate but never run
  a hot tick (e.g. weirdest tests at `world.rs:1921,1940`; local
  `child_genome` at 2034, 2082 that are never pushed) correctly omit the
  resync. This is a sensible deviation from the original plan (R3) — the
  added `resync_hot_mirrors_at` helper is dead-code-allowed and cleanly
  scoped.
- **Reviewer grep recipe.** `rg "genome\.(size|...)|genomes\[…\]\.(size|...)"
  src/world.rs src/vision.rs` returns only justified cold-path lines: HoF
  clone (895), birth radius (953), `is_valid_action` (1308–9, R8),
  test-only patches (1447+).
- **Tests landed: T1, T2, T3** in `creature.rs::tests` (`creature.rs:357–455`).
  All three pass. Acceptance unchanged (3/3 pass — golden hash matches).
- **Threaded clippy clean** (`cargo clippy --all-targets --features threads
  -- -D warnings`). Default clippy clean. `cargo fmt --check` clean.
  `pnpm build` clean (web/, 599 KB wasm).

### Blocking issues

None.

### Non-blocking observations

- Plan §D2 originally listed only `push` and `remove_indices` as mirror
  writers; the implementation adds a third writer
  (`resync_hot_mirrors_at`) for test fixtures. The doc-comments on each
  field were updated to include it. Sensible deviation, no risk.
- The `count_carrion_overlap` and `compute_is_at_wall` rewrites (Site E)
  use the mirror with a `// perf-5: mirror` comment. Consistent.

### Measured performance delta

Acceptance suite (`cargo test --release --test acceptance acceptance_t10000`),
warm-cache, 3 runs each:

- **HEAD (perf-5):** 2.61s, 2.71s → median ~2.66s
- **HEAD~1 (perf-4):** 3.62s, 3.06s → median ~3.06s

Delta: **~0.4s faster, ~13% wall-clock reduction** on the 10k-tick
acceptance run. Initial cold-cache runs (~5s) and the cited perf-3
baseline (~3.11s, noisy) are dominated by I/O variance. This matches the
expected SoA-locality win (the 6 benefiting phases now stream the 7 hot
scalars from a dense 29-byte working set instead of a 204-byte AoS
Genome, fitting in L2 at the working population).

### Divergence from plan

- Added `resync_hot_mirrors_at` (not in original plan; covers R3
  mitigation cleanly).
- Otherwise faithful.

*End of code review.*
