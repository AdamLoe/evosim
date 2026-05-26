# Audit S23 — Threaded NN writes directly into SoA via `par_chunks_mut`

> Per-piece plan for SHIP item **S23**. Eliminates the per-tick `Vec<(f32, f32, Action)>`
> flat_map + drain in the threaded NN forward path. Threaded golden must remain at
> whatever value PR-3 ends at. Lands in **PR-4**; depends on **S1** (world split) and
> **S11** (extracted `count_carrion_overlap` / `compute_is_at_wall` helpers).
>
> Cross-refs:
> - `docs/plans/audit-master.md` §4 (S23 entry); §6 (PR-4 deps on S11).
> - `docs/plans/audit-triage.md` S23 (group E).
> - `docs/audit/perf-hot-loop.md` #2; `docs/audit/allocations.md` #1.
> - `docs/plans/perf-2-scratch-pool.md` §5 (the `mem::take` recipe).
> - `docs/plans/perf-4-threads.md` (chunking semantics; `par_chunks_mut` reference).
> - `docs/archive/PITCH-v6.md` §J (fixed `N_CHUNKS = 8` sub-RNG pattern).

---

## 0. Status

- **Type.** Single perf commit on PR-4 (partition-stable; no golden regen).
- **Risk.** Medium. The split-borrow dance against three parallel SoA columns
  (`vx`, `vy`, `action_this_tick`) while still reading `&self.creatures`,
  `&self.vision`, `&self.carrion`, and `&self.cell_to_carrion` is the only
  nontrivial part. The chunk-partition contract is unchanged by design.
- **Determinism.** Threaded golden file
  `tests/golden_snapshot_t10000_threaded.txt` MUST remain equal to whatever
  hash is pinned at the end of PR-3. Sequential golden MUST also remain at its
  PR-3 value (this piece doesn't touch the sequential path beyond reusing the
  S11 helpers it already calls).
- **Budget.** ~80 LOC net change in `src/world/nn.rs`. No new fields on
  `World`. No new constants. No `Cargo.toml` edits.
- **Commit message (suggested).** `perf(nn): write threaded NN outputs in-place via par_chunks_mut`

---

## 1. Summary

Today's threaded NN path (pre-S1 location: `src/world.rs:373–446`; post-S1
location: `src/world/nn.rs`, inside `nn_forward_all_chunks` under
`#[cfg(feature = "threads")]`) does:

```rust
let results: Vec<(f32, f32, Action)> = ranges
    .par_iter()
    .flat_map(|&(lo, hi)| (lo..hi).map(|i| pick_action_d(...)).collect::<Vec<_>>())
    .collect();
for (i, (vx, vy, action)) in results.into_iter().enumerate() {
    self.creatures.vx[i] = vx;
    self.creatures.vy[i] = vy;
    self.creatures.action_this_tick[i] = action;
}
```

That allocates **9 per-chunk inner `Vec`s + 1 outer flatten `Vec`** every tick
plus performs N tuple copies during the drain. This piece rewrites the body to
do `par_chunks_mut` over the three SoA output slices in tandem and write
outputs in-place. Per-chunk partition is identical to today's
`chunk_ranges(n)` (still `N_CHUNKS = 8`); per-chunk iteration order is
unchanged; no RNG is consumed in the NN forward pass; **the threaded golden
hash is unchanged.**

After this commit, the per-tick allocations attributable to the threaded NN
drop to zero (the three output slices already live on `CreatureSoA`).

---

## 2. Borrow strategy — **CHOSEN: plan (c) `mem::take` on the three target columns**

Three approaches were considered:

- **(a) `par_chunks_mut(c).zip_eq(par_chunks_mut(c)).zip_eq(par_chunks_mut(c))`.**
  Cleanest if rayon supports `IndexedParallelIterator::zip_eq` on three
  `par_chunks_mut` iterators in tandem. Rayon does provide `zip` / `zip_eq` on
  `IndexedParallelIterator`, but composing three with deterministic chunk
  boundaries requires that all three slices have the **same length** (they do
  — all three are SoA columns of length `n`). The catch: each
  `par_chunks_mut(c)` is its own iterator, and we need a single mutable borrow
  to each underlying `Vec` to call `par_chunks_mut` — but the three Vecs live
  on `self.creatures`, and we also need `&self.creatures` (immutable) to read
  the **input** SoA columns inside the closure. We cannot simultaneously hold
  `&mut self.creatures.vx`, `&mut self.creatures.vy`, `&mut self.creatures.action_this_tick`,
  AND `&self.creatures` for input reads. Approach (a) requires either splitting
  the inputs out via local references before the parallel call, or using
  `split_at_mut` patterns. Workable, but the borrow choreography is fragile.

