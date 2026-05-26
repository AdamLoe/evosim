# S26 — `cell_to_carrion` → CSR layout

**PR:** PR-4 "per-tick perf followups".
**Depends on:** S1 (world split paths), S24 (introduces a new reader in PR-3 that this piece rewrites).
**Determinism impact:** none (same per-cell membership; intra-cell ordering preserved).
**Effort:** M.
**Golden regen:** none. Both `tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt` must remain at the PR-3-pinned values.

---

## 1. Summary

Today `cell_to_carrion` is a `Vec<Vec<u32>>` of length `HASH_DIM*HASH_DIM = 14_400`. Each of the 14,400 inner vecs carries a 24-byte header (ptr/len/cap) **even when empty** — ~345 KB of header memory, none of it in L1, plus a heap allocation per non-empty cell that is reused only across rebuilds, not freed when carrion churns.

This plan replaces that layout with a CSR (Compressed Sparse Row) shape mirroring `SpatialGrid` in `src/grid.rs`:

```
struct CarrionIndex {
    starts:  Vec<u32>,   // len = HASH_DIM*HASH_DIM + 1 (sentinel terminator)
    indices: Vec<u32>,   // len = carrion.len()
}
```

Per-cell carrion membership is **unchanged**. Iteration order within each cell is **preserved** (insertion order, i.e. by ascending carrion index). The build follows the exact 2-pass pattern in `src/grid.rs:47-67`: count pass, prefix sum, write pass with a `cursors` scratch.

Memory after: 1 heap buffer of `4 * (14_401)` bytes for `starts` (~56 KB, contiguous) and 1 heap buffer of `4 * carrion.len()` for `indices`. Cache-friendly reads; zero per-cell allocations.

---

## 2. Type design

Add to `src/vision.rs` (the file that owns the build today):

```rust
/// CSR-style per-cell carrion lookup. Replaces the legacy `Vec<Vec<u32>>`.
/// `starts.len() == HASH_DIM * HASH_DIM + 1`; the final entry is the
/// past-the-end sentinel (equal to `indices.len()`).
/// Mirrors `SpatialGrid` (`src/grid.rs`).
pub(crate) struct CarrionIndex {
    pub(crate) starts: Vec<u32>,
    pub(crate) indices: Vec<u32>,
    /// Scratch cursors for the scatter pass (same trick as perf-3 on SpatialGrid).
    /// Length always == starts.len(); reused via `copy_from_slice(&starts)`.
    cursors: Vec<u32>,
}

impl CarrionIndex {
    pub(crate) fn new() -> Self {
        let total = HASH_DIM * HASH_DIM;
        let starts = vec![0u32; total + 1];
        let cursors = vec![0u32; starts.len()];
        Self {
            starts,
            indices: Vec::with_capacity(256),
            cursors,
        }
    }

    /// Full rebuild from a flat carrion list. O(C + cells).
    pub(crate) fn rebuild(&mut self, carrion: &[Carrion]) { /* see §3 */ }

    /// Indices of carrion in cell `cell_idx`. Empty slice if none.
    #[inline]
    pub(crate) fn cell(&self, cell_idx: usize) -> &[u32] {
        let s = self.starts[cell_idx] as usize;
        let e = self.starts[cell_idx + 1] as usize;
        &self.indices[s..e]
    }
}

impl Default for CarrionIndex {
    fn default() -> Self { Self::new() }
}
```

Visibility: `pub(crate)` so `World` (in `src/world/mod.rs`) and the threaded NN reader in `src/world/nn.rs` can hold a `&CarrionIndex` and call `.cell(idx)`. The `starts`/`indices` fields stay `pub(crate)` to support the existing inline-iteration idiom in `vision.rs::raycast` and the threaded NN inline reader (matches how `SpatialGrid::{starts,indices}` are exposed to `vision.rs::raycast` today).

`VisionPass::cell_to_carrion` changes type:

