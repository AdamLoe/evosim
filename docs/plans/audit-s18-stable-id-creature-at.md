# S18 — Stable-id `creature_at` + `creature_idx_by_id`

**Audit anchors:** `architecture:B2`, `wasm-api:S9,M2`, `web-wasm:1.7`.
**PR:** PR-2 (wasm boundary cleanup).
**Depends on:** S1 (path translation — but `src/wasm_api.rs` is unchanged by S1, see §6).
**Pairs with:** S20 (grid-backed `creature_at` body) — see §5.
**Effort:** M.
**Determinism impact:** none.

---

## 1. Summary

Change `WorldHandle::creature_at(wx, wy, tolerance)` to return the **stable
creature id** (`Option<f64>` at the wasm boundary, representing a `u64`)
instead of the transient SoA index (`Option<u32>`). Add a sibling helper
`creature_idx_by_id(id: f64) -> Option<u32>` so callers that still need the
SoA index for direct-index APIs (currently `creature_inspect_json`,
`creature_inspect_json_by_id` is **not** being added) can resolve it on
demand.

On the TS side, the click handler in `web/src/rail/inspector.ts` stores the
returned **id** once. Each frame, `refreshInspector` calls
`world.creature_idx_by_id(stored_id)` instead of scanning the
`creature_ids_buffer` Float64Array. The 2-second creature-died placeholder
(DECISIONS E.24) still fires when `creature_idx_by_id` returns `None`.

This is a pure API-shape change; no simulation arithmetic is touched. The
two goldens (`tests/golden_snapshot_t10000.txt` and the threaded variant)
are unaffected.

---

## 2. Function signatures

### Before (`src/wasm_api.rs:216-238`)

```rust
/// Returns the SoA index of the topmost creature whose body circle (or
/// tap-tolerance bubble) contains (world_x, world_y), or None.
/// O(N); called only on click (≤ 1 Hz).
///
/// Returns an index, not a stable id — see DECISIONS.
#[wasm_bindgen]
pub fn creature_at(&self, world_x: f32, world_y: f32, tolerance_world: f32) -> Option<u32> {
    let n = self.inner.creatures.len();
    for i in 0..n {
        let dx = self.inner.creatures.x[i] - world_x;
        let dy = self.inner.creatures.y[i] - world_y;
        let body_r = self.inner.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
        let r = body_r + tolerance_world;
        if dx * dx + dy * dy <= r * r {
            return Some(i as u32);
        }
    }
    None
}
```

### After

```rust
/// Returns the stable id of the topmost creature whose body circle (or
/// tap-tolerance bubble) contains (world_x, world_y), or None.
/// O(N) today; pairs with S20 (grid-backed scan).
///
/// The id is returned as `Option<f64>` because wasm-bindgen does not
/// auto-bridge `u64`. f64 mantissa is exact up to 2^53 (DECISIONS E.21),
/// which is far above any v1-session id count. JS callers must cast
/// the f64 back to whatever id type they retain (number is fine for
/// the lifetime of a session; BigInt if a future session crosses 2^53).
#[wasm_bindgen]
pub fn creature_at(&self, world_x: f32, world_y: f32, tolerance_world: f32) -> Option<f64> {
    let n = self.inner.creatures.len();
    for i in 0..n {
        let dx = self.inner.creatures.x[i] - world_x;
        let dy = self.inner.creatures.y[i] - world_y;
        let body_r = self.inner.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
        let r = body_r + tolerance_world;
        if dx * dx + dy * dy <= r * r {
            return Some(self.inner.creatures.id[i] as f64);
        }
    }
    None
}

/// Resolves a stable creature id back to its current SoA index, or
/// None if the creature is dead (was removed from the SoA).
///
/// O(N) linear scan over `creatures.id`. Acceptable at v1 scale
/// (population <2k); a hashmap-backed lookup is deferred (see §3).
/// Called by the inspector at most once per frame while open.
///
/// `id` is `f64` for wasm-bindgen compatibility (see DECISIONS E.21);
/// it must round-trip a `u64` exactly, which is guaranteed for any
/// id below 2^53.
#[wasm_bindgen]
pub fn creature_idx_by_id(&self, id: f64) -> Option<u32> {
    let needle = id as u64;
    self.inner
        .creatures
        .id
        .iter()
        .position(|&x| x == needle)
        .map(|i| i as u32)
}
```

