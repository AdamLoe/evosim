# perf-2 — Pool per-tick scratch Vecs onto `World`

> Detailed plan for the perf-2 piece of the v1.1 perf+UI pass. Reads as a
> self-contained brief for a Sonnet implementer. See `docs/plans/perf+ui-master.md`
> §5 "perf-2" and §8 Resolved Decision 3 for the master-plan context.

---

## 0. Status

- **Type.** Single golden-safe commit on the perf track.
- **Risk.** Low. One nontrivial implementation detail (split-borrow on the
  repulsion `neighbors` buffer — see §5).
- **Determinism.** Safe — values stored are identical to today; only the
  allocation lifecycle changes. Existing pinned golden
  `tests/golden_snapshot_t10000.txt` (`0xc35be8a7905c7f05`) must remain green.
- **Budget.** ~120 LOC across `src/world.rs` only. No new deps. No spec edits.
- **Commit message (suggested).** `perf(sim): pool per-tick scratch vecs on World`

---

## 1. Decisions (locked here so the implementer doesn't re-litigate)

1. **All nine Vecs land in this commit.** Per orchestrator Resolved Decision 3
   (master §8.3): `fx`, `fy`, `neighbors`, `damage`, `gain`, `cooldown_set`,
   `attempted_eat`, `attempted_scavenge`, `got_a_bite`. The `eat_and_scavenge`
   inner `candidates: Vec<usize>` (currently world.rs:635) is **out of scope**
   for this plan — it lives inside a per-creature match arm and the
   split-borrow it would need duplicates the `neighbors` complexity for a much
   smaller win. (Hotpath §2c estimates the eat path is taken by a fraction of
   creatures per tick.) Leaving it for a later perf pass keeps this commit
   focused.
2. **Naming convention.** Every promoted Vec gets a `scratch_` prefix on the
   `World` field. This satisfies the cross-reviewer's R4 check (master §6):
   no collision with the existing `vision` / `cell_to_carrion` /
   `pending_extinction_check` / `profile` / `events` / `sliders` / `peak_*` /
   `founder_*` fields. Field names: `scratch_fx`, `scratch_fy`,
   `scratch_neighbors`, `scratch_damage`, `scratch_gain`,
   `scratch_cooldown_set`, `scratch_attempted_eat`,
   `scratch_attempted_scavenge`, `scratch_got_a_bite`.
