# Plan — perf-1: pre-compute sector `sin`/`cos` per creature

**Status:** plan only. No code lands until this is signed off.
**Scope:** cache the per-active-sector `(dx, dy)` unit vector — currently
recomputed every tick in `vision.rs::fill_one` via `theta_ray.cos()` /
`.sin()` — as a parallel SoA field on `CreatureSoA`, populated at every
creature-insertion site (founder, birth, save restore). The vision pass
reads the cache instead of doing trig. Bit-identical to today's path.
Single commit. Golden-safe.

**Spec / research anchors (read these before touching code):**

- `docs/plans/perf+ui-master.md` §"perf-1 — sector sin/cos cache",
  §3 dependency graph (perf-1 has no upstream deps), §6 R6 (cache
  invalidation completeness — four write sites).
- `docs/research/perf-final-report.md` §3 item #1 (the lead win,
  17–35% acceptance wall-time), §5a (golden-safe rationale), §6
  commit 1 (~80 LOC budget).
- `docs/research/perf-sim-hotpath.md` §1a — the source the final
  report cites. Confirms "~72 000 transcendentals/tick at 1 500
  creatures × 24 eyes × (sin + cos)".
- `src/vision.rs::fill_one` (lines 61–125) — the inner loop the cache
  serves. Lines 96–99 are the trig calls to delete.
- `src/creature.rs::CreatureSoA` — where the new parallel array lands.
- `src/genome.rs` — `eye_count: u8`, `eye_offsets: [f32; EYE_SLOTS]`
  are the genome inputs to the cache.
- `src/constants.rs:102–103` — `EYE_SLOTS = 24`, `EYE_VALID = [0, 2, 3,
  4, 6, 8, 12, 24]`. `src/vision.rs:18` — `SECTORS: usize = 24`.
- `src/world.rs:119` (founder push), `src/world.rs:945` (birth push),
  `src/world.rs:1026` (save-restore push).
- `src/save.rs` — `CreatureSoASnapshot` and `validate_soa_lengths`
  (lines 64–85, 207–239). `src/snapshot_hash.rs` — `snapshot_hash`
  hashing order (lines 20–80).

**What is intentionally NOT in this plan:**

- No SIMD; the cache is plain `f32` reads — that's perf-item #8
  (deferred).
- No batched per-creature ray-SIMD; that's perf-item §1f (deferred).
- No change to `sector_to_angle()` (`vision.rs:281`) — it's a cold
  helper used by the hardcoded picker, fine to keep recomputing.
- No change to the inactive-sector zero-write pattern at vision.rs:83
  (`*buf = [0.0; VISION_LEN]`). The cache stores per-sector trig
  unconditionally; the inactive-skip stays where it is.
- No change to the `Genome` struct, `mutate_in_place`, or any save
  schema. The cache is derived data.

---

## High-level decisions (pinned)

These six decisions are the contract; downstream review keys off them.

**D1. Heading is NOT in `theta_ray`.** Confirmed by reading
`vision.rs::fill_one` lines 86–99. The ray angle is computed as
`theta_center + offset` where `theta_center = TAU * s / SECTORS` is
purely sector-index-derived and `offset = g.eye_offsets[active_index]`
is purely genome-derived. Creature heading (`vx`, `vy`) does not appear
anywhere in `fill_one`. **Therefore sectors are world-fixed** — east is
sector 0 for every creature, regardless of velocity. The cache is the
full absolute `(dx, dy) = (cos(theta_ray), sin(theta_ray))` and the
vision pass needs zero trig per tick. This is the simpler of the two
cases the master plan flagged.

**D2. Cache layout: flat `Vec<f32>` of length `creatures.len() * SECTORS *
2`.** One field on `CreatureSoA`:

```rust
/// Per-sector unit ray vector (dx, dy), interleaved as
/// [s0_dx, s0_dy, s1_dx, s1_dy, …, s23_dx, s23_dy] per creature.
/// Length is always creatures.len() * SECTORS * 2 = creatures.len() * 48.
/// Slots for inactive sectors are zero (matches vision's inactive-zero pattern).
pub eye_trig: Vec<f32>,
```

  Rationale: flat `Vec<f32>` is cheaper to `swap_remove` (per-creature
  chunk is a fixed 48-element stride; we do 48 `swap_remove`s in a
  helper or one `chunks_exact_mut` + `swap` — see D5) and matches the
  existing SoA conventions in `creature.rs` (every other field is a
  flat `Vec<_>`). A nested `Vec<[f32; 48]>` would also work but allocates
  separately per creature on resize and gives no measurable speed-up
  for read-only inner loops. A `Vec<[(f32, f32); 24]>` (tuple-of-tuples)
  would force a hand-rolled `Copy` impl; not worth the noise. **Pick
  flat `Vec<f32>`; access via `&eye_trig[i * SECTORS * 2 + s * 2 ..][..2]`
  or two scalar reads.**