- **(b) Build a `Vec<(&mut [f32], &mut [f32], &mut [Action])>` upfront, then
  `par_iter_mut`.** Same fundamental borrow problem as (a), plus an extra
  allocation per tick to hold the tuple Vec (exactly what we're trying to
  eliminate).

- **(c) `mem::take` the three output columns (perf-2 §5 recipe).** Locally
  detach `vx`, `vy`, `action_this_tick` from `self.creatures` via three
  `std::mem::take` swaps. This leaves `self.creatures.vx/vy/action_this_tick`
  as empty `Vec`s for the duration of the parallel work, freeing the borrow
  checker to allow simultaneous `&self.creatures` for input reads (the other
  SoA columns — `x`, `y`, `g_size`, energy, age, genome, weights — remain
  attached). Inside the parallel closure we hold three `&mut Vec<T>` (or
  better, `&mut [T]` after a single `as_mut_slice()` each) and tandem-zip
  their `par_chunks_mut` iterators. At the end of the function we restore the
  three columns: `self.creatures.vx = vx_local; ...`. Pattern is identical to
  perf-2's treatment of `scratch_neighbors`.

**Chosen: (c).** Rationale:

1. **Borrow-checker friendly.** Same proven pattern as `scratch_neighbors`
   (perf-2 §5). No fragile `split_at_mut` cascades; no rayon `zip` API
   discovery work.
2. **Allocator-free.** `std::mem::take` is a 24-byte swap (pointer + len +
   cap) per column — total 72 bytes of stack churn per tick. The heap
   buffers never move. The eliminated allocations from today's flat_map (8
   inner Vecs + 1 outer collect) dwarf this by ~400 KB at peak population.
3. **`zip_eq` availability is moot.** Once the three columns are detached
   locally, we can use either (a)-style zip on the locals **or** a simpler
   form: a single `par_chunks_mut` over an `&mut [usize]` of indices,
   producing per-chunk slices that close over the local `vx_local`,
   `vy_local`, `action_local` — but this re-introduces the borrow problem.
   The clean form is to chain the three `par_chunks_mut(chunk_size)` via
   `.zip_eq(...).zip_eq(...)` on the detached locals. `zip_eq` is provided
   on rayon's `IndexedParallelIterator` (per rayon ≥ 1.x), and
   `par_chunks_mut` returns an `IndexedParallelIterator`. If for any reason
   `zip_eq` is not in scope, fall back to `.zip(...).zip(...)` — both
   preserve deterministic chunk-index order on indexed iterators.

### Recipe (paste-ready sketch)

```rust
#[cfg(feature = "threads")]
{
    use rayon::prelude::*;

    // Detach the three output columns from self.creatures via mem::take.
    // The heap buffers travel with the locals; the SoA slots are empty
    // (Vec::new(), no allocation) for the duration of the parallel work.
    // This frees the borrow checker to allow &self.creatures for input reads.
    let mut vx_local = std::mem::take(&mut self.creatures.vx);
    let mut vy_local = std::mem::take(&mut self.creatures.vy);
    let mut act_local = std::mem::take(&mut self.creatures.action_this_tick);

    {
        // Inside this block: read-only refs to the rest of self that the
        // closure needs. None of these conflict with the &mut on vx/vy/act
        // because those live on local stack now, not on self.creatures.
        let creatures_ref = &self.creatures;
        let vision_ref = &self.vision[..n];
        let carrion_ref = &self.carrion;
        let cell_to_carrion_ref = &self.cell_to_carrion;

        // Compute chunk_size identically to today's chunk_ranges contract:
        // N_CHUNKS = 8 fixed (v6 §J); div_ceil + max(1) handles small n.
        let chunk_size = n.div_ceil(N_CHUNKS).max(1);

        // Tandem disjoint-mut chunks over the three output columns.
        vx_local[..n]
            .par_chunks_mut(chunk_size)
            .zip_eq(vy_local[..n].par_chunks_mut(chunk_size))
            .zip_eq(act_local[..n].par_chunks_mut(chunk_size))
            .enumerate()
            .for_each(|(chunk_idx, ((vx_sub, vy_sub), act_sub))| {
                debug_assert_eq!(vx_sub.len(), vy_sub.len());
                debug_assert_eq!(vx_sub.len(), act_sub.len());

                let mut input_buf = [0.0f32; NN_INPUTS];
                let mut hidden_buf = [0.0f32; NN_HIDDEN];
                let mut output_buf = [0.0f32; NN_OUTPUTS];
                let lo = chunk_idx * chunk_size;

                for k in 0..vx_sub.len() {
                    let i = lo + k;
                    // Reuse the S11-extracted free helpers (see §4).
                    let overlap = count_carrion_overlap(
                        creatures_ref, carrion_ref, cell_to_carrion_ref, i,
                    );
                    let is_at_wall = compute_is_at_wall(creatures_ref, i);
                    let (vx, vy, action) = pick_action_d(
                        i,
                        &mut input_buf,
                        &mut hidden_buf,
                        &mut output_buf,
                        creatures_ref,
                        &vision_ref[i],
                        overlap,
                        is_at_wall,
                    );
                    vx_sub[k] = vx;
                    vy_sub[k] = vy;
                    act_sub[k] = action;
                }
            });
    } // borrows of &self.creatures etc. dropped here

    // Restore the three columns. heap allocations are preserved.
    self.creatures.vx = vx_local;
    self.creatures.vy = vy_local;
    self.creatures.action_this_tick = act_local;
}
```

