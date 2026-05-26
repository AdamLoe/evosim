# S39 — Threaded=Sequential, Save/Load, and Chunk-Count Invariant Tests

**Audit pass:** v1.1 (2026-05) — **revised 2026-05-24** per review feedback (see §12 changelog).
**PR slot:** PR-3 (Determinism + Correctness + Regen)
**Depends on:** S1 (world split → `src/world/nn.rs` exists), S10 (chunk_ranges debug-assert + cross-partition invariant assertion), S11 (extracted `count_carrion_overlap` / `compute_is_at_wall`), S12 (`validate_save` hardens the load path).
**Determinism impact:** none (tests-only). The two small `pub(crate)` exposures and the one runtime toggle added in this commit are observation-only on the happy path — see §6.
**Effort:** S (was S; bumped slightly by the runtime-toggle plumbing in §3 and the partition-extraction in §5, but both stay well under the M threshold).

---

## 1. Summary

Add three new tests covering load-bearing invariants the current acceptance suite does not enforce:

1. **`acceptance_threaded_actually_matches_sequential_t10000`** — *(in `tests/acceptance.rs`, only under `--features threads`)*. In **one** test process: construct two `World`s with the same seed; run world A 10k ticks via the normal `tick_once` (threaded NN forward); run world B 10k ticks with a runtime toggle (`pub(crate) force_sequential_nn: bool`) set so the threaded code path short-circuits to the sequential branch; assert `snapshot_hash(&A) == snapshot_hash(&B)`. This actually exercises both code paths at runtime — a divergence in the threaded NN would flip A's hash without touching B's, and the assertion would fail loudly. The post-regen golden file is **not** consulted: the test compares two live runs, so the regen ceremony can never make this test tautological.

2. **`save_load_hash_equal_immediately_after_load`** — *(in `tests/acceptance.rs`, **runs under BOTH feature sets**)*. For `n ∈ {0, 1, 200, 2000}`: build a fresh `World`, step `n` ticks, `SaveV1::from_world` → JSON → `from_str` → `World::from_save_v1`, then `snapshot_hash(&original)` and `snapshot_hash(&loaded)` **before any further `tick_once`** and assert they are equal. Catches fields silently zeroed on load (e.g. `pending_extinction_check`, `live_species_count`). Runs in both default and `--features threads` builds because the save/load round-trip is a determinism invariant that must hold in both — threaded-only load divergences (e.g. a future rayon-thread-pool-dependent reconstruction) would slip through a cfg-gated test.

3. **Chunk-partition invariant test** — *(in `src/world/nn.rs` post-S1, as a `#[cfg(test)] mod tests` entry)*. For `n ∈ {0, 1, 7, 8, 9, 100, 1500, N_CHUNKS, N_CHUNKS + 1}` assert:
   - returned ranges contiguously partition `[0, n)` with no overlaps;
   - first range starts at 0, last range ends at `n`;
   - returned chunk count is `<= N_CHUNKS`;
   - `chunk_ranges(n)` and `chunk_base_size(n)` agree (the base size, applied as a stride to `par_chunks_mut`, produces the same partition as `chunk_ranges`).
   - **vision.rs no longer inlines the partition formula.** This commit extracts `pub(crate) fn chunk_base_size(n: usize) -> usize` from `src/world/nn.rs` and rewrites `src/vision.rs:74` to call it. The duplication is removed at the source — see §5 and §6.

---

## 2. Test placements

| Test | File (post-S1) | cfg gate |
|---|---|---|
| (a) `acceptance_threaded_actually_matches_sequential_t10000` | `tests/acceptance.rs` | `#[cfg(feature = "threads")]` |
| (b) `save_load_hash_equal_immediately_after_load` | `tests/acceptance.rs` | **none** — runs in both feature builds |
| (c) `chunk_partition_invariants_and_vision_agreement` | `src/world/nn.rs` `mod tests` | none (default + threaded both) |

**Re test (a):** Without `--features threads`, the threaded NN code path is not compiled in, so comparing "threaded == sequential" is meaningless (both would resolve to the same sequential branch). The test stays gated to the threaded build. In that build it constructs two worlds in a single process, drives each through 10k ticks where world B uses the new `pub(crate) force_sequential_nn` toggle so its `nn_forward_all_chunks` short-circuits to the sequential branch, and compares hashes. The post-regen golden values are not read — the assertion is between two live hashes. This means: (i) the regen ceremony cannot make the test pass tautologically; (ii) a future divergence between the threaded and sequential NN code branches is caught immediately, regardless of what the goldens say.