3. **No `MAX_POPULATION` constant added.** None exists today
   (`grep MAX_POPULATION src/` returns nothing; the existing capacity hint is
   `CreatureSoA::with_capacity(2048)` at world.rs:109). Pre-allocating the
   scratch Vecs to a fixed cap is **out of scope for this plan** — it belongs
   to `perf-final-report.md` §6 commit 4 (item #9) as its own piece. This
   plan uses `Vec::new()` at construction and lets `Vec::resize` grow the
   high-water mark naturally on the first tick. Existing peak-population
   workloads (acceptance test peaks at ~1 500 creatures) realloc at most a
   handful of times before the high-water settles. Compatible with the future
   pre-alloc commit — that work simply replaces `Vec::new()` with
   `Vec::with_capacity(MAX_POPULATION)`.
4. **Reset policy.** `fill(default)` for the four `Vec<bool>` flag arrays
   AND for the two `Vec<f32>` accumulators (`fx`, `fy`, `damage`, `gain`),
   because every consumer reads every slot post-loop and stale values from
   the previous tick would corrupt physics. `clear()` for the
   `scratch_neighbors: Vec<usize>` because it is a per-creature scratch that
   is overwritten by the next `for_each_in_radius` call; only `clear()` (not
   `fill`) makes semantic sense for the "fresh list each iteration" pattern.
5. **`shrink_to_fit` is forbidden.** The whole point is to keep the
   high-water-mark allocation. Implementer must not call `shrink_to_fit` or
   `truncate` on any scratch field.
6. **No `#[serde(skip)]` annotations needed on `World`.** `World` itself does
   NOT derive `Serialize`/`Deserialize`. Save/load round-trips through
   `SaveV1::from_world(&World)` and `World::from_save_v1(SaveV1)` — both are
   manual, field-by-field. New `World` fields are automatically excluded from
   the save just by not appearing in `SaveV1`. This mirrors how
   `vision: Vec<VisionBuf>`, `cell_to_carrion`, `pending_extinction_check`,
   and `profile` are already excluded today (master §3 confirms; see
   `src/world.rs:91–101` for the existing pattern, `src/save.rs:23–62` for
   the explicit `SaveV1` field list).
7. **Snapshot hash unchanged.** `snapshot_hash::snapshot_hash`
   (`src/snapshot_hash.rs:21–80`) hashes tick, creature SoA columns, sun,
   carrion, species list, and RNG only. It never touches `fx`/`fy`/eat
   accumulators. Promoting these to fields does not require any edit to
   `snapshot_hash.rs`. The implementer must NOT add the new fields to the
   hash function.

---

## 2. Inventory of Vecs to promote (verified against `src/world.rs`)

All line numbers below are read directly from the current source on `main`.
The implementer should re-confirm them with `grep -n 'vec!\|Vec::with_capacity' src/world.rs`
before editing (the line numbers may have drifted by a handful by the time
this plan is consumed).

| # | Current field | Type | Allocation site | Size | Reset rule | New `World` field |
|---|---|---|---|---|---|---|
| 1 | `fx` | `Vec<f32>` | `world.rs:480` — `let mut fx = vec![0.0_f32; n];` | `creatures.len()` | `fill(0.0)` (every slot summed into post-loop) | `scratch_fx` |
| 2 | `fy` | `Vec<f32>` | `world.rs:481` — `let mut fy = vec![0.0_f32; n];` | `creatures.len()` | `fill(0.0)` | `scratch_fy` |
| 3 | `neighbors` | `Vec<usize>` | `world.rs:487` — `let mut neighbors: Vec<usize> = Vec::with_capacity(16);` (inside the per-creature `for i in 0..n` loop) | up to ~16 typical (`for_each_in_radius` neighbors with `j > i`) | `clear()` at the top of each iteration | `scratch_neighbors` |
| 4 | `damage` | `Vec<f32>` | `world.rs:611` — `let mut damage = vec![0.0_f32; n];` | `creatures.len()` | `fill(0.0)` (every slot subtracted from energy post-loop, line 717) | `scratch_damage` |
| 5 | `gain` | `Vec<f32>` | `world.rs:612` — `let mut gain = vec![0.0_f32; n];` | `creatures.len()` | `fill(0.0)` (every slot added to energy post-loop, line 716) | `scratch_gain` |
| 6 | `cooldown_set` | `Vec<bool>` | `world.rs:613` — `let mut cooldown_set = vec![false; n];` | `creatures.len()` | `fill(false)` | `scratch_cooldown_set` |
| 7 | `attempted_eat` | `Vec<bool>` | `world.rs:614` — `let mut attempted_eat = vec![false; n];` | `creatures.len()` | `fill(false)` | `scratch_attempted_eat` |
| 8 | `attempted_scavenge` | `Vec<bool>` | `world.rs:615` — `let mut attempted_scavenge = vec![false; n];` | `creatures.len()` | `fill(false)` | `scratch_attempted_scavenge` |
| 9 | `got_a_bite` | `Vec<bool>` | `world.rs:616` — `let mut got_a_bite = vec![false; n];` | `creatures.len()` | `fill(false)` | `scratch_got_a_bite` |

**Other `vec!` / `Vec::new` / `Vec::with_capacity` calls confirmed NOT in scope:**

- `world.rs:635` `candidates: Vec<usize> = Vec::with_capacity(8)` inside
  `eat_and_scavenge` — Decision 1 above; out of scope.
- `world.rs:784–786` `dead: Vec<usize>` and `species_lost: Vec<u32>` in
  `collect_deaths` — bounded by per-tick death count (typically 0–50), low
  priority per hotpath §2e.
- `world.rs:1053` `vision: Vec<VisionBuf>` in `from_save_v1` — already
  pooled via the long-lived `World::vision` field; this is the restore
  initializer, not a per-tick alloc.
- `world.rs:139, 162, 163, 1106, 1107` — `carrion`, `cell_to_carrion`,
  `pending_extinction_check` initializers; already long-lived `World`
  fields.
- `world.rs:363` `let results: Vec<(f32, f32, Action)> = ranges...collect()`
  inside the `#[cfg(feature="threads")]` rayon block — that's the
  per-chunk collect, not pool-able in the same shape and out of scope here
  (perf-4 owns the threaded path).

---

## 3. Files and signatures

**Only `src/world.rs` is edited.** No new modules. No constants. No `Cargo.toml`
changes. No edits to `save.rs`, `snapshot_hash.rs`, `creature.rs`, `grid.rs`,
or any test file outside the new unit tests added below.

### 3a. `World` struct additions (after the `profile` field at line ~101)

Add a single comment block grouping the new fields, then nine field
declarations, e.g.:

```
// Per-tick scratch buffers, promoted from in-function `vec!` to long-lived
// fields to eliminate ~300 KB/tick allocator pressure (perf-final-report
// §3 item 2). Excluded from SaveV1 and from snapshot_hash by omission —
// mirrors the `vision` / `cell_to_carrion` / `profile` pattern.
scratch_fx: Vec<f32>,
scratch_fy: Vec<f32>,
scratch_neighbors: Vec<usize>,
scratch_damage: Vec<f32>,
scratch_gain: Vec<f32>,
scratch_cooldown_set: Vec<bool>,
scratch_attempted_eat: Vec<bool>,
scratch_attempted_scavenge: Vec<bool>,
scratch_got_a_bite: Vec<bool>,
```

Visibility: keep `pub(crate)` or private (default). They are implementation
details; nothing outside `world.rs` should read them.

### 3b. `World::new` initializer additions (in the `Self { ... }` block at lines ~132–165)

Each new field initialized to `Vec::new()`. The first tick's `resize` will
grow to `n` (which is 1 for the founder, then climbs as births fire).
Compatible with the future pre-alloc commit (perf-final-report §6 commit 4)
which would replace `Vec::new()` with `Vec::with_capacity(MAX_POPULATION)`.

### 3c. `World::from_save_v1` additions (in the `Ok(World { ... })` block at lines ~1076–1110)

Same as 3b — initialize each scratch field to `Vec::new()`. The first tick
after restore will resize to the loaded population. This matches the
pattern already used for `cell_to_carrion: Vec::new()` and
`pending_extinction_check: Vec::new()` at lines 1106–1107.

### 3d. `apply_movement_and_repulsion` edits (function spans lines ~427–559)

At the top of the repulsion phase, after the wall-time movement loop and
the `self.grid.rebuild(...)` at line 478:

```
// Reset/grow the force accumulators in place. resize() is O(1) when the
// capacity is already ≥ n (the common case after the first tick).
self.scratch_fx.resize(n, 0.0);
self.scratch_fy.resize(n, 0.0);
self.scratch_fx.fill(0.0);
self.scratch_fy.fill(0.0);
```

The `resize` + `fill` pair is intentional: `resize` only zero-fills the
**newly added** elements when growing, so an explicit `fill(0.0)` is
required to clear stale values in the existing range. On the common path
where `n` equals the high-water mark, `resize` is a single capacity check
(O(1) noop) and `fill` is the only real work — a single tight memset that
the allocator/codegen handles in a fraction of the cost of a fresh `vec![]`.

Then convert the `fx[i] += …` / `fy[i] += …` writes (lines 508–522, four
write sites in the contact branch + four in the co-located branch) to read
through `self.scratch_fx` and `self.scratch_fy`. **The implementer needs to
deal with the borrow checker here** — see §5 for the recipe. The clean-write
back-to-position loop at lines 528–556 reads `fx[i]` and `fy[i]`; those
reads become `self.scratch_fx[i]` and `self.scratch_fy[i]`.

### 3e. `eat_and_scavenge` edits (function spans lines ~606–719)

Replace the six `vec![...; n]` calls at lines 611–616 with six
`resize` + `fill` pairs:

```
self.scratch_damage.resize(n, 0.0);
self.scratch_gain.resize(n, 0.0);
self.scratch_cooldown_set.resize(n, false);
self.scratch_attempted_eat.resize(n, false);
self.scratch_attempted_scavenge.resize(n, false);
self.scratch_got_a_bite.resize(n, false);
self.scratch_damage.fill(0.0);
self.scratch_gain.fill(0.0);
self.scratch_cooldown_set.fill(false);
self.scratch_attempted_eat.fill(false);
self.scratch_attempted_scavenge.fill(false);
self.scratch_got_a_bite.fill(false);
```

(Same rationale as 3d for the `resize` + `fill` pair.)

Then convert every read/write of `damage[…]`, `gain[…]`, `cooldown_set[…]`,
`attempted_eat[…]`, `attempted_scavenge[…]`, `got_a_bite[…]` to the
`self.scratch_*` prefix. The borrow conflicts inside this function are
**already structurally avoided** today (the function clones
`self.creatures.genomes[i]` at line 625 / 671 specifically to release the
`&self.creatures` borrow before the inner index loop), so no extra
split-borrow gymnastics are needed beyond the `neighbors` case in §5.
Specifically: the writes at lines 663, 664, 665, 666 (inside `Action::Eat`)
and 686 (inside `Action::Scavenge`) and the post-loop reads at lines 695–718
all go to `&mut self.scratch_*[idx]` cleanly, because none of those lines
simultaneously borrow another `&mut self.creatures` field.

---

## 4. Lifecycle rules (the contract)

For each promoted Vec:

1. **Construction.** Initialized to `Vec::new()` in `World::new` and in
   `World::from_save_v1`.
2. **Per-tick resize.** At the top of every consuming function, call
   `resize(n, default)` where `n = self.creatures.len()` and `default` is
   the field's element default (`0.0` for `f32`, `false` for `bool`). This
   is O(1) when `capacity >= n`, which holds after the first tick at peak.
3. **Per-tick reset.** Immediately after `resize`, call `fill(default)`
   over the full length to zero-out stale values from the previous tick.
   (`resize` alone only writes the newly-added tail.) Exception:
   `scratch_neighbors` uses `clear()` at the top of each inner-loop
   iteration; see §5.
4. **No `shrink_to_fit` / `truncate`.** Implementer must NOT shrink these
   Vecs — the whole point is to keep the high-water-mark allocation. If
   `creatures.len()` drops sharply (mass extinction), the scratch Vec
   retains the larger capacity. That's fine; the cost is wasted bytes, not
   wasted cycles.
5. **No outside reads.** These fields are private to `world.rs`. The
   wasm-bindgen surface (`wasm_api.rs`) and tests do not reference them.

---

## 5. Split-borrow plan for `scratch_neighbors`

This is the only nontrivial change. The current code at lines 487–525 is:

```
let mut neighbors: Vec<usize> = Vec::with_capacity(16);
self.grid.for_each_in_radius(xi, yi, search, |j| {
    if j > i { neighbors.push(j); }
});
for j in neighbors { /* reads self.creatures.x/y/genomes, writes fx/fy */ }
```

If we naively rewrite the closure to push into `&mut self.scratch_neighbors`,
the closure borrows `self` mutably while `self.grid.for_each_in_radius` is
also borrowing `self.grid` (which lives on `self`) — Rust won't let us hold
`&mut self.scratch_neighbors` and `&self.grid` simultaneously through `self`.

**Recipe (use this — locked):** `std::mem::take` the field into a local for
the duration of the inner loop, then put it back. This is idiomatic Rust
for "I need to mutate one field while borrowing the rest of self."

```
// At the top of the per-creature for i in 0..n loop body, after the
// xi/yi/ri/search computations:
let mut neighbors = std::mem::take(&mut self.scratch_neighbors);
neighbors.clear();
self.grid.for_each_in_radius(xi, yi, search, |j| {
    if j > i { neighbors.push(j); }
});
for &j in &neighbors {
    // existing body: reads self.creatures.x/y/genomes,
    // writes self.scratch_fx/self.scratch_fy
    // (NOTE: writes to fx/fy are now &mut self.scratch_fx[i] etc;
    //  those don't conflict with the &mut self.scratch_neighbors
    //  because neighbors is currently a local, not a self field.)
}
self.scratch_neighbors = neighbors;
```

The `std::mem::take(&mut self.scratch_neighbors)` replaces the field with
`Vec::new()` (allocator-free — `Vec::new()` does not allocate) for the
duration of the inner work, then writes the populated Vec back at the end of
the iteration. The high-water-mark allocation travels with the local `neighbors`
binding and returns to the field intact.

**Panic safety:** if the closure passed to `for_each_in_radius` or the inner
`for &j in &neighbors` loop panics, `self.scratch_neighbors` is left as
`Vec::new()` (the high-water-mark allocation is lost). No drop guard is used —
losing the high-water-mark allocation on a panicking tick is acceptable, since
if the closure panics the World is poisoned anyway and no further ticks run.

**Why this preserves the perf win:** `std::mem::take` is a 24-byte
swap — pointer + len + cap — not an allocation. The actual heap buffer
never moves. Over the whole tick we do `n` swaps (each ~3 word moves) but
zero malloc/free round-trips. Compare to today's `n` × `Vec::with_capacity(16)`
+ `n` × drop. Net: ~96 kB heap churn per tick eliminated, per hotpath §2b.

**Alternative considered, rejected:** refactor the inner loop into a free
function `fn repel_pair(creatures: &mut CreatureSoA, scratch_fx: &mut [f32], …)`.
This works but moves ~80 lines around for a marginal readability gain. The
`mem::take` pattern is local, idiomatic, and matches what the master plan
called out explicitly (§5 perf-2 briefing). Use `mem::take`.

---

## 6. Integration points

### 6a. `SaveV1` (`src/save.rs:23–62`)

**No edits.** The promoted fields are scratch buffers that get reset at the
top of every tick; they hold no information across the save/load boundary.
By NOT adding them to `SaveV1`, we get free exclusion from both save
serialization and the round-trip equality tests
(`save.rs::tests::f26_round_trip_preserves_creature_state` at lines
293–354, and the dedicated determinism canary at lines 356–377).

Confirm by reading `src/save.rs:138–200` (the `SaveV1::from_world`
implementation) — it lists every captured field by name; if a field isn't
named there, it isn't saved.

### 6b. `snapshot_hash::snapshot_hash` (`src/snapshot_hash.rs:21–80`)

**No edits.** The hash function reads tick, creature SoA columns (id, x, y,
vx, vy, energy, age, genome, brain weights, nn_mutation_rate), sun,
carrion, species, RNG — and nothing else. Scratch buffers are by definition
zero (or stale-from-previous-tick) at the snapshot boundary, but they're
not in the hash either way. Implementer must NOT add the new fields to the
hash function.

### 6c. Acceptance test (`tests/acceptance.rs`)

**No edits.** The §16 golden test runs 10 000 ticks against pinned hash
`0xc35be8a7905c7f05`. Since scratch promotion is value-preserving (decision
above), the hash must remain identical after this commit. If it diverges,
the implementer has accidentally changed semantics — diagnose first; do not
re-bootstrap the golden.

### 6d. `wasm_api.rs`

**No edits.** The wasm-bindgen surface does not expose any of these fields.

### 6e. Profiler

**No interaction.** The `profile_span!` blocks in `apply_movement_and_repulsion`
and `eat_and_scavenge` (master §3 references the perf-timing widget) wrap
the existing function bodies. The promotion changes what's inside those
spans; the span structure itself is unchanged. Implementer should expect
the `movement_and_repulsion` and `eat_and_scavenge` rows in the profiler
table to drop slightly after this commit — that's the win.

---

## 7. Tests

Add three tests to the existing `#[cfg(test)] mod tests` block in
`src/world.rs` (around the file's existing test block; the implementer can
find an anchor with `grep -n '#\[cfg(test)\]' src/world.rs`).

### 7a. `scratch_fx_fy_zeroed_at_tick_start`

```
// pseudocode — implementer fleshes out
#[test]
fn scratch_fx_fy_zeroed_at_tick_start() {
    let mut w = World::new("perf2-zeroing");
    for _ in 0..5 { w.tick_once(); }
    // After 5 ticks, scratch_fx and scratch_fy should be sized to current
    // population. Run one more tick; immediately after movement+repulsion,
    // the post-write-back state of scratch_fx / scratch_fy is the residual
    // last-iteration value. We assert instead that on the NEXT tick, the
    // first thing apply_movement_and_repulsion does is zero them out.
    // Easiest test: write deliberate sentinel values, run one tick, assert
    // back to plausible repulsion-force range (|f| <= REPULSION_MAX).
    let n = w.creatures.len();
    // Pre-poison.
    for i in 0..n {
        w.scratch_fx[i] = 999.0;
        w.scratch_fy[i] = -999.0;
    }
    w.tick_once();
    // After a tick, every entry must be a real repulsion force in [-REPULSION_MAX, REPULSION_MAX]
    // (or zero if no neighbor contact). 999.0 sentinel must be gone.
    for i in 0..w.creatures.len() {
        assert!(w.scratch_fx[i].abs() <= REPULSION_MAX + 1e-3, "scratch_fx[{i}] not reset");
        assert!(w.scratch_fy[i].abs() <= REPULSION_MAX + 1e-3, "scratch_fy[{i}] not reset");
    }
}
```

**Note on indirection:** the sentinel-recovery assertion is indirect. A
`999.0` value that survived the `resize` + `fill` would teleport creatures
via the write-back loop (lines 528–556), triggering the wall clamp, rather
than necessarily leaving an out-of-range value in `scratch_fx` at tick end.
The test still catches the bug — a surviving sentinel corrupts creature
positions, and the force-range check fires because 999.0 is above
`REPULSION_MAX` — but implementers should not tighten the assertion to
something stricter (e.g., asserting the exact force value computed from
neighbor geometry). The current bound check is sufficient.

(Test requires the scratch fields to be `pub(crate)` or in the same module
to read directly. Implementer's call — either make them `pub(crate)` or
expose a `#[cfg(test)] fn scratch_fx(&self) -> &[f32]` getter.)

### 7b. `scratch_grows_with_population`

```
#[test]
fn scratch_grows_with_population() {
    let mut w = World::new("perf2-grow");
    let n0 = w.creatures.len();
    // Run until population grows past the initial scratch capacity. The
    // founder is 1; births should add creatures over the first ~500 ticks.
    for _ in 0..500 { w.tick_once(); }
    let n1 = w.creatures.len();
    assert!(n1 > n0, "test setup: population should have grown");
    // Scratch Vecs MUST be at least as long as the current population.
    assert!(w.scratch_fx.len() >= n1);
    assert!(w.scratch_fy.len() >= n1);
    assert!(w.scratch_damage.len() >= n1);
    assert!(w.scratch_gain.len() >= n1);
    assert!(w.scratch_cooldown_set.len() >= n1);
    assert!(w.scratch_attempted_eat.len() >= n1);
    assert!(w.scratch_attempted_scavenge.len() >= n1);
    assert!(w.scratch_got_a_bite.len() >= n1);
    // No panic across 500 ticks of growth confirms resize handles growth.
}
```

### 7c. Acceptance regression — already covered

`cargo test --release --test acceptance` must still pass against the
existing pinned golden. This is the single best regression catch for
"implementer accidentally changed semantics while pooling". No new test
needed; just confirm it stays green.

### 7d. Save round-trip — already covered

`save.rs::tests::f26_round_trip_preserves_creature_state` and
`f26_round_trip_preserves_rng` already verify that the post-restore world
ticks identically. Since the scratch fields aren't in `SaveV1`, the test
auto-confirms they're correctly reset on the post-load first tick. No new
test needed.

---

## 8. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | Split-borrow on `scratch_neighbors` fights the borrow checker. | Use the `std::mem::take` recipe in §5 verbatim. If the compiler still complains, the inner loop is reading from `self.creatures` via field access — that's fine; the take only released `self.scratch_neighbors`, not `self.creatures` or `self.grid`. |
| R2 | `clear()` vs `resize(n, default)` semantics confused. | Locked in §1 decision 4 and Table §2. The four `Vec<bool>` fields and the two `Vec<f32>` accumulator fields use `resize` + `fill` (every slot read post-loop). Only `scratch_neighbors` uses `clear()` (per-iteration scratch). |
| R3 | Field naming collides with an existing `World` field. | `scratch_` prefix is globally unique on `World` today (verified via `grep '^\s*pub\? *scratch_' src/world.rs` returning nothing). Master §6 R4 explicitly mandates this prefix. |
| R4 | New fields accidentally land in `SaveV1` or `snapshot_hash`. | They CAN'T land in `SaveV1` unless the implementer manually adds them to the struct AND to `SaveV1::from_world` AND to `from_save_v1` (three coordinated edits). The acceptance test catches the snapshot-hash case if the implementer mistakenly adds them to `snapshot_hash`. §6a/b above explicitly says "no edits"; the cross-reviewer should diff `save.rs` and `snapshot_hash.rs` against `main` and assert no changes. |
| R5 | `resize` zero-fills the tail but not the head, leaving stale values from a prior tick when population shrinks. | The explicit `fill(default)` after every `resize` covers this. Locked in §1 decision 4 and §4 rule 3. |
| R6 | Allocator-pool-vs-syscall hypothesis is wrong and the perf win doesn't materialize. | Acceptance test wall-time should drop a measurable amount (per perf-final-report §3 item 2: 0.05–0.15 ms/tick * 10 000 ticks = 0.5–1.5 s off the 2.31 s baseline at peak — but the average tick is much lighter than peak, so realistic savings are 0.1–0.4 s). If wall-time is unchanged, the commit still ships — the correctness is the gate, not the speed. Profiler validation belongs to a follow-on review, not this commit. |

---

## 9. Sequencing

1. Read `src/world.rs` and confirm the nine line numbers in §2 against the
   current source (they may have drifted ±5 lines from this plan).
2. Add the nine fields to the `World` struct.
3. Add the nine `Vec::new()` initializers to `World::new`.
4. Add the nine `Vec::new()` initializers to `World::from_save_v1`.
5. Rewrite `apply_movement_and_repulsion`:
   - Replace lines 480–481 with the `resize` + `fill` pair for
     `scratch_fx` / `scratch_fy`.
   - Apply the `std::mem::take` split-borrow recipe (§5) to
     `scratch_neighbors`.
   - Update the `fx[…]` / `fy[…]` index expressions to
     `self.scratch_fx[…]` / `self.scratch_fy[…]` in the contact-write
     branch (current lines 508–511, 519–522) and in the wall-clamp
     write-back loop (current lines 528–556).
6. Rewrite `eat_and_scavenge`:
   - Replace lines 611–616 with the six `resize` + `fill` pairs.
   - Update every `damage[…]` / `gain[…]` / `cooldown_set[…]` /
     `attempted_eat[…]` / `attempted_scavenge[…]` / `got_a_bite[…]` read
     and write to the `self.scratch_*` prefix.
7. `cargo build --release` — fix borrow-checker errors per §5.
8. `cargo test --lib` — confirm unit tests pass.
9. `cargo test --release --test acceptance` — confirm golden hash matches.
   If it doesn't, diagnose; do NOT re-bootstrap.
10. Add the two new unit tests (§7a, §7b).
11. `cargo fmt --check && cargo clippy --all-targets -- -D warnings`.
12. `pnpm --filter ./web build` — confirm wasm build is clean.
13. Commit with the suggested message in §0.

---

## 10. Stretch / out-of-scope (compatible follow-ups)

- **`MAX_POPULATION` pre-allocation** (perf-final-report §6 commit 4, item
  #9): replace `Vec::new()` in §3b and §3c with
  `Vec::with_capacity(MAX_POPULATION)`. Requires the `MAX_POPULATION`
  constant to land in `src/constants.rs` first. This plan is forward-
  compatible — the change becomes a one-line edit per scratch field.
- **Pool `candidates: Vec<usize>` in `eat_and_scavenge`** (decision 1):
  duplicate the `std::mem::take` recipe in §5 against
  `self.scratch_eat_candidates`. Out of scope here to keep the diff
  focused and the split-borrow change isolated to a single site.
- **Pool `dead: Vec<usize>` and `species_lost: Vec<u32>` in
  `collect_deaths`** (hotpath §2e): low-priority; bounded per-tick by
  death count.

---

## 11. Downstream merge note for perf-5

_This section is for the perf-5 implementer, not for the perf-2 implementer._

perf-5 rewrites `g.size` / `g.eat_efficiency` (and other hot-genome) read
sites in `apply_movement_and_repulsion` (lines ~485, 496, 531) and
`eat_and_scavenge` (lines ~625–688). Those line numbers are quoted in perf-5's
plan against the pre-perf-2 source. After perf-2 lands, two things shift:

1. **Line numbers drift.** The nine `scratch_*` field declarations and the
   `resize` + `fill` preambles in both functions add ~30 lines to `world.rs`.
   Re-confirm line numbers with `grep` before editing.
2. **Local variable names change inside the loop bodies.** What perf-5's plan
   reads as `fx[i]` / `fy[i]` / `gain[i]` etc. are now
   `self.scratch_fx[i]` / `self.scratch_fy[i]` / `self.scratch_gain[i]` etc.
   perf-5's genome-field rewrites are at *different* indices in those same
   lines — the edits are textually disjoint — but the implementer must not
   accidentally revert the `self.scratch_*` accessor form when rewriting the
   genome reads alongside them.
3. **`for &j in &neighbors` loop header.** The `for j in neighbors` loop in
   `apply_movement_and_repulsion` becomes `for &j in &neighbors` under perf-2's
   `mem::take` recipe (§5 above). perf-5 must not re-write that loop header
   back to `for j in neighbors`.

---

## 12. Citations

- `docs/plans/perf+ui-master.md` §5 "perf-2 — scratch Vec pooling" — the
  master-plan piece briefing this plan expands.
- `docs/plans/perf+ui-master.md` §8.3 — Resolved Decision: `neighbors` is
  in scope; split-borrow is the implementer's job.
- `docs/plans/perf+ui-master.md` §6 R4 — `scratch_` prefix mandate, no
  field-name collisions.
- `docs/research/perf-final-report.md` §3 item 2 + §6 commit 2 — perf win
  ranking, ~120 LOC budget, golden-safe.
- `docs/research/perf-sim-hotpath.md` §2a (fx/fy), §2b (neighbors), §2c
  (candidates — declared out of scope), §2d (six eat/scavenge vecs) —
  exact byte counts and the `mem::take` rationale.
- `src/world.rs:91–101` — existing pattern for excluding transient fields
  (`vision`, `cell_to_carrion`, `pending_extinction_check`, `profile`)
  from save/hash.
- `src/save.rs:23–62`, `src/save.rs:138–200` — `SaveV1` struct and
  manual `from_world` constructor confirming the field-by-field exclusion
  contract.
- `src/snapshot_hash.rs:21–80` — hash input list; scratch buffers
  intentionally absent.

*End of perf-2 plan.*

---

## Revision history

- **v1 (initial).** Plan authored against `src/world.rs` on `main`; all nine
  line numbers verified via `grep`. Approved with no must-fix items.
- **v2 (this version).** Applied two should-fix clarifications from the
  per-piece review: (1) panic-safety assumption for `mem::take` made explicit
  in §5; (2) indirection of test 7a's sentinel-recovery assertion flagged
  explicitly so the implementer does not accidentally tighten it. Added §11
  "Downstream merge note for perf-5" to capture the cross-review M2 finding
  (perf-5 must rebase on perf-2's `self.scratch_*` accessors and the changed
  `for &j in &neighbors` loop header).

---

## Code review

**Verdict: APPROVE.** Commit `c382916` implements the plan faithfully with no blocking issues.

### Verified correct

1. **All 9 scratch_* fields present** on `World` with the locked `scratch_` prefix and the documented comment block (world.rs:99–112). Types match the table in §2 exactly.
2. **Reset semantics correct per field.** `resize(n, 0.0)` + `fill(0.0)` for `scratch_fx`, `scratch_fy`, `scratch_damage`, `scratch_gain`. `resize(n, false)` + `fill(false)` for the four bool flag arrays. `scratch_neighbors` uses `clear()` per inner iteration via the `mem::take` recipe. Matches §1 decision 4 and §4 rule 3.
3. **Split-borrow on `neighbors`** uses the §5 verbatim recipe: `std::mem::take(&mut self.scratch_neighbors)` at iteration start, `for &j in &neighbors`, write-back via `self.scratch_neighbors = neighbors` at iteration end (world.rs:514–559). No drop guard, consistent with the explicit panic-safety acceptance in §5.
4. **`for j in neighbors` → `for &j in &neighbors`** rewrite landed (world.rs:531). perf-5 merge note in §11 stands.
5. **`from_save_v1` initialization** adds all 9 `Vec::new()` initializers at world.rs:1148–1156, matching the §3c contract.
6. **`World::new` initialization** adds all 9 `Vec::new()` initializers at world.rs:178–186.
7. **Save/hash exclusion confirmed.** `pub struct World` at world.rs:47 has no `#[derive(...)]` line. `git diff` shows zero changes to `src/save.rs` and `src/snapshot_hash.rs`. SaveV1 round-trip and snapshot hash exclusion are structural.
8. **Tests landed and pass.** `scratch_fx_fy_zeroed_at_tick_start` and `scratch_grows_with_population` (world.rs:2210–2266) match §7a/§7b. Both green under `cargo test --release --lib scratch`.
9. **Acceptance regression clean.** `cargo test --release --test acceptance` — 3/3 pass. Golden hash `0xc35be8a7905c7f05` preserved.
10. **Clippy + fmt + pnpm build all clean.** `cargo fmt --check` silent; `cargo clippy --all-targets -- -D warnings` clean; `cd web && pnpm build` succeeds (599 kB wasm, 36 kB JS).
11. **perf-5 merge note preserved** at §11 of the plan doc.
12. **Eat path writes are clean.** All 6 write sites (`scratch_attempted_eat[i]`, `scratch_damage[j]`, `scratch_gain[i]`, `scratch_cooldown_set[i]`, `scratch_got_a_bite[i]`, `scratch_attempted_scavenge[i]`) and the 6 post-loop reads compile without split-borrow gymnastics (consistent with §3e — the `clone()` of `genomes[i]` at world.rs:625/671 still releases the `&self.creatures` borrow).

### Blocking issues

None.

### Non-blocking observations

- **`scratch_neighbors` `clear()` is redundant** after `std::mem::take` since `take` swaps in a fresh `Vec::new()` (always empty). The `neighbors.clear()` call at world.rs:519 is a no-op on the first iteration and is correct (cheap) on subsequent ones — leaving it in is defensive and clear. Not worth changing.
- **`Vec<bool>` over `Vec<u8>` or a bitset.** The four flag arrays remain `Vec<bool>` per the plan. A bitset would save memory but is explicitly out of scope here and changes semantics (`fill(false)` becomes a different op). Fine to defer.
- **No `pub(crate)` visibility narrowing.** Fields are private (default), which the tests handle by being in the same `mod tests` block. Matches §3a's "implementer's call" guidance.
- **Eat-path `candidates: Vec<usize>`** remains in-function per §1 decision 1 — out of scope.

### Measured perf delta

`cargo test --release --test acceptance acceptance_t10000` wall time (3 runs): **2.29s, 2.59s, 2.69s → median 2.59s**. Compared to perf-1 cited baseline of ~2.69s, that's ~0.10s (~3.7%) improvement on the t10000 golden — within run-to-run noise on a single workload but trending the right direction. Full 3-test acceptance suite: 3.30s, 2.99s, 3.51s → median 3.30s. The perf win is dominated by allocator-pressure reduction, which acceptance (single founder seed, ~1500-creature peak) underweights vs. real interactive workloads where allocator contention would matter more. Correctness gate (golden hash) is the dispositive check and passes.