**Note on `Option<u64>` at the wasm boundary.** wasm-bindgen does not have
native `u64` bridging on stable (BigInt support exists but the project's
existing pattern, per DECISIONS E.21, is f64 — see `creature_ids_buffer`
returning `Float64Array`). We match that pattern.

---

## 3. Implementation of `creature_idx_by_id`

Linear scan over `self.inner.creatures.id: Vec<u64>`. Body shown above.

**Why not a hashmap?** A `HashMap<u64, u32>` would be O(1) per lookup but:

- It must be maintained on every birth and death, adding a write per
  push/swap_remove site (currently free).
- At v1 population scale (<2k) the linear scan over a `Vec<u64>` is a
  few microseconds — far below the per-frame budget.
- Adds RandomState seeding risk (R6 lint surface, see S9).

A hashmap-backed lookup is **deferred to v1.2** when populations may grow
past ~5k. Document this in the code comment above the function.

---

## 4. TS migration

Two edits in `web/src/rail/inspector.ts`. No file deletion, no new
imports.

### 4a. `refreshInspector` (lines 133-180): replace the buffer scan

**Before** (lines 133-180):

```ts
export function refreshInspector(
  world: WorldHandle,
  idsBuffer: Float64Array,
  rail: RailState,
): void {
  const box = getInspectorBox();
  if (box && box.style.display === "none") return;
  if (state.kind !== "selected") return;
  const { creatureId } = state;

  // Build id → index map.
  let foundIdx = -1;
  for (let k = 0; k < idsBuffer.length; k++) {
    if (idsBuffer[k] === creatureId) {
      foundIdx = k;
      break;
    }
  }

  if (foundIdx < 0) {
    // Creature died.
    if (!state.diedAt) {
      state.diedAt = performance.now();
      set("ins-species", "Creature died");
      set("ins-action", "—");
    }
    if (performance.now() - state.diedAt > 2000) {
      clearSelection(rail);
    }
    return;
  }

  const jsonStr = world.creature_inspect_json(foundIdx);
  ...
}
```

**After:**

```ts
export function refreshInspector(
  world: WorldHandle,
  _idsBuffer: Float64Array,   // kept for signature compat; no longer read
  rail: RailState,
): void {
  const box = getInspectorBox();
  if (box && box.style.display === "none") return;
  if (state.kind !== "selected") return;
  const { creatureId } = state;

  // Stable-id lookup (S18). Returns Option<u32> via the wasm boundary;
  // wasm-bindgen surfaces `Option<T>` as `T | undefined`.
  const foundIdx = world.creature_idx_by_id(creatureId);

  if (foundIdx === undefined) {
    // Creature died. 2-second placeholder per DECISIONS E.24.
    if (!state.diedAt) {
      state.diedAt = performance.now();
      set("ins-species", "Creature died");
      set("ins-action", "—");
    }
    if (performance.now() - state.diedAt > 2000) {
      clearSelection(rail);
    }
    return;
  }

  const jsonStr = world.creature_inspect_json(foundIdx);
  ...
}
```

**Note on the unused `idsBuffer` parameter.** The caller in
`web/src/main.ts:298` (`pollRail(rail, world!, ids)`) and inside
`web/src/rail/index.ts` still pass `ids` for other consumers
(stats, highlights). We leave the parameter in `refreshInspector`'s
signature for now (prefix with `_` to silence TS), and a follow-up
cleanup (out of scope for S18) can drop it from `refreshInspector`
and possibly from `pollRail`'s call sequence if no other consumer
needs it. The implementer must NOT do that cleanup as part of S18;
it widens the diff.

### 4b. `installCanvasClickHandler` (lines 232-242): store the id