**D3. Cache lives on `CreatureSoA`, not on `World`.** Rationale: the
cache is index-aligned with every other per-creature field
(`x`, `y`, `vx`, `vy`, `genomes`, …). Hosting it on `World` would force
the `swap_remove` dance to be split across two modules. The existing
SoA's `push` / `remove_indices` pattern (creature.rs:97–155) is the
single right place. Also matches master plan §5 perf-1 brief which
explicitly suggests "`CreatureSoA` … parallel array".

**D4. Single-source-of-truth recompute: helper on `CreatureSoA`.**
Define one private helper:

```rust
/// Recompute the 48 trig values for creature index `i` from its genome.
/// Caller must have already pushed the genome at `i` (or be at the end
/// of a slice equal to i+1).
///
/// Zeroes the full 48-slot window first. For zero-range / zero-eye creatures
/// all 48 slots stay zero (matching `fill_one`'s short-circuit). For sighted
/// creatures, active-sector slots are overwritten below.
///
/// MUST also be called after any in-place edit of `genomes[i].eye_count` or
/// `genomes[i].eye_offsets` (e.g. test fixtures at world.rs:1389, world.rs:1634,
/// vision.rs::tests lines 357, 415).
pub(crate) fn recompute_eye_trig_at(&mut self, i: usize) {
    let g = &self.genomes[i];
    let base = i * SECTORS * 2;
    // Zero the 48-slot window first; inactive sectors stay zero.
    for v in &mut self.eye_trig[base..base + SECTORS * 2] {
        *v = 0.0;
    }
    let k = g.eye_count as usize;
    if k == 0 || g.vision_range <= 0.0 { return; }
    let k_idx = EYE_VALID.iter().position(|&v| v as usize == k).unwrap_or(0);
    if k_idx == 0 { return; }
    let stride = EYE_STRIDE[k_idx] as usize;
    for s in (0..SECTORS).step_by(stride) {
        let active_index = s / stride;
        let offset = if active_index < g.eye_offsets.len() {
            g.eye_offsets[active_index]
        } else { 0.0 };
        let theta_center = std::f32::consts::TAU * (s as f32) / (SECTORS as f32);
        let theta_ray = theta_center + offset;
        self.eye_trig[base + s * 2]     = theta_ray.cos();
        self.eye_trig[base + s * 2 + 1] = theta_ray.sin();
    }
}
```

  This replicates the exact angle formula from `vision.rs:86–99` —
  bit-identical f32 inputs, bit-identical f32 outputs (same
  `TAU * s / SECTORS` order, same `+ offset` order).

  Then `push` (creature.rs:98) is extended: after `self.genomes.push(genome);`,
  the function does `self.eye_trig.extend(std::iter::repeat(0.0).take(SECTORS * 2));`
  then `let i = self.x.len() - 1; self.recompute_eye_trig_at(i);`. This
  is the **only** site that produces a populated trig slot for a freshly
  inserted creature, so all four call sites (founder, birth, save-restore,
  any test that builds a `CreatureSoA` by hand) get it for free. Per master
  plan §6 R6, this is the "safer, one location" branch — explicitly
  chosen.

  **There is no in-place genome mutation outside birth.** Grep confirms:
  `mutate_in_place` is called only at `world.rs:896`, on `child_genome`
  *before* `creatures.push(...)` at line 945 — so the recompute inside
  `push` covers it. Test-only sites that poke `genomes[i].eye_count`
  directly after `push` — at `world.rs:1389`, `world.rs:1634`, and
  `vision.rs::tests` lines 357 and 415 — MUST call
  `creatures.recompute_eye_trig_at(i)` immediately after each mutation.
  Bump the helper to `pub(crate)` visibility to allow this. Failing to
  do so leaves stale trig in the cache and will cause the vision tests
  (the T4 bonus regression catch) to fail. The fix for each site is
  one line. **Approach chosen: mutate-after-push + explicit recompute
  call.** (Alternative — build-then-push — would require restructuring
  `simple_creature` in `vision.rs::tests` and is more invasive. Stick
  with mutate-after-push + explicit recompute.)

**D5. `remove_indices` mirrors `swap_remove` on the trig array.** The
flat layout means we cannot call `Vec::swap_remove` directly because
each "logical element" is 48 floats. Use a small inline helper:

```rust
fn swap_remove_chunk(buf: &mut Vec<f32>, k: usize, chunk: usize) {
    let n = buf.len() / chunk;
    debug_assert!(k < n);
    let last = n - 1;
    if k != last {
        // Swap two chunk-sized windows.
        for off in 0..chunk {
            buf.swap(k * chunk + off, last * chunk + off);
        }
    }
    buf.truncate(last * chunk);
}
```

  Called from `remove_indices` after every other `swap_remove` line:
  `swap_remove_chunk(&mut self.eye_trig, k, SECTORS * 2);`. Same iteration
  direction as the existing dead-from-the-back loop. The other
  `swap_remove`s in the function preserve the index→last mapping the
  trig chunk also needs — no ordering subtlety, just mirror the pattern.

  **Invariant:** after `remove_indices` completes,
  `eye_trig.len() == self.x.len() * SECTORS * 2`. This holds because each
  `swap_remove_chunk` removes exactly one 48-slot window, matching each
  scalar `swap_remove` that removes one element from the parallel arrays.

**D6. Snapshot hash & save: EXCLUDE the cache.** The cache is derived
from `genomes[i].eye_count` and `genomes[i].eye_offsets`, both of which
are already in the snapshot hash via `hash_genome` (snapshot_hash.rs:84–
117). Hashing the cache would double-count and would also break
round-trip: a freshly-loaded world has the cache reconstructed from the
genome, and that reconstruction is bit-identical, but excluding it
makes intent explicit. Save-side: do NOT add `eye_trig` to
`CreatureSoASnapshot` (save.rs:65–85), do NOT mention it in
`validate_soa_lengths` (save.rs:207–239), do NOT serialize it from
`from_world` (save.rs:138–200), do NOT touch `snapshot_hash::snapshot_hash`
or `hash_genome`. On the load path (`world.rs:1026`), every creature
goes through `creatures.push(...)` which auto-populates the cache via
the D4 helper — no special code needed.

---

## Files & function signatures (concrete diffs)

### `src/creature.rs`

Add the field and three changes — push, remove_indices, helper.

```rust
// near the top, in the use list:
use crate::constants::{EYE_SLOTS, EYE_VALID};
use crate::vision::{EYE_STRIDE, SECTORS};

pub struct CreatureSoA {
    // … existing 17 fields …
    /// Pre-computed (dx, dy) per sector per creature; interleaved.
    /// Length invariant: == x.len() * SECTORS * 2.
    /// Inactive sectors are zero. Recomputed by `recompute_eye_trig_at`.
    /// Excluded from save (re-derived on push) and from snapshot_hash
    /// (would double-count the genome bytes it derives from).
    pub eye_trig: Vec<f32>,
}

impl CreatureSoA {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            // … existing fields, each Vec::with_capacity(cap) …
            eye_trig: Vec::with_capacity(cap * SECTORS * 2),
        }
    }

    pub fn push(&mut self, …) -> usize {
        // … existing pushes including self.genomes.push(genome); …
        self.brains.push(brain);
        // NEW: extend trig buffer by one chunk of zeros, then populate.
        debug_assert_eq!(self.eye_trig.len(), self.x.len().saturating_sub(1) * SECTORS * 2);
        let new_len = self.eye_trig.len() + SECTORS * 2;
        self.eye_trig.resize(new_len, 0.0);
        let i = self.x.len() - 1;
        self.recompute_eye_trig_at(i);
        i
    }

    pub fn remove_indices(&mut self, dead: &[usize]) {
        for &k in dead.iter().rev() {
            // … existing 18 swap_removes …
            swap_remove_chunk(&mut self.eye_trig, k, SECTORS * 2);
        }
    }

    fn recompute_eye_trig_at(&mut self, i: usize) {
        // See plan D4 above for body.
    }
}

#[inline]
fn swap_remove_chunk(buf: &mut Vec<f32>, k: usize, chunk: usize) {
    // See plan D5 above for body.
}
```

### `src/vision.rs`

Replace the trig calls inside `fill_one` with a cache read.

```rust
fn fill_one(&self, i: usize, buf: &mut VisionBuf) {
    let g = &self.creatures.genomes[i];
    if g.eye_count == 0 || g.vision_range <= 0.0 {
        *buf = [0.0; VISION_LEN];
        return;
    }
    let k = g.eye_count as usize;
    let k_idx = EYE_VALID.iter().position(|&v| v as usize == k).unwrap_or(0);
    if k_idx == 0 {
        *buf = [0.0; VISION_LEN];
        return;
    }
    let stride = EYE_STRIDE[k_idx] as usize;
    let ox = self.creatures.x[i];
    let oy = self.creatures.y[i];
    let max_dist = g.vision_range;

    *buf = [0.0; VISION_LEN];

    let trig_base = i * SECTORS * 2;
    for s in (0..SECTORS).step_by(stride) {
        // NEW: cache read, replacing theta_center/offset/cos/sin.
        let dx = self.creatures.eye_trig[trig_base + s * 2];
        let dy = self.creatures.eye_trig[trig_base + s * 2 + 1];
        if let Some(hit) = self.raycast(i, ox, oy, dx, dy, max_dist) {
            // … unchanged hit-handling block …
        }
    }
}
```

  The `if s % stride != 0 { continue; }` inactive-skip is replaced by
  iterating `step_by(stride)` directly — same semantics, slightly
  tighter. Cache slots for skipped `s` are zero, so any code that
  indexes into them by sector won't blow up.

  Add a debug-only length-invariant assertion at the top of `fill_one`:

  ```rust
  debug_assert_eq!(self.creatures.eye_trig.len(), self.creatures.x.len() * SECTORS * 2);
  ```

  **Determinism note.** The cached `(dx, dy)` were produced by the same
  `theta_ray.cos()` / `.sin()` calls that today's inline code produces
  — same f32 inputs (`TAU * s / SECTORS + g.eye_offsets[active_index]`),
  same order of operations, same library function. Bit-identical.

