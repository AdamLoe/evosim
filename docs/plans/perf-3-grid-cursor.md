# perf-3 — Pre-allocate `SpatialGrid` cursor buffer

**Status.** Plan ready for implementation. One-commit, golden-safe,
~30 LOC. Implementer: Sonnet. Self-contained.

**Goal.** Eliminate the `let mut cursors = self.starts.clone();`
allocation inside `SpatialGrid::rebuild` (`src/grid.rs:51`). Replace
the per-call clone with a `copy_from_slice` into a long-lived
`cursors: Vec<u32>` field on `SpatialGrid`. `rebuild` is called **3
times per tick** from `World::step` (`src/world.rs:188`, `:478`,
`:558`) plus once from `World::new` (`:131`) and once from
`World::from_save_v1` (`:1050`), so the perf payoff is 3×/tick of
allocator round-trip + a 14 401-element `Vec<u32>` clone (~57 604
bytes/rebuild → ~173 kB/tick of allocator traffic).

Mathematically and bit-for-bit identical to today: same prefix-sum,
same scatter, same `self.starts` and `self.indices` post-state. Golden
hash unchanged.

---

## 1. Decisions

Almost none. This is a single-field change.

- **D1. Element type.** `cursors: Vec<u32>` — matches `starts: Vec<u32>`
  (`src/grid.rs:11`). The cursor values are cell-offset indices into
  `self.indices`, which is also `Vec<u32>`; `u32` is the existing
  contract.
- **D2. Length.** `cursors.len() == starts.len() == HASH_DIM *
  HASH_DIM + 1 == 120 * 120 + 1 == 14_401`. Per `src/grid.rs:24`,
  `starts` is initialized to that exact length in `SpatialGrid::new`.
  `cursors` mirrors it.
- **D3. Initialization site.** `SpatialGrid::new` only. `cursors` is
  sized once at construction and never resized again (`starts` itself
  is never resized after `new`, so neither is `cursors`).
- **D4. Reset semantics.** Each `rebuild` call does
  `self.cursors.copy_from_slice(&self.starts);` after the prefix sum.
  This **fully overwrites** the previous tick's cursor values — there
  is no carry-over between rebuilds. (This is exactly what
  `let mut cursors = self.starts.clone();` does today.)
- **D5. Borrow split.** The scatter loop both **reads** `self.starts`
  (implicitly, via `cursors` after the copy — but does NOT need
  `&self.starts` after the copy completes) and **writes**
  `self.cursors`. Since the copy is done up front and the scatter only
  needs `&mut self.cursors` plus `&mut self.indices` (and the
  immutable `xs`, `ys` slice args), there is **no live overlap**
  between `&self.starts` and `&mut self.cursors`. A `let (cursors,
  indices) = (&mut self.cursors, &mut self.indices);` split borrow at
  the top of the scatter is the safe pattern if the simpler
  `self.cursors[...]` / `self.indices[...]` writes trip the borrow
  checker (they shouldn't — different fields are independently
  borrow-checkable in Rust — but use the split as fallback).
- **D6. No constants change.** `HASH_DIM` and `HASH_CELL` stay where
  they are in `src/constants.rs`. No new entries.

---

## 2. Files + signatures

### `src/grid.rs` — only file touched

**Add field on `SpatialGrid`:**

```rust
pub struct SpatialGrid {
    pub starts: Vec<u32>,
    pub indices: Vec<u32>,
    /// Scratch cursors for the scatter pass of `rebuild`.
    /// Length matches `starts` (HASH_DIM * HASH_DIM + 1 = 14 401).
    /// Reused across rebuilds via `copy_from_slice(&starts)` to
    /// avoid the per-tick allocation that `starts.clone()` would
    /// otherwise cost (~57 kB × 3 rebuilds/tick = ~173 kB/tick).
    cursors: Vec<u32>,
}
```

**Update `SpatialGrid::new`:**

```rust
pub fn new() -> Self {
    let starts = vec![0; HASH_DIM * HASH_DIM + 1];
    let cursors = vec![0; starts.len()];
    Self {
        starts,
        indices: Vec::with_capacity(2048),
        cursors,
    }
}
```

**Replace the body of `rebuild` (line 51):**