**Before** (lines 232-242):

```ts
const idx = world.creature_at(wx, wy, toleranceWorld);

if (idx === undefined || idx === null) {
  clearSelection(rail);
} else {
  const jsonStr = world.creature_inspect_json(idx);
  if (jsonStr) {
    const data: CreatureInspectJson = JSON.parse(jsonStr);
    openInspector(data, rail);
  }
}
```

**After:**

```ts
// S18: creature_at now returns a stable id (Option<f64> at the boundary,
// surfaced as `number | undefined`). We immediately resolve it to an
// idx for the first inspect call, then store the id in state for
// per-frame re-resolution via creature_idx_by_id.
const id = world.creature_at(wx, wy, toleranceWorld);

if (id === undefined || id === null) {
  clearSelection(rail);
} else {
  const idx = world.creature_idx_by_id(id);
  if (idx === undefined) {
    // Race: creature died between creature_at and the inspect call.
    // Extremely unlikely (same call stack, no step), but defend.
    clearSelection(rail);
    return;
  }
  const jsonStr = world.creature_inspect_json(idx);
  if (jsonStr) {
    const data: CreatureInspectJson = JSON.parse(jsonStr);
    // openInspector already keys state by `data.id`, so the stored
    // creatureId is correct without any extra plumbing.
    openInspector(data, rail);
  }
}
```

**Note:** `openInspector` already sets `state.creatureId = data.id`
(line 100 of the existing file). The returned id from `creature_at`
and `data.id` will match. No state-shape change needed.

### 4c. DECISIONS update

Add to `DECISIONS.md` under E.21 (or append a new audit-v1.1 line):

```
v1.1 audit (S18): creature_at now returns stable id (Option<f64>);
creature_idx_by_id resolves id → idx for callers that need SoA-direct
calls (currently creature_inspect_json). TS inspector no longer scans
the ids buffer per frame.
```

---

## 5. Coordination with S20

S20 changes the **body** of `creature_at` (grid-backed scan via
`SpatialGrid::for_each_in_radius`); S18 changes the **return type**.
Both pieces touch the same function.

**Recommendation: land both pieces in one commit.** Reasons:

- Smaller review surface for the cross-reviewer (one diff against
  `creature_at` instead of two sequential ones).
- Avoids a transitional state where the wrapper returns `Option<u32>`
  via a grid scan, only to be flipped to `Option<f64>` the next day.
- The Rust test `creature_at_finds_founder` (currently at
  `src/wasm_api.rs:490`) is rewritten **once** to expect a `Some(f64)`
  matching the founder's id, instead of being rewritten twice.

**If sequenced separately:** S20 lands first (body change, return type
unchanged), then S18 lands second (return type change, body untouched).
Either order is mechanically safe; the recommendation above is about
reviewer ergonomics only.

The cross-reviewer should confirm at PR-2 end that the post-merge body
of `creature_at` (i) uses the grid (S20) and (ii) returns
`Option<f64>` holding `creatures.id[i]` (S18).

---

## 6. Path notes

`src/wasm_api.rs` is **not split** by S1. All edits in this plan
target `src/wasm_api.rs` at the line numbers cited (216-238 for
`creature_at`, after that block for the new `creature_idx_by_id`).

S1's path-translation table will not affect this plan. The internal
calls `self.inner.creatures.id`, `.x`, `.y`, `.genomes` are field
accesses on the `World` struct (re-exported from `src/world/mod.rs`
post-S1); no path change visible at this call site.

---

## 7. Step-by-step implementation order

1. **Add `creature_idx_by_id`.** Place it directly after
   `creature_at` in `src/wasm_api.rs` (around the current line 239).
   Run `cargo build` to confirm wasm-bindgen accepts `Option<f64>`
   and `Option<u32>` at the boundary (both are supported).

2. **Change `creature_at` return type** from `Option<u32>` to
   `Option<f64>`. Update the body to return
   `Some(self.inner.creatures.id[i] as f64)` instead of
   `Some(i as u32)`. Update the doc comment per §2.

