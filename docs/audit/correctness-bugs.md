# Correctness audit — src/*.rs

Scope: ~5-minute hunt for off-by-one, RNG/determinism, NaN, overflow, race-conditions,
golden-snapshot divergence, save/load round-trip, mutation edge cases.

Severity scale: critical (silent-data-corruption / divergence) | high | med | low.

---

## C1 — `place_hotspots` fallback never resets `attempts`  (sun.rs:114–139)
**Severity:** med (latent; unreachable in current config but RNG-trap if config changes)

After the `if attempts > 2000` branch fires once, `attempts` stays > 2000. From then on
every subsequent iteration of the `while placed < HOTSPOT_COUNT` loop *also* takes the
fallback branch in addition to whatever the upper block produced this iteration.
Worse: in the iteration where `ok == true` AND `attempts > 2000`, **two** hotspots
are pushed (one from `if ok`, one from the fallback). Beyond writing a 4th slot of a
3-slot array (out-of-bounds panic on `out[placed]`), the RNG draws diverge from any
implicitly-assumed contract.

```rust
if attempts > 2000 {
    out[placed] = (rng.uniform(lo, hi), rng.uniform(lo, hi));
    placed += 1;
}
```

Fix: reset `attempts = 0` after fallback, OR use `else if attempts > 2000`, OR break
out of the loop entirely after fallback fills the slot. Best: `else if`.

---

## C2 — `geom_skip` off-by-one in `Brain::child_from` mutation walk (brain.rs:171–178)
**Severity:** low (semantics consistent — confirm against spec)

```rust
let mut i = rng.geom_skip(p);
while i < n {
    child.weights[i] += rng.normal() * sigma;
    let step = rng.geom_skip(p);
    i = i.saturating_add(step).saturating_add(1);
}
```

`geom_skip(p)` returns "number of trials before the next event" for a Bernoulli(p).
Treating its return value as a direct *index* (first iteration) and then as a *gap*
(subsequent iterations) means the first weight has slightly different mutation
probability than the rest. Concretely: first mutated index is `~Geom(p)-1` while
subsequent gaps are `~Geom(p)`. For `p=0.02` (default) the bias is tiny but real
and shows up as an RNG-arithmetic mismatch vs. the spec's claimed "every weight
has independent mutation prob p". Either:
 - use `Bernoulli(p)` per weight (correct but slower), or
 - make first index `step` not `geom_skip` (consistent semantics).

This affects the §16 golden — any change here re-bootstraps `golden_snapshot_t10000.txt`.

---

## C3 — `unit()` precision: 24-bit, but used in 32-bit `f32 → usize` paths (rng.rs:29–33, 66–75)
**Severity:** low (works for all current values, but fragile)

`unit()` returns 24-bit precision (`bits >> 40`). `geom_skip` accepts `p: f32`
and does `(1.0 - p).ln()`. With `p` near 1.0 (e.g. mutation_rate_multiplier=20 ×
NN_MUT_RATE_DEFAULT=0.02 = 0.4) the result is fine. But the path `p = 1.0` short-
circuits to 0 BEFORE the slider multiplier-clamp catches it. Slider can drive
`nn_mutation_rate * multiplier` above 1.0; the `.clamp(0.0,1.0)` at brain.rs:170
guards that. OK in practice — flagging for the audit trail.

---

## C4 — snapshot_hash does NOT cover all sim-determining state (snapshot_hash.rs:21–80)
**Severity:** med (golden file misses divergences)

Hashed: tick, id, x, y, vx, vy, energy, age, genome (all), brain weights,
nn_mutation_rate, sun.current, sun.capacity, carrion (xy/pool/age), species
(id+anchor_genome), RNG state.

**Missing (read by tick logic, can diverge silently):**
- `creatures.digestion_cooldown[i]` — affects Eat validity (decode_action)
- `creatures.species_id[i]` and `parent_species_id[i]` — affects extinction & speciation
- `creatures.cumulative_upkeep[i]` — affects carrion pool on death
- `creatures.last_action[i]` / `action_this_tick[i]` — affects one-hot NN input
- `creatures.max_size_reached[i]`, `distance_travelled[i]` — affects HoF + first-move
- `creatures.birth_tick[i]`
- `sun.demand[k]` — zeroed each tick so usually fine
- `carrion.sun_cell` and `carrion.id` — `sun_cell` returns energy to a specific cell
  on decay
- `species.list[k].child_count` / `parent_id` / `depth` / `name` / `died_tick` /
  `anchor_brain_weights`

A bug that flipped digestion_cooldown or species_id by one tick would NOT be
caught by the golden. Recommend either hashing the SaveV1 bytes (canonical-ish
via serde with `preserve_order`) or extending `hash_genome`-style coverage to
every persisted scalar.

Also: `write_f32` hashes `f32::to_bits()` directly. A NaN payload from any
trait drift would create different bit patterns for "identical" worlds across
platforms. Recommend `if v.is_nan() { write_u32(0x7fc00000) }` canonicalization.
Note that `-0.0 != 0.0` in bits — already a real risk for energy after `max(0.0)`
clamps that go through `(-0.0)`.

