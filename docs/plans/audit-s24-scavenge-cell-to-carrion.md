# S24 — `Action::Scavenge` uses `cell_to_carrion`

**PR:** PR-3 (determinism + correctness + regen)
**Effort:** S
**Depends on:** S1 (world split — implementer reads the post-split path)
**Determinism impact:** **verify-then-decide**. See §3.

---

## 1. Summary

Replace the linear scan over **all** `self.carrion` inside the `Action::Scavenge`
arm of `eat_and_scavenge` with a 3×3-cell sweep over the already-cached
`cell_to_carrion` index. This turns an `O(scavengers × |carrion|)` inner
loop into an `O(scavengers × ~9 cells)` loop, mirroring the cell-sweep that
`count_carrion_overlap` already uses (`src/world.rs:1170-1202`,
post-S1: `src/world/mod.rs`).

Same "first-match-wins" semantics: today the code does `break` on the first
carrion whose body-circle contains the scavenger position; the new code
also does `break` on the first matching carrion encountered in the 3×3 sweep.

**This piece keeps the OLD `Vec<Vec<u32>>` layout for `cell_to_carrion`.**
S26 in PR-4 changes that layout to CSR and includes a follow-up edit to
the reader site introduced here (see §4).

---

## 2. Determinism plan — the load-bearing question

Today: "first match wins over the full `self.carrion` vec, iterated in
insertion order."

New: "first match wins over carrion in 9 grid cells, iterated in a fixed
y-outer / x-inner cell order, and within each cell in the order stored in
that cell's inner Vec (which is itself insertion order, see §9 risk (d))."

These two orders **may differ** because:
- a creature's body circle (radius ≤ `BODY_RADIUS_PER_SIZE * size`, typically
  ≤ ~5u) covers only a small subset of carrion;
- the linear scan happens to hit them in `carrion.push` order;
- the cell-sweep hits them in `(cy-1..=cy+1, cx-1..=cx+1)` order, then
  cell-insertion order within each cell.

For two carrion both inside the scavenger's circle but in different cells,
the answer to "which one is eaten this tick" can change.

That change is observable in the golden hash. We resolve this empirically
(see §5 workflow): if the bootstrap acceptance hash is unchanged from
`0xb76e907c6221f7f5`, S24 ships in PR-3 alongside the S7+S8 regen with no
extra ceremony (the regen captures any drift anyway). If it changes, we
note the divergence in the DECISIONS line and let the same PR-3 regen
capture it.

Either way: **no separate regen ceremony for S24.** It rides PR-3's single
regen.

---

## 3. Determinism impact (one-line)

**verify-then-decide.** Worst case: rides PR-3's S7+S8 regen.

---

## 4. Layout coordination with S26

- PR-3 (this piece) reads `self.cell_to_carrion[cell_idx]` where
  `cell_to_carrion: Vec<Vec<u32>>` — the field shape used today.
- PR-4 S26 changes the field to `CarrionIndex { starts, indices }` CSR
  and is responsible for translating this reader site (and every other
  reader) to `&indices[starts[cell] .. starts[cell+1]]`. The S26 plan
  doc explicitly notes this touchpoint.

Implementer of S24 **must not** introduce a CSR layout here. Keep the
exact same access pattern as the existing `count_carrion_overlap`
helper at `src/world.rs:1170-1202` (post-S1: `src/world/mod.rs`).

---

## 5. Bootstrap-first workflow (mandatory ordering)

1. Land S1 (world split) and rebase onto the resulting `src/world/tick.rs`.
2. Implement the change as described in §6 below — code only, no regen.
3. Run, in this order, without any `EVOSIM_WRITE_GOLDEN*` env var:
   ```bash
   cargo fmt
   cargo clippy --all-targets -- -D warnings
   cargo test
   cargo test --release --test acceptance
   ```
4. Capture stdout from the acceptance run (specifically the hash line for
   `acceptance_t10000`). Compare to the pinned value
   `0xb76e907c6221f7f5`.