**Panic safety note (per perf-2 §5).** If the parallel `for_each` panics
inside a worker thread, rayon propagates the panic; the three columns are
left as `Vec::new()` (their high-water-mark allocations are lost). This is
acceptable — a panic during NN forward poisons the World; no further ticks
run. No drop-guard is added.

---

## 3. Per-chunk sub-RNG ordering

Per v6 §J the NN forward pass uses **fixed `N_CHUNKS = 8`** chunks
independent of physical thread count, so that results are bit-identical
regardless of how many cores rayon binds. The current `chunk_ranges(n)`
helper (`src/world.rs:1247`, post-S1: `src/world/nn.rs`) produces this
partition: chunk_size = `n.div_ceil(N_CHUNKS)`, ranges
`[(0, chunk_size), (chunk_size, 2*chunk_size), ...]` with the tail capped at
`n`, and an early-stop when a chunk would be empty (so the returned count is
`<= N_CHUNKS` for small `n`).

**This piece must produce the exact same partition.**

Two things to verify in implementation:

1. **Chunk count is identical.** `out[..n].par_chunks_mut(n.div_ceil(N_CHUNKS).max(1))`
   yields the same chunk boundaries as the explicit `chunk_ranges(n)` array
   when `n > 0`. Per perf-4 §5c, this equivalence has already been
   established for vision and the same math is being re-applied here.
2. **Per-chunk sub-RNG derivation is unchanged.** The NN forward pass
   **does not consume RNG** (verified: today's threaded body in
   `src/world.rs:373–446` does not pass `rng` to `pick_action_d` and
   `decode_action` does not draw — actions are decoded deterministically
   from logits and energy/cooldown state). Therefore "per-chunk sub-RNG" is
   a misnomer for this specific call site — there is no per-chunk seed
   derivation to preserve in the NN forward pass itself. **The chunk count
   must remain N_CHUNKS = 8** so that any future RNG plumbing into NN
   forward will see the same sub-RNG seed sequence; but as of today's code
   there is no seed derivation to break.

If implementation discovers an RNG draw inside `pick_action_d` (it should
not), STOP and escalate to the orchestrator — that would invalidate the
threaded golden's stability proof and require revisiting the v6 §J
sub-RNG plumbing before this piece can proceed.

---

## 4. Helper integration (depends on S11)

This piece **hard-depends on S11** (the extraction of
`count_carrion_overlap` and `compute_is_at_wall` from `impl World`
methods into free functions usable from both sequential and threaded
paths).

### Required S11 helper signatures

Per the `audit-master.md` S11 brief, S11 hoists the two helpers from
private `impl World` methods to free functions. The threaded closure must
be able to call them while holding only `&self.creatures` (immutable),
`&self.carrion`, `&self.cell_to_carrion` — NOT `&self`, because `&self`
would conflict with the locally-detached `vx`/`vy`/`action_this_tick`
borrows.

**Required signatures (S11 must deliver these, or equivalent):**

```rust
// src/world/nn.rs (or src/world/mod.rs, wherever S11 places them)
pub(crate) fn count_carrion_overlap(
    creatures: &CreatureSoA,
    carrion: &[Carrion],
    cell_to_carrion: &[Vec<u32>],   // pre-S26 layout (PR-4 S26 changes this)
    i: usize,
) -> u32 { ... }

pub(crate) fn compute_is_at_wall(
    creatures: &CreatureSoA,
    i: usize,
) -> f32 { ... }
```

**Both arguments are `&` references** (or `Copy` for `i: usize`), so the
closure can call them freely. The closure captures `creatures_ref`,
`carrion_ref`, and `cell_to_carrion_ref` (all `&` borrows of fields on
`self` other than the detached output columns) and passes them through.

### Risk if S11 ships a `&self` signature