---

## C5 — `World::from_save_v1` does not validate cross-references (world.rs:1061–1167)
**Severity:** med (panic on corrupted/malicious save)

`validate_soa_lengths` only checks columns are equal-length. Several panics
follow without bounds checks:
 - `world.rs:1015` `self.species.list[cand as usize]` — if `creatures.species_id[i]`
   ≥ `species.list.len()` from a tampered save, `finalize_extinctions` panics.
 - `species.rs:90` `&self.list[id as usize]` in `SpeciesRegistry::get` — same
   risk from every `species.get(self.creatures.species_id[i])` site.
 - `world.rs:1119` `SpeciesRegistry::from_snapshot(list, max_id + 1)` — `max_id`
   could be `u32::MAX`; overflows. Use `checked_add`.
 - `world.rs:1101` `vision: vec![[0.0f32; VISION_LEN]; n]` is fine, but
   `cell_to_carrion: Vec::new()` means the first tick’s `run_vision_pass` must
   populate it (it does, OK).
 - Genome fields are not clamped to [MIN, MAX]; a save with `eye_count = 7`
   (not in `EYE_VALID`) would be treated as blind via `unwrap_or(0)` (vision)
   but `recompute_eye_trig_at` similarly short-circuits. No panic, but eyes
   silently disabled.

Fix: in `from_save_v1`, validate every `species_id[i] < species.list.len()`
and every genome bound, returning `LoadError::StructuralError`.

---

## C6 — Threaded NN-forward result ordering depends on rayon flat_map order  (world.rs:373–446)
**Severity:** low (currently safe; documented invariant — flag for future)

`results: Vec<(f32, f32, Action)> = ranges.par_iter().flat_map(|...| (lo..hi).map(...).collect()).collect();`
then `results.into_iter().enumerate()` maps directly to creature indices.

rayon's `par_iter().flat_map(...).collect()` IS order-preserving by contract,
but if anyone refactors to `par_iter().flat_map_iter(...)` or to `for_each`
with shared mutable target, the index→creature mapping silently breaks. The
dual-golden currently catches it post-hoc, but a comment-level assert
(`assert_eq!(results.len(), n)` then explicit index mapping) would be
cheaper insurance. Recommend storing `(usize, vx, vy, action)` to be explicit.

Also: vision uses `out[..n].par_chunks_mut(chunk_size)` with
`chunk_size = n.div_ceil(N_CHUNKS).max(1)`. For `n < N_CHUNKS` this produces
FEWER than `N_CHUNKS` chunks (e.g. n=3 → chunk_size=1 → 3 chunks of 1).
`chunk_ranges` (used by NN) produces 8 chunks with several empty. They
partition the same elements, but if the comment-asserted "matches chunk_ranges
for all n" is ever relied on (e.g. for per-chunk RNG plumbing later), this
is a foot-gun. Document the divergence in the threaded comment block at
vision.rs:67–74 / world.rs:1247–1251.

---

## C7 — Eye-offset mutation: silently mutates inactive slots after eye_count grows  (genome.rs:163–191)
**Severity:** low (evolutionary fairness, not a panic/divergence)

Order of operations in `mutate_in_place`:
1. eye_count mutates (may go up or down)
2. `n_active = self.eye_count as usize` (post-mutation value)
3. eye_offsets[0..n_active] mutate

If eye_count grew 4→6, slots 4 and 5 (previously 0.0 since dormant) now get a
mutation roll using `r.eye_offsets * rate_multiplier`. Functionally they
inherit Gaussian noise on top of 0.0 instead of starting at the canonical
sector-center. Subtle but stable for the golden.

The species_distance metric handles this via `n_shared = min(eye_count)`, so
dormant slots don't bias species clustering. No correctness impact today,
but `species_distance` does NOT shadow eye_count change cost across the
*old* slots — only the categorical jump cost is applied once.

---

## C8 — `decode_action` NaN handling falls through to first-index tiebreak  (world.rs:1322–1339)
**Severity:** med (NaN propagation hides brain explosions)

```rust
order.sort_by(|&a, &b| logits[b].partial_cmp(&logits[a]).unwrap_or(Ordering::Equal).then(a.cmp(&b)));
```

If any of the 6 output logits is NaN, all comparisons against it return
`Equal`, so the NaN logit sorts via first-index tiebreak (effectively logit=0).
This silences the symptom but the underlying NaN persists into next tick's
last_action one-hot (still valid) and into `vx = output[0].tanh() * speed`,
where `tanh(NaN) = NaN`. That NaN then enters `apply_movement_and_repulsion`
where `vx * vx + vy * vy` produces NaN, `speed_cap * speed_cap` compare is
false, and NaN propagates to position. The clamp at lines 565–586 uses
`<` / `>` comparisons (NaN-false), so position can become NaN and persist.