3. **Update TS callers** in `web/src/rail/inspector.ts` per §4a and
   §4b. The single Rust→TS contract change is: `creature_at` now
   returns `number | undefined` (was already `number | undefined`,
   but the semantic meaning shifts from index to id — TS sees no
   type-level change because both u32 and f64 surface as `number`).

4. **Test.** Per §8.

5. **Coordinate with S20.** If landing in one commit, fold the S20
   body change in here. If sequenced, leave the body as-is (still
   linear scan) and let S20 PR replace it.

---

## 8. Test plan

### 8a. Rust: rename and update existing test

`src/wasm_api.rs:488-508` — the existing `creature_at_finds_founder`:

```rust
/// E.21 + S18: creature_at returns the founder's stable id at center.
#[test]
fn creature_at_returns_stable_id() {
    use crate::constants::WORLD_SIZE;
    let handle = WorldHandle::new("e21-creature-at");
    let cx = WORLD_SIZE * 0.5;
    let cy = WORLD_SIZE * 0.5;
    // Founder's stable id is the first id assigned in World::new().
    // We don't hardcode the value; read it from the handle.
    let founder_id = handle.inner.creatures.id[0] as f64;
    // Zero tolerance: hit exactly at the center.
    let result = handle.creature_at(cx, cy, 0.0);
    assert_eq!(result, Some(founder_id), "founder id must be found at world center");
    // With tolerance: hit just outside the body radius should still hit.
    let result_tol = handle.creature_at(cx + 2.0, cy, 3.0);
    assert_eq!(result_tol, Some(founder_id), "founder must be found within tolerance radius");
    // Far outside any creature — should return None even with tolerance.
    let miss = handle.creature_at(0.0, 0.0, 1.5);
    assert!(miss.is_none(), "empty corner must return None");
}
```

Note: this assumes `WorldHandle` exposes `pub(crate)` access to
`inner` from the test module (it does — `wasm_api.rs` tests are in
the same file). If S35 tightens `inner` to private in a later piece,
add a `#[cfg(test)] pub(crate) fn founder_id(&self) -> u64` helper.

### 8b. Rust: new tests for `creature_idx_by_id`

```rust
/// S18: creature_idx_by_id resolves a live id back to its SoA index.
#[test]
fn creature_idx_by_id_finds_existing() {
    let handle = WorldHandle::new("s18-idx-by-id");
    let founder_id = handle.inner.creatures.id[0];
    let idx = handle.creature_idx_by_id(founder_id as f64);
    assert_eq!(idx, Some(0), "founder must be at index 0");
}

/// S18: creature_idx_by_id returns None for a never-existed id.
#[test]
fn creature_idx_by_id_returns_none_for_dead() {
    let handle = WorldHandle::new("s18-idx-dead");
    // u64::MAX is guaranteed never to be allocated in any v1 session.
    let idx = handle.creature_idx_by_id(u64::MAX as f64);
    assert!(idx.is_none(), "unknown id must return None");
}
```

A stronger variant (out of scope for S18, defer if implementer
prefers): step the world until the founder dies, then assert
`creature_idx_by_id(founder_id) == None`. Not required because
the never-existed id test already exercises the None branch.

### 8c. TS-side manual check (dev-server flow)

Per `docs/dev-server-prompt.md`, port 47821. The implementer is
**not** responsible for an automated UI test; manual verification
via the dev-server is the contract. The check is:

1. `pnpm typecheck` clean.
2. `pnpm build` clean (pre-existing static+dynamic-import warning is
   acceptable per master plan §10).
3. (Optional, not gated) dev-server → click a creature → inspector
   opens → values update per frame → speed up sim to 100× → wait
   for that creature to die → inspector shows "Creature died" for
   ~2 s then closes.

### 8d. Acceptance regressions

Existing `cargo test --release --test acceptance` and
`cargo test --release --features threads --test acceptance` must
still pass with byte-identical goldens. S18 touches only the
wasm boundary; no sim state.

---

## 9. Determinism impact