```rust
pub struct VisionPass<'a> {
    pub creatures: &'a CreatureSoA,
    pub carrion: &'a [Carrion],
    pub grid: &'a SpatialGrid,
    pub cell_to_carrion: &'a CarrionIndex,   // was &'a Vec<Vec<u32>>
}
```

`build_cell_to_carrion(carrion: &[Carrion], dst: &mut Vec<Vec<u32>>)` is **deleted**; its work moves into `CarrionIndex::rebuild`.

---

## 3. Build algorithm (exact, mirrors `SpatialGrid::rebuild`)

`src/grid.rs:47-67` is the canonical reference. Mirror it byte-for-byte in structure:

```rust
pub(crate) fn rebuild(&mut self, carrion: &[Carrion]) {
    let total = HASH_DIM * HASH_DIM;
    debug_assert_eq!(self.starts.len(), total + 1);

    // (i) zero starts
    self.starts.iter_mut().for_each(|s| *s = 0);

    // (ii) count pass: starts[cell+1] += 1
    for c in carrion {
        let cell = SpatialGrid::cell_of(c.x, c.y);
        // cell is guaranteed < total by SpatialGrid::cell_of's clamp; mirror grid.rs (no extra bounds check).
        self.starts[cell + 1] += 1;
    }

    // (iii) prefix sum: convert counts to offsets
    for k in 1..self.starts.len() {
        self.starts[k] += self.starts[k - 1];
    }

    // (iv) write pass with cursors (no per-call alloc — perf-3 trick)
    self.indices.clear();
    self.indices.resize(carrion.len(), 0);
    self.cursors.copy_from_slice(&self.starts);
    for (ci, c) in carrion.iter().enumerate() {
        let cell = SpatialGrid::cell_of(c.x, c.y);
        let pos = self.cursors[cell] as usize;
        self.indices[pos] = ci as u32;
        self.cursors[cell] += 1;
    }
}
```

Key correctness invariants (assert with `debug_assert!`):

- After step (iii): `self.starts[total] == carrion.len() as u32`.
- After step (iv): for every `k`, `self.cursors[k] == self.starts[k + 1]` (i.e. every slot in every cell got filled).

Add both `debug_assert!`s at end of `rebuild`; they catch a future regression where `SpatialGrid::cell_of`'s clamp is dropped or carrion gains an off-grid coordinate.

**Insertion-order preservation:** the write pass iterates `carrion.iter().enumerate()` in ascending `ci` order. For each carrion, the cursor for its cell starts at `starts[cell]` and increments monotonically. Therefore the slot pattern within each cell is `[ci0, ci1, ci2, ...]` where `ci0 < ci1 < ci2` is the ascending-carrion-index subsequence that maps to that cell — **identical to** the old `dst[cell].push(ci as u32)` loop in `build_cell_to_carrion`. No determinism delta. (See §9 risk (a).)

---

## 4. Reader-site updates

There are exactly **three** read sites today, plus the field declaration and one builder call site, plus the **fourth** reader introduced by S24 in PR-3 which this piece must translate.