5. Report to the orchestrator:
   - **If hash matches** `0xb76e907c6221f7f5`: S24 lands in PR-3 standalone;
     the planned PR-3 regen for S7+S8 covers any byte sequence whether or
     not S24 contributes drift (it didn't this time).
   - **If hash differs**: report the new hex value. S24 is folded into the
     PR-3 regen batch and the DECISIONS line for v1.1 audit is amended to
     mention "S24 scavenge cell-sweep reorder" alongside S7/S8 attributions.
     Per §7 (e) of `audit-master.md`, divergence between sequential and
     threaded after PR-3 regen is acceptable as long as both files are
     deterministic against themselves.
6. Do **not** add an explicit regen step in this piece's commit chain.
   The S7+S8 commit pair handles all of PR-3's regen in one ceremony per
   `audit-master.md` §8.

---

## 6. Step-by-step implementation

### 6.1 Read the existing block

Open the post-S1 file `src/world/tick.rs` (pre-S1: `src/world.rs:710-731`).
Locate the `Action::Scavenge => { ... }` arm inside `eat_and_scavenge`.
Pre-S1 source for reference:

```rust
Action::Scavenge => {
    self.scratch_attempted_scavenge[i] = true;
    let scav_eff_i = self.creatures.g_scav_eff[i];
    if scav_eff_i <= 0.0 {
        continue;
    }
    let r_i = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
    let xi = self.creatures.x[i];
    let yi = self.creatures.y[i];
    let want = SCAVENGE_GAIN_COEFF * scav_eff_i;
    for c in &mut self.carrion {
        let dx = c.x - xi;
        let dy = c.y - yi;
        let d2 = dx * dx + dy * dy;
        if d2 <= r_i * r_i {
            let take = c.pool.min(want);
            c.pool -= take;
            self.scratch_gain[i] += take;
            break;
        }
    }
}
```

### 6.2 Replace with 3×3 cell sweep

Use **exactly** the cell-iteration pattern from `count_carrion_overlap`
(`src/world.rs:1179-1200`, post-S1: `src/world/mod.rs`):

```rust
Action::Scavenge => {
    self.scratch_attempted_scavenge[i] = true;
    let scav_eff_i = self.creatures.g_scav_eff[i];
    if scav_eff_i <= 0.0 {
        continue;
    }
    let r_i = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
    let xi = self.creatures.x[i];
    let yi = self.creatures.y[i];
    let want = SCAVENGE_GAIN_COEFF * scav_eff_i;
    let r2 = r_i * r_i;
    let cx = (xi / HASH_CELL).floor() as i32;
    let cy = (yi / HASH_CELL).floor() as i32;
    let dim = HASH_DIM as i32;
    'sweep: for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                continue;
            }
            let cell_idx = ny as usize * HASH_DIM + nx as usize;
            for &ci in &self.cell_to_carrion[cell_idx] {
                let c = &mut self.carrion[ci as usize];
                let ddx = c.x - xi;
                let ddy = c.y - yi;
                if ddx * ddx + ddy * ddy <= r2 {
                    let take = c.pool.min(want);
                    c.pool -= take;
                    self.scratch_gain[i] += take;
                    break 'sweep;
                }
            }
        }
    }
}
```

### 6.3 Verify imports

`src/world/tick.rs` must `use crate::constants::{HASH_DIM, HASH_CELL};`
(or whatever paths the S1 split lands on). If `HASH_CELL` / `HASH_DIM`
are not already in scope, add the import.

### 6.4 Iteration order (LOCKED — must not deviate)

```
for dy in -1, 0, 1:          # y-outer
    for dx in -1, 0, 1:      # x-inner
        for &ci in &self.cell_to_carrion[cell_idx]:   # stored order
            ...
```

Any other order — `for dx`-outer, reverse, randomized, sorted by distance —
breaks the "first match" semantics and silently changes the golden in a way
the bootstrap workflow may not catch (because it'd still be deterministic,
just *different*-deterministic). The reviewer must `diff` this block against
`count_carrion_overlap` to confirm identical loop structure.

### 6.5 Run the bootstrap acceptance test (§5).

### 6.6 Report hash status to orchestrator (§5).

---

## 7. Path notes

- Post-S1: `src/world/tick.rs` (inside `eat_and_scavenge`).
- `cell_to_carrion` field lives on `World` (post-S1: `src/world/mod.rs`),
  rebuilt once per tick before vision (see
  `World::run_vision_pass` / `build_cell_to_carrion` in
  `src/vision.rs:323-338`).
- Carrion accumulation timing: deaths add to `self.carrion` in step 9 of
  the tick (`collect_deaths` / `decay_carrion`). `eat_and_scavenge` runs
  in step 6, so the `cell_to_carrion` rebuilt at the start of this tick
  is correctly synchronized with `self.carrion` for the scavenge read —
  same invariant `count_carrion_overlap` already relies on.
- Constants `HASH_DIM` and `HASH_CELL` live in `src/constants.rs`.

---

## 8. Test plan

Add to a `#[cfg(test)] mod tests` block in `src/world/tick.rs` (or wherever
the world tick tests rehome under S1):

### 8.1 `scavenge_finds_carrion_in_neighbor_cell`

Construct a tiny `World` (`World::new` then `population = 1`-equivalent
hand-placement), push a single creature with `Action::Scavenge`-friendly
genome (`scav_eff > 0`, `size` large enough that `r_i > 0`), and place a
single `Carrion` whose `(x, y)` falls in `(cx+1, cy)` — i.e. the neighbor
cell to the east — but within the creature's body circle.

Call the tick boundary that drives `eat_and_scavenge` (either drive a
full `tick_once` after forcing `action_this_tick[0] = Action::Scavenge`,
or call `eat_and_scavenge` directly if it's `pub(crate)`). Assert:
- `self.scratch_gain[0] > 0.0` (the scavenge succeeded), OR
- `self.creatures.energy[0]` increased by approximately `want` after
  bookkeeping, AND
- the carrion's `pool` decreased by the same `take` amount.

### 8.2 `scavenge_returns_none_when_no_carrion_in_3x3`

Same setup, but place the single carrion in a cell at `(cx+5, cy+5)` —
far outside the 3×3 sweep. Drive the scavenge action. Assert:
- `self.scratch_gain[0] == 0.0`,
- the carrion's `pool` is unchanged.

This confirms (a) we don't fall back to the old full-vec scan, and (b) we
correctly *skip* carrion outside the 3×3 window even when they'd be within
`r_i` if we measured Euclidean distance ignoring the cell filter (use a
carrion whose distance < `r_i` but cell-distance > 1).

### 8.3 Existing scavenge tests

Run the existing suite (`cargo test`). Any pre-existing test exercising
the scavenge code path must still pass byte-identically (under the
"hash unchanged" branch) or under the regen'd golden (under the "hash
drifted" branch — captured by the S7+S8 regen ceremony).

---

## 9. Risk register

(a) **Iteration order divergence.** The single biggest risk. The 3×3
   sweep MUST be `for dy { for dx { for &ci in inner_vec { ... } } }`
   with `dy ∈ -1..=1` and `dx ∈ -1..=1` ascending. Any reordering —
   `dx`-outer, descending, distance-sorted — silently changes which
   carrion is the "first match" when two are in the body circle, which
   means the test bootstrap might still pass (any deterministic order
   yields *some* hash) but the golden lands on the wrong value. Reviewer
   must visually `diff` the loop structure against `count_carrion_overlap`.

(b) **Boundary cells.** When `cx == 0`, `cy == 0`, `cx == HASH_DIM-1`,
   or `cy == HASH_DIM-1`, some of the 9 cells are out of bounds. Handle
   identically to `count_carrion_overlap`: `if nx < 0 || ny < 0 || nx >= dim || ny >= dim { continue; }`.
   **Do NOT** use a clamp that *substitutes* the home cell for the missing
   neighbor — that would double-iterate the home cell and bias results.
   Also do not skip the entire sweep when on a wall — the home cell
   (`dx=0, dy=0`) is still in range and must be visited.

(c) **Home-cell-first semantics.** The home cell `(cx, cy)` is visited
   on the iteration `(dy=0, dx=0)`, which is the *fifth* of the nine
   sweep slots, not the first. This means a carrion in the home cell is
   NOT necessarily the first hit; if a carrion exists in the NW cell
   (`dy=-1, dx=-1`) AND in the home cell, NW wins under the new code.

   The existing `count_carrion_overlap` does the same NW-first ordering
   for its *counting* job (where order doesn't matter), so we're
   inheriting that same convention. The OLD scavenge code's "first match"
   semantics ordered by `carrion.push` order, not by spatial position,
   so this is the principal source of the determinism drift discussed
   in §2. Document and accept; the bootstrap workflow catches the drift.

(d) **Two carrion in the same cell.** `build_cell_to_carrion`
   (`src/vision.rs:323-338`) iterates `carrion.iter().enumerate()` in
   order and `push`es each `ci` into its cell's inner Vec. So the inner
   Vec is in insertion order = `carrion` vec order. For two carrion in
   the same cell, the new code therefore picks the lower-`ci` one —
   which matches the old code's "first match in `&mut self.carrion`
   loop". Within-cell, no drift.

(e) **CSR layout follow-up.** S26 in PR-4 changes
   `Vec<Vec<u32>>` to CSR. The S26 plan must update the reader site
   landed by this piece. If S24's reader site uses an idiom that S26
   doesn't anticipate (e.g. an extracted helper), S26's planner may
   miss it. Mitigation: this piece inlines the cell sweep directly
   in the `Action::Scavenge` arm — no helper extraction — so S26's
   grep for `self.cell_to_carrion[` will catch it.

(f) **`break 'sweep` label name.** The implementer's chosen label must
   not collide with any outer label in the surrounding function (none
   currently exists). Use `'sweep` consistently to keep the diff readable.

---

## 10. Acceptance criteria

1. Bootstrap acceptance hash status reported to orchestrator (§5).
2. Two new lib tests (§8.1, §8.2) pass under default and `--features threads`.
3. All existing tests in `src/world/tick.rs` (or wherever scavenge tests
   rehome under S1) pass either byte-identically (hash unchanged) or
   under the PR-3 regen'd golden (hash drifted — same S7+S8 regen
   ceremony captures it).
4. `cargo fmt -- --check` clean; `cargo clippy --all-targets -- -D warnings`
   clean (default and `--features threads`).
5. No new public API surface introduced. No CSR layout change.

---

## 11. Locked scope (do not expand)

- Do **not** change to CSR layout — that is S26 in PR-4.
- Do **not** change `Action::Scavenge` semantics beyond "first match in
  the 3×3 cell sweep instead of first match in the global carrion vec."
  In particular: do not switch to "best match by distance," do not add a
  tiebreaker, do not add randomness, do not change `r_i` derivation.
- Do **not** add a regen step inside this piece's commits. PR-3's one
  regen ceremony handles all hash drift.
- Do **not** add an `#[inline]` attribute to the loop body or extract
  a helper; the loop is small enough that LLVM handles inlining, and a
  helper would complicate S26's CSR rewrite.

---

## 12. Pattern reference

`World::count_carrion_overlap` at `src/world.rs:1170-1202` (post-S1:
`src/world/mod.rs`). Copy the cell-sweep skeleton verbatim; the only
differences are (a) we use `&mut self.carrion[ci as usize]` to mutate
`pool`, and (b) we `break 'sweep` on first match instead of counting.

## 13. Cross-reference

- `docs/plans/audit-master.md` §4 (S24 entry).
- `docs/plans/audit-master.md` §7 (d) — S24 hash-drift verification.
- `docs/plans/audit-master.md` §7 (h) — S26 CSR vs S24 reader coordination.
- `docs/plans/audit-master.md` §8 — golden-regen ceremony details.
- `docs/audit/perf-hot-loop.md` §4 — original sketch and impact estimate.
- `docs/plans/audit-triage.md` S24 — triage notes.

---

## Review feedback

**Verdict: APPROVE (no blocking issues).**

The plan is unusually careful for an S-piece, and the determinism reasoning is sound. Code-spot-checks confirm every claim in the plan.

### Spot-check confirmations

1. **`HASH_DIM` vs `SUN_DIM`** — confirmed `count_carrion_overlap`
   (`src/world.rs:1181`, `let dim = HASH_DIM as i32;`) uses `HASH_DIM`,
   not `SUN_DIM`. `build_cell_to_carrion` (`src/vision.rs:325`) sizes
   `dst` to `HASH_DIM * HASH_DIM`. The S24 plan's snippet uses
   `HASH_DIM` correctly throughout (§6.2 lines 162–171). The master
   plan's "SUN_DIM" wording was indeed a typo and the S24 planner
   caught it. Good.

2. **Boundary check exhaustive for all four edges.** The reference
   helper uses `if nx < 0 || ny < 0 || nx >= dim || ny >= dim`. The
   S24 plan's §6.2 snippet uses the identical predicate. This covers
   `cx==0`, `cy==0`, `cx==HASH_DIM-1`, `cy==HASH_DIM-1`, AND the
   diagonal corners — exhaustive. Risk register §9 (b) explicitly
   forbids the "clamp-substitute" anti-pattern, which is the right
   call.

3. **Within-cell determinism.** `build_cell_to_carrion`
   (`src/vision.rs:332–336`) iterates `carrion.iter().enumerate()` and
   `push`es into the cell's inner Vec → insertion order. The §9 (d)
   note is correct: for two carrion in the *same* cell, the new code
   picks the lower-`ci`, which matches the old "first match in
   `&mut self.carrion`" → no within-cell drift. Only across-cell
   reorder is in play.

4. **Synchronization between `cell_to_carrion` and `self.carrion`.**
   §7 path notes correctly identify that `eat_and_scavenge` runs in
   tick step 6 while deaths/decay run in step 9, so the cached
   `cell_to_carrion` from this tick's vision rebuild is consistent
   with `self.carrion` at the scavenge read point. Verified by
   reading the existing scavenge block at `src/world.rs:710–731`
   which already reads `self.carrion` at this same point without
   regard to the cache — the cache was built earlier in the tick
   from the same vec, no insertions or deletions intervene.

### On the home-cell-first alternative

The plan picks NW-first (y-outer/x-inner ascending, matching
`count_carrion_overlap`). The alternative — visit `(cx, cy)` first,
then the 8 neighbors — would more faithfully preserve "carrion in
the same cell as me wins over carrion in a neighbor cell", which is
a closer analog to the old code's tendency to find geometrically
"local" carrion first (since carrion in distant cells are usually
also distant in `carrion.push` order via spatial co-location of
deaths).

**Recommendation: stay with NW-first (current plan).** Three reasons:

- The reviewer's job here is determinism preservation in *expectation
  of change*, not preservation of the *exact* pre-change semantics.
  PR-3 already regens, so any deterministic order is acceptable.
- Matching `count_carrion_overlap` line-for-line means a future
  maintainer can grep for the cell-sweep pattern and trust both call
  sites behave identically. A bespoke home-first sweep here would
  fork the convention for marginal benefit.
- S26 (CSR rewrite) has to touch both sites; uniform iteration order
  makes that mechanical. A second pattern would force S26 to think
  twice about which order to use in each spot.

The §9 (c) discussion documents this trade-off explicitly — good.

### On the S26 touchpoint visibility

§4 + §9 (e) + §11 ("do not extract a helper") guarantee S26's
planner can grep `self.cell_to_carrion[` and find both sites. Inline
is the right call. The §12 pattern reference even tells the S26
implementer exactly where the two reader sites live.

One small improvement: consider adding a one-line code comment at
the new scavenge sweep, e.g. `// Mirror of count_carrion_overlap's
3x3 sweep; S26 (CSR layout) updates both sites.`  This is a hint to
the implementer, not a blocker — the implementer is free to add it
or skip it.

### On the bootstrap recipe

§5 gives the exact command sequence (`cargo fmt`, `cargo clippy
--all-targets -- -D warnings`, `cargo test`, `cargo test --release
--test acceptance`) and the pinned hash to compare against
(`0xb76e907c6221f7f5`). Clear, complete, executable. The "no
`EVOSIM_WRITE_GOLDEN*` env var" caveat is the critical bit for a
bootstrap run — explicitly called out. Good.

One implicit gap: §5 should also tell the implementer to run the
acceptance test under **both** the default features and
`--features threads` (since PR-3's regen ceremony handles both
hashes, and the master plan §7 (e) explicitly notes
sequential-vs-threaded divergence is acceptable post-regen). The
current §5 only mentions the default-feature acceptance hash. Not
blocking — the orchestrator can decide from a single-feature
report — but worth a one-line clarification.

### On PR-3 placement

Master plan row PR-3 includes S24, and §7 (d) describes the
verify-then-decide branching. The S24 plan §5 implements that
branching correctly: either the hash is unchanged (S24 lands
standalone in PR-3) or it drifts (S24 is folded into the S7+S8
regen). PR-3 placement is correct.

### Non-blocking nits

- **N1 (minor):** §6.3 says "or whatever paths the S1 split lands
  on" for the import — fine, but S1's plan should pin the
  `crate::constants::{HASH_DIM, HASH_CELL}` path. Leave as-is; S1
  reviewer handles.
- **N2 (minor):** §8.2 test asks for a carrion whose "distance
  < `r_i` but cell-distance > 1." For default `HASH_CELL` and
  default `BODY_RADIUS_PER_SIZE`, this combination may be
  empty/impossible if `r_i < HASH_CELL` always holds. The
  implementer should verify the geometry is constructible before
  writing the test; if not, weaken the assertion to "carrion at
  `(cx+5, cy+5)` whose Euclidean distance is also > `r_i`" — still
  proves the cell-sweep is bounded, just not the specific
  "in-radius-but-out-of-cells" edge case. Non-blocking; flag for
  the implementer.
- **N3 (cosmetic):** §6.4's "iteration order LOCKED" callout is
  excellent. Consider also asserting in code:
  `debug_assert!(HASH_DIM >= 3);` so a future shrink of `HASH_DIM`
  doesn't quietly invalidate the boundary logic. Cosmetic.

### Severity summary

- **Blocking issues: 0**
- **Major (non-blocking): 0**
- **Minor / clarifications: 3** (N1, N2, N3 above + the §5 threads
  acceptance run note).

The plan is ready to ship. The determinism story is the strongest
part: the bootstrap-first workflow with a pinned reference hash
correctly contains the verify-then-decide risk, and the §9 risk
register correctly identifies iteration order as the load-bearing
invariant.