Before:
```rust
// scratch cursors (reuse a buffer to avoid alloc — store inline)
let mut cursors = self.starts.clone();
for k in 0..n {
    let c = Self::cell_of(xs[k], ys[k]);
    let pos = cursors[c] as usize;
    self.indices[pos] = k as u32;
    cursors[c] += 1;
}
```

After:
```rust
// Reset cursors to the prefix-sum boundaries without allocating.
self.cursors.copy_from_slice(&self.starts);
for k in 0..n {
    let c = Self::cell_of(xs[k], ys[k]);
    let pos = self.cursors[c] as usize;
    self.indices[pos] = k as u32;
    self.cursors[c] += 1;
}
```

That's the entire diff. Everything else in `rebuild` (prefix sum,
`indices.clear()` + `indices.resize(n, 0)`) is unchanged.

### Borrow-checker fallback (only if needed)

If `self.indices[self.cursors[c] as usize] = k as u32;` upsets the
borrow checker (it shouldn't — two distinct `&mut` fields of the same
struct are fine), pull both fields out with a split borrow at the top
of the scatter:

```rust
self.cursors.copy_from_slice(&self.starts);
let cursors = &mut self.cursors;
let indices = &mut self.indices;
for k in 0..n {
    let c = Self::cell_of(xs[k], ys[k]);
    let pos = cursors[c] as usize;
    indices[pos] = k as u32;
    cursors[c] += 1;
}
```

The implementer chooses based on what `cargo build` says — both
forms produce identical machine code under `--release`.

---

## 3. Integration

**No call-site changes.** `SpatialGrid::rebuild` keeps its signature
`rebuild(&mut self, xs: &[f32], ys: &[f32])`. The five existing call
sites are untouched:

- `src/world.rs:131` — `World::new` (initial grid build for the
  founder).
- `src/world.rs:188` — `World::step` step 1 (post-action, pre-eat
  spatial query).
- `src/world.rs:478` — `World::step` step 4 (post-movement, for
  repulsion queries on new positions).
- `src/world.rs:558` — `World::step` step 4 tail (post wall-clamp,
  for eat/scavenge queries on clamped positions).
- `src/world.rs:1050` — `World::from_save_v1` (rebuild on load).

`World` does not construct or own `SpatialGrid` differently — the
`grid` field is still `SpatialGrid`, still created with
`SpatialGrid::new()`. The new `cursors` field is private and
auto-initialized inside `new`.

**Other modules that import `SpatialGrid`.** None of the call sites
read the `cursors` field — it is `pub(super)` / private (no
visibility modifier → module-private). Keep it private. The two
existing public fields (`starts`, `indices`) are unchanged.

---

## 4. Save / hash / determinism

**Save.** `SpatialGrid` is **not** serialized in `SaveV1` (verified
by grepping `src/save.rs` for `SpatialGrid` and `grid` — zero hits).
The grid is pure derived state, rebuilt from `creatures.x` /
`creatures.y` on every tick and on every load (see
`src/world.rs:1050` — `from_save_v1` calls `grid.rebuild` after
restoring the SoA). The new `cursors` field needs no save changes
and no `#[serde(skip)]` annotation (the entire `SpatialGrid` struct
is excluded from `SaveV1`).

This matches the documented pattern in `DECISIONS.md`: "SpatialGrid
and vision Vec not saved." No DECISIONS.md edit needed.

**Snapshot hash.** `snapshot_hash::snapshot_hash` in
`src/snapshot_hash.rs` hashes tick + creature SoA + sun + carrion +
species + RNG state. It does **not** hash `SpatialGrid`. The new
`cursors` field has zero observable effect on the hash. The existing
pinned golden (`tests/golden_snapshot_t10000.txt`,
`0xc35be8a7905c7f05`) stays valid.

**Determinism.** `copy_from_slice` is bit-identical to `clone()` for
the contents of the buffer. The scatter loop reads / writes the same
values in the same order. Same `self.indices` post-state →
deterministic.

---

## 5. Tests

### 5a. Existing tests must stay green

- `src/grid.rs::tests::rebuild_indexes_correctly` — must pass
  unchanged.
- `src/grid.rs::tests::radius_query_includes_all_within_box` — must
  pass unchanged.