| # | File:line (today / pre-S1) | Post-S1 path | Old idiom | New idiom |
|---|---|---|---|---|
| 1 | `src/vision.rs:235` (`VisionPass::raycast` DDA loop) | unchanged (vision.rs is not split by S1) | `for &ci in &self.cell_to_carrion[cell_idx] { … }` | `for &ci in self.cell_to_carrion.cell(cell_idx) { … }` |
| 2 | `src/world.rs:410` (threaded inline NN, count_carrion_overlap equivalent) | `src/world/nn.rs` after S1 (the `#[cfg(feature="threads")]` branch of `nn_forward_all_chunks`) | `for &ci in &cell_to_carrion_ref[cell_idx] { … }` | `for &ci in cell_to_carrion_ref.cell(cell_idx) { … }` |
| 3 | `src/world.rs:1191` (sequential `count_carrion_overlap` helper) | After S11+S1: free fn in `src/world/nn.rs` (S11 hoists this helper out of `impl World`). Confirm exact post-S11 path before editing. | `for &ci in &self.cell_to_carrion[cell_idx] { … }` (or, after S11, `for &ci in &cell_to_carrion[cell_idx]`) | `for &ci in cell_to_carrion.cell(cell_idx) { … }` |
| 4 | **S24's new site** in `src/world/tick.rs` (`eat_and_scavenge`, `Action::Scavenge` arm; introduced by S24 in PR-3 with the OLD `Vec<Vec<u32>>` layout — see audit-master.md §4 S24 brief and §7 watchlist (h)) | `src/world/tick.rs` | S24 ships in PR-3 with `for &ci in &self.cell_to_carrion[cell_idx] { … }` inside a 3×3 sweep `for dy in -1i32..=1 { for dx in -1i32..=1 { … } }` | `for &ci in self.cell_to_carrion.cell(cell_idx) { … }` — preserve the y-outer, x-inner 3×3 sweep order exactly as S24 wrote it. |

Also update:

- **Builder call site:** `src/world/mod.rs::run_vision_pass` (today `src/world.rs:1229`):
  - Old: `build_cell_to_carrion(&self.carrion, &mut self.cell_to_carrion);`
  - New: `self.cell_to_carrion.rebuild(&self.carrion);`
- **Field declaration on `World`** (today `src/world.rs:94`, post-S1 `src/world/mod.rs`):
  - Old: `cell_to_carrion: Vec<Vec<u32>>,`
  - New: `cell_to_carrion: CarrionIndex,` (with `use crate::vision::CarrionIndex;` at the top of `world/mod.rs`).
- **Constructor initializers** (today `src/world.rs:175` in `World::new` and `:1154` in the `from_save_v1` shim):
  - Old: `cell_to_carrion: Vec::new(),`
  - New: `cell_to_carrion: CarrionIndex::new(),`
- **`use` import** at top of `src/world/mod.rs`: drop `build_cell_to_carrion` from the existing `use crate::vision::{build_cell_to_carrion, …};` import line; add `CarrionIndex` to that import (or add a fresh `use crate::vision::CarrionIndex;`).
- **Test helpers in `src/vision.rs` `mod tests`** (lines 355-359 `make_carrion_index`, and call sites at 388/412/443/494/536): update `make_carrion_index` to return `CarrionIndex` and the `VisionPass { cell_to_carrion: &cell_to_carrion, … }` field assignments stay the same (just the type changes).

Search command to verify completeness before commit:

```
grep -nE 'cell_to_carrion|build_cell_to_carrion' src/**/*.rs src/*.rs
```

After the change there must be **zero** matches of the form `Vec<Vec<u32>>` and **zero** matches of `build_cell_to_carrion`.

---

## 5. Path notes (S1 path-translation)

- `src/vision.rs` is **not** moved or split by S1. All vision.rs sites stay at the same paths.
- World-side reader paths use the post-S1 paths per audit-master.md §4 S1:
  - `World` struct + field declaration → `src/world/mod.rs`.
  - `nn_forward_all_chunks` (sequential and threaded branches) → `src/world/nn.rs`.
  - `count_carrion_overlap` → after S11 it is a free `pub(crate) fn` in `src/world/nn.rs` (S11 hoists both helpers out of `impl World`). After S11, both the sequential `build_nn_input` site and the threaded inline site call this single free fn — so this CSR change touches **one** helper instead of two paths.
  - `run_vision_pass` → `src/world/mod.rs`.
  - `eat_and_scavenge` (the function S24 adds its new reader inside) → `src/world/tick.rs`.

S11 lands in PR-3; S26 lands in PR-4. If S11 has correctly unified the sequential and threaded carrion-overlap paths into one helper, this plan only edits one carrion-overlap reader (not two). If for any reason S11 has NOT collapsed the threaded inline (reviewer-flagged regression per audit-master §7 (c)), the S26 implementer must update both sites — the reader-update list in §4 above already covers that case.