Fix: detect NaN in `decode_action` (or before tanh) and force `Rest` + zero
velocity. Also assert `creatures.x[i].is_finite()` somewhere on the tick
hot path (debug-only) to catch first-occurrence.

---

## C9 — `mutate_f32` / `mutate_u32` clamp does NOT recover from NaN  (genome.rs:262–275)
**Severity:** low (only fires if `rng.normal()` ever returns NaN)

`f32::clamp` returns NaN when self is NaN, and Rust's `clamp` docs explicitly
say it propagates NaN. `(*value + delta).clamp(lo, hi)` will leave the value
NaN if either input is NaN. `mutate_u32` is worse: NaN cast to u32 is defined
as 0 in modern Rust (saturating), so it'd silently snap the genome trait to 0.

`rng.normal()` uses rejection-sampled Box–Muller with `s > 0.0 && s < 1.0`,
so it cannot produce NaN under correct unit/symm. Defense-in-depth:
`*value = if next.is_finite() { next.clamp(lo, hi) } else { *value };`.

---

## C10 — `chunk_ranges`/vision `par_chunks_mut` produce different chunk **counts** for n < N_CHUNKS  (world.rs:1252, vision.rs:74)
**Severity:** low

For `n = 3, N_CHUNKS = 8`:
 - `chunk_ranges(3)` returns 8 chunks: `(0,1),(1,2),(2,3),(3,3),(3,3),(3,3),(3,3),(3,3)`.
 - `par_chunks_mut(chunk_size=1)` yields 3 chunks.

Both partition `0..3` identically, but if any future code reads `ranges.len()`
assuming `N_CHUNKS`, vision will surprise it. The comment at world.rs:1247
correctly warns about partition equality; consider tightening it to "same
partition shape, possibly fewer non-empty chunks".

---

## C11 — `child_genome.mutate_in_place` called inside `handle_births` mutates BEFORE the species-distance check  (world.rs:943–991)
**Severity:** med (correct per current spec — but easy to misread)

```rust
let mut child_genome = self.creatures.genomes[i].clone();
child_genome.mutate_in_place(&mut self.rng, ...);
let child_brain = Brain::child_from(...);
...
let dist = species_distance(&child_genome, &anchor.anchor_genome, ...);
```

This is correct: speciation is judged on the post-mutation child vs the
*parent's species anchor*, which is what v6 §H prescribes. But the anchor
is `self.species.get(parent_species)` — i.e., it might be the parent's
*current* species, which is itself a descendant of the founder.
`founder_genome_anchor` (used for "weirdest" HoF) is separate. Verify the
spec wants species comparison vs the *speciation-event ancestor* anchor and
not the founder anchor — both reads are plausible from the comment. Flag.

---

## C12 — `events.rs::EventLog` derives `Default` but `ring_cap=0`  (events.rs:41–66)
**Severity:** low

`#[derive(Default)]` sets `ring_cap = 0`. If anything constructs `EventLog::default()`
(unlikely — `EventLog::new()` is used), the recent ring will pop on every push.
`rehydrate_event_log` uses `EventLog::new()` so it's safe today. Consider
removing the derive or implementing `Default` to call `new()`.

---

## C13 — `eat_and_scavenge` allocates a fresh `candidates` Vec per Eat (world.rs:676)
**Severity:** low (perf, not correctness)

`let mut candidates: Vec<usize> = Vec::with_capacity(8);` is allocated inside
every Eat iteration. The pattern elsewhere is to promote scratch to a `World`
field; this one was missed during the perf-2 pass. No correctness impact.

---

## Verified-clean spot-checks

- `SpatialGrid::rebuild` prefix-sum + cursor-copy: counts are added to
  `starts[c+1]` and prefix-summed; cursors then advance. Cursors length
  invariant maintained (line 18 comment). Correct.
- `SpatialGrid::for_each_in_radius` bounds clamps with `.max(0)` then
  `.clamp(0, HASH_DIM-1)`. Inclusive `lo..=hi` loop ranges. Correct.
- `Brain::forward` SIMD matches scalar within 1e-5 (tested). Layer-2
  has no activation — matches spec.
- `decode_action` first-index tiebreak via `.then(a.cmp(&b))` over a
  pre-sorted `[0..6]` array — stable (sort_by is stable). Correct.
- Vision DDA `ray_circle_hit` origin-inside returns None — prevents
  self-bite via co-located bodies. Correct.
- save → from_save_v1 → tick → snapshot_hash matches reference for
  `save_load_step_preserves_determinism` test (RNG state restored
  bit-exact via `Xoshiro256PlusPlus` serde). Verified by existing test.

---

## Suggested priority for fixes

1. **C1** (sun.rs fallback) — trivial; fix even though latent.
2. **C5** (save validation) — small, hardens against future tampered/corrupt saves.
3. **C8** (NaN in decode_action) — defensive, prevents silent NaN persistence.
4. **C4** (snapshot_hash coverage) — needs golden re-bootstrap, schedule with next golden bump.
5. **C2** (geom_skip semantics) — only if spec says "iid Bernoulli per weight"; also requires golden re-bootstrap.