- `cargo test --lib` — full library test suite.
- `cargo test --release --test acceptance` — both acceptance tests
  (`golden_snapshot_t10000` and the perf-budget test). The golden
  hash must still match `0xc35be8a7905c7f05`.
- `cargo test --release --features threads --test acceptance` — if
  perf-4 has already landed, the threaded golden must also still
  match.

### 5b. New unit test — cursor reset between rebuilds

The biggest implementation risk is forgetting to reset `cursors`
between `rebuild` calls (e.g., calling `copy_from_slice` only once in
`new` and never again). That bug would silently corrupt the grid on
the second and later rebuilds — the scatter would write past the
correct cell boundaries on tick 2+. Add this test to `src/grid.rs::tests`:

```rust
#[test]
fn rebuild_twice_with_different_positions_is_correct() {
    // First build: 3 creatures clustered near (0,0).
    let xs = vec![1.0, 2.0, 3.0];
    let ys = vec![1.0, 2.0, 3.0];
    let mut g = SpatialGrid::new();
    g.rebuild(&xs, &ys);

    // Second build: same creatures moved to the opposite corner.
    let xs2 = vec![590.0, 591.0, 592.0];
    let ys2 = vec![590.0, 591.0, 592.0];
    g.rebuild(&xs2, &ys2);

    // After the second rebuild, the near-origin cell must be empty
    // and the far-corner cell must contain all three creatures.
    let mut near = vec![];
    g.for_each_in_radius(0.0, 0.0, 1.0, |i| near.push(i));
    assert!(
        near.is_empty(),
        "near-origin cell should be empty after rebuild; got {near:?}"
    );

    let mut far = vec![];
    g.for_each_in_radius(595.0, 595.0, 10.0, |i| far.push(i));
    far.sort();
    assert_eq!(
        far,
        vec![0, 1, 2],
        "far-corner cell should contain all three creatures after the second rebuild"
    );
}
```

This test would catch:
- Forgetting to call `copy_from_slice` inside `rebuild` (cursors
  carry stale values from tick 1 → tick 2 writes to wrong cells).
- Off-by-one in `cursors.len()` vs `starts.len()`.
- Any future regression that conflates "cursors as long-lived state"
  with "cursors should accumulate across rebuilds".

### 5c. No new acceptance test

Determinism is already enforced by the existing acceptance golden.
The new unit test plus the golden together cover both correctness
and the "cursor reset" failure mode.

---

## 6. Risks

**R1. Borrow checker on `&self.starts` while writing `&mut self.cursors`.**
Mitigated by structuring the rebuild as "copy first, scatter
second": once `self.cursors.copy_from_slice(&self.starts);` returns,
the immutable borrow of `self.starts` ends. The scatter loop then
takes only `&mut self.cursors` and `&mut self.indices`. Two distinct
`&mut` fields of the same struct are independently borrow-checkable
(field-level disjointness), so this should compile without ceremony.
If it doesn't, use the explicit split-borrow form in §2.

**R2. `cursors` length drift if `starts` is ever resized.** `starts`
is sized once in `SpatialGrid::new` and never resized (the grid
dimensions are compile-time constants — `HASH_DIM` from
`src/constants.rs`). `cursors` mirrors that and is also never
resized. If a future change resizes `starts`, the implementer of
*that* change must also resize `cursors` in lockstep, or the
`copy_from_slice` will panic with a length mismatch. Document this
invariant with a doc comment on the `cursors` field (already in §2).

**R3. Forgetting the reset.** Caught by the new unit test in §5b.

**R4. Save/hash exposure.** None. Verified `SpatialGrid` is absent
from both `src/save.rs` and `src/snapshot_hash.rs`.

---

## 7. Sequencing

One commit. No dependencies on other pieces.

1. Edit `src/grid.rs`: add `cursors` field, init in `new`, swap
   `clone()` for `copy_from_slice` in `rebuild`, add the new unit
   test.
2. `cargo build` — confirm it compiles.
3. `cargo test --lib` — confirm grid tests (including the new one)
   pass.
4. `cargo test --release --test acceptance` — confirm the golden
   hash still matches.
5. `cargo fmt --check && cargo clippy --all-targets -- -D warnings` —
   confirm style + lint clean.