**Re test (b):** Drop the cfg gate. Save/load semantics must hold in both feature builds, and an `#[cfg(not(feature = "threads"))]` gate hides the case where `from_save_v1` reconstructs `World` correctly under default features but incorrectly under `--features threads` (e.g. a future field that depends on rayon's thread-pool init, or a future `pub(crate)` scratch that is freshly-allocated-vs-loaded only on one path). The implementer must also move `use evosim::save::SaveV1;` out from under its current `#[cfg(not(feature = "threads"))]` gate at `tests/acceptance.rs:7-8` so the import is visible in both builds. `SaveV1` itself is feature-flag-independent; only the existing F.26 test's local import is gated, and that gate can simply be removed. CI then runs test (b) twice (once per feature flag), at a combined cost of ~3 s on top of the existing acceptance suite (~1.6 s × 2 feature builds; see §9 (d)).

**Re the existing `save_load_step_preserves_determinism` (out of scope but flagged):** that test is similarly gated to `#[cfg(not(feature = "threads"))]` (line 117) and would benefit from the same dual-build treatment. **Do not** expand S39 to also un-gate it — that is a separate test-hardening pass. Note this observation in DECISIONS.md or the next-pass triage so it is not forgotten.

**Re test (c):** Lives in `src/world/nn.rs` post-S1 (the same file that hosts `chunk_ranges` after the split). Compiles in both feature builds. The cross-partition equivalence is asserted by **calling the real shared base-size function** that `vision.rs` will now call, rather than re-inlining the formula a third time. See §5 for the full design and §6 for the required `pub(crate) fn chunk_base_size` exposure.

---

## 3. Test (a) detail — `acceptance_threaded_actually_matches_sequential_t10000`

### Approach: runtime two-path comparison in one process (chosen)

The test runs **both** code paths at runtime, in a single test process, from the same seed, and asserts hash equality. **No golden file is read.**

The prior plan's "compare-to-golden" design was a tautology: both `tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt` are regenerated in the same PR-3 §8 ceremony, so asserting `seq_golden == thr_golden` re-states a property the regen step already established. A genuine threaded-vs-sequential drift introduced after regen would propagate into BOTH goldens on the next regen and the test would continue to pass. That fails the central goal of S39: catch a real drift between the threaded and sequential NN code paths.

### Mechanism: `pub(crate) force_sequential_nn: bool`

The S39 commit adds a runtime toggle on `World`:

- New field `pub(crate) force_sequential_nn: bool` on `World` (defaults to `false`). Lives in `src/world/mod.rs` post-S1; serialized as `#[serde(default, skip_serializing_if = "std::ops::Not::not")]` so it does not appear in saves and round-trips to the default value on load (out-of-band, observation-only test knob).
- At the top of `nn_forward_all_chunks` (post-S1: `src/world/nn.rs`), the threaded branch is wrapped:

  ```rust
  #[cfg(feature = "threads")]
  {
      if self.force_sequential_nn {
          // Fall through to the sequential branch below.
      } else {
          /* existing rayon par_iter / flat_map / collect path */
          return;
      }
  }
  // Sequential branch — runs in default builds always, and in threaded builds
  // when force_sequential_nn is true.
  for &(lo, hi) in ranges { /* unchanged sequential body */ }
  ```

  The sequential branch becomes the fallthrough; the threaded branch early-returns. Behavior under default features is unchanged (the cfg-gated block is absent). Under `--features threads` with the toggle false, behavior is unchanged (threaded path runs as today). Only the test sets the toggle.

- The `force_sequential_nn` field is excluded from `snapshot_hash` (S7's planner is responsible for not adding observation-only fields to the hash; flag this here for the cross-reviewer).
- The field is **not** in `to_save_v1` / `from_save_v1`; both sides reconstruct the default `false`. Test (b) is unaffected.

### Test body (illustrative)

```rust
#[cfg(feature = "threads")]
#[test]
fn acceptance_threaded_actually_matches_sequential_t10000() {
    // World A — threaded NN forward (production path under --features threads).
    let mut wa = World::new(SEED);
    debug_assert!(!wa.force_sequential_nn, "default toggle must be false");
    for _ in 0..TICKS {
        if !wa.tick_once() { break; }
    }
    let hash_a = snapshot_hash(&wa);

    // World B — same seed, but the threaded NN branch short-circuits to the
    // sequential body. Vision still uses rayon (vision is RNG-free and the
    // partition is bit-identical regardless of thread count; see §5).
    let mut wb = World::new(SEED);
    wb.force_sequential_nn = true;
    for _ in 0..TICKS {
        if !wb.tick_once() { break; }
    }
    let hash_b = snapshot_hash(&wb);

    assert_eq!(
        hash_a, hash_b,
        "threaded NN forward must produce a bit-identical hash to the \
         sequential NN forward at t={TICKS} under the pinned seed; \
         got threaded={hash_a:#018x}, sequential={hash_b:#018x}. \
         A divergence here means the threaded code path (rayon par_iter / \
         flat_map / drain) has drifted from the sequential branch; bisect \
         src/world/nn.rs::nn_forward_all_chunks.",
    );
}
```

### Cost

Doubles the 10k-tick wall-clock vs the existing `acceptance_t10000_threaded`. At ~3-5 s threaded release on a CI core, the test adds ~6-10 s to `cargo test --release --features threads --test acceptance`. Well within the 8 s PERF_BUDGET_MS that the existing thread test already pins (the test does NOT inherit that budget — it has no perf-gate assertion of its own; the existing threaded acceptance test continues to enforce it for the single-world case).

### Rejected alternatives

1. **Compare-to-golden (the previous plan).** Tautological after regen; does not catch genuine drift. Rejected.
2. **`ThreadPoolBuilder::new().num_threads(1)`-scoped run.** Runs the threaded code path with 1 thread, which still goes through `par_iter`/`flat_map`/`collect` — bit-identical to today's threaded path but does NOT exercise the sequential branch. Catches nothing the threaded golden does not. Rejected.
3. **Test-only `pub(crate) fn nn_forward_sequential_for_test` method.** Equivalent in effect to the runtime toggle but heavier: requires either duplicating `tick_once` logic into a `tick_once_with_sequential_nn` variant, or threading a function pointer through `tick_once`. The runtime toggle costs one boolean field and one `if` branch; both are O(1) per tick and observation-only. Rejected as overkill.

The chosen design exposes the minimum surface (one `pub(crate)` field, one runtime branch), and the surface is documented as test-only with a `// observation-only test knob` comment at the field declaration.

---

## 4. Test (b) detail — `save_load_hash_equal_immediately_after_load`

### Mechanics

No cfg gate. The test compiles and runs under both `cargo test --test acceptance` and `cargo test --features threads --test acceptance`.

For each `n` in `[0, 1, 200, 2000]`:

1. `let mut w = World::new(SEED);`
2. `for _ in 0..n { assert!(w.tick_once(), "world died before n={n} under pinned seed"); }` *(the assert guards against early world-end at n=2000; if a future change makes the world die in <2000 ticks under the pinned seed, the test fails loudly rather than masking the issue)*.
3. `let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");`
4. `let save: SaveV1 = serde_json::from_str(&json).expect("deserialize");`
5. `let loaded = World::from_save_v1(save).expect("from_save_v1");`
6. `let h_orig = snapshot_hash(&w);`
7. `let h_loaded = snapshot_hash(&loaded);`
8. `assert_eq!(h_orig, h_loaded, "save→load at n={n} diverged before any further step");`

**Critical:** step 6 and 7 happen **before** any `tick_once` on `loaded`. The existing `save_load_step_preserves_determinism` test re-steps 500 ticks before comparing, so a field that is zeroed on load but re-derived during the next step would mask the bug. Test (b) closes that gap directly.

### Removing the cfg gate

The previous plan placed test (b) under `#[cfg(not(feature = "threads"))]`. That gate would hide threaded-only load divergences (e.g. a future `pub(crate)` field or scratch buffer whose construction depends on the rayon thread-pool init state). Save/load semantics must hold under both feature builds, so the gate is removed.

Implementer step:

- At `tests/acceptance.rs:7-8`, move `use evosim::save::SaveV1;` out from under its existing `#[cfg(not(feature = "threads"))]` gate so the import resolves in both feature builds. `SaveV1` itself is feature-flag-independent; only the existing test's local import is gated, and the gate exists for historical reasons (the import was added alongside `save_load_step_preserves_determinism`, which IS gated). Removing the gate on the import does not affect that older test.
- Test (b) function has no cfg attribute. It runs once per `cargo test` invocation per feature flag — twice total in CI.

### Coverage detail per n

- `n = 0`: **smoke-only.** Both sides hash an empty/initial world; this case asserts `from_save_v1(to_save_v1(World::new))` does not panic and produces a structurally-equal world per the hash function's current field coverage. If n=0 fails, the bug is in `World::new` or `to_save_v1` itself, not in the field list. Kept because the cost is ~0 ms and a regression here would otherwise show up only as a confusing failure at n=1.
- `n = 1`: one-tick world — first NN forward has run, vision is populated, but no births/deaths have occurred under the pinned seed.
- `n = 200`: enough ticks for first births and possible first deaths; exercises `pending_extinction_check` if any species hit zero.
- `n = 2000`: enough ticks for multi-generation churn, multiple speciation events, and stable carrion stream. This is the most "interesting" case structurally and is the primary signal for the dual-build run.

### Interaction with S7

The substantive coverage of test (b) is "every newly-hashed field is either included in `to_save_v1`/`from_save_v1` OR is reset on load in a way that matches the original at the same tick." The S7 planner is responsible for ensuring this property; test (b) is the test that catches violations. S39's planner cannot pre-validate this — the test simply asserts the invariant.

### Interaction with S12

S12 (`validate_save`) hardens `from_save_v1` against malformed inputs but does NOT change accept-path semantics for valid saves. Test (b) flows entirely through the accept path, so S12 is a transitive prerequisite only in the sense that the load path must be robust enough to not panic on the (well-formed) round-tripped inputs — which it must be regardless of S12.

---

## 5. Test (c) detail — `chunk_partition_invariants_and_vision_agreement`

### Source-of-truth extraction (chosen): single shared partition

The previous plan asserted vision-vs-NN partition agreement by **re-inlining** the partition formula a third time inside the test. That entrenches the duplication the test is supposed to guard against: a future refactor of `vision.rs:74` could drift, and as long as the author also updates the inline formula in the test to match, the test would pass while `vision.rs` and `chunk_ranges` disagree on disk.

S39 fixes this at the source. The commit:

1. Promotes `chunk_ranges` (post-S1: `src/world/nn.rs`) to `pub(crate)`.
2. Adds a new `pub(crate) fn chunk_base_size(n: usize) -> usize { n.div_ceil(N_CHUNKS).max(1) }` next to `chunk_ranges`. Documented as the single source of truth for the partition stride used by every `par_chunks_mut` call in the simulation.
3. Rewrites `src/vision.rs:74` from `let chunk_size = n.div_ceil(crate::constants::N_CHUNKS).max(1);` to `let chunk_size = crate::world::nn::chunk_base_size(n);`. No behavior change (the function returns the same value), but vision now consumes the shared function rather than inlining the formula.
4. Test (c) imports `chunk_ranges` and `chunk_base_size` and asserts they agree for every `n` in the test set. Because `vision.rs:74` now calls `chunk_base_size` directly, the test transitively pins vision's partition stride too — any future drift in either site is impossible: there is only one site.

### Test body

```rust
#[cfg(test)]
mod tests {
    use super::{chunk_base_size, chunk_ranges};
    use crate::constants::N_CHUNKS;

    /// S39 test (c): chunk_ranges partitions [0, n) cleanly AND agrees with
    /// the chunk-base-size that drives every par_chunks_mut call site.
    ///
    /// The cross-partition equivalence is asserted by CALLING the real shared
    /// chunk_base_size function (which vision.rs:74 also calls post-S39),
    /// not by re-inlining the formula. This means a future refactor of either
    /// chunk_ranges or chunk_base_size cannot silently desync the NN and
    /// vision partitions — they share one source of truth.
    #[test]
    fn chunk_partition_invariants_and_vision_agreement() {
        let cases = [0usize, 1, 7, 8, 9, 100, 1500, N_CHUNKS, N_CHUNKS + 1];
        for &n in &cases {
            let ranges = chunk_ranges(n);

            // (i) non-empty-range count <= N_CHUNKS.
            let non_empty = ranges.iter().filter(|(lo, hi)| lo < hi).count();
            assert!(
                non_empty <= N_CHUNKS,
                "n={n}: non-empty chunks {non_empty} exceed N_CHUNKS {N_CHUNKS}",
            );

            // (ii) first range starts at 0.
            assert_eq!(ranges[0].0, 0, "n={n}: first range must start at 0");

            // (iii) last range ends at n.
            assert_eq!(
                ranges[N_CHUNKS - 1].1, n,
                "n={n}: last range must end at n",
            );

            // (iv) contiguous and non-overlapping.
            for k in 0..N_CHUNKS - 1 {
                assert_eq!(
                    ranges[k].1, ranges[k + 1].0,
                    "n={n}: gap or overlap between chunk {k} and {}",
                    k + 1,
                );
            }

            // (v) coverage: sum of widths equals n.
            let total: usize = ranges.iter().map(|(lo, hi)| hi - lo).sum();
            assert_eq!(total, n, "n={n}: ranges do not cover [0,n)");

            // (vi) Cross-partition equivalence. Reconstruct the partition that
            //      par_chunks_mut(chunk_base_size(n)) would produce and assert
            //      it matches chunk_ranges. This is the property vision.rs:74
            //      relies on post-S39: vision drives par_chunks_mut with the
            //      stride returned by chunk_base_size, and that stride must
            //      yield the same boundaries as chunk_ranges.
            let base = chunk_base_size(n);
            let mut reconstructed = [(0usize, 0usize); N_CHUNKS];
            for k in 0..N_CHUNKS {
                let lo = (k * base).min(n);
                let hi = ((k + 1) * base).min(n);
                reconstructed[k] = (lo, hi);
            }
            assert_eq!(
                ranges, reconstructed,
                "n={n}: chunk_ranges output disagrees with the partition that \
                 par_chunks_mut(chunk_base_size(n)) would produce; the two \
                 must stay in lockstep because vision.rs and NN forward both \
                 depend on it.",
            );

            // (vii) n=0 special-case: chunk_base_size returns 1 (not 0) so
            //       par_chunks_mut does not panic on an empty slice. Pin this.
            if n == 0 {
                assert_eq!(base, 1, "n=0: chunk_base_size must return 1 to keep \
                                     par_chunks_mut(stride) well-defined on []");
                for &(lo, hi) in &ranges {
                    assert_eq!(lo, 0, "n=0: all ranges must be (0, 0)");
                    assert_eq!(hi, 0, "n=0: all ranges must be (0, 0)");
                }
            }
        }
    }
}
```

### Notes

- `N_CHUNKS + 1` is included so the `n > N_CHUNKS` boundary is exercised (this is where chunks become `base=1` and the partition switches from "some chunks of size > 1" to "all chunks size 1, one extra slot").
- `n = N_CHUNKS` (== 8 currently) is the exact-fit case where every chunk has length 1.
- `n = 0` and `n = 1` are the degenerate cases; the n=0 special-case (vii) pins the `.max(1)` guard inside `chunk_base_size`, since `par_chunks_mut(0)` would panic.
- The cross-partition assertion in (vi) is reconstructed from the **shared** `chunk_base_size`, not from a re-inlined formula. If a future change to `chunk_base_size` or `chunk_ranges` desyncs them, this test fails. If a future contributor adds a new `par_chunks_mut` call site, they SHOULD use `chunk_base_size(n)` to inherit the invariant (note this in `chunk_base_size`'s doc comment per §6).

### Scope expansion justification

This extraction adds two `pub(crate)` items and rewrites one line in `vision.rs`. Net diff: ~10 lines across two files. Well below the threshold for a separate plan piece. The alternative ("call both functions but accept that vision.rs still inlines its formula and test (c) inlines it again") would leave the duplication intact — exactly the anti-pattern the test is meant to prevent. The extraction is small, defensible, and lands inside S39's commit because S39 is the test that depends on it. The S10 planner is independently adding `debug_assert!` inside `chunk_ranges`; the two pieces are complementary and the cross-reviewer at end of PR-3 must confirm both have landed.

---

## 6. Path notes and required exposures (preconditions for the implementer)

S39's commit MUST include the following small `pub(crate)` exposures and edits. The implementer (sonnet) handles them inside S39's own commit — no separate plan piece is needed.

### New `pub(crate)` items in `src/world/mod.rs` (post-S1)

- **`pub(crate) force_sequential_nn: bool`** — new field on `World`. Default `false`. Initialized in `World::new` to `false`. Excluded from `snapshot_hash` (see S7 cross-piece note in §9 (e)). Excluded from `to_save_v1` / `from_save_v1` (reconstructed to default on load). Doc comment: `// S39 test (a) observation-only knob: when true, nn_forward_all_chunks's threaded branch short-circuits to the sequential branch. NEVER set by production code.`

### New `pub(crate)` items in `src/world/nn.rs` (post-S1)

- **`pub(crate) fn chunk_ranges(n: usize) -> [(usize, usize); N_CHUNKS]`** — promote from `fn` to `pub(crate)`. Already documented; no semantic change.
- **`pub(crate) fn chunk_base_size(n: usize) -> usize`** — new function. Body: `n.div_ceil(N_CHUNKS).max(1)`. Doc comment:
  ```
  /// Single source of truth for the chunk stride used by every `par_chunks_mut`
  /// call in the simulation (vision.rs and nn_forward_all_chunks). Returns the
  /// stride such that `slice.par_chunks_mut(chunk_base_size(n))` yields the
  /// same partition as `chunk_ranges(n)`. The `.max(1)` guard keeps the
  /// stride well-defined when n == 0 (rayon's par_chunks_mut(0) panics).
  ///
  /// New `par_chunks_mut` call sites MUST use this function rather than
  /// inlining the formula; S39 test (c) pins the invariant.
  ```
- The runtime `if self.force_sequential_nn { /* fall through */ }` branch at the top of `nn_forward_all_chunks`'s threaded path. See §3 for the structural pattern.

### Edits to `src/vision.rs:74`

- Replace `let chunk_size = n.div_ceil(crate::constants::N_CHUNKS).max(1);` with `let chunk_size = crate::world::nn::chunk_base_size(n);`. One-line change. No behavior change (the function returns the same value). Removes the third copy of the partition formula.

### Edits to `tests/acceptance.rs`

- Remove the `#[cfg(not(feature = "threads"))]` gate on line 7's `use evosim::save::SaveV1;` so the import resolves in both feature builds.
- Add test (a) under `#[cfg(feature = "threads")]`.
- Add test (b) with NO cfg gate.

### Files NOT touched by S39

- `src/world/save_v1.rs` (post-S1): no changes — `force_sequential_nn` is excluded from save/load.
- `src/snapshot_hash.rs`: no changes (test (a) does not modify the hash function; S7 owns hash field additions and is responsible for excluding `force_sequential_nn`).
- `src/lib.rs`: no changes (no new public surface; `pub(crate)` is internal).

### Cross-reviewer checklist (S39 specific, addition to master plan §7)

The PR-3 cross-reviewer must verify:

- `grep -nE 'n\.div_ceil\(N_CHUNKS\)\.max\(1\)|n\.div_ceil\(crate::constants::N_CHUNKS\)\.max\(1\)' src/` returns ONE hit: the body of `chunk_base_size` in `src/world/nn.rs`. Any other hit is a regression.
- `grep -n 'force_sequential_nn' src/snapshot_hash.rs src/world/save_v1.rs` returns ZERO hits.
- `chunk_base_size` and `chunk_ranges` are both `pub(crate)`, not `pub`.

---

## 6b. Sequencing relative to S1

`src/world/nn.rs` does not exist pre-S1. S39 in its entirety depends on S1 (per the existing dependency declaration). If a future re-sequencing puts S39 ahead of S1, test (c) and the `chunk_base_size` extraction fall back to `src/world.rs`'s existing `mod tests` block and `pub(crate) fn` location respectively; the runtime `force_sequential_nn` toggle stays on `World` regardless of file layout.

---

## 7. Step-by-step implementation order

S39's three tests have different dependencies and may land in two commits — one before the PR-3 regen ceremony, one after.

### Pre-regen commit (lands alongside S1, S10, S11, S12 outputs)

1. **Test (c)** (chunk-partition invariants + cross-partition equivalence). Depends on S1 (`src/world/nn.rs` exists). No golden interaction — this is a pure unit test in `src/world/nn.rs::mod tests`. Lands together with:
   - the `pub(crate) fn chunk_base_size` extraction in `src/world/nn.rs`;
   - the `vision.rs:74` rewrite to call `chunk_base_size`;
   - the `chunk_ranges` `fn → pub(crate) fn` promotion.

   This commit can land as soon as S1 is in, even if S10/S11/S12 are still in flight. Provides immediate value: if a refactor breaks the partition equivalence, this test catches it before goldens are regen'd. S10's own `debug_assert!` and its `chunk_ranges_partition_invariant` test are complementary — keep both per §9 (c).

2. **Test (b)** (save/load equality). Depends on S12 landing first so the load path is hardened (S12 is a transitive prereq — the actual blocker is just that `from_save_v1` does not panic on round-tripped well-formed inputs, which it must do regardless of S12). Lands in the same pre-regen commit as test (c). Includes the cfg-gate removal on the `SaveV1` import per §4.

3. **`force_sequential_nn` plumbing** (the `pub(crate)` field on `World` + the runtime branch at the top of `nn_forward_all_chunks`'s threaded path). Lands in the pre-regen commit. **No behavior change** in either feature build when the toggle is false (default). Adding the field and the branch before S7+S8's regen does NOT affect the regen output: the field is excluded from `snapshot_hash`, `to_save_v1`, and `from_save_v1`, and the runtime branch is `if false { /* dead in production */ }`. The cross-reviewer at PR-3 end verifies this via the §6 grep checklist.

### Post-regen commit (lands as the final PR-3 commit)

4. **Test (a)** (`acceptance_threaded_actually_matches_sequential_t10000`). Even though test (a) no longer reads any golden file (per the §3 rewrite), it asserts hash equality between two runs of 10k ticks — both runs are sensitive to the new S7+S8 hash bytes. Landing test (a) before S7+S8 regen means the test would run against the OLD hash function, and any drift in the threaded NN that the OLD hash happens to mask but the NEW hash catches (or vice versa) would land in PR-3 looking like a test-(a) bug rather than an S7+S8 interaction. To keep the PR-3 narrative clean, schedule test (a) as the **final** commit of PR-3, after the regen ceremony and after `cargo test --release --features threads --test acceptance` is green on the new golden.

   Lighter alternative if the orchestrator prefers one S39 commit: land test (a) in the pre-regen commit too. It will still pass under the old hash function (the two paths agree under any hash function that respects the same field set). The cost is a less-clean PR-3 narrative: a S39 commit landing pre-regen would be re-verified post-regen rather than authored against the final state. The plan's recommendation is the split (test a post-regen) per master plan §8's existing note.

---

## 8. Determinism impact

None. All three tests are observation-only. None of them call `tick_once` on a world that does not already exist for some other purpose, and none mutate global state. Neither golden file changes as a result of S39.

---

## 9. Risk register

(a) **Test (a): a real threaded-vs-sequential drift surfaces as a test-(a) failure, not as a golden mismatch.** This is by design — test (a) now compares two live hashes, not a hash against a golden. If a future contributor changes the threaded NN code path (e.g. switches the rayon collection strategy) and inadvertently changes the result bytes, test (a) fails with a clear message ("threaded NN forward must produce a bit-identical hash to the sequential NN forward"). The cross-reviewer and the contributor know exactly where to bisect: `src/world/nn.rs::nn_forward_all_chunks`'s threaded branch. The previous plan's "compare-to-golden" design would have allowed such a drift to persist until the NEXT regen ceremony (when both goldens would silently flip together). Mitigation: none needed — the test does the right thing on failure. Document in DECISIONS.md if test (a) ever has to be weakened (e.g. a deliberate cross-thread reduction order change).

(b) **Test (a): the `force_sequential_nn` field leaks into the hash or save.** If S7's hash-coverage extension accidentally includes the new `force_sequential_nn` field, test (a) would still pass (both worlds default to `false` and the toggle changes only for world B AFTER `World::new` returns), but a save written by world B would carry the toggle and a subsequent load would reconstruct it with whatever default S7's planner chose. Mitigation: §6's grep checklist (`grep -n 'force_sequential_nn' src/snapshot_hash.rs src/world/save_v1.rs` returns zero hits). The cross-reviewer enforces this at PR-3 end.

(c) **Test (b): n=0 dilution.** n=0 is smoke-only per §4 (both sides hash empty/initial state; the test asserts no panic on the round trip). Kept because the cost is ~0 ms and a regression here surfaces as a clear failure rather than a confusing failure at n=1. If a future contributor wants stronger n=0 signal, the alternative is a "tick 0 with one debug-seeded carrion" scenario; that requires a test-only seed knob and is explicitly deferred.

(d) **Test (b): hash-field-vs-load-field invariant.** Post-S7 (extended hash coverage), if any newly-hashed field is reset on load (e.g. `pending_extinction_check` is reset to empty on load per the pre-S1 `world.rs:1155`), test (b) at n=0 still passes (both sides empty), but at n=200 or n=2000 could fail. The S7 planner is responsible for not adding fields to the hash that are reset on load OR for including them in `to_save_v1`/`from_save_v1`; test (b) is the test that catches violations. Dual-build execution (per §4) catches the threaded-only variant of this class of bug.

(e) **Test (c) cross-partition coupling with S10.** S10's plan adds a `debug_assert!` inside `chunk_ranges` for the same invariants test (c) checks, plus a unit test exercising `n ∈ {0,1,7,8,9,100,1500}`. Test (c) adds `N_CHUNKS` and `N_CHUNKS + 1` to that set, adds the n=0 special-case pin for `chunk_base_size`'s `.max(1)` guard, AND asserts cross-partition equivalence by calling the now-shared `chunk_base_size`. The two are complementary — keep S10's test as-is (it serves the debug-assert reasoning) and land test (c) alongside it under a non-colliding name. Suggested: S10 keeps `chunk_ranges_partition_invariant` and S39 uses `chunk_partition_invariants_and_vision_agreement`.

(f) **`chunk_base_size` extraction risk.** The extraction is a one-line move (the formula is the same in both sites today). Risk: a copy-paste error during extraction silently changes the stride. Mitigation: test (c)'s assertion (vi) catches any divergence between `chunk_ranges` and `chunk_base_size`, and the existing threaded acceptance test (`acceptance_t10000_threaded`) catches a stride change because vision's partition would change and the threaded golden would drift. If the extraction is done correctly, both tests pass with byte-identical goldens (S37's bootstrap-style verify-before-pin pattern applies here too — run `cargo test --release --features threads --test acceptance` after the extraction and before any other PR-3 changes; if the golden drifts, the extraction was incorrect).

(g) **Test (b) cost under dual builds.** n=2000 at ~0.8 ms/tick release is ~1.6 s; combined across `n ∈ {0, 1, 200, 2000}`, the test runs ~2.2 s per feature build, ~4.4 s total. Well within CI test budgets. Test (a) adds another ~6-10 s under `--features threads` (two 10k-tick runs). Total PR-3 acceptance suite cost growth: ~10-13 s. Acceptable. If CI runs `cargo test` rather than `cargo test --release`, debug-mode timings are ~30× slower (~5 minutes for test (b)+test (a) combined); CI MUST run `--release` for the acceptance suite. Confirm by reading `.github/workflows/*.yml` — out of scope for this plan, but flag if not already configured.

(h) **Existing `save_load_step_preserves_determinism` left feature-gated.** That test (line 117 of `tests/acceptance.rs`) is gated to `#[cfg(not(feature = "threads"))]` and would benefit from the same dual-build treatment S39 applies to test (b). Out of scope for S39 per the user's instruction. Note this in DECISIONS.md or the next-pass triage so the gap is tracked.

---

## 10. Acceptance criteria

After S39 lands:

- `cargo test --release --test acceptance` passes (3 existing + test (b) = 4 default-feature acceptance tests pass; goldens unchanged from PR-3-pinned values).
- `cargo test --release --features threads --test acceptance` passes (1 existing + test (a) + test (b) = 3 threaded acceptance tests pass; test (b) compiles and runs in BOTH feature builds per §4).
- `cargo test --lib` and `cargo test --lib --features threads` both pass (test (c) included in both as a unit test in `src/world/nn.rs`).
- `cargo clippy --all-targets -- -D warnings` and `cargo clippy --all-targets --features threads -- -D warnings` clean.
- `cargo fmt -- --check` clean.
- §6 grep checklist: `grep -nE 'n\.div_ceil\(N_CHUNKS\)\.max\(1\)|n\.div_ceil\(crate::constants::N_CHUNKS\)\.max\(1\)' src/` returns exactly one hit (the body of `chunk_base_size`); `grep -n 'force_sequential_nn' src/snapshot_hash.rs src/world/save_v1.rs` returns zero hits.

No new acceptance-time files (no new golden files; existing two are reused).

CI workflow does not need editing — the existing `cargo test` and `cargo test --features threads` invocations pick up all three new tests automatically.

---

## 11. Out of scope (locked)

- **Do not redesign the acceptance harness.** The existing `EVOSIM_WRITE_GOLDEN=1` bootstrap pattern, golden-file format, and per-test asserts are preserved verbatim.
- **Do not modify `chunk_ranges`'s body.** Adding the `debug_assert!` inside the function is S10's responsibility. S39 promotes `chunk_ranges` to `pub(crate)` and adds the sibling `chunk_base_size` function next to it; it does NOT change `chunk_ranges`'s internal logic. The two pieces (S10 and S39) edit the same file but different lines; the cross-reviewer at PR-3 end confirms no conflict.
- **Do not assert specific hash values inline.** Test (b) uses `assert_eq!` between two live hashes (no golden involved); test (a) uses `assert_eq!` between two live hashes (no golden involved — see §3 rewrite). Only the existing `acceptance_t10000` and `acceptance_t10000_threaded` tests still read golden files.
- **Do not add `#[ignore]` markers.** All three tests are unconditional within their cfg gates (test (a) under `--features threads`; test (b) and (c) under all features).
- **Do not extend the test set beyond the three named tests.** test-gaps.md has many more candidates (P1's `nn_forward_threaded_matches_sequential_single_tick`, P2's mutation tests, the `snapshot_json` / `from_json` wasm round-trip, the existing `save_load_step_preserves_determinism` dual-build expansion, etc.); those are explicitly DEFERRED per `audit-triage.md` D28/D29 to a future test-hardening pass. The reviewer's flagged "snapshot_json round-trip" gap is noted in DECISIONS as a known gap, NOT folded into S39.
- **Do not un-gate `save_load_step_preserves_determinism`.** That existing test stays under `#[cfg(not(feature = "threads"))]` for this pass. S39's new test (b) provides the dual-build save/load assertion at four ns; the existing test's 1000-tick-then-step-500 scenario can be expanded in a future pass.

---

## Review feedback

**Verdict:** APPROVE WITH CHANGES — sound design, three real blocking issues plus several should-fix items the planner should resolve before sonnet implements.

### Blocking issues (must fix before implementation)

**B1 — Test (a) is a tautology against its own golden after regen ceremony.** The chosen design reads `tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt` and asserts the two golden values are equal. But both golden files are regenerated in the **same** §8 ceremony: sequential under default features, threaded under `--features threads`. If the regen produces equal goldens, test (a) merely re-asserts a property the regen ceremony already established — nothing in the *running* threaded code path is compared to the sequential one. The test catches a future drift (someone regens only one golden) but does NOT catch the case the §1 wording implies ("promotes equality to a hard invariant"); equality already IS an invariant of the regen step. The first `assert_eq!(live, thr_golden, ...)` is also redundant with `acceptance_t10000_threaded`'s existing identical assertion (`tests/acceptance.rs:228-231`). **Fix:** either (i) explicitly document in §3 that test (a) is a **divergence canary** for the regen ceremony itself (catches regen-only-one-file mistakes and post-PR-3 drift in only one of the two paths) and drop the live-vs-thr-golden assertion as duplicative; or (ii) actually run the sequential and threaded code paths in the same test process — the planner's "rejected alternative" is closer to the truth-statement the master plan intends, even if more invasive. The current §3 framing conflates "compare-to-golden" with "compare threaded-to-sequential" in a way that will confuse the reviewer when test (a) passes despite a real threaded/sequential drift (because both goldens regenerated together to the new drifted values).

**B2 — Cross-partition formula duplication is a known anti-pattern; the plan codifies it instead of fixing it.** §5 (vi) and §9 (c) both acknowledge that `vision.rs:74` and `src/world.rs:1253`'s `chunk_ranges` use the same `n.div_ceil(N_CHUNKS).max(1)` formula but live in two places, and that S10 will add a `debug_assert!` to one of them. Test (c) inlines the formula a THIRD time. This is the exact desync risk the test is supposed to prevent — yet the test itself participates in the duplication, so a future refactor that changes vision.rs's partition will (a) likely also be made consistent with the inline test formula by the same author, (b) leave a `chunk_ranges` (in `src/world/nn.rs`) that the test compares ONLY against its own inlined copy of vision's formula, not against vision.rs itself. Net: the test would pass while vision.rs and `chunk_ranges` actually disagree on disk. **Fix:** export a single `pub(crate) fn chunk_ranges(n: usize) -> [(usize, usize); N_CHUNKS]` from `src/world/nn.rs` (or `src/constants.rs`) and replace vision.rs's inline `par_chunks_mut(chunk_size)` with `par_chunks_mut(chunk_ranges(n)[0].1.max(1))` OR use the ranges directly to drive the parallel iterator. Then test (c) imports the one function. This is a small S10 scope expansion the S39 plan should explicitly request from the S10 planner, with an §6 path-coordination note. If S10's planner refuses, the S39 plan must downgrade its claim — "this test pins the formula at three sites manually" is honest, "the test guards against drift" is not.

**B3 — Test (b) `n=0` is structurally weak and the plan's §9(b) admits it.** The plan correctly identifies that at `n=0`, transient fields are empty on both sides and the test passes by construction (both hashes hash empty sets). This means the n=0 case provides no signal beyond "save/load doesn't panic on an empty world." Worse, the §9(b) risk note observes that the S7 planner is the actual gatekeeper for the field-list correctness ("test (b) is the test that catches violations") — but if every newly-hashed field is also empty at tick 0, n=0 misses it. The plan should either (a) drop n=0 (it dilutes the test signal and adds ~0 coverage); or (b) replace n=0 with a "tick 0 then add a single carrion via a debug seed knob" scenario; or (c) explicitly note n=0 is a no-panic smoke and the substantive cases are 1/200/2000. Currently §4's bullet list claims n=0 "exercises the empty/initial state code path through save/load" which is overstated. **Fix:** rewrite §4's n=0 coverage bullet as "smoke-only: confirms `from_save_v1(to_save_v1(World::new))` does not panic and produces a hash-equal world; substantive coverage comes from n>=1." Add explicit guidance that if n=0 fails, the bug is in `World::new` or `to_save_v1` itself, not in the hash field list.

### Should-fix issues (non-blocking but reviewer should weigh in)

**S1 — Test (b) under `#[cfg(not(feature = "threads"))]` actively hides threaded-load bugs.** The planner's rationale (§2 "Re test (b)") is "matches `save_load_step_preserves_determinism`'s pattern; uses `evosim::save::SaveV1` which is only imported in non-threaded builds today" — that is the bug, not the rationale. `SaveV1` is feature-flag-independent in the actual save.rs module; the cfg gate on the import in `tests/acceptance.rs:7-8` exists only because the existing F.26 test sits behind that gate. If `from_save_v1` has a threaded-only divergence (e.g. a future `pub(crate)` field that depends on rayon thread-pool init), test (b) under default features won't catch it. The save/load round-trip is a determinism invariant; it should hold under BOTH builds. **Fix:** make test (b) `#[cfg(all())]` (i.e. no cfg gate), move the `use evosim::save::SaveV1;` out from under its `#[cfg(not(feature = "threads"))]` gate at `tests/acceptance.rs:7-8`, and let it run twice in CI (once per feature flag). Test (b) does not need rayon to compile — the import is the only blocker. Cost: <1s extra CI per build; benefit: catches threaded-load drift the current plan misses.

**S2 — `cfg`-gate documentation gap for reviewers reading the test list.** §2's table shows test (a) under `#[cfg(feature = "threads")]`. A reviewer scanning just `tests/acceptance.rs` after S39 lands will see four `#[test]` functions under `#[cfg(not(feature = "threads"))]` and two under `#[cfg(feature = "threads")]`. There is no in-file comment explaining the asymmetry. **Fix:** require the implementer to add a module-level doc-comment block at the top of `tests/acceptance.rs` enumerating the test matrix (which test runs under which feature, and why). This is documentation churn the S39 plan should mandate, otherwise the BUILD-REPORT-style "test count by feature" line gets confusing.

**S3 — `n=2000` rebuild on every run is wasteful when test (b) runs in BOTH builds (if S1 above is adopted).** §9(d) estimates ~1.6s for n=2000 alone; running test (b) under both features doubles that. Consider parameterizing test (b) so n=2000 only runs under one feature (the threaded one, since it's the canonical perf path), with n=0/1/200 running under both. Plan should either accept the ~3s cost or document the asymmetry.

### Coverage gaps the plan acknowledges but does not patch

- **No round-trip test for `world.snapshot_json()` ↔ `WorldHandle::from_json`** (the §4 briefing in master plan §4 S39 mentions this API, but the actual test in §4 of the S39 plan uses `serde_json::to_string(&SaveV1::from_world(&w))` directly, bypassing the public wasm API surface that real load operations use). The JS load path is `from_json(snapshot_json())`, not `from_str(SaveV1::from_world())`. **Fix:** add a fourth test or extend test (b) to also exercise the `snapshot_json` / `from_json` path. The plan's §11 "do not extend beyond three tests" lock is too restrictive given this is the actual user-facing load path.
- **No test for `snapshot_hash` stability across endianness.** S8 changes the RNG hash to direct LE u64; the plan's §8 piggyback comment says "both native and wasm32 are LE so `write_u64` is fine" but no test pins this. Out of scope for S39, but the planner should note it for a future pass.
- **No test for `validate_save` rejecting crafted payloads with the new test (b).** S12 owns that test (per audit-master.md §4 S12), but the plan should cross-reference it explicitly so the cross-reviewer at PR-3 end doesn't ask "where's the rejection-path test?" and conclude S39 should have included it.

### PR-3 regen ceremony order — confirmed correct, with one nit

The plan's §7 sequencing (tests c/b before regen, test a after) is correct given test (a)'s dependency on the regen output. However, audit-master.md §8 (regen ceremony) does NOT mention that test (a) lands AFTER the regen — it describes a four-step ceremony ending at "step 4: confirm all 4 tests pass." After S39, that becomes "5 tests" (4 default + 2 threaded... wait, 4+2 = 6). The master plan §8 step 4 number is stale. **Fix request to master plan:** add a §8 step 5 — "If S39 test (a) lands in this ceremony, schedule it as the FINAL commit of PR-3, after the regen-and-verify pass." The current master plan does not coordinate this; an orchestrator following §8 literally will land test (a) before regen and watch CI fail. The S39 plan correctly identifies this in §7 but cannot fix the master plan from inside its own doc — should be raised as a cross-piece note.

### Final summary

Three blocking issues (B1: test (a)'s framing vs reality; B2: cross-partition formula stays duplicated despite the test's stated goal; B3: n=0 dilutes test (b) without disclosure). Three should-fix issues (S1: test (b) cfg gate hides threaded-load drift; S2: test-matrix docs missing; S3: n=2000 cost asymmetry). Two coverage gaps worth annotating (snapshot_json round-trip absent; cross-reference to S12's rejection-path test). One master plan coordination ask (§8 step ordering). All three tests are individually well-specified; the blocking work is conceptual clarity, not mechanics.

---

## Plan-update changelog (2026-05-24)

This revision addresses the three blocking issues and the most directly relevant should-fix items from the review above. Out-of-scope items are explicitly noted and not folded in.

### §1 Summary
- Renamed test (a) to `acceptance_threaded_actually_matches_sequential_t10000` to reflect the new design.
- Reframed test (a) as a runtime two-path comparison in one process, not a compare-to-golden.
- Removed the cfg gate description from test (b)'s bullet; it now explicitly runs under both feature sets.
- Replaced the "test (c) uses the same formula as vision.rs" wording with a description of the new source-of-truth extraction (`chunk_base_size` shared between NN and vision).

### §2 Test placements
- Test (b) row: cfg gate set to **none**, with rationale that save/load semantics must hold in both builds.
- Added a paragraph on the existing `save_load_step_preserves_determinism` being similarly under-tested but explicitly OUT OF SCOPE for S39.
- Test (c) row: updated to reflect that cross-partition equivalence now asserts via the shared `chunk_base_size` function rather than via re-inlined formula.

### §3 Test (a) detail — FULL REWRITE (B1 fix)
- New design: two `World`s in one process, one threaded NN, one with `force_sequential_nn = true`, assert hash equality.
- No golden file is consulted by test (a); the regen ceremony cannot make this test tautological.
- Documented the runtime toggle (`pub(crate) force_sequential_nn: bool`), its exclusion from `snapshot_hash` / `to_save_v1` / `from_save_v1`, and the rationale for choosing it over a test-only `nn_forward_sequential_for_test` method.
- Rejected three alternatives explicitly (compare-to-golden, num_threads(1) scoped pool, test-only method).
- Updated cost estimate: ~6-10 s under `--features threads`, well within budget.

### §4 Test (b) detail — partial rewrite (B3 + S1 fix)
- Dropped the `#[cfg(not(feature = "threads"))]` gate; documented the corresponding edit to `tests/acceptance.rs:7-8`.
- Rewrote the n=0 coverage bullet as smoke-only with explicit guidance on what an n=0 failure means.
- Added the dual-build cost estimate (~3 s combined for test (b) per CI run).
- Reorganized the "Interaction with..." subsection into two parts (S7 and S12), reflecting that S7 is the actual gatekeeper for the field-list invariant test (b) catches.

### §5 Test (c) detail — FULL REWRITE (B2 fix)
- Removed the inline `n.div_ceil(N_CHUNKS).max(1)` formula from the test body.
- Added the `chunk_base_size` extraction (one new function in `src/world/nn.rs`, one-line rewrite in `src/vision.rs:74`) as a scope add to S39 itself, with explicit justification.
- Test (c) now calls both `chunk_ranges` and `chunk_base_size` and reconstructs the partition from the shared base size; vision.rs transitively pins the invariant by calling the same function in production.
- Added invariant (vii): n=0 special-case pin for the `.max(1)` guard.

### §6 Path notes — FULL REWRITE
- Renamed to "Path notes and required exposures (preconditions for the implementer)".
- Enumerated every `pub(crate)` item the implementer must add: `force_sequential_nn` field on `World`, `chunk_base_size` function, `chunk_ranges` promotion to `pub(crate)`.
- Documented the `vision.rs:74` one-line rewrite.
- Documented the `tests/acceptance.rs:7-8` cfg-gate removal on the `SaveV1` import.
- Added a §6 cross-reviewer checklist with two grep commands.
- Moved the S1-sequencing note into a new §6b subsection.

### §7 Step order — full rewrite
- Reorganized into two commits: pre-regen (tests c and b + `force_sequential_nn` plumbing) and post-regen (test a only).
- Justified the split (clean PR-3 narrative; test (a) authored against the final hash function).
- Documented the lighter alternative (single S39 commit) and explained why it is not preferred.

### §9 Risk register — full rewrite
- (a) New: test (a) drift-detection design rationale.
- (b) New: `force_sequential_nn` field leakage risk + grep mitigation.
- (c) Updated: n=0 dilution explicitly acknowledged.
- (d) Updated: hash-field-vs-load-field invariant under dual builds.
- (e) Updated: S10 coupling, suggested non-colliding test names.
- (f) New: `chunk_base_size` extraction risk + verify-before-pin pattern.
- (g) Updated: cost under dual builds.
- (h) New: existing `save_load_step_preserves_determinism` flagged as out-of-scope gap.

### §10 Acceptance criteria
- Updated test counts under threaded build to reflect test (b) running in both builds (was 2 threaded tests, now 3).
- Added the §6 grep checklist to the acceptance criteria.

### §11 Out of scope
- Clarified that `chunk_ranges`'s body is not touched (S10 owns the `debug_assert!`), but S39 promotes it to `pub(crate)` and adds `chunk_base_size` next to it.
- Clarified that test (a) no longer reads any golden file.
- Added explicit DEFER notes for the snapshot_json round-trip and the `save_load_step_preserves_determinism` dual-build expansion (reviewer's coverage-gap callouts; correctly OUT of S39's scope per the user's instruction).

### Items deliberately NOT applied from the review
- **Reviewer S2 (test-matrix doc comment at the top of `tests/acceptance.rs`)**: low-value churn; the asymmetric cfg gates are explained in the plan and in DECISIONS. Not adding documentation churn for a 6-test file.
- **Reviewer S3 (n=2000 asymmetric under dual builds)**: keeping n=2000 in both feature builds. The ~3 s cost is acceptable; asymmetric coverage would create a different kind of confusion (why does only the threaded build run n=2000?).
- **Reviewer coverage-gap callouts (snapshot_json round-trip, validate_save rejection-path test)**: noted as out-of-scope per the user's "DO NOT remove the existing Review feedback section, but also don't expand scope" guidance. The §11 lock now mentions them explicitly so future planners see the deferral.
- **Reviewer master-plan §8 step-ordering nit**: cannot be fixed from inside this doc; raised to the orchestrator via the existing §7 note. The S39 plan is internally consistent regardless of whether the master plan is updated.