**None.** `creature_at` and `creature_idx_by_id` read-only access
`creatures.x`, `.y`, `.genomes`, `.id`. No writes to simulation
state. Both goldens unchanged.

---

## 10. Risk register

(a) **`Option<u64>` over wasm-bindgen.** `wasm-bindgen` supports
`u64` via BigInt on recent versions but the project's established
pattern (DECISIONS E.21, `creature_ids_buffer`) uses `f64` to dodge
BigInt friction in TS callers. We follow that pattern. The f64
mantissa is exact for any integer up to 2^53; the audit
(`docs/audit/wasm-api.md` and DECISIONS E.21) confirms v1 sessions
do not approach this. Document in the rust doc comment that the
contract is "f64 carries u64, valid up to 2^53".

(b) **`creature_idx_by_id` is O(N) per inspector frame.** At v1
population (<2k) this is a few microseconds — measurable but
well under the per-frame budget (the existing per-frame TS scan
is the same cost, just paid in JS). The replaced TS scan was over
the full ids buffer (also O(N)), so the change is a wash
algorithmically; the win is removing one wasm→JS Float64Array
materialization per frame (the inspector path no longer requires
the `creature_ids_buffer` call to exist on its behalf — though
other consumers in `pollRail` still call it). A hashmap-backed
lookup is deferred to v1.2; this plan documents that.

(c) **Inspector race: creature dies between `creature_at` and
`creature_inspect_json` in the click handler.** Extremely unlikely
(same call stack, no `step()` between them), but the
post-S18 code path adds a small window because we re-resolve via
`creature_idx_by_id` after `creature_at`. Defended in §4b: if
`creature_idx_by_id` returns `undefined`, the click is treated as
"empty space" and `clearSelection` fires. Acceptable per existing
DECISIONS E.24 semantics.

(d) **`refreshInspector` signature has an unused parameter** after
the change. Annotated with leading `_` to silence TS lints; full
parameter removal is deferred to a non-S18 follow-up to keep this
diff scoped to the API change.

(e) **Cross-piece conflict with S20** mitigated by §5 (recommend
one commit). If sequenced, the cross-reviewer must explicitly
confirm both changes land before declaring PR-2 complete.

---

## 11. Acceptance criteria

- `creature_at` returns `Option<f64>` carrying a stable creature id.
- `creature_idx_by_id(id) -> Option<u32>` exists and works for live
  and dead ids.
- `web/src/rail/inspector.ts:149-156` (the per-frame `idsBuffer`
  scan) is replaced by a single `world.creature_idx_by_id(creatureId)`
  call.
- Clicking a creature still opens the inspector with correct fields.
- Clicking empty space still returns `null` / clears selection.
- The 2-second "Creature died" placeholder still fires when the
  selected creature is removed from the SoA.
- `cargo fmt -- --check` clean.
- `cargo clippy --all-targets -- -D warnings` clean.
- `cargo clippy --all-targets --features threads -- -D warnings` clean.
- `cargo test` clean (including renamed `creature_at_returns_stable_id`
  and two new `creature_idx_by_id_*` tests).
- `cargo test --features threads` clean.
- `cargo test --release --test acceptance` — both goldens unchanged
  (`0xb76e907c6221f7f5`).
- `cargo test --release --features threads --test acceptance` —
  threaded golden unchanged.
- `pnpm typecheck` clean.
- `pnpm build` clean (pre-existing chunking warning acceptable).
- Commit messages conventional (`refactor:` or `feat:`).

---

## 12. Out-of-scope reminder

- Do **not** redesign the inspector UI.
- Do **not** change `creature_ids_buffer`'s Float64Array shape or
  remove the function (other callers in `pollRail` still use it).
- Do **not** add a hashmap-backed id→idx lookup (deferred to v1.2).
- Do **not** add `creature_inspect_by_id` (would duplicate the
  `creature_inspect_json(idx)` API; the two-call pattern
  `creature_idx_by_id` → `creature_inspect_json` is the contract).