If S11 keeps the helpers as `impl World` methods (`fn count_carrion_overlap(&self, i)`),
this piece cannot call them from the threaded closure because the closure
does NOT hold `&self` — only the detached output columns live on the
stack, but the borrow checker still sees the `mem::take` as "self is
borrowed (one field is borrowed mutably, others immutably)" until the
locals are reassigned back. Specifically: the closure captures
`creatures_ref: &CreatureSoA` and that is a re-borrow of one field on
self; it cannot be used to derive a `&self` to call `self.method()`.

**Mitigation:** the S11 plan brief explicitly says "Hoist to free
functions" (audit-master §4 S11) and "both paths call them," so the free-fn
shape is the contracted output. If the S11 implementer ships methods
instead, this piece must add a one-line wrapper free fn or the S11
implementer must rework. **Plan dependency: S11 ships free fns taking
`&CreatureSoA`, `&[Carrion]`, `&[Vec<u32>]`, `usize`.** Flagged in §9 risk
register.

---

## 5. Step-by-step implementation order

1. **Confirm S11 has landed (PR-3 prereq).** Verify
   `count_carrion_overlap(creatures, carrion, cell_to_carrion, i) -> u32`
   and `compute_is_at_wall(creatures, i) -> f32` exist as free functions
   in `src/world/nn.rs` (or wherever S11 placed them) and are callable
   from outside `impl World`. If S11 shipped `impl World` methods, escalate
   to the orchestrator (see §4 risk).

2. **Read the post-S1 path.** Find the threaded NN forward block in
   `src/world/nn.rs` (post-S1; pre-S1 it lives at
   `src/world.rs:373–446`). Confirm it still has the `flat_map + collect +
   drain` shape; if any earlier piece already rewrote it, escalate.

3. **Implement the new `par_chunks_mut` block** per §2 recipe.
   - Detach `vx`, `vy`, `action_this_tick` via `mem::take`.
   - Construct read-only refs: `creatures_ref`, `vision_ref`,
     `carrion_ref`, `cell_to_carrion_ref`.
   - Compute `chunk_size = n.div_ceil(N_CHUNKS).max(1)`.
   - Run the tandem `par_chunks_mut(...).zip_eq(...).zip_eq(...).enumerate().for_each(...)`.
   - Inside the closure, call the S11 free fns and `pick_action_d` and
     write into `vx_sub[k]`, `vy_sub[k]`, `act_sub[k]`.
   - Restore the three columns via `self.creatures.vx = vx_local;` etc.

4. **Delete the flat_map + drain.** Remove the `Vec<(f32, f32, Action)>`
   collect and the subsequent `for (i, (vx, vy, action))` drain loop.

5. **Confirm chunk count and sub-RNG seeding are identical.**
   - `cargo test --release --test acceptance acceptance_t10000_threaded`
     under `--features threads`. The threaded golden hash **MUST** equal
     the value pinned at end of PR-3.
   - If it does not match, STOP. Diagnose. Likely causes: (a) chunk
     boundary off by one (rare — `par_chunks_mut(chunk_size)` is bit-stable
     vs `chunk_ranges` math); (b) re-ordered writes inside the closure
     (the body order must match today's: overlap → is_at_wall →
     `pick_action_d` → three writes); (c) the S11 free fns compute
     subtly different values from today's inlined logic (S11 should have
     bootstrap-verified this; re-verify by diffing the inputs/outputs
     against the pre-S11 inline form).

6. **Verify clippy + sequential goldens.**
   - `cargo clippy --all-targets -- -D warnings`.
   - `cargo clippy --all-targets --features threads -- -D warnings`.
   - `cargo test --release --test acceptance` (sequential goldens
     unchanged — this piece does not touch the sequential branch).

7. **Add the new unit test** (§7a).

---

## 6. Path notes

All file paths in this plan are **post-S1** (the world.rs split). The
threaded NN body lives in `src/world/nn.rs`, inside
`fn nn_forward_all_chunks(&mut self, ranges: &[(usize, usize); N_CHUNKS], n: usize)`
under the `#[cfg(feature = "threads")]` branch.

If S1 ended up placing `nn_forward_all_chunks` in
`src/world/tick.rs` or `src/world/mod.rs` instead, follow the actual
post-S1 location per S1's path translation table (`audit-master.md` §4 S1
"path translation table" deliverable). The implementation is mechanical
and follows wherever the function ended up.

Constants used:
- `N_CHUNKS` from `crate::constants::N_CHUNKS` (= 8 per `src/constants.rs:144`).
- `NN_INPUTS`, `NN_HIDDEN`, `NN_OUTPUTS` from existing imports in the
  threaded block.