---

## 6. Step-by-step implementation order

Execute in this order; each step compiles and runs `cargo test`:

1. **Add `CarrionIndex` type + `rebuild` in `src/vision.rs`** (next to `build_cell_to_carrion`, do not yet delete the old fn). Add unit test `csr_carrion_membership_matches_old_layout` (see §8 (a)) that builds both layouts side-by-side and asserts identical per-cell membership and intra-cell ordering. Run `cargo test --lib vision::tests`.

2. **Replace the field on `World`** (`src/world/mod.rs`): change `cell_to_carrion: Vec<Vec<u32>>` to `cell_to_carrion: CarrionIndex`. Update the two constructor sites (`World::new` and the `from_save_v1` placeholder) to `CarrionIndex::new()`. Update the `VisionPass` struct in `src/vision.rs` so its `cell_to_carrion` field is `&'a CarrionIndex`. Code does not compile yet — proceed to step 3.

3. **Update every reader site listed in §4** in one commit. After this step `cargo build` is green.

4. **Update `run_vision_pass`** to call `self.cell_to_carrion.rebuild(&self.carrion)` instead of `build_cell_to_carrion(&self.carrion, &mut self.cell_to_carrion)`.

5. **Delete `build_cell_to_carrion`** from `src/vision.rs`. Update the `use` import in `src/world/mod.rs` to drop it. Update the test helper `make_carrion_index` in `src/vision.rs::tests` to construct a `CarrionIndex` via `CarrionIndex::new()` + `.rebuild()`.

6. **Run both acceptance feature sets:**
   ```
   cargo test --release --test acceptance
   cargo test --release --features threads --test acceptance
   ```
   Both must produce the PR-3-pinned golden hashes byte-for-byte. If they don't, the most likely culprit is an intra-cell ordering bug in the S24 reader translation (§9 (a)/(c)); diff against the old `Vec<Vec<u32>>` build via the `csr_carrion_membership_matches_old_layout` test.

7. **Clippy + fmt:**
   ```
   cargo fmt
   cargo clippy --all-targets -- -D warnings
   cargo clippy --all-targets --features threads -- -D warnings
   ```

8. **Commit** as one logical unit (suggested message: `perf(vision): pack cell_to_carrion into CSR layout`).

---

## 7. Determinism impact

**None.** Justification:

- Per-cell membership is determined solely by `SpatialGrid::cell_of(c.x, c.y)`, which is unchanged.
- Intra-cell ordering is preserved: the new write pass iterates carrion in ascending index order with a monotonic cursor per cell, producing the same `[ci0, ci1, ci2, …]` sequence per cell that the old `dst[cell].push(ci as u32)` loop produced (which also iterated `carrion.iter().enumerate()` in ascending `ci` order).
- All reader idioms iterate the per-cell slice in the same order as before (the new `.cell(idx)` returns a contiguous slice traversed via `for &ci in …`).
- The change touches neither the RNG, nor `snapshot_hash`, nor any field that flows into the hash.

Therefore both goldens (`tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt`) must remain at the PR-3-pinned values. This is the **acceptance test**: any drift indicates an implementation bug.

---

## 8. Test plan

Add to `src/vision.rs::tests`:

**(a) `csr_carrion_membership_matches_old_layout`** — construct a carrion list with ~50 carrion spread across ~20 cells (include duplicates per cell, include the empty-list case). Build both layouts side by side:

```rust
// Old layout (inline for the test only)
let mut old: Vec<Vec<u32>> = vec![Vec::new(); HASH_DIM * HASH_DIM];
for (ci, c) in carrion.iter().enumerate() {
    let cell = SpatialGrid::cell_of(c.x, c.y);
    old[cell].push(ci as u32);
}
// New layout
let mut new = CarrionIndex::new();
new.rebuild(&carrion);
// Assert per-cell equality of slices.
for cell in 0..HASH_DIM * HASH_DIM {
    assert_eq!(&old[cell][..], new.cell(cell), "cell {cell} membership/order differs");
}
```