- Do **not** refactor `pollRail`'s parameter list to drop `ids`
  (that's a separate cleanup once S21 / S22 reshape per-frame I/O).
- Do **not** touch S20's grid-backed scan body unless landing both
  in one commit per §5.

---

## Review feedback

**Verdict:** APPROVE WITH MINOR REVISIONS. The plan is sound, well-scoped, and
the f64-for-u64 pattern correctly mirrors `creature_ids_buffer`. A handful of
small issues below — none blocking — plus one nit worth removing.

### Issues

**(1) [minor] f64 mantissa boundary explanation is sound but worth pinning.**
The plan correctly notes "exact up to 2^53". `World::new` starts ids at 1 and
increments on every birth; at ~5–20 births/tick over a multi-million-tick
session you reach 2^53 ≈ 9×10^15 only after geological time. The bound is
fine for v1. The plan documents this; nothing further required. **Severity:
informational.**

**(2) [nit] Defensive `creature_idx_by_id` check post-`creature_at` in §4b
is redundant — REMOVE.** The proposed click handler does:
```ts
const id = world.creature_at(wx, wy, toleranceWorld);
if (id === undefined) { clearSelection(rail); }
else {
  const idx = world.creature_idx_by_id(id);  // <-- redundant
  if (idx === undefined) { clearSelection(rail); return; }
  const jsonStr = world.creature_inspect_json(idx);
  ...
}
```
There is **no `step()` between `creature_at` and `creature_inspect_json`** —
same call stack, same synchronous JS callback, no rayon background work. The
"extremely unlikely race" the plan describes is actually **impossible** under
the existing `step()`-on-RAF model. The defensive check adds two wasm calls
and a branch on every click for no actual safety. Either:
- (preferred) Drop the `creature_idx_by_id` round-trip and have
  `creature_at` itself return the idx for the immediate inspect call. But
  this re-introduces the old API. Better:
- Skip the round-trip; pass the id directly. Since we don't yet have
  `creature_inspect_json_by_id`, the cleaner approach is: in the click
  handler only, do the idx lookup once but **don't** treat its `undefined`
  case as a "race" — treat it as a `debug_assert`-style bug. If you want
  belt-and-braces, leave the check but state it's defensive against future
  invariant breakage, not a real race. The plan's framing ("race ... defend")
  is misleading. **Severity: nit / docs.**

Note however that this also means `openInspector(data, rail)` will run with
`data.id` matching the id we just queried — the per-frame `refreshInspector`
will then call `creature_idx_by_id` once per frame anyway, so removing the
click-time check loses very little.

**(3) [minor] TS migration surface confirmed complete.** Independent grep
across `web/src/` shows only TWO call sites of `creature_at`:
`web/src/rail/inspector.ts:232` (the click handler). The `idsBuffer` per-frame
scan lives at `web/src/rail/inspector.ts:151-156`. The plan covers both.
The `idsBuffer` parameter still flows from `web/src/main.ts:295` →
`web/src/rail/index.ts:95,118` → `inspector.ts:135`. The plan correctly
leaves `creature_ids_buffer` alive for other consumers — confirmed correct,
no other inspector-related callers exist. **Severity: confirmation only.**

**(4) [minor] `refreshInspector` unused-parameter approach — prefer removal
over `_` prefix IF callers are easy to update.** The plan keeps
`_idsBuffer: Float64Array` for "signature compat." But the caller at
`web/src/rail/index.ts:118` already passes `idsBuffer` from a single
upstream source — removing the parameter is a 2-line edit (drop from
`refreshInspector` signature, drop from the call site). The plan explicitly
forbids this cleanup ("widens the diff"), which is over-cautious for a
2-line drop. **Recommendation:** allow the implementer to drop the
parameter in this same diff; it shrinks the wasm boundary and reduces
future confusion. If kept with `_` prefix, TS/eslint will accept it
silently (TS does not warn on `_`-prefixed unused params by default), but
this leaves an obvious orphan parameter that the next reader has to
mentally explain. **Severity: minor / discretionary.**

**(5) [minor] Race confirmation.** The plan's §10(c) correctly identifies
that the inspector "race" is a non-issue at the click site (same call
stack). The frame-loop race (creature dies between `creature_idx_by_id`
and `creature_inspect_json` within `refreshInspector`) is **also**
impossible — `refreshInspector` runs on RAF, well outside any
`step()`/rayon work. The stable-id design handles this cleanly: id-based
state is decoupled from idx churn. **Severity: confirmation only.**

**(6) [minor] `creature_idx_by_id_returns_none_for_dead` test name is
misleading.** The test uses `u64::MAX` (a never-existed id), not a dead
id. The plan acknowledges this ("never-existed id") and defers the
stronger variant. Recommendation: **rename the test** to
`creature_idx_by_id_returns_none_for_unknown` to match what it actually
asserts. Add the stronger "step-until-dead" variant inline — it's ~5
extra lines and exercises the actual death path (the SoA `swap_remove`),
which the unknown-id test does not. **Severity: minor / test-quality.**

**(7) [minor] 2-second placeholder test — gap in coverage.** The plan
relies on manual dev-server check (§8c step 3) for the placeholder
behavior. This is acceptable per the master plan's TS-test policy, but
a Rust-side test could assert that `creature_idx_by_id` returns `None`
immediately after the creature is `swap_remove`d (e.g., construct a
World with 2 creatures, call internal death helper or step until one
dies, assert the dead id resolves to `None` and the live id still
resolves correctly). Worth adding. **Severity: minor / coverage.**

**(8) [minor] S20 coordination — one-commit recommendation is correct
and does NOT create a giant diff.** Combined diff size estimate:
- S18: ~15 lines Rust (1 fn body change + 1 new fn + doc), ~30 lines TS
  (2 edits in inspector.ts), ~1 test rename + 2 new tests.
- S20: ~30 lines Rust (replace linear scan with grid scan, adjust
  radius math), 1 unit test added.
Total: ~80 lines plus tests. Well under any "giant diff" threshold. The
combined diff is actually **easier** to review because the reviewer sees
the final state of `creature_at` in one place. **Approve the one-commit
recommendation.** **Severity: confirmation only.**

**(9) [minor] `Option<u32>` over wasm-bindgen for `creature_idx_by_id`.**
The plan returns `Option<u32>` from `creature_idx_by_id` (the new fn)
while `creature_at` returns `Option<f64>` (the changed fn). On the TS
side both surface as `number | undefined`, which is consistent. Confirm
that wasm-bindgen `Option<u32>` does not collide with the JS `number`
representation for valid SoA indices up to 2^32 — it doesn't (u32 fits
in JS number exactly up to 2^32-1, well above v1 populations). **No
issue.** **Severity: confirmation only.**