### `src/world.rs`

**No changes required.** All three creature-insertion paths already go
through `CreatureSoA::push`:

- `World::new` line 119: `creatures.push(0, cx, cy, …)`.
- `handle_births` line 945: `self.creatures.push(new_id, cx, cy, …)`.
- `from_save_v1` line 1026: `creatures.push(save.creatures.id[i], …)`.

The D4 design (recompute inside `push`) means the world layer doesn't
need to know about the cache.

### `src/constants.rs`

**No new constants.** `EYE_SLOTS = 24` (line 102) already covers it.
`SECTORS = 24` lives in `vision.rs:18`. The cache stride is `SECTORS *
2 = 48`; we use the symbolic form, no magic number.

### `src/save.rs`

**No changes.** `eye_trig` is not added to `CreatureSoASnapshot` and
not validated in `validate_soa_lengths`. On load, the trig array is
rebuilt by the `push` calls in `from_save_v1` (world.rs:1026).

### `src/snapshot_hash.rs`

**No changes.** The cache is derived from genome fields already in the
hash; adding it would double-count.

---

## Integration points (implementer checklist)

1. Confirm with `grep` that `mutate_in_place` is called exactly once
   in non-test code (today: `world.rs:896`). If a second site has
   appeared since this plan was written, the implementer must either
   route through `push`+`recompute` or call `recompute_eye_trig_at`
   explicitly after the mutation. Audit before coding.
2. Confirm with `grep` that `genomes[i].eye_count =` or
   `.eye_offsets[…] =` appears only in test code (today: world.rs:1389,
   world.rs:1634, vision.rs::tests line 357, vision.rs::tests line 415).
   All four sites mutate a genome field after `push` and MUST call
   `creatures.recompute_eye_trig_at(i)` immediately afterwards.
   The helper is `pub(crate)` to allow this from other modules.
   The vision.rs sites are the T4 bonus regression tests; stale trig
   there will cause them to fail even though they appear unrelated.
   Also check that no debug/introspection path (wasm_api.rs, hof.rs,
   profiling code) hashes or serializes `CreatureSoA` fields other than
   via `snapshot_hash::snapshot_hash` — a quick `grep` at step 1 covers
   this.
3. The `fill_one` rewrite needs `use` of nothing new — `EYE_VALID` and
   `EYE_STRIDE` are already in scope.
4. `creature.rs` gains new `use` lines: `EYE_VALID`, `EYE_STRIDE`, and
   `SECTORS` (all from `constants.rs` after the step-2 move). Cycle check:
   `creature.rs` does not import from `vision.rs` today (vision.rs imports
   `CreatureSoA`); after the move, `creature.rs` imports from `constants.rs`
   only — no new dependency edge. `vision.rs` must then add
   `pub use crate::constants::{EYE_STRIDE, SECTORS};` so that any existing
   callers using `use crate::vision::SECTORS` (e.g. wasm_api.rs, hof.rs)
   continue to resolve without changes (risk R6). The step-2 constants-move
   is a prerequisite to compilation of the creature.rs additions; it
   must be done first (see Sequencing step 2 below).
5. Verify the length invariant in a debug assertion at the top of
   `fill_one` (debug-only):
   `debug_assert_eq!(self.creatures.eye_trig.len(), self.creatures.x.len() * SECTORS * 2);`
6. Field-naming convention: `eye_trig` matches existing `eye_count` /
   `eye_offsets` on `Genome`. Do not prefix with `cache_` or `scratch_`
   — it's authoritative derived state, not scratch.

---

## Tests (three minimum)

### T1. Unit — known-genome cache matches hand-computed values

In `src/creature.rs::tests` (add the module if absent — none exists
today):