**(b) `csr_iteration_order_is_insertion_order`** — three carrion at the same cell with IDs 7, 3, 11 pushed in that order into the carrion vec → `index.cell(cell)` must yield `[0, 1, 2]` (the carrion indices, not the IDs) in that order. Confirms the test explicitly that the cursor-based scatter preserves insertion order.

**(c) `csr_empty_and_full_corners`** — empty carrion list → all cells empty; one carrion → exactly one cell has membership `[0]`, every other cell empty; carrion at world corners `(0,0)` and `(WORLD_SIZE - 0.01, WORLD_SIZE - 0.01)` go into the correct corner cells.

**(d) Acceptance (existing, must remain green at the PR-3-pinned goldens):**

```
cargo test --release --test acceptance
cargo test --release --features threads --test acceptance
```

Both feature sets must report all tests pass with the same golden hashes pinned at the end of PR-3 (see audit-master.md §8). No regen.

---

## 9. Risk register

**(a) Intra-cell ordering regression.** *Highest risk.* The OLD `Vec<Vec<u32>>` layout pushes carrion onto each inner vec in ascending `ci` order (the build loop is `for (ci, c) in carrion.iter().enumerate() { dst[cell].push(ci as u32); }`). The new CSR write pass also iterates `carrion.iter().enumerate()` in ascending `ci` order and uses a monotonic cursor per cell — preserving the same intra-cell ordering. Test (a) above is the dedicated regression guard. If goldens drift in step 6 of §6, suspect this first.

**(b) Memory layout assumptions.** `starts` is always `HASH_DIM * HASH_DIM + 1 = 14_401` u32s (~56 KB, one contiguous heap buffer). `indices` is sized to `carrion.len()` (small — typically tens to low thousands). `cursors` is the same size as `starts`. Both are bounded and tiny relative to the SoA columns. No new allocation pressure; in fact a strict reduction (one 56 KB buffer replaces 14,400 Vec headers totalling 345 KB).

**(c) S24 reader translation is the most error-prone update.** S24 lands in PR-3 with the OLD `Vec<Vec<u32>>` layout inside a 3×3 cell sweep in `eat_and_scavenge`. The S26 implementer must:
  - Preserve the y-outer, x-inner cell-iteration order S24 used (`for dy in -1i32..=1 { for dx in -1i32..=1 { … } }`).
  - Preserve the `if nx < 0 || ny < 0 || nx >= dim || ny >= dim { continue; }` bounds check unchanged.
  - Translate **only** the inner line `for &ci in &self.cell_to_carrion[cell_idx]` to `for &ci in self.cell_to_carrion.cell(cell_idx)`. Do not refactor anything else inside `eat_and_scavenge` in this commit.
  - Preserve the existing `break 'outer` / `break` control flow from S24 verbatim (determinism of "first match" depends on it).

  This translation is mechanical but the cross-reviewer (audit-master §7 (h)) is explicitly asked to verify it.

**(d) Visibility creep.** `pub(crate)` on `starts`/`indices` exposes the buffers to any caller in the crate. This matches how `SpatialGrid` already exposes its `starts`/`indices` (and S35 in PR-1 tightened them from `pub` to `pub(crate)`). Don't widen to `pub` — wasm-bindgen does not need it.

**(e) Test helper churn.** `vision.rs::tests::make_carrion_index` is used by 5 tests (lines 388, 412, 443, 494, 536). Its return type changes from `Vec<Vec<u32>>` to `CarrionIndex`; the callers store the result and pass `&...` into `VisionPass { cell_to_carrion: &..., … }` — the field type change in `VisionPass` makes the helper change a pure mechanical rename.

