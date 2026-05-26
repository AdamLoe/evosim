# S11 — Extract `count_carrion_overlap` and `compute_is_at_wall` from threaded NN inline

**PR:** PR-3 (determinism + correctness + regen)
**Depends on:** S1 (world split).
**Pairs with:** S23 (PR-4) which re-touches the threaded NN to switch from
`flat_map`+drain to `par_chunks_mut` direct write — S23 reuses the helpers
this piece extracts.
**Effort:** S (~30 min).
**Determinism impact:** **verify-then-decide** — refactor only; should be
byte-identical. Bootstrap-confirm before pinning. If hashes differ, the
implementation has accidentally altered math; bisect and fix; **do not regen**.
The PR-3 regen ceremony at end-of-pass will pin new bytes for S7+S8;
S11's byte-identity check happens BEFORE that ceremony.

---

## 1. Summary

The sequential NN path at `src/world.rs:354-355` (post-S1: `src/world/nn.rs`)
calls two private `impl World` helpers `count_carrion_overlap(&self, i)` and
`compute_is_at_wall(&self, i)` (at `src/world.rs:1174` / `:1208`). The threaded
NN path inlines an equivalent block of math at `src/world.rs:393-425`
(post-S1: same `src/world/nn.rs`). The two helpers carry a
`#[cfg_attr(feature = "threads", allow(dead_code))]` annotation that
admits the duplication: under `--features threads` the sequential block
is `#[cfg(not(feature = "threads"))]`-gated out, so the helpers are
genuinely dead under that build — hence the silencer.

This piece hoists the two helpers so **both call sites use the same code path**.
After the change, the `#[cfg_attr(... dead_code)]` annotations are removed:
the helpers are reachable from the threaded closure too, so they are no
longer dead under either build.

The motivation comes from `docs/audit/architecture.md` §D2 (and §F1, which
restates D2's risk: any new SoA field added to `build_nn_input` or the
inputs to `pick_action_d` must be wired into the threaded inline too, and
nothing but the dual-golden test catches it). Audit `correctness-bugs.md`
§C10 is adjacent (chunk_ranges vs par_chunks_mut count mismatch) but is
handled separately by S10.

---

## 2. Helper signatures (free fns, `pub(crate)`)

Read the existing private impls at `src/world.rs:1174-1218` (post-S1:
location TBD per S1's path translation table, but per the master plan §4
S1 module map both helpers move to `src/world/mod.rs` as `impl World` blocks;
this piece moves them OUT of `impl World` and into free `pub(crate)` fns at
top of `src/world/nn.rs` so the threaded closure can call them without
holding `&self`).

**Recommended signatures** (mirroring `ray_circle_hit` free-fn pattern at
`src/vision.rs:283`):

```rust
/// Count carrion blobs overlapping creature i's body circle, using the
/// cached per-cell carrion index. Identical math under sequential and
/// threaded NN paths (audit S11).
pub(crate) fn count_carrion_overlap(
    creatures: &crate::creature::CreatureSoA,
    carrion: &[crate::carrion::Carrion],
    cell_to_carrion: &[Vec<u32>],
    i: usize,
) -> u32 { /* body lifted from src/world.rs:1174-1202 */ }

/// Returns 1.0 if creature i is within WALL_THRESHOLD_PAD of any wall,
/// else 0.0. Uses creature radius (size * BODY_RADIUS_PER_SIZE) for
/// consistency with physics step. Identical under both NN paths (S11).
pub(crate) fn compute_is_at_wall(
    creatures: &crate::creature::CreatureSoA,
    i: usize,
) -> f32 { /* body lifted from src/world.rs:1208-1218 */ }
```

Rationale for free-fn (not `impl World` method):
- The threaded path borrows `&self.creatures` immutably and `&self.cell_to_carrion`
  immutably AND will later (S23) take `&mut self.creatures.vx[chunk]` style
  splits. A method on `&self` re-borrows the WHOLE `World`, which conflicts
  with the per-chunk mutable splits S23 needs. Free fns taking the precise
  borrow scope work cleanly inside `par_iter` / `par_chunks_mut` closures.