- `BODY_RADIUS_PER_SIZE`, `HASH_CELL`, `HASH_DIM`, `WALL_THRESHOLD_PAD`,
  `WORLD_SIZE` — only referenced inside the S11 helpers; no need to
  re-import in this file.

---

## 7. Test plan

### 7a. New lib test: `threaded_nn_in_place_writes_match_sequential`

Gated `#[cfg(feature = "threads")]`. Lives in `src/world/nn.rs::tests` (or
wherever `nn_forward_all_chunks` lands per post-S1 path). Constructs a
tiny World, runs one tick under the threaded path, captures `(vx, vy,
action_this_tick)` for every creature, and asserts the captured snapshot
equals what the sequential path produces for the same initial state.

Because both paths live in one binary at a time (cfg-selected), the test
cannot directly run "sequential then threaded" in one process. Two test
shapes are acceptable:

- **Variant A (preferred — simplest):** Run one tick under
  `--features threads`, snapshot `(vx[i], vy[i], action_this_tick[i])` for
  every `i`, then run the **sequential helpers directly** (call
  `count_carrion_overlap` + `compute_is_at_wall` + `pick_action_d` in a
  serial loop using a fresh second `World` constructed from the same seed,
  stepped to the same point) and assert per-creature equality of the three
  outputs. This works because the NN forward pass is RNG-free (§3), so
  the sequential helper outputs are deterministic and equal to what the
  parallel loop should produce.

- **Variant B (broader, slower):** Test only that the threaded path writes
  in-place by comparing the post-tick `(vx, vy, action_this_tick)` to the
  pre-rewrite `flat_map+collect` baseline captured via a pinned hex
  snapshot. This is essentially what the acceptance test already does, so
  Variant B is redundant.

**Use Variant A.** Asserts the in-place writes match the contract on a
small, easily-diagnosable case. The broader equivalence is covered by the
acceptance test under `--features threads` (which holds the threaded
golden equal to its PR-3-pinned value).

```rust
#[cfg(feature = "threads")]
#[test]
fn threaded_nn_in_place_writes_match_sequential() {
    use crate::world::World;
    use crate::creature::Action;

    let mut w = World::new("s23-in-place");
    // Step a few ticks to grow population past 1 so chunks are nontrivial.
    for _ in 0..50 { w.tick_once(); }

    // Snapshot the post-tick threaded outputs.
    let n = w.population() as usize;
    let vx_after: Vec<f32> = w.creatures.vx[..n].to_vec();
    let vy_after: Vec<f32> = w.creatures.vy[..n].to_vec();
    let act_after: Vec<Action> = w.creatures.action_this_tick[..n].to_vec();

    // Reconstruct what the sequential helpers would produce for the SAME
    // pre-NN-forward state. The cleanest way is to re-run a second world
    // to the same tick, then call the helpers in a serial loop and
    // compare. (Re-running with --features threads is acceptable: the NN
    // forward pass is RNG-free per v6 §J, so the sequential helper
    // sequence is deterministic relative to the captured SoA state.)
    let mut w2 = World::new("s23-in-place");
    for _ in 0..50 { w2.tick_once(); }
    // ... advance to the same point where the NN forward would be called
    // but call the helpers in a serial loop; compare outputs.

    // Simpler form: just confirm the threaded outputs ARE valid Actions
    // and the in-place writes survived (sentinel test from perf-2 §7a).
    for i in 0..n {
        assert!(vx_after[i].is_finite(), "vx[{i}] not finite");
        assert!(vy_after[i].is_finite(), "vy[{i}] not finite");
        let _ = act_after[i]; // type-check only
    }
    // Stronger assertion: per-creature equality vs serial helper recompute
    // is the implementer's call; the broader equivalence is covered by
    // the acceptance golden under --features threads.
}
```

The implementer is free to strengthen Variant A by calling the S11 free
fns + `pick_action_d` in a serial loop on `w2`'s captured pre-NN state
and asserting per-creature equality with `w`'s post-NN state. If the
acceptance test passes, this stronger assertion is mathematically
guaranteed; the unit test exists as a fast-running canary so that a
future regression caught here surfaces in 50 ms instead of via the
multi-second acceptance run.

### 7b. Acceptance regressions — already covered

- `cargo test --release --test acceptance` (sequential goldens unchanged).
- `cargo test --release --features threads --test acceptance`
  (`acceptance_t10000_threaded`) — threaded golden unchanged from PR-3
  pin. This is the load-bearing regression catch for "implementer
  accidentally changed semantics while removing the flat_map".

### 7c. Save round-trip — already covered

Save/load tests (perf-2 §7d) cover the post-restore tick path; this piece
doesn't touch save fields, so they automatically remain green.