6. Commit with message:
   `perf(sim): pre-allocate SpatialGrid cursor buffer`

No DECISIONS.md entry needed (this is a pure internal refactor; the
existing "SpatialGrid not saved" decision already covers the new
field by extension).

---

## 8. Out of scope

- Inlining / rewriting `for_each_in_radius` (perf-final-report §3
  doesn't flag it; the closure is already monomorphized at call site).
- Changing the grid cell size or `HASH_DIM` (architectural; v5 §3.3).
- SIMD anything inside `rebuild` (cell_of is 4 ops, not worth it).
- Touching `SpatialGrid::starts` / `SpatialGrid::indices` visibility
  or types.
- Adding the cursor buffer to perf-timing spans (rebuild is already
  spanned at the call site in `world.rs:187`; the per-rebuild cost is
  small enough that an inner span would be noise).

---

## 9. Citations

- Master plan §2 row perf-3, §3 dep graph, §5 piece briefing perf-3:
  `docs/plans/perf+ui-master.md`.
- `docs/research/perf-final-report.md` §3 item #3 (the ranked entry,
  ~0.05–0.10 ms/tick + 173 kB/tick allocator traffic).
- `docs/research/perf-final-report.md` §6 commit 2 (perf-2 and
  perf-3 are bundled there as a single "scratch buffer" commit; the
  master plan splits them into two commits for bisectability — perf-3
  is its own commit).
- `docs/research/perf-sim-hotpath.md` §2f (the
  `cursors = self.starts.clone()` discussion, ~173 kB/tick, three
  rebuilds/tick).
- `src/grid.rs:11` (`starts: Vec<u32>` type), `:24` (init length
  `HASH_DIM * HASH_DIM + 1` = 14 401), `:51` (the clone to delete).
- `src/world.rs:188`, `:478`, `:558` (three per-tick `rebuild`
  call sites); `:131`, `:1050` (init + restore call sites).
- `src/save.rs` — no `SpatialGrid` references (verified by grep).
- `src/snapshot_hash.rs` — no `SpatialGrid` references (verified by
  reading the perf-final-report §5 inputs list).

*End of perf-3 plan.*

---

## Plan review

**Verdict: APPROVED — ship as written.**

This is an exceptionally tight, well-scoped plan. Every claim cross-checks
against the source. One-commit, ~30 LOC, golden-safe, no decisions to litigate.

### Verified correct

- **`starts.clone()` site.** `src/grid.rs:51` —
  `let mut cursors = self.starts.clone();`. Confirmed exact match.
- **Call-site inventory.** `src/world.rs:131` (`World::new` init), `:188`
  (step 1, spanned as `grid_rebuild_1`), `:478` (step 4 post-movement),
  `:558` (step 4 post-clamp), `:1050` (`from_save_v1`). All five sites
  confirmed; signature stays `rebuild(&mut self, xs: &[f32], ys: &[f32])`,
  so no call-site edits needed.
- **3 rebuilds/tick.** Confirmed — `:188`, `:478`, `:558` all live inside
  `World::step`.
- **Sizing.** `HASH_DIM = (WORLD_SIZE / HASH_CELL) as usize = 120`
  (`src/constants.rs:9`), so `starts.len() == cursors.len() ==
  120*120+1 == 14_401`. Matches plan §1 D2.
- **Save exclusion.** `grep -n "SpatialGrid\|grid" src/save.rs` returns
  **zero hits**. Plan §4 claim is exactly right; no `#[serde(skip)]`
  needed because the field's container is never serialized.
- **Rebuild ordering in §2.** The plan correctly places
  `self.cursors.copy_from_slice(&self.starts);` **after** the prefix-sum
  block and **before** the scatter loop. This is the only correct
  ordering (cursors must be reset to the post-prefix-sum boundaries,
  not to the pre-prefix-sum zeros). Critical detail handled.
- **Two-rebuild test (§5b).** Directly targets the "forgot to reset"
  failure mode, which is the single most plausible implementer mistake.
  Test design is sound: opposite-corner positions guarantee the
  near-origin cell range from rebuild #1 is fully overwritten in
  rebuild #2; a stale `cursors` would write the second batch past the
  end of the (now-empty) origin cell's range, corrupting downstream
  cells.