- Sequential path is unaffected: it just passes `&self.creatures`,
  `&self.carrion`, `&self.cell_to_carrion` instead of `self`.
- Pattern reference: `src/vision.rs:283` `ray_circle_hit` — free fn used
  by both sequential and parallel vision passes.

Note on `cell_to_carrion` type: S26 (PR-4) replaces `Vec<Vec<u32>>` with a
CSR `CarrionIndex { starts, indices }`. **S11 ships using the OLD
`&[Vec<u32>]` signature.** S26 will translate this signature when it
lands; the cross-piece note at §7(h) of `audit-master.md` covers it.

---

## 3. Call-site updates

### 3a. Sequential `nn_forward_all_chunks` block — `src/world.rs:347-371`

**Before:**
```rust
#[cfg(not(feature = "threads"))]
{
    for &(lo, hi) in ranges {
        let mut input_buf = [0.0f32; NN_INPUTS];
        let mut hidden_buf = [0.0f32; NN_HIDDEN];
        let mut output_buf = [0.0f32; NN_OUTPUTS];
        for i in lo..hi {
            let overlap = self.count_carrion_overlap(i);
            let is_at_wall = self.compute_is_at_wall(i);
            let (vx, vy, action) = pick_action_d(
                i, &mut input_buf, &mut hidden_buf, &mut output_buf,
                &self.creatures, &self.vision[i], overlap, is_at_wall,
            );
            self.creatures.vx[i] = vx;
            // ...
        }
    }
}
```

**After:**
```rust
#[cfg(not(feature = "threads"))]
{
    for &(lo, hi) in ranges {
        let mut input_buf = [0.0f32; NN_INPUTS];
        let mut hidden_buf = [0.0f32; NN_HIDDEN];
        let mut output_buf = [0.0f32; NN_OUTPUTS];
        for i in lo..hi {
            let overlap = count_carrion_overlap(
                &self.creatures, &self.carrion, &self.cell_to_carrion, i,
            );
            let is_at_wall = compute_is_at_wall(&self.creatures, i);
            let (vx, vy, action) = pick_action_d(
                i, &mut input_buf, &mut hidden_buf, &mut output_buf,
                &self.creatures, &self.vision[i], overlap, is_at_wall,
            );
            self.creatures.vx[i] = vx;
            // ...
        }
    }
}
```

### 3b. Threaded inline at `src/world.rs:391-435`

**Before** (inside the `flat_map` closure):
```rust
.map(|i| {
    // Inline carrion overlap + is_at_wall for the threaded path.
    let xi = creatures_ref.x[i];
    let yi = creatures_ref.y[i];
    let ri = creatures_ref.g_size[i] * BODY_RADIUS_PER_SIZE;
    let r2 = ri * ri;
    let cx_cell = (xi / HASH_CELL).floor() as i32;
    let cy_cell = (yi / HASH_CELL).floor() as i32;
    let dim = HASH_DIM as i32;
    let mut overlap = 0u32;
    for dy in -1i32..=1 { /* ... 22 LOC of inline ... */ }
    let near = xi.min(yi).min(WORLD_SIZE - xi).min(WORLD_SIZE - yi);
    let is_at_wall = if near < ri + WALL_THRESHOLD_PAD { 1.0f32 } else { 0.0f32 };
    pick_action_d(i, &mut input_buf, &mut hidden_buf, &mut output_buf,
                  creatures_ref, &vision_ref[i], overlap, is_at_wall)
})
```

**After:**
```rust
.map(|i| {
    let overlap = count_carrion_overlap(
        creatures_ref, carrion_ref, cell_to_carrion_ref, i,
    );
    let is_at_wall = compute_is_at_wall(creatures_ref, i);
    pick_action_d(i, &mut input_buf, &mut hidden_buf, &mut output_buf,
                  creatures_ref, &vision_ref[i], overlap, is_at_wall)
})
```