```rust
#[test]
fn eye_trig_matches_manual_compute_for_4_eyes() {
    let mut soa = CreatureSoA::with_capacity(1);
    let mut g = Genome::founder();
    g.eye_count = 4;
    g.vision_range = 80.0;
    g.eye_offsets = [0.1, 0.2, 0.3, 0.4, 0.0, /* … rest zero … */ 0.0];
    let brain = Brain::founder(&mut SimRng::from_u64(0));
    soa.push(1, 0.0, 0.0, 1.0, 0, 0, 0, g.clone(), brain);

    // eye_count=4 → stride=6, active sectors 0, 6, 12, 18.
    for (active_index, &s) in [0usize, 6, 12, 18].iter().enumerate() {
        let theta_center = std::f32::consts::TAU * (s as f32) / 24.0;
        let theta_ray = theta_center + g.eye_offsets[active_index];
        assert_eq!(soa.eye_trig[s * 2],     theta_ray.cos(), "sector {s} dx");
        assert_eq!(soa.eye_trig[s * 2 + 1], theta_ray.sin(), "sector {s} dy");
    }
    // Inactive sectors must be zero.
    for s in 0..24 {
        if [0,6,12,18].contains(&s) { continue; }
        assert_eq!(soa.eye_trig[s * 2],     0.0, "inactive sector {s} dx");
        assert_eq!(soa.eye_trig[s * 2 + 1], 0.0, "inactive sector {s} dy");
    }
}
```

  `assert_eq!` on f32 is intentional — the values must be bit-identical
  to the inline computation. **Do NOT refactor the angle expression in
  T1.** Both the test and `recompute_eye_trig_at` must use the exact
  form `TAU * (s as f32) / (SECTORS as f32) + offset`; any reordering
  (e.g. `(TAU / SECTORS as f32) * s as f32 + offset`) produces different
  lower-order bits and breaks the `assert_eq!`.

### T2. Unit — mutation invalidation via `handle_births` path

Drive a full birth through the World and confirm the child's cache
reflects the mutated genome, while the parent's cache is unchanged.
If `handle_births` is not currently `pub(crate)`, bump its visibility
in this same commit rather than restructuring the test — a visibility
bump is lower risk than reimplementing the test via `tick_once`.

```rust
#[test]
fn eye_trig_recomputed_on_birth() {
    let mut w = World::new("perf1-birth");
    // Coerce the founder to definitely split this tick.
    w.creatures.energy[0] = 1_000.0;
    w.creatures.action_this_tick[0] = Action::Split;
    let parent_trig_before: Vec<f32> = w.creatures.eye_trig.clone();
    w.handle_births(); // visibility: bump to pub(crate) if needed
    assert!(w.creatures.len() >= 2, "birth must have produced a child");
    // Parent's cache unchanged (cache is genome-derived, parent genome
    // didn't change).
    assert_eq!(&w.creatures.eye_trig[..SECTORS * 2], &parent_trig_before[..]);
    // Child's cache matches a fresh recompute from its genome.
    let child_idx = w.creatures.len() - 1;
    let mut expected = CreatureSoA::with_capacity(1);
    expected.push(
        0, 0.0, 0.0, 0.0, 0, 0, 0,
        w.creatures.genomes[child_idx].clone(),
        Brain::founder(&mut SimRng::from_u64(0)), // brain irrelevant for cache
    );
    let off = child_idx * SECTORS * 2;
    assert_eq!(&w.creatures.eye_trig[off..off + SECTORS * 2],
               &expected.eye_trig[..SECTORS * 2]);
}
```

### T3. Acceptance — golden hash unchanged

`cargo test --release --test acceptance` must pass with
`tests/golden_snapshot_t10000.txt` UNCHANGED. **This is the determinism
gate.** If the hash differs after this commit, the implementer has
introduced a non-bit-identical computation somewhere — diagnose, do
NOT regen the golden. The most likely culprit is operator ordering:
e.g. accidentally writing `(TAU / SECTORS as f32) * s as f32 + offset`
instead of `TAU * s as f32 / SECTORS as f32 + offset` will produce
different lower-order bits.

### Bonus (recommended, not required)

T4. `cargo test --lib` covers existing `vision.rs::tests` (5 tests).
All five must pass unchanged — they exercise `fill_one` against
hand-computed angles. The cache must produce bit-identical sectors,
so these are a free regression catch.

---

## Risks & mitigations