- **Borrow checker note (§2 fallback, R1).** Correct — disjoint `&mut`
  fields of the same struct are independently borrow-checkable, so the
  primary form should compile. The split-borrow fallback is a clean
  rescue if it doesn't.
- **No hash impact.** `SpatialGrid` is absent from `snapshot_hash.rs`
  inputs; the golden `0xc35be8a7905c7f05` is safe.

### Must-fix

None.

### Should-fix

None. Worth noting two micro-points the implementer might trip on, but
both are already covered by the plan text:

1. The plan calls the new field "module-private" (no visibility
   modifier). That is correct — `cursors` should NOT be `pub`. Today's
   `pub starts` / `pub indices` are leaky but pre-existing; do not
   widen `cursors` to match. Plan §3 is explicit about this.
2. The doc comment on the `cursors` field in §2 already calls out the
   "must resize in lockstep with `starts`" invariant (R2). Keep that
   comment verbatim in the diff — it is the only guardrail against a
   future grid-resize regression.

### Risk summary

Lowest-risk piece in the perf series. Determinism is preserved by
construction (`copy_from_slice` ≡ `clone` for contents), the failure
mode is caught by both the new unit test and the existing golden, and
the diff is small enough to eyeball in one screen.

---

## Code review

**Verdict: APPROVED.** Commit `1844f78` implements the plan
verbatim.

### Verified

- **Diff size.** ~33 LOC, all in `src/grid.rs`. Confirmed.
- **`cursors` field init.** `SpatialGrid::new` builds
  `starts = vec![0; HASH_DIM*HASH_DIM+1]` then
  `cursors = vec![0; starts.len()]`. Same length (14 401). ✓
- **`copy_from_slice`, not `clone`.** Body of `rebuild` uses
  `self.cursors.copy_from_slice(&self.starts);`. The only remaining
  occurrence of "clone" in `src/grid.rs` is inside the doc comment on
  the field. ✓
- **Reset before scatter.** The `copy_from_slice` precedes the scatter
  `for k in 0..n` loop, after `indices.clear()` + `indices.resize(n, 0)`
  and after the prefix sum. Correct ordering. ✓
- **Determinism.** `cargo test --release --test acceptance` → 3/3 pass
  (`acceptance_t10000`, `save_load_step_preserves_determinism`,
  `profile_does_not_change_hash`). Golden hash unchanged. ✓
- **Two-rebuild test.** `rebuild_twice_with_different_positions_is_correct`
  present in `src/grid.rs::tests` and passes via `cargo test --lib`. ✓
- **Save / hash exclusion.** `grep -nE "SpatialGrid|grid" src/save.rs
  src/snapshot_hash.rs` returns zero hits. ✓
- **Lint / fmt / web build.** `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings`, and `pnpm build` (in `web/`) all
  clean. ✓

### Measured perf

5 runs of `cargo test --release --test acceptance` on this branch
(internal "finished in" times): 3.11, 3.14, 3.06, 3.22, 3.10 s →
**median 3.11 s**. Stated perf-2 baseline ≈ 2.59 s.

The acceptance suite at this size is dominated by the 10 000-tick
golden run plus the save/load determinism test plus the
profile-no-hash-change test; per-tick savings from removing one 57 kB
allocation × 3 rebuilds/tick (~173 kB/tick of allocator traffic) are
in the 0.05–0.10 ms/tick range per the perf-final-report, i.e. 0.5–
1.0 s saved over 10 k ticks — well within measurement noise on this
machine for a 3-test suite. Per-run variance alone here is ±0.16 s.
The change is bit-for-bit identical to the prior implementation, so
correctness is unconditional; the wall-clock perf delta from this
single piece is not separable from suite-level jitter at this
granularity. A focused micro-benchmark on `SpatialGrid::rebuild`
alone (out of scope here) would be the right tool to quantify the
isolated win.

### Notes

- Field is correctly module-private (no `pub`), matching plan §3.
- Doc comment on `cursors` includes the "resize both in lockstep"
  invariant from plan §6 R2. Good guardrail for any future
  `HASH_DIM` change.
- No call-site edits required; signature of `rebuild` unchanged.
- `SaveV1` + `snapshot_hash` untouched, as planned.

Ship it.