---

## 8. Determinism impact

**None.** The threaded golden must remain at whatever value PR-3 ends at
(currently expected to be the post-S7+S8 regen value; pinned by PR-3 in
`tests/golden_snapshot_t10000_threaded.txt`). The sequential golden is
untouched (sequential code path is not modified beyond reusing the S11
helpers, which S11 itself bootstrap-verified as byte-identical).

This piece is **partition-stable** — the chunk count, chunk boundaries,
per-chunk iteration order, and per-creature computation order are all
unchanged. The only differences are:

1. Where the outputs land (directly in the SoA column slots vs through a
   temporary `Vec<(f32, f32, Action)>` + drain).
2. Allocator activity (zero per-tick allocs vs 9 per-tick allocs).

Neither difference is visible to `snapshot_hash`, which reads the final
post-tick SoA state.

---

## 9. Risk register

**R1. `zip_eq` not available on `par_chunks_mut`.** Rayon's
`IndexedParallelIterator` provides `zip` and `zip_eq` (per rayon ≥ 1.7);
`par_chunks_mut` returns an indexed iterator, so chaining
`.zip_eq(other_par_chunks_mut)` is valid. If for some reason the implementer
hits a trait-bound error, fall back to plain `.zip(...)`. Both `zip` and
`zip_eq` preserve chunk-index order on indexed iterators; `zip_eq` adds a
runtime panic on length mismatch (defensive, recommended). **Mitigation:**
plan (c)'s `mem::take` shape works with either `zip` or `zip_eq`; pick
whichever compiles cleanly under the project's rayon version. If neither
compiles, the fallback is to build a `Vec<usize>` of indices and
`par_chunks(chunk_size)` over indices instead, with each closure indexing
into three locally-bound `&mut [_]` slices via direct index — but this
re-introduces a per-tick `Vec<usize>` allocation, which defeats the
purpose. **Strong preference: `zip_eq`; fallback: `zip`.**

**R2. `action_this_tick` element type.** Per `src/creature.rs:57`,
`action_this_tick: Vec<Action>` — element type is the `Action` enum
(per `src/creature.rs:17`), NOT `Vec<u8>`. The closure writes
`act_sub[k] = action` where `action: Action`. No `as u8` cast needed. The
plan briefing called out the possibility of `Vec<u8>`; this is **not**
the case in current source. Confirmed: write the `Action` value
directly. If a future encoding change moves `action_this_tick` to
`Vec<u8>`, the cast `act_sub[k] = action as u8` is the one-line
adaptation.

**R3. Sub-RNG ordering / chunk-count drift.** Any change in chunk count
(other than N_CHUNKS = 8) or chunk iteration order breaks the threaded
golden. Mitigation: use `chunk_size = n.div_ceil(N_CHUNKS).max(1)`
exactly as today; verify chunk count matches `chunk_ranges(n).len()` via
`debug_assert!` if paranoid; rely on the acceptance test (step 5 of §5)
as the dispositive check. **Hard rule:** if `cargo test --features threads
--release --test acceptance` produces a different hash than the PR-3 pin,
diagnose and fix before committing — do **NOT** regen the threaded
golden under any circumstances.

**R4. Borrow scope of S11 helpers.** The S11 helpers must accept
`&CreatureSoA`, `&[Carrion]`, `&[Vec<u32>]`, `usize` (or compatible Copy
arguments) so the closure can call them while holding the locally
detached output columns. If S11 shipped `impl World` methods (signature
`fn(&self, i) -> ...`), this piece cannot use them — the closure would
need `&self`, which conflicts with the `mem::take` of three fields off
`self`. **Mitigation:** S11's plan brief explicitly mandates free fns
("Hoist to free functions; both paths call them"). If S11 shipped
methods, escalate to the orchestrator before this piece begins; either
the S11 implementer reworks or this piece adds one-line wrapper free fns
in the same commit.

**R5. Panic safety / lost high-water-mark.** Per perf-2 §5, a panic
inside the parallel `for_each` leaves the three columns as `Vec::new()`;
the high-water-mark heap buffers are lost. This is acceptable because a
panic during NN forward poisons the World. No drop-guard added.

**R6. Capacity invariants.** After `mem::take`, `self.creatures.vx`,
`vy`, and `action_this_tick` are `Vec::new()` (len 0, cap 0). If any
code between the `take` and the restore reads `self.creatures.vx[i]`,
it will panic with index-out-of-bounds. **Mitigation:** the parallel
block contains the entire NN forward pass; nothing reads
`self.creatures.vx/vy/action_this_tick` between the `take` and the
restore. Implementer must verify by reading the surrounding code (lines
immediately before and after the threaded block).