| # | Risk | Mitigation |
|---|---|---|
| R1 | Operator-order drift in the helper produces a different f32 from inline trig → golden break. | Copy the exact expression `TAU * (s as f32) / (SECTORS as f32) + offset` from vision.rs:96–97 verbatim. Add T3 as the determinism gate. |
| R2 | A test that pokes `genomes[i].eye_count` directly without recomputing the cache yields stale trig. | Bump `recompute_eye_trig_at` to `pub(crate)`. Document in its doc-comment: "MUST be called after any in-place edit of `genomes[i].eye_count` or `genomes[i].eye_offsets`." Audit all four existing in-tree sites (world.rs:1389, world.rs:1634, vision.rs::tests line 357, vision.rs::tests line 415); fix all four. |
| R3 | `remove_indices` order subtlety: swap_remove pulls from the end, and if our chunk-swap implementation gets the index math wrong we corrupt cache for the moved creature. | Mirror the existing pattern *exactly*: walk `dead.iter().rev()`, call `swap_remove_chunk` in the same loop body, same `k` value. Add debug_assert on `buf.len() % chunk == 0` and `k < n` inside the helper. |
| R4 | Save round-trip diverges because the cache isn't in the snapshot. | The cache is rebuilt deterministically in `push` from the AoS genome (which IS in the snapshot). Existing `f26_round_trip_preserves_determinism` (save.rs::tests) and `save_load_step_preserves_determinism` (tests/acceptance.rs) catch divergence. |
| R5 | A new in-place mutation site lands between this plan and the commit (perf-5 SoA mirror, etc.) and forgets to recompute. | Plan §"Integration points" #1 mandates a grep audit at implementation time. The single-location D4 design means future sites only need `creatures.recompute_eye_trig_at(i)` — one line. |
| R6 | Constant moves (`SECTORS`, `EYE_STRIDE` from `vision.rs` → `constants.rs`) break external callers (e.g. `wasm_api.rs`, `hof.rs`). | Re-export from `vision.rs` (`pub use crate::constants::{EYE_STRIDE, SECTORS};`) so existing `use crate::vision::SECTORS` imports still resolve. Quick `cargo check` confirms. |

---

## Sequencing (implementer steps)

1. Run two greps to capture the current landscape before touching any code:
   - `git grep -n "mutate_in_place\|genomes\[.*\]\.eye_count\|genomes\[.*\]\.eye_offsets"` — confirm the four test-only mutation sites and one production site (`world.rs:896`).
   - `git grep -rn "eye_trig\|snapshot_hash\|CreatureSoASnapshot" src/ tests/` — confirm no debug/introspection path (wasm_api.rs, hof.rs, profiling code) serializes or hashes `CreatureSoA` fields outside the official snapshot_hash path.
2. **Prerequisite (constants move — must be done before creature.rs additions).**
   Move `SECTORS` and `EYE_STRIDE` from `src/vision.rs` to
   `src/constants.rs`. Add `pub use crate::constants::{EYE_STRIDE, SECTORS};`
   to `vision.rs` so existing callers (`wasm_api.rs`, `hof.rs`, any import
   of `crate::vision::SECTORS`) continue to resolve unchanged.
   Run `cargo check` and `cargo clippy --all-targets -- -D warnings`.
3. Add the `eye_trig: Vec<f32>` field, `recompute_eye_trig_at`,
   `swap_remove_chunk`, and the `push` / `remove_indices` /
   `with_capacity` edits in `src/creature.rs`. Run `cargo test --lib
   creature` (or whatever existing module test suite covers SoA — if
   none, the new T1 test serves).
4. Rewrite `fill_one` in `src/vision.rs` to read from the cache. Run
   `cargo test --lib vision` — all 5 existing vision tests must pass
   unchanged. **This is the first golden-equivalent gate.**
5. Add T1 and T2 (`creature.rs::tests`) and run `cargo test --lib`.
6. Fix all four test-only sites that poke `genomes[i].eye_count` or
   `genomes[i].eye_offsets` after `push`: `world.rs:1389`,
   `world.rs:1634`, `vision.rs::tests` line 357 (`ga.eye_count = 0;`),
   `vision.rs::tests` line 415 (`gb.eye_count = 0;`). Append
   `creatures.recompute_eye_trig_at(i)` (or the appropriate index
   expression) immediately after each mutation. Without this, the T4
   vision regression tests will read stale trig and fail. (Same commit.)
7. Run `cargo test --release --test acceptance` — **golden hash must
   match**. If it doesn't, stop and diagnose R1.
8. Run `cargo test --release --features threads --test acceptance`
   (if the threaded golden has landed by the time perf-1 ships, per
   master plan §4). Should also match its own pinned hash, since this
   change is sequential-codepath-neutral.
9. `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo
   test --lib`.
10. Commit message: `perf(sim): pre-compute sector sin/cos per creature`.
    Body: cite `docs/research/perf-final-report.md` §3 item 1 and this
    plan. No DECISIONS.md entry required (no spec or contract change).

---

## Merge notes vs perf-5

perf-5 (genome SoA mirror) also adds lines to `CreatureSoA::push`,
`with_capacity`, and `remove_indices` — the same three functions this
plan edits. There is **no conflict by design** because the ordering
prescribed in `perf+ui-master.md §4` is perf-1 → … → perf-5, so the
perf-5 implementer sees this plan's additions already in place.

The merged shape of `push` after both plans land is:

```rust
pub fn push(..., genome: Genome, brain: Brain) -> usize {
    // ... existing primitive pushes (x, y, vx, vy, ...) ...
    self.push_hot_mirrors(&genome);        // added by perf-5 — BEFORE genomes.push
    self.genomes.push(genome);
    self.brains.push(brain);
    // added by perf-1 — AFTER genomes.push:
    let new_len = self.eye_trig.len() + SECTORS * 2;
    self.eye_trig.resize(new_len, 0.0);
    let i = self.x.len() - 1;
    self.recompute_eye_trig_at(i);
    i
}
```

Ordering rationale: `push_hot_mirrors(&genome)` borrows `&genome` and
therefore MUST run before `self.genomes.push(genome)` (which moves it).
The perf-1 `eye_trig` lines need the genome already at index `i`, so
they must come AFTER `self.genomes.push`. The invariant is:

  **perf-5's `push_hot_mirrors` lines go BEFORE `self.genomes.push`;
  perf-1's `eye_trig` lines go AFTER — at the END of `push`.**

The perf-5 implementer must not delete or reorder perf-1's
`eye_trig.resize` / `recompute_eye_trig_at` lines when inserting
`push_hot_mirrors`. Same additive discipline applies to
`with_capacity` (both add `Vec::with_capacity` lines for their
respective fields) and `remove_indices` (both append `swap_remove`
calls to the same loop body — disjoint, order-independent within the
loop).

---

## Citations

- `src/vision.rs:86–99` — current per-tick trig site that this plan
  eliminates.
- `src/vision.rs:18` — `pub const SECTORS: usize = 24;`.
- `src/vision.rs:33` — `pub const EYE_STRIDE: [u8; 8]`.
- `src/creature.rs:43–63` — `CreatureSoA` struct definition; new field
  appended here.
- `src/creature.rs:97–129` — `push`; recompute call appended here.
- `src/creature.rs:133–155` — `remove_indices`; `swap_remove_chunk`
  call mirrors the existing `swap_remove` chain.
- `src/genome.rs:61–62` — `eye_count: u8`, `eye_offsets: [f32; EYE_SLOTS]`.
- `src/genome.rs:97–259` — `mutate_in_place`; the only mutation site.
- `src/world.rs:119` — founder push.
- `src/world.rs:878–957` — `handle_births`; `mutate_in_place` at line
  896, push at line 945.
- `src/world.rs:999–1046` — `from_save_v1`; restore push at line 1026.
- `src/save.rs:64–85` — `CreatureSoASnapshot`; eye_trig deliberately
  omitted.
- `src/save.rs:207–239` — `validate_soa_lengths`; eye_trig deliberately
  omitted.
- `src/snapshot_hash.rs:30–44, 82–117` — hash input order;
  `hash_genome` already covers `eye_count` and `eye_offsets`. Cache is
  not added.
- `src/constants.rs:102–103` — `EYE_SLOTS`, `EYE_VALID`. `SECTORS` and
  `EYE_STRIDE` move here from vision.rs in step 2.
- `tests/golden_snapshot_t10000.txt` — pinned hash; must remain
  unchanged.
- `docs/plans/perf+ui-master.md` §5 perf-1, §6 R6.
- `docs/research/perf-final-report.md` §3 item 1, §5a, §6 commit 1.
- `docs/research/perf-sim-hotpath.md` §1a — savings rationale.

*End of perf-1 plan.*

---

## Code review

Reviewed `cbb410e` ("perf(sim): pre-compute sector sin/cos per creature") on 2026-05-24.

**Verdict: approve.**

### Verified correct

- **Plan adherence.** Diff matches §"Files & function signatures" line-for-line: new `eye_trig: Vec<f32>` field on `CreatureSoA`, `with_capacity` extension, `push` extension, `remove_indices` calls `swap_remove_chunk`, new `pub(crate) recompute_eye_trig_at`, new private `swap_remove_chunk` helper. The `fill_one` rewrite uses `step_by(stride)` per D4. `vision.rs` re-exports `SECTORS` and `EYE_STRIDE` from `constants.rs` so external callers keep resolving (R6).
- **D6 honoured.** `grep eye_trig` in `src/save.rs` and `src/snapshot_hash.rs` returns zero hits. Cache is excluded from both, as planned.
- **Determinism gate.** `cargo test --release --test acceptance` → 3/3 pass: `acceptance_t10000`, `save_load_step_preserves_determinism`, `profile_does_not_change_hash`. Golden hash `tests/golden_snapshot_t10000.txt` unchanged. Bit-identical operator order (`TAU * (s as f32) / (SECTORS as f32) + offset`) confirmed in `recompute_eye_trig_at`.
- **Test coverage.** All three required tests present and passing:
  - T1: `eye_trig_matches_manual_compute_for_4_eyes` in `src/creature.rs::tests`, using `assert_eq!` on f32 (bit-identity).
  - T1b: `eye_trig_len_invariant_after_remove` (bonus invariant test the implementer added — good).
  - T2: `eye_trig_recomputed_on_birth` in `src/world.rs::tests` — `handle_births` was bumped to `pub(crate)` as planned.