Note: `creatures_ref`, `carrion_ref`, `cell_to_carrion_ref` are already
captured by the closure at `src/world.rs:379-382`; no new borrows needed.

### 3c. Delete annotations

Remove both `#[cfg_attr(feature = "threads", allow(dead_code))]` lines at
the old `:1173` and `:1207`. Helpers are now free fns reachable under both
feature builds; the annotation is no longer needed and clippy under
`--all-targets --features threads -- -D warnings` would otherwise be
silent about future genuine dead-code in the same area.

---

## 4. Path notes (post-S1)

Per `docs/plans/audit-master.md` §4 S1, the module map is:

- Sequential `nn_forward_all_chunks` body, threaded closure body,
  `chunk_ranges`, `N_CHUNKS`, `build_nn_input`, `pick_action_d` all live
  in `src/world/nn.rs`.
- `count_carrion_overlap` / `compute_is_at_wall` currently live in
  `impl World` at `src/world.rs:1174-1218`. Per master §4 S1 they move to
  `src/world/mod.rs` as `impl World` blocks. **S11 takes them OUT of
  `impl World` and places them as free `pub(crate)` fns at the top of
  `src/world/nn.rs`.** Both call sites are also in `src/world/nn.rs`,
  so no cross-module imports change.
- The implementer MUST consult S1's published "path translation table"
  for the exact line numbers in the post-S1 tree.

If S1 has not yet landed when S11 is implemented (unlikely given PR
ordering — S1 is in PR-1, S11 in PR-3 — but document for safety): apply
the same edits to `src/world.rs` at the pre-split paths and the change
is still well-defined; only the file names differ.

---

## 5. Step-by-step implementation order

### (i) Confirm both code blocks compute identical math — load-bearing check

Read `src/world.rs:393-425` (threaded inline) and `src/world.rs:1174-1218`
(sequential helpers) side-by-side. The planner has already done this
equivalence table:

| Quantity | Sequential (`count_carrion_overlap`) | Threaded inline | Equal? |
|---|---|---|---|
| `xi, yi` | `self.creatures.x[i], .y[i]` | `creatures_ref.x[i], .y[i]` | yes |
| `ri` | `self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE` | `creatures_ref.g_size[i] * BODY_RADIUS_PER_SIZE` | yes |
| `r2` | `ri * ri` | `ri * ri` | yes |
| `cx`/`cx_cell` | `(xi / HASH_CELL).floor() as i32` | `(xi / HASH_CELL).floor() as i32` | yes |
| `cy`/`cy_cell` | `(yi / HASH_CELL).floor() as i32` | `(yi / HASH_CELL).floor() as i32` | yes |
| `dim` | `HASH_DIM as i32` | `HASH_DIM as i32` | yes |
| outer loop | `dy in -1..=1` | `dy in -1..=1` | yes |
| inner loop | `dx in -1..=1` | `dx in -1..=1` | yes |
| bounds check | `nx<0 \|\| ny<0 \|\| nx>=dim \|\| ny>=dim → continue` | same | yes |
| `cell_idx` | `ny as usize * HASH_DIM + nx as usize` | same | yes |
| carrion iter | `for &ci in &self.cell_to_carrion[cell_idx]` | `for &ci in &cell_to_carrion_ref[cell_idx]` | yes |
| `ddx, ddy` | `c.x - xi, c.y - yi` | same | yes |
| accept | `ddx*ddx + ddy*ddy <= r2` | same | yes |
| accumulator | `count += 1` then return `count` | `overlap += 1` then use `overlap` | yes |