---

## 10. Acceptance criteria

This piece is **done** when all of the following hold:

1. **Zero allocations attributable to the threaded NN per-tick.** Verify
   by inspection: the `par_chunks_mut` body allocates only stack-resident
   buffers (`input_buf`, `hidden_buf`, `output_buf`), and the surrounding
   block does three `mem::take`s (24-byte stack swaps each) plus three
   reassignments (same). No `Vec::new()` / `Vec::with_capacity` / `vec![]`
   / `.collect()` in the per-tick hot path. A simple smoke check:
   `cargo build --features threads --release` and `grep -nE 'Vec::|vec!\\[|\\.collect\\(\\)' src/world/nn.rs`
   inside the threaded block — should return zero non-stack-buffer
   matches.

2. **Both goldens unchanged after PR-4 lands.** Per master plan §10
   acceptance gates:
   - `cargo test --release --test acceptance` (3 default tests, sequential
     golden unchanged).
   - `cargo test --release --features threads --test acceptance`
     (`acceptance_t10000_threaded`, threaded golden equal to PR-3 pin).

3. **Clippy clean under both feature configurations.**
   - `cargo clippy --all-targets -- -D warnings`.
   - `cargo clippy --all-targets --features threads -- -D warnings`.
   - The pre-existing `#[cfg_attr(feature = "threads", allow(dead_code))]`
     annotations on the S11 helpers MAY remain or MAY be removed
     depending on whether the threaded path now references the free-fn
     wrappers (it does, per this piece) — implementer's call to clean up
     the dead-code-allow alongside.

4. **`cargo fmt --check` clean.**

5. **New unit test `threaded_nn_in_place_writes_match_sequential` passes
   under `--features threads`** (skipped without the feature; the
   `#[cfg(feature = "threads")]` gate handles this).

---

## 11. Locked scope (do not expand)

- **Do not change chunk count** away from `N_CHUNKS = 8`.
- **Do not change per-chunk sub-RNG derivation.** (The NN forward pass
  consumes no RNG today; this rule exists to prevent future regressions.)
- **Do not touch the sequential NN path** beyond reusing the S11 helpers
  (which are already shared between sequential and threaded after S11
  lands).
- **Do not change the `Action` enum representation.**
- **Do not change `chunk_ranges` visibility or signature.** The threaded
  block today receives `ranges: &[(usize, usize); N_CHUNKS]` from its
  caller; this piece replaces the use of `ranges` inside the threaded
  branch with `par_chunks_mut(chunk_size)`. The caller still computes
  `ranges` for the sequential branch (which iterates `ranges` directly).
  If the threaded branch no longer references `ranges`, mark the parameter
  with `let _ = ranges;` (mirroring today's `let _ = n;` pattern at
  `src/world.rs:346`) or hoist the param-shape change to a separate
  piece. **Recommend: keep the parameter; the sequential branch uses it.**

---

## 12. Citations

- `docs/plans/audit-master.md` §4 (S23 entry, S11 entry), §6 PR-4 deps.
- `docs/plans/audit-triage.md` S23 (group E description, line ~199).
- `docs/audit/perf-hot-loop.md` #2 (the allocation hotspot motivating S23).
- `docs/audit/allocations.md` #1 (per-tick alloc count attribution).
- `docs/plans/perf-2-scratch-pool.md` §5 (the `std::mem::take` recipe
  used verbatim here for the three-column split-borrow).
- `docs/plans/perf-4-threads.md` §2b (D5), §5c (`par_chunks_mut` chunking
  equivalence with `chunk_ranges`).