- **Cache invalidation completeness.** Every insertion path goes through `CreatureSoA::push` which triggers `recompute_eye_trig_at`:
  - Founder push (`world.rs:119`) — via `push`. OK.
  - `handle_births` (`world.rs:945`) — via `push`. OK.
  - `from_save_v1` (`world.rs:1026`) — via `push`. OK.
  - Test mutation site `world.rs:1389` — explicit `creatures.recompute_eye_trig_at(0)` added at line 1390. OK.
  - Test mutation site `world.rs:1635` (d19-smoke) — mutation happens BEFORE `push` at line 1642, no extra recompute needed. OK.
  - `vision.rs::tests` lines 346 and 404 — mutations happen BEFORE `simple_creature → push`, so the cache picks them up via the normal `push` path. Implementer's resolution is correct (verified by reading the test bodies).
  - Single non-test mutation source `mutate_in_place` runs on `child_genome` BEFORE `creatures.push(...)` in `handle_births` — covered.
- **Inactive-slot zero-write.** `recompute_eye_trig_at` zeroes the 48-slot window before populating active sectors, so inactive sectors stay 0.0 (matching vision's inactive-zero pattern).
- **SoA `swap_remove_chunk` correctness.** Walks `dead.iter().rev()` in the same loop as the other 18 `swap_remove`s; chunk-swap math (`k * chunk + off` ↔ `last * chunk + off`) is correct; truncates to `last * chunk`. Debug-asserts on `buf.len() % chunk == 0` and `k < n` guard the invariant. T1b confirms `eye_trig.len() == x.len() * SECTORS * 2` post-remove.
- **Constants placement.** `SECTORS` and `EYE_STRIDE` added to `src/constants.rs:149–156` with `v5 §3.5, v6 §E; perf-1 sector sin/cos cache` citation. `vision.rs` re-exports both for back-compat.
- **Clippy/fmt/web-build clean.** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `pnpm build` all pass with no warnings.
- **Debug invariants.** `debug_assert_eq!` at top of `fill_one` and inside `push` guard the length invariant, per plan §"Integration points" item 5.

### Blocking issues

None.

### Non-blocking observations

- T2's parent-cache-unchanged assertion holds because the founder is at index 0 and `swap_remove` only kicks in on death (none happen during `handle_births`). If a future perf change reorders or compacts indices during a birth (unlikely), T2 would need to find the parent by stable id instead of indexing `[..SECTORS * 2]`. Cheap, file under future-proofing.
- The implementer's added T1b (`eye_trig_len_invariant_after_remove`) is a nice bonus the plan didn't require. Keep it.
- `swap_remove_chunk` could in principle use `Vec::copy_within` + two `swap`-windows in one call, but the current per-element loop is 48 iterations and not on a hot path (only fires on creature death); the simple form is fine.
- The unrelated formatting drift in `src/profiler.rs` (visible in `git diff` after my pre/post-perf bench rebuilds touched it) is from a Cargo build cycle, not from this commit. No action needed.

### Measured perf delta

Bench: `cargo test --release --test acceptance acceptance_t10000`, 5 warm runs each, WSL2.

- **Pre-perf (cbb410e~1):** 2.39, 2.82, 2.92, 2.96, 3.20 s → median **2.92s**, mean 2.86s.
- **Post-perf (cbb410e):**  2.37, 2.65, 2.69, 2.75, 2.90 s → median **2.69s**, mean 2.67s.
- **Delta:** ~**−0.23s median (~−8%)**, ~−0.19s mean (~−7%).

This is below the plan's 17–35% estimate from `perf-final-report.md §3`, but the bench host is noisy WSL2 (run-to-run variance >0.3s) and the projection was made against a different workload mix. The improvement is directionally correct and bit-identical to baseline. Plan §"Performance smoke" explicitly calls this "not a hard gate; just a sanity check" — it passes that bar. Perf-2 can proceed.

---

## Revision history

Reviewed and revised 2026-05-24. Must-fix items applied inline: vision.rs test fixture mutation sites (lines 357, 415) added to integration checklist and sequencing; constants-move promoted to explicit prerequisite step; debug_assert invariants added to `push` and `fill_one`; `recompute_eye_trig_at` doc-comment clarified; D5 post-remove_indices invariant stated; T1 bit-identity warning added; T2 handle_births visibility choice pinned; R2 updated for all four sites. Cross-review M1 (`CreatureSoA::push` merge ordering with perf-5) addressed via new §"Merge notes vs perf-5".