| Quantity | Sequential (`compute_is_at_wall`) | Threaded inline | Equal? |
|---|---|---|---|
| `r`/`ri` | `g_size[i] * BODY_RADIUS_PER_SIZE` | `g_size[i] * BODY_RADIUS_PER_SIZE` | yes |
| `near` | `x.min(y).min(WORLD_SIZE-x).min(WORLD_SIZE-y)` | same with `xi, yi` | yes |
| threshold | `near < r + WALL_THRESHOLD_PAD` | `near < ri + WALL_THRESHOLD_PAD` | yes |
| return | `1.0` else `0.0` (f32 by inference, function returns `f32`) | `1.0f32` else `0.0f32` (typed) | yes — both end up `f32` |

**Verdict: identical math.** Refactor is expected to be byte-identical.

If any cell in this table is incorrect under post-S1 line shifts (the
implementer reads the code and finds a divergence), STOP and report to
orchestrator before continuing — that's a latent bug, not a refactor.

### (ii) Move helpers to top of `src/world/nn.rs`

- Cut the two `impl World` methods from `src/world/mod.rs` (post-S1
  location of the existing `:1174-1218` block).
- Paste as free `pub(crate) fn`s near the top of `src/world/nn.rs`,
  above `nn_forward_all_chunks`. Use the signatures in §2 above.
- Update doc comments minimally: replace "Used in the sequential
  (non-threads) path; threads path inlines equivalent logic" with
  "Used by both sequential and threaded NN paths (audit S11)."
- Imports: the helpers need `BODY_RADIUS_PER_SIZE`, `HASH_CELL`,
  `HASH_DIM`, `WORLD_SIZE`, `WALL_THRESHOLD_PAD`. Confirm `nn.rs`
  imports them; if not, add the imports.

### (iii) Update sequential call site

Replace `self.count_carrion_overlap(i)` and `self.compute_is_at_wall(i)`
with the free-fn calls (see §3a). Sequential block; trivial.

### (iv) Update threaded call site

Delete the 30+ inlined lines in the `.map(|i| { ... })` closure body
(`src/world.rs:393-425`); replace with two free-fn calls (see §3b).
Confirm the closure captures (`creatures_ref`, `carrion_ref`,
`cell_to_carrion_ref`) are still all used — they are, by the helpers.

### (v) Delete `#[cfg_attr(... dead_code)]` annotations

Remove both. Helpers are now genuinely reachable under both feature
builds.

### (vi) Verify

Run **in order**, both feature sets:

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features threads -- -D warnings
cargo test
cargo test --features threads
cargo test --release --test acceptance              # byte-identity check
cargo test --release --features threads --test acceptance  # threaded byte-identity check
```

Both acceptance hashes MUST still be `0xb76e907c6221f7f5` (the pinned
pre-PR-3 value). If either differs, see §7 risk register (a): the
implementation has altered math; bisect.

---

## 6. Test plan

Add tests to `src/world/nn.rs` `#[cfg(test)] mod tests` (or wherever the
existing `chunk_ranges_partition` / `build_nn_input` tests landed
per S1):

### Test 1: `helpers_match_legacy_inline_math`

A unit test that constructs a tiny World (5 creatures placed across the
map with at least one near a wall and one near a carrion blob),
manually computes the expected `count_carrion_overlap` and
`compute_is_at_wall` values by re-implementing the math in the test
body, and asserts the helper output equals the recomputation for each
creature. This pins the helper math against drift independent of which
NN path runs.

### Test 2: `sequential_and_threaded_use_same_helpers` (under `#[cfg(feature = "threads")]`)

```rust
#[cfg(feature = "threads")]
#[test]
fn sequential_and_threaded_use_same_helpers() {
    // Build a deterministic mini-world with 8 creatures and 3 carrion blobs.
    // Run tick_once() once; capture creatures.vx, .vy, .action_this_tick.
    // (Threaded path is the actually-running one under the feature flag.)
    // Then for each creature i, manually call count_carrion_overlap(...) and
    // compute_is_at_wall(...) and assert they return the same values used by
    // the NN path (i.e. assert the helpers' outputs are bit-identical to what
    // the threaded inline used to compute).
}
```