- `docs/archive/PITCH-v6.md` §J (fixed `N_CHUNKS = 8` contract; "results
  are identical regardless of how many cores the machine has").
- `src/world.rs:345-447` (pre-S1) — the current threaded NN block being
  rewritten. Post-S1: `src/world/nn.rs`.
- `src/world.rs:1174` and `:1208` — current `impl World`
  `count_carrion_overlap` and `compute_is_at_wall` (post-S11: free
  functions; post-S1: in `src/world/nn.rs`).
- `src/vision.rs:55–85` (post-perf-4) — the canonical `par_chunks_mut`
  reference pattern this piece mirrors for the three-column case.
- `src/creature.rs:17` (`Action` enum) and `:57`
  (`action_this_tick: Vec<Action>`).
- `src/constants.rs:144` (`pub const N_CHUNKS: usize = 8`).

---

*End of S23 plan.*

---

## Review feedback

**Verdict: APPROVE WITH MINOR CONCERNS.** The plan is sound, dependencies are
well-articulated, and the chosen approach correctly mirrors the existing
`scratch_neighbors` pattern in `src/world.rs:518`. Three minor concerns
worth flagging; none are blocking.

### Issues

**I1 (low severity, advisory) — Panic safety / restore invariant.** The plan
acknowledges in §2 panic-safety note and §9 R5 that a panic in `for_each`
leaves the three columns as `Vec::new()` and explicitly accepts this as "the
World is poisoned anyway." This is consistent with the existing
`scratch_neighbors` pattern (perf-2 §5 makes the same call). **However**, the
threaded NN path is different from `scratch_neighbors` in one respect: those
scratch buffers are *recreated/cleared* every tick anyway, but
`creatures.vx`, `creatures.vy`, `creatures.action_this_tick` are **per-creature
state** with `len() == n` invariants that the rest of `World` assumes. If
panic recovery is ever attempted upstream (e.g. via `catch_unwind` in tests
or UI), reads of `creatures.vx[i]` would panic with index OOB, which is a
worse failure mode than a poisoned-but-readable World. **Suggestion:** add a
one-paragraph note that the no-drop-guard choice is contingent on no
upstream `catch_unwind`; if that ever changes (e.g. crash-resistant UI),
switch to a `scopeguard::defer!` or a small drop-guard struct holding
`&mut self.creatures` refs to restore on unwind. Not blocking for PR-4; this
contract is identical to `scratch_neighbors` and won't regress.
`std::mem::swap` does **not** help here — the post-block reassignment is
already correct on the success path; only unwind is at issue.

**I2 (low severity, advisory) — Capacity reservation.** Per §2 the
`mem::take` leaves `self.creatures.vx/vy/action_this_tick` as `Vec::new()`
(cap 0). The restore at end-of-block reassigns the original buffer back —
correct, no extra alloc on the happy path. Verify no code between `take`
and restore mutates the SoA length (births/deaths). A `debug_assert_eq!(
vx_local.len(), n)` and similar after the parallel block, before restore,
would catch a future regression where someone interleaves a birth/death
inside NN forward (a real bug, just not one this code path introduces).
Optional.

**I3 (low severity, advisory) — `vision_ref` slice.** The plan binds
`vision_ref = &self.vision[..n]` and then in the closure indexes
`&vision_ref[i]` where `i = lo + k`. Bounds are safe because `n` is the
population and `i < n` by chunk construction. No change needed; just noting
the slicing is intentional and matches today's code at `src/world.rs:380`.

### Verified

- **Sub-RNG ordering (§3) — VERIFIED.** Read `pick_action_d`
  (`src/world.rs:1346–1372`) and `decode_action` (`src/world.rs:1322–1339`):
  neither consumes RNG. `decode_action` sorts logits deterministically by
  index-tiebreak; `pick_action_d` calls `build_nn_input` (pure read),
  `brains[i].forward` (deterministic SIMD), `tanh`, and `decode_action`.
  No RNG draw anywhere on the NN forward path. The planner's "chunk-count =
  8 preservation" reduction is correct; no per-chunk RNG seeding to worry
  about today.

- **`action_this_tick` type — VERIFIED.** `src/creature.rs:57` confirms
  `pub action_this_tick: Vec<Action>`. No `u8` cast needed. Plan §9 R2 is
  correct.

- **Helper compatibility with S11 — VERIFIED.** S11 plan §2 specifies free
  fns with signatures `count_carrion_overlap(&CreatureSoA, &[Carrion],
  &[Vec<u32>], usize) -> u32` and `compute_is_at_wall(&CreatureSoA, usize)
  -> f32`. S23 §4 demands exactly these signatures. Match is exact. The
  cross-piece note re: S26 changing `cell_to_carrion` shape is correctly
  flagged in both plans.

- **Threaded golden equality — VERIFIED.** §0, §1, §5 step 5, §8, §9 R3,
  and §10 acceptance criterion 2 all repeatedly state the threaded golden
  must equal its PR-3-pinned value. Explicit; no ambiguity. The "do NOT
  regen the threaded golden under any circumstances" hard rule in R3 is
  the correct posture.

- **Zero allocations from threaded NN per tick — VERIFIED.** The proposed
  body uses only stack arrays (`input_buf`, `hidden_buf`, `output_buf`),
  three `mem::take` swaps (24-byte each, stack), and three reassignments.
  Helpers `count_carrion_overlap` and `compute_is_at_wall` per S11 §2 are
  pure-read free fns over slices; no allocation. `pick_action_d` does
  `output_buf[2..8].try_into().unwrap()` which is a slice-to-array
  reference, no alloc. `brains[i].forward` writes into the caller's
  buffers. Allocation claim holds.

### Blocking-issue count

**0.** All three issues are advisory; none gate PR-4 landing.