**(f) `from_save_v1` placeholder.** The post-S1 `from_save_v1` in `src/world/save_v1.rs` initializes `cell_to_carrion: Vec::new()` (today line 1154). After this change it becomes `CarrionIndex::new()`. The first call to `run_vision_pass` will rebuild it fully, so initial state is unobservable; just don't leave `Vec::new()` and rely on type inference to do the wrong thing.

---

## 10. Acceptance criteria

A commit lands cleanly only if **all** of the following hold:

- [ ] Both goldens unchanged from the values pinned at the end of PR-3:
      - `cargo test --release --test acceptance` → pass.
      - `cargo test --release --features threads --test acceptance` → pass.
- [ ] Every reader site listed in §4 updated; in particular S24's `Action::Scavenge` reader in `src/world/tick.rs` is on the new layout.
- [ ] The old `cell_to_carrion: Vec<Vec<u32>>` field on `World` is gone; `grep -nE 'Vec<Vec<u32>>' src/` returns no carrion-related hit.
- [ ] `build_cell_to_carrion` is deleted; `grep -nE 'build_cell_to_carrion' src/` is empty.
- [ ] `cargo clippy --all-targets -- -D warnings` and the `--features threads` variant both clean.
- [ ] `cargo fmt -- --check` clean.
- [ ] The 3 new lib tests in §8 (a)(b)(c) pass.
- [ ] Allocation-count drop is observable. A simple check: before the change, `src/vision.rs::build_cell_to_carrion` calls `dst.resize_with(total, Vec::new)` once and then `cell.clear()` 14,400 times per tick (no allocs after the first tick, but 14,400 inner Vec headers permanently resident); after the change, `CarrionIndex::rebuild` allocates only on growth of `self.indices` (bounded by max carrion count, typically capacity-stable after the first few ticks) and zero per-cell. Document this in the commit message; no automated allocator counter is required.

---

## Locked scope (do NOT change in this commit)

- Spatial-cell membership rule (`SpatialGrid::cell_of`) — unchanged.
- Carrion lifecycle (spawn/decay/swap_remove) — unchanged.
- No incremental update — full rebuild per tick, mirroring the existing `SpatialGrid::rebuild` and `build_cell_to_carrion` cadence. If a future profile shows the rebuild itself is hot, that is a separate piece.

---

## Cross-piece note (for the orchestrator and the PR-4 cross-reviewer)

This is watchlist item (h) in audit-master.md §7. The cross-reviewer must:

1. `grep -nE 'cell_to_carrion' src/` post-merge and confirm every match is either the `CarrionIndex` field, a `.rebuild(...)` call, or a `.cell(...)` read — **no** `&self.cell_to_carrion[...]` indexing into a `Vec<Vec<u32>>` style remains.
2. Inspect the `eat_and_scavenge` reader (S24's site) and confirm the 3×3 sweep ordering and break-out behavior matches what S24 shipped in PR-3.
3. Re-run both acceptance feature sets and confirm the goldens are byte-identical to the PR-3-pinned values.

---

## Review feedback

**Verdict: APPROVE WITH NITS.** The plan is implementation-ready. Algorithm is correct, reader coverage is complete, S24 coordination is explicit, determinism reasoning is sound, and the memory accounting checks out.

**Independently verified against the repo:**

- `grep -nE 'cell_to_carrion|build_cell_to_carrion' src/` returns matches at: `vision.rs:42` (`VisionPass` field), `vision.rs:235` (raycast reader #1), `vision.rs:323` (builder definition), `vision.rs:357` (test-helper builder call), `vision.rs:388/412/443/494/536` (5 test call sites), `world.rs:19` (import), `:94` (field decl), `:175` (ctor in `World::new`), `:382` (threaded `cell_to_carrion_ref` alias), `:410` (threaded NN reader #2), `:1154` (`from_save_v1` ctor), `:1191` (sequential `count_carrion_overlap` reader #3), `:1229` (builder call), `:1234` (passed into `VisionPass`). Every site the plan enumerates is real and accounted for; nothing was missed. The S24 reader at `src/world/tick.rs` (PR-3) is correctly identified as reader #4.
- CSR algorithm hand-trace on (3 carrion, 2 cells, distribution A/B/A): yields `starts=[0,2,3]`, `indices=[0,2,1]` → `cell(0)=[0,2]`, `cell(1)=[1]`. Insertion-order preserved within cell A. Matches `SpatialGrid::rebuild` (`src/grid.rs:47-67`) byte-for-byte in structure.
- `cursors` scratch field: matches the documented perf-3 trick on `SpatialGrid` (`src/grid.rs:13-19`); avoids the per-rebuild `starts.clone()` allocation. Sound.
- `pub(crate)` visibility: `src/wasm_api.rs` does not touch `cell_to_carrion`. Verified via grep — no wasm boundary needs `pub`. S35 (PR-1) does plan to tighten `SpatialGrid::{starts,indices}` from `pub` to `pub(crate)` per `audit-master.md:286`, so the symmetry claim holds *after S35*.
- Memory accounting: `Vec<T>` header on 64-bit = 3 × 8 B = 24 B (ptr/len/cap). 14,400 × 24 = 345,600 B ≈ 345 KB. Claim is exact.
- Determinism: both goldens explicitly pinned at PR-3 values in §7 and §10.

**Nits (non-blocking):**

1. **Stale `pub` claim about `SpatialGrid` siblings.** §9 (d) says *"this matches how `SpatialGrid` already exposes its `starts`/`indices` (and S35 in PR-1 tightened them from `pub` to `pub(crate)`)."* On the current `main`, `src/grid.rs:11-12` still has them as `pub`. The "tightened" tense is fine because S35 lands in PR-1 (before PR-4), but if S35 slips, this piece's `pub(crate)` on `CarrionIndex::{starts,indices}` becomes inconsistent with neighboring code. Suggest adding one line: "If S35 has not landed by PR-4 start, file a blocker — do not widen `CarrionIndex` to `pub` to match the stale `SpatialGrid` surface." (~30 sec to add; not blocking.)

2. **§3 algorithm drops the `if cell < total` guard** that the old `build_cell_to_carrion` has at `src/vision.rs:334`. The plan justifies dropping it via `SpatialGrid::cell_of`'s clamp (`src/grid.rs:40-44`), which is correct — `cell_of` always returns `< HASH_DIM*HASH_DIM`. The plan's mention is brief ("cell is guaranteed < total by SpatialGrid::cell_of's clamp"); fine as written, but the `debug_assert_eq!(self.starts[total], carrion.len() as u32)` proposed at the end of `rebuild` is the actual safety net. Both belt-and-suspenders elements are present. No change needed.

3. **§4 table column header drift.** Row 3's "Old idiom" cell shows two alternatives separated by *"or, after S11"*; the implementer should know which to expect. Since the plan locks S11 to PR-3 (and §5 confirms it's a prerequisite by PR-4), the second alternative is the canonical one. Minor cleanup opportunity, not a defect.

4. **`carrion.len() == 0` edge case.** When `self.carrion` is empty (early game, mass extinction recovery), `indices.resize(0, 0)` is a no-op and the write loop runs zero times, leaving `starts` filled with zeros after prefix sum. `cell(idx)` returns `&indices[0..0]` for every cell. Correct, but not explicitly called out in §8 (c). The test plan's `(c)` covers "empty carrion list → all cells empty" — fine.

**Blocking issues: 0.**

**Summary (<80 words):** Plan is implementation-ready. CSR algorithm mirrors `SpatialGrid::rebuild` correctly; hand-traced on a 3-carrion/2-cell example with insertion order preserved. All 14 grep'd reader/builder/ctor/test sites in the live tree are accounted for, plus the S24-introduced reader in PR-3. Memory math (345 KB) is exact; `pub(crate)` is safe (no wasm boundary). Determinism rationale is rigorous; both goldens pinned at PR-3 values. Two cosmetic nits noted; nothing blocking.