The cross-feature byte-identity is also enforced by:
- The PR-3 acceptance tests on both feature sets (unchanged).
- S39's new `acceptance_threaded_matches_sequential_t10000` (lands later in PR-3).

### Test 3: extend `build_nn_input_centered_founder` if present

If the existing test at `src/world.rs:1492` (post-S1: `src/world/nn.rs`)
exercises `build_nn_input` for the founder, no extension is strictly
needed; S11 doesn't change `build_nn_input`. Skip unless the implementer
wants extra coverage. Document the skip in the commit body.

---

## 7. Risk register

**(a) Math divergence between blocks.** The equivalence table in §5(i) is
the load-bearing check. If the implementer reads the code and the table
is wrong (e.g. one block uses `radius_for_overlap` derived differently),
the refactor cannot be a pure hoist — it would change behavior. STOP and
report. Indicator: any post-S11 golden bytes differ from
`0xb76e907c6221f7f5` (pre-PR-3-S7/S8 pinned value).

**(b) Borrow-checker on threaded path.** The free fns take
`&CreatureSoA, &[Carrion], &[Vec<u32>], usize`. Inside the existing
`flat_map` closure the captures `creatures_ref`, `carrion_ref`,
`cell_to_carrion_ref` are already `&` borrows, so the helper calls are
compatible. **S23** (PR-4) will rewrite the threaded path to
`par_chunks_mut` over `(vx, vy, action_this_tick)`; the helper still
takes only `&CreatureSoA` (not `&mut`), so it nests inside `par_chunks_mut`
without conflict. Validate by ensuring the helpers do NOT take any field
that S23 will need mutably (they don't — they only read `x, y, g_size`).

**(c) Removing `#[allow(dead_code)]` may reveal latent dead-code warnings.**
The annotation is narrow (`feature = "threads"` only). Once removed,
`cargo clippy --all-targets --features threads -- -D warnings` will
re-evaluate. If it surfaces other dead code in `world/nn.rs`, fix or
document — do not re-add the silencer.

**(d) `cell_to_carrion` layout change coming in S26.** S11 ships with
`&[Vec<u32>]`. S26 (PR-4) will change the helper signature to take
`&CarrionIndex` (CSR). This is a known follow-up; the cross-piece note
at audit-master §7(h) covers it. S11 does NOT pre-emptively use a CSR
shape — that would create work-in-progress code with no caller.

**(e) Ordering / iteration determinism.** Both blocks iterate
`for &ci in &cell_to_carrion[cell_idx]` in Vec push order; the carrion
index is built deterministically by `build_cell_to_carrion`
(`src/vision.rs:323`). No HashMap iteration involved. The helper
preserves this ordering trivially.

---

## 8. Pair with S23 (PR-4)

S23 replaces the threaded NN's `flat_map → Vec → drain` with
`par_chunks_mut` direct writes into `(vx, vy, action_this_tick)` slices.
S23 needs:

- The threaded chunk closure to be CALLABLE from inside a
  `par_chunks_mut` of `&mut CreatureSoA` slices.
- The helpers `count_carrion_overlap` and `compute_is_at_wall` to take
  only **immutable** borrows of `creatures`, `carrion`, `cell_to_carrion`
  so they nest cleanly inside the per-chunk `&mut [f32]` borrows.

S11 leaves the code in exactly that shape: helpers take `&CreatureSoA`
(immutable); the threaded closure now consists of two helper calls plus
`pick_action_d` (also immutable on `creatures_ref`). When S23 lands, it
restructures the outer `par_iter().flat_map(...).collect()` but the
inner closure body (the helper calls) ports unchanged.

**Do not** preempt S23 in this piece. Leave the threaded site using
`par_iter().flat_map(...).collect()` exactly as today, with just the
math inside the closure replaced by helper calls. The S23 planner will
do the outer-loop rewrite separately. (Locked scope, per §11 of this
plan.)

---

## 9. Determinism impact — verify-then-decide

Refactor only; expected byte-identical. Bootstrap workflow:

1. Apply the change.
2. Run `cargo test --release --test acceptance` (default feature).
   Expected: PASS with golden `0xb76e907c6221f7f5`.
3. Run `cargo test --release --features threads --test acceptance`.
   Expected: PASS with golden `0xb76e907c6221f7f5`.
4. If both PASS → S11 is shipped clean; the PR-3 regen ceremony for
   S7+S8 happens at end of PR-3 unchanged.
5. If either hash differs → **the change has accidentally altered
   math**. STOP. Bisect by:
   (a) re-comparing the equivalence table in §5(i) against the actual
       diff; (b) reverting one helper at a time to localize the drift;
       (c) reporting the root cause to the orchestrator. **Do NOT
   regen.** A drift here indicates either the original duplication
   was non-equivalent (a latent bug the audit didn't catch — flag and
   discuss), or the hoist introduced a subtle change (e.g. f32 literal
   inference: `1.0` in fn returning `f32` vs `1.0f32` is identical, but
   re-confirm).

---

## 10. Acceptance criteria

- Both NN call sites (sequential + threaded) call `count_carrion_overlap`
  and `compute_is_at_wall` as free `pub(crate)` fns; no inlined
  carrion-overlap or wall-clamp math remains in either site.
- The two `#[cfg_attr(feature = "threads", allow(dead_code))]` lines are
  deleted.
- `grep -nE 'count_carrion_overlap|compute_is_at_wall' src/world/` shows
  exactly: one definition each, two call sites each (sequential block,
  threaded closure). Reviewer §7(c) of master plan runs this grep.
- `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --features threads -- -D warnings` are all
  clean.
- `cargo test` and `cargo test --features threads` both pass.
- Both golden tests on both feature builds still produce
  `0xb76e907c6221f7f5` (verify-then-decide; any divergence is a bug,
  not a regen trigger — see §9).
- New test `helpers_match_legacy_inline_math` passes.
- New test `sequential_and_threaded_use_same_helpers` (gated
  `#[cfg(feature = "threads")]`) passes.
- Commit follows conventional format (`refactor: hoist NN helpers to free
  fns shared by sequential and threaded paths (audit S11)`).

---

## 11. Locked scope (do NOT do in this piece)

- **Do not** rewrite the threaded outer loop from `flat_map().collect()`
  to `par_chunks_mut`. That's S23 in PR-4.
- **Do not** change `cell_to_carrion` layout to CSR. That's S26 in PR-4.
- **Do not** add `#[inline]` to the new helpers; if perf benefit is
  desired, route through S33's inline-pass list (PR-1).
- **Do not** rename, retype, or add new fields to `count_carrion_overlap`
  or `compute_is_at_wall` (e.g. taking a custom `Radius` newtype).
  Keep the signatures minimal and call-site-friendly for S23/S26.
- **Do not** regenerate goldens. S7+S8 in PR-3 will trigger one regen at
  end of PR-3; S11 is byte-identity-checked BEFORE that.

---

## Review feedback

**Verdict: APPROVE WITH MINOR FIXES.** The plan is well-scoped, the equivalence
analysis is sound, the borrow strategy is correctly chosen for S23
compatibility, and the verify-then-decide stance is appropriate. The issues
below are presentation/precision nits that the implementer should address
before applying; none are blocking but #1, #2, #3 should be fixed in-plan
before handoff.

### Equivalence verification (independent)

I cross-read `src/world.rs:393-425` (threaded inline) and
`src/world.rs:1170-1218` (sequential helpers) line by line. Confirmed
identical:

- Field reads (`x`, `y`, `g_size`); identical `* BODY_RADIUS_PER_SIZE`
  multiplier (both bear the `perf-5: mirror` comment).
- Loop bounds `-1i32..=1` (inclusive both ends) on outer and inner; no
  off-by-one.
- Bounds check uses `>= dim` (exclusive upper) and `< 0` — identical.
- `cell_idx = ny as usize * HASH_DIM + nx as usize` — identical (row-major
  with `nx` as column, consistent with `build_cell_to_carrion`).
- Carrion iteration `for &ci in &[...][cell_idx]` — same Vec push order
  in both blocks (the underlying `cell_to_carrion: Vec<Vec<u32>>` is
  shared state).
- `ddx*ddx + ddy*ddy <= r2` — same inclusive boundary.
- `is_at_wall` uses `near < ri + WALL_THRESHOLD_PAD` — strict `<`,
  identical in both. f32 literal inference (`1.0`/`0.0` in a fn returning
  `f32`) yields the same `f32` values as the explicit `1.0f32` / `0.0f32`
  in the threaded block. **Verified equivalent.**

One micro-detail the plan should call out: the sequential helper uses
local names `cx`, `cy`, `count`; the threaded inline uses `cx_cell`,
`cy_cell`, `overlap`. The plan's §5(i) table notes this. Naming the
free fn's locals consistently with the doc'd existing pattern
(sequential's `cx`, `cy`, `count`) is the lowest-friction choice — but
either is fine since they're scoped locally.

### Numbered issues

**1. [MEDIUM] §3a "Before" code is wrong / misleading.** The "Before"
sketch shows `let overlap = self.count_carrion_overlap(i);` — but at
`src/world.rs:347-371` today there is no such call. The actual current
sequential block (`src/world.rs:354-371`) inlines the same pattern as
`self.count_carrion_overlap(i)` _conceptually_ but the planner's "Before"
already shows the helper-using form, which would suggest the refactor is
a no-op for the sequential path. Recheck: the current sequential block
already calls `self.count_carrion_overlap(i)` and `self.compute_is_at_wall(i)`
(per the `#[cfg_attr(... dead_code)]` annotation in §1 which only makes
sense if the sequential path uses them). If so, §3a's "Before" is
correct; the §3a "After" simply switches `self.foo(i)` → `foo(&self.creatures, ...)`.
The plan should explicitly state that the sequential change is purely
"method-call → free-fn-call", whereas the threaded change is "delete 30
LOC of inlined math, replace with two helper calls". Right now the two
edits are presented symmetrically and that obscures where the real
deletion happens.

**2. [LOW] §3b post-refactor unused captures.** After the threaded
closure body shrinks to two helper calls + `pick_action_d`, the
captures `creatures_ref`, `carrion_ref`, `cell_to_carrion_ref`, `vision_ref`
are still used. Confirmed. **However**, the constants imported at the
top of `nn.rs` post-S1 (`BODY_RADIUS_PER_SIZE`, `HASH_CELL`, `HASH_DIM`,
`WORLD_SIZE`, `WALL_THRESHOLD_PAD`) may become unused in `nn.rs` body
itself (used only inside the moved helpers). If the helpers live in the
same module the imports stay used; if S1's post-split routes them
differently (e.g. helpers in `world/mod.rs` constants imported there
already), no churn. The plan should add a step in §5(ii): "After moving,
run `cargo clippy --features threads` and fix any `unused_import`
warnings in `nn.rs` arising from the now-extracted math."

**3. [LOW] §6 Test 2 is described in prose only, not concrete.** The
test body comment says "manually call ... and assert they return the
same values used by the NN path" — but the NN path consumes the values
and discards them (only `vx`, `vy`, `action_this_tick` are written).
There is no way to assert "the helper output equals what the threaded
inline used" because the threaded inline no longer exists post-S11. The
test as proposed is actually just: "construct a world, call the helpers
directly, check against hand-computed values" — i.e. identical to
Test 1. Either:
  - Drop Test 2 (Test 1 already provides math coverage, and golden tests
    + S39's cross-mode test cover the call-site integration), or
  - Re-spec Test 2 as: "run `tick_once()` under `--features threads`,
    capture the resulting `vx/vy/action_this_tick`, then on a clone of
    the pre-tick world run `tick_once()` without `--features threads`
    (impossible from inside a single binary) ..." — which doesn't work,
    so the cross-mode comparison can only happen via S39's separate
    acceptance binary. Recommend: drop Test 2 and rely on Test 1 +
    S39's acceptance for coverage. Note this in the plan.

**4. [LOW] §5(vi) verify command sequence missing `cargo build --features threads`.**
A pure-`clippy` then `test` sequence will catch errors, but explicitly
adding `cargo build` (or `cargo check`) for both feature sets before
running tests gives a faster failure signal on the borrow-checker
issue (b) in §7. Add `cargo check --features threads` before the
`cargo clippy` line, or accept that `cargo clippy` covers `cargo check`.

**5. [LOW] §7(c) and §10 acceptance grep is too narrow.** The grep
pattern `'count_carrion_overlap|compute_is_at_wall'` will match both
the definitions and the call sites, giving 4 expected matches per
binary path. The plan §10 says "one definition each, two call sites
each" — but each call site is one line, and the definitions are one
line each (the `pub(crate) fn` signature). The implementer should
expect grep to return ~4-6 lines (definitions + sequential + threaded
+ any test references). The exact count depends on whether Test 1
references the helpers by name. Plan should clarify: "Expected grep
hit count: 2 definitions + 2 sequential call lines + 2 threaded call
lines + N test references."

**6. [INFO] §2 free-fn vs method choice is correct.** I verified
against S23 plan §1's `mem::take`-based borrow recipe
(`docs/plans/audit-s23-threaded-nn-par-chunks-mut.md:131-146`). S23
calls `mem::take` on `vx`, `vy`, `action_this_tick` BEFORE the
`par_chunks_mut`, then forms `creatures_ref = &self.creatures` for the
helper call sites. Since the helpers take `&CreatureSoA` (not
`&mut`), and only read `x`, `y`, `g_size` (none of which are
mem::take'd), the helpers nest cleanly inside the disjoint `&mut [..]`
chunk borrows. **A method on `&self`** would re-borrow the whole
World, including the already-mem::take'd-but-still-aliased
`self.creatures` fields, which is fine for `&self` but would conflict
with future `&mut self.creatures.*` operations in S23 (e.g. the
restore after the parallel block). Free-fn is the right call.

**7. [INFO] §7(c) "dead-code from removing the silencer" is sound.**
The `#[cfg_attr(feature = "threads", allow(dead_code))]` lines silence
ONLY `dead_code` for these two specific items. Removing them re-enables
the lint on these items, which post-S11 will not fire (both items are
called from both call sites). No collateral dead-code surfacing
expected. If the implementer sees new warnings, they should be
investigated — likely they would belong to other unrelated items in
the module and predate S11.

**8. [INFO] Determinism command sequence in §9.** The four-step bootstrap
workflow is exactly right. Suggest one addition: capture the hashes
from a baseline pre-edit run for both feature sets first (so the
implementer has the expected value cached locally), then apply the
edit, then re-run. The currently pinned value `0xb76e907c6221f7f5`
must match the actual head-of-PR-3 pre-S11 hash; if it doesn't (e.g.
because S4/S5/S6 in PR-3 land before S11 and shifted the hash without
regenerating), the plan's hash citation is stale. Add a step 0:
"Record current acceptance hash before applying S11 by running the
acceptance tests on HEAD; use that as the comparison target rather
than the literal `0xb76e907c6221f7f5`."

### Blocking issues: 0

All issues are LOW or MEDIUM and pertain to plan precision rather than
correctness. The hoist itself is mechanically sound and the
verify-then-decide stance protects against latent bugs.