### Clippy/eslint cleanliness

- Rust side: `creature_idx_by_id` body uses `.iter().position(...)`,
  clippy-clean. The new `as f64` and `as u64` casts are lossless within
  the documented range; no clippy warning expected.
- TS side: leading `_` on unused params is accepted by TS with
  `noUnusedParameters: true` (default in `strict` mode silences `_`
  prefix). The project's tsconfig should be checked; if it has a
  stricter rule, `// eslint-disable-next-line` is required. **Worth a
  one-line check** by the planner: `grep noUnusedParameters web/tsconfig.json`.
  If `noUnusedParameters: true` AND no underscore exemption, removing
  the parameter (per issue 4) is cleanest.

### Summary table

| # | Severity | Action |
|---|---|---|
| 1 | info | none |
| 2 | nit | Remove the defensive `creature_idx_by_id` in §4b OR reframe as "defensive against future invariant breakage, not a race" |
| 3 | info | none — surface confirmed complete |
| 4 | minor | Consider dropping the unused parameter outright |
| 5 | info | none — stable-id design handles races correctly |
| 6 | minor | Rename test to `..._returns_none_for_unknown` |
| 7 | minor | Add a step-until-dead Rust test variant |
| 8 | info | none — one-commit recommendation approved |
| 9 | info | none — type bridging confirmed |

**Blocking issues: 0.**
