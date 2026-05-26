# S12 — `validate_save(&SaveV1) -> Result<(), LoadError>` (DoS / H1)

Status: PLANNED
Owner: planner = opus, implementer = sonnet (retry opus), code-reviewer = opus
Lands in: **PR-3** (determinism + correctness + regen). Standalone within PR-3 — does not depend on S7/S8 and does not contribute to the regen.
Sources: `docs/plans/audit-master.md` §4 (S12 entry); `docs/plans/audit-triage.md` SHIP S12; `docs/audit/security.md` H1; `docs/audit/correctness-bugs.md` C5.

---

## 1. Summary

Add `pub(crate) fn validate_save(save: &SaveV1) -> Result<(), LoadError>` to `src/save.rs`. Call it at the **top** of `World::from_save_v1` (post-S1: `src/world/save_v1.rs`), immediately after the `schema_version` check and before any other work (including the existing `validate_soa_lengths` call — `validate_save` calls that internally OR runs first; see §4).

The validator performs the six-to-seven structural integrity checks that the loader currently *assumes* hold but never verifies. With `panic = "abort"` in the wasm release profile (`Cargo.toml:55`), any panic kills the runtime, so an attacker-controlled save that violates these assumptions is a guaranteed-crash DoS. Today's `from_save_v1` indexes blindly into `species.list`, treats `sun.{capacity,current,demand}` as `SUN_DIM²`-length, casts `carrion[k].sun_cell` straight into sun arrays, and computes `max_id + 1` without overflow check. `validate_save` closes every one of those holes by returning `LoadError::StructuralError(reason)`, which JS already routes to the schema-mismatch modal (`web/src/main.ts:197-204` shows the catch-all warns and surfaces the same modal for any non-schema error string).

No behavior change on the happy path: every legitimate save produced by `SaveV1::from_world` (today, and after every other PR-3 commit) must pass `validate_save` unchanged. The S39 `save_load_hash_equal_immediately_after_load` test (lands in same PR-3) is the integration canary for that.

---

## 2. Validation rules

All failures return `LoadError::StructuralError(reason)`. Messages use the format documented per rule.

### (a) Sun array lengths

```rust
let expected = SUN_DIM * SUN_DIM; // 400
if save.sun.capacity.len() != expected {
    return Err(LoadError::StructuralError(format!(
        "sun.capacity len {} != SUN_DIM*SUN_DIM ({expected})",
        save.sun.capacity.len()
    )));
}
// same for save.sun.current and save.sun.demand
```

Why: `SunMap::recompute_capacity` (`src/sun.rs:65-77`) and the hot per-cell loop at `src/world.rs:627-635` (post-S1: `src/world/tick.rs`) panic on length mismatch.

### (b) `creatures.species_id[i]` references an existing species

Build a `HashSet<u32>` ONCE from `save.species.list.iter().map(|s| s.id)`, then per creature do a `contains` check:

```rust
use std::collections::HashSet;
let species_ids: HashSet<u32> = save.species.list.iter().map(|s| s.id).collect();
for (i, sid) in save.creatures.species_id.iter().enumerate() {
    if !species_ids.contains(sid) {
        return Err(LoadError::StructuralError(format!(
            "creature {i} species_id {sid} not in species list"
        )));
    }
}
```

**R6 lint compatibility note (see §8(a)):** this uses `HashSet::contains` only — no `.iter()`, no `.into_iter()`, no iteration over the set. The build step iterates the `Vec<Species>` (deterministic by index), then we use the set as a pure membership oracle. The S9 lint rule (added in same PR-3) forbids `HashSet::iter` / `HashSet::into_iter`; `contains` is allowed.

Why: `SpeciesRegistry::get(id)` at `src/species.rs:90` panics with `&self.list[id as usize]` if `id ≥ list.len()`. The loader reads `species_id` columns blindly and the very next tick hits this via `src/world.rs:480, :811, :854, :960` (post-S1: `src/world/tick.rs`) and `src/wasm_api.rs:249` (`creature_inspect_json`).

### (c) `parent_species_id[i]` similarly

Same membership check against the same `species_ids` set. The current schema stores `parent_species_id` as `u32` (not `Option<u32>`) — the convention is "sentinel = `species_id[i]` for founders (no distinct parent)" per the SoA shape in `src/save.rs:77`. **Planner judgement:** because there is no `Option` and no documented sentinel value separate from "any valid id", the rule reduces to "must be in `species_ids`". If the implementer discovers a documented sentinel (e.g. `u32::MAX` for "no parent"), allow that one extra value:

```rust
if *psid != u32::MAX && !species_ids.contains(psid) {
    return Err(LoadError::StructuralError(format!(
        "creature {i} parent_species_id {psid} not in species list"
    )));
}
```

The implementer must `grep -n "parent_species_id" src/world*.rs src/world/ src/species.rs src/creature.rs` before writing this rule and document the chosen sentinel in the function doc-comment. Default to "must be in set" (no sentinel) if the grep finds none.

### (d) Per-carrion `sun_cell`

```rust
let sun_cells = SUN_DIM * SUN_DIM;
for (k, c) in save.carrion.iter().enumerate() {
    if c.sun_cell >= sun_cells {
        return Err(LoadError::StructuralError(format!(
            "carrion {k} sun_cell {} >= SUN_DIM*SUN_DIM ({sun_cells})",
            c.sun_cell
        )));
    }
}
```

Why: per `src/carrion.rs:14` `sun_cell: usize`; the decay/return path indexes into `sun.demand`/`sun.current` and panics on OOB (see security.md H1 bullet 2).

### (e) Finite + range checks on every f32

For each creature `i ∈ 0..n` (post `validate_soa_lengths`):

| Field | Rule | Failure message |
|---|---|---|
| `x[i]` | `is_finite() && (0.0..WORLD_SIZE).contains(&x)` | `"creature {i} x {x} not in [0, WORLD_SIZE)"` |
| `y[i]` | same | same with `y` |
| `vx[i]`, `vy[i]` | `is_finite()` | `"creature {i} vx/vy {v} non-finite"` |
| `energy[i]` | `is_finite()` | `"creature {i} energy non-finite"` |
| `cumulative_upkeep[i]` | `is_finite()` | same form |
| `distance_travelled[i]` | `is_finite()` | same form |
| `max_size_reached[i]` | `is_finite()` | same form |
| `genomes[i]` every `f32` field | `is_finite()` | `"creature {i} genome.{field} non-finite"` |
| `brains[i].weights[w]` (every element) | `is_finite()` | `"creature {i} brain.weights[{w}] non-finite"` |

Genome `f32` fields (per `src/genome.rs:54-70`): `size`, `photosynth_efficiency`, `eat_efficiency`, `scavenge_efficiency`, `move_speed`, `eye_offsets[0..EYE_SLOTS]`, `vision_range`, `armor`, `bite_reach`, `pigment_r`, `pigment_g`, `pigment_b`, plus every f32 inside `mutation_rates: TraitMutationRates`. The implementer should write a small helper `fn check_genome_finite(i: usize, g: &Genome) -> Result<(), LoadError>` so this stays readable.

`max_age` is `u32` — no finite check needed. `digestion_cooldown`, `age`, `birth_tick`, `id`, `species_id`, `parent_species_id`, `last_action`, `action_this_tick` are integer/enum — no finite check needed (range is encoded in the type).

Why: `f32::NAN` / `±INFINITY` coordinates flow into `SpatialGrid::rebuild` → `cell_index_for` → `as usize` cast (per security.md H1 bullet 5), with downstream OOB panics. NaN propagates into `decode_action` logits too — although S5 (PR-3 same batch) handles the runtime NaN case, killing it at load time is cheaper and removes one defensive code path's exposure.

### (f) `max_id` overflow

The current code at `src/world.rs:1118-1119` (post-S1: `src/world/save_v1.rs`) does:

```rust
let max_id = save.species.list.iter().map(|s| s.id).max().unwrap_or(0);
let species = SpeciesRegistry::from_snapshot(save.species.list, max_id + 1);
```

If a save sets a species `id = u32::MAX`, `max_id + 1` wraps in release (and panics in debug). In `validate_save`:

```rust
if let Some(max_id) = save.species.list.iter().map(|s| s.id).max() {
    if max_id.checked_add(1).is_none() {
        return Err(LoadError::StructuralError(format!(
            "species max_id {max_id} would overflow on +1"
        )));
    }
}
```

**Note on briefing wording:** the master plan §4 S12 rule (f) cites `creatures.max_id.checked_add(1)`; the actual code site computes `max_id` over `save.species.list`, not `save.creatures`. This plan implements the check against the actual site (the species `max_id + 1` on line ~1119). If a future schema adds a `creatures.max_id` field, extend the rule then.

### (g) Hard creature-count cap

```rust
const VALIDATE_SAVE_MAX_CREATURES: usize = 100_000;
if save.creatures.id.len() > VALIDATE_SAVE_MAX_CREATURES {
    return Err(LoadError::StructuralError(format!(
        "creature count {} exceeds hard cap {VALIDATE_SAVE_MAX_CREATURES}",
        save.creatures.id.len()
    )));
}
```

Why: bounds the SoA allocation at load time. Per triage REJECT R12, the full `big-wins #2` MAX_POPULATION work is out of scope for this pass — this hard cap is the partial mitigation, scoped strictly to the load path. Live sim populations are typically <2k (perf-5 reports cite "population <2k"); 100k is a 50× safety margin that no legitimate v1 save will hit.

---

## 3. Function placement

**Decision: `src/save.rs`**, next to `validate_soa_lengths`. The call from `from_save_v1` becomes a single `validate_save(&save)?;` line. Rationale:

- `validate_save` is pure on `&SaveV1` — it never touches `World`, `SunMap`, `SpeciesRegistry`, or any runtime type. Co-locating with `SaveV1` keeps the wire-shape and its invariants in one file.
- The existing pattern is already there: `validate_soa_lengths` (~line 207) lives in `save.rs` and is called from `from_save_v1`. `validate_save` mirrors that exactly.
- S1's module split moves `from_save_v1` to `src/world/save_v1.rs` but leaves `src/save.rs` untouched (see master plan §4 S1: "S1 does NOT split `src/save.rs`"). Placing `validate_save` in `save.rs` keeps it stable across S1's mechanical move.
- `pub(crate) fn` (not `pub fn`) — only the loader needs to call it; no external API surface.

Visibility of `validate_save` is `pub(crate)`. `validate_save` should call `validate_soa_lengths(&save.creatures)?` internally as its first step — that consolidates "structural validation of SaveV1" into one entry point. After this lands, `from_save_v1` no longer calls `validate_soa_lengths` directly (replace the line at `src/world.rs:1061` post-S1: `src/world/save_v1.rs`).

---

## 4. Implementation order

Strict sequential within S12:

1. **Write `validate_save` + helpers in `src/save.rs`.** Add the function below `validate_soa_lengths`. Add the `use crate::constants::{SUN_DIM, WORLD_SIZE};` import. Internally call `validate_soa_lengths` first (so callers get one entry point). Run `cargo build` to confirm it compiles.

2. **Write the 7 negative tests (+1 positive)** in the existing `#[cfg(test)] mod tests` block in `src/save.rs`. See §5. Run `cargo test --lib save::tests` — all 8 must pass.

3. **Wire the call.** In `src/world/save_v1.rs` (post-S1) at the top of `from_save_v1` after the schema-version check, replace:
   ```rust
   let n = validate_soa_lengths(&save.creatures)?;
   ```
   with:
   ```rust
   crate::save::validate_save(&save)?;
   let n = save.creatures.id.len(); // already validated above
   ```
   Run `cargo test --lib save::tests` (round-trip tests in `save.rs`) and `cargo test --lib world` — all existing tests still pass.

4. **Full test sweep.** Run `cargo test`, `cargo test --features threads`, and `cargo test --release --test acceptance` (both feature sets). Goldens unchanged (no determinism impact). `pnpm typecheck` + `pnpm build` (no TS change, but smoke for the wasm boundary). Manually craft a corrupt save (e.g. truncate `sun.capacity` to 10 elements in a test JSON), load via `WorldHandle::from_json`, and confirm the JS surface shows the schema-mismatch modal rather than a wasm panic.

---

## 5. Test plan

All tests live in `src/save.rs` inside the existing `#[cfg(test)] mod tests` block.

### Helper

```rust
fn minimal_valid_save() -> SaveV1 {
    // World::new(seed).to_save_v1() with a small population — uses the real
    // construction path so we know it's structurally valid.
    World::new("s12-validate").to_save_v1()
}
```

### `validate_save_accepts_real_world_save`

```rust
#[test]
fn validate_save_accepts_real_world_save() {
    let save = minimal_valid_save();
    assert!(validate_save(&save).is_ok(), "fresh world save must validate");

    // And one that has stepped (so carrion / species mutations are populated).
    let mut w = World::new("s12-stepped");
    for _ in 0..500 { w.tick_once(); }
    let save = w.to_save_v1();
    assert!(validate_save(&save).is_ok(), "t=500 save must validate");
}
```

### Seven negative tests

Each constructs a minimal `SaveV1` that violates exactly one rule and asserts `LoadError::StructuralError(message)` with a substring match on the message format documented in §2.

```rust
fn assert_structural(result: Result<(), LoadError>, substr: &str) {
    match result {
        Err(LoadError::StructuralError(s)) => {
            assert!(s.contains(substr), "got message {s:?}, expected substr {substr:?}");
        }
        other => panic!("expected StructuralError({substr:?}), got {other:?}"),
    }
}

#[test]
fn validate_save_rejects_short_sun_arrays() {  // rule (a)
    let mut save = minimal_valid_save();
    save.sun.capacity.truncate(10);
    assert_structural(validate_save(&save), "sun.capacity len");
}

#[test]
fn validate_save_rejects_unknown_species_id() {  // rule (b)
    let mut save = minimal_valid_save();
    save.creatures.species_id[0] = 9_999;
    assert_structural(validate_save(&save), "species_id 9999");
}

#[test]
fn validate_save_rejects_unknown_parent_species_id() {  // rule (c)
    let mut save = minimal_valid_save();
    save.creatures.parent_species_id[0] = 9_999;
    assert_structural(validate_save(&save), "parent_species_id 9999");
}

#[test]
fn validate_save_rejects_oob_carrion_sun_cell() {  // rule (d)
    let mut save = minimal_valid_save();
    // Force a carrion entry (fresh worlds may have none).
    save.carrion.push(crate::carrion::Carrion {
        id: 1, x: 0.0, y: 0.0, pool: 1.0, age: 0, sun_cell: 9_999,
    });
    assert_structural(validate_save(&save), "carrion 0 sun_cell 9999");
}

#[test]
fn validate_save_rejects_non_finite_position() {  // rule (e)
    let mut save = minimal_valid_save();
    save.creatures.x[0] = f32::NAN;
    assert_structural(validate_save(&save), "creature 0 x");
    let mut save = minimal_valid_save();
    save.creatures.y[0] = f32::INFINITY;
    assert_structural(validate_save(&save), "creature 0 y");
    let mut save = minimal_valid_save();
    save.creatures.energy[0] = f32::NAN;
    assert_structural(validate_save(&save), "energy non-finite");
    let mut save = minimal_valid_save();
    save.creatures.brains[0].weights[0] = f32::NAN;
    assert_structural(validate_save(&save), "brain.weights[0] non-finite");
}

#[test]
fn validate_save_rejects_species_id_overflow() {  // rule (f)
    let mut save = minimal_valid_save();
    if let Some(s) = save.species.list.last_mut() { s.id = u32::MAX; }
    assert_structural(validate_save(&save), "would overflow");
}

#[test]
fn validate_save_rejects_oversize_population() {  // rule (g)
    let mut save = minimal_valid_save();
    // Cheap: grow ONE column past the cap to trigger the check before
    // validate_soa_lengths (the cap check must run BEFORE the lengths check —
    // or the length check passes only if all columns grow together; either
    // way, the cap message must surface).
    save.creatures.id = vec![0u64; 100_001];
    assert_structural(validate_save(&save), "exceeds hard cap");
}
```

**Note on test ordering inside `validate_save`:** rule (g) (hard cap) MUST run BEFORE `validate_soa_lengths` so that the cap message surfaces even when columns are mismatched. Order inside `validate_save`:

1. (g) hard cap on `save.creatures.id.len()`
2. `validate_soa_lengths(&save.creatures)?` (existing)
3. (a) sun lengths
4. (f) species `max_id` overflow
5. Build `species_ids: HashSet<u32>`
6. (b), (c) per-creature species id membership
7. (d) per-carrion `sun_cell`
8. (e) per-creature finite + range checks (positions first to fail fast on the most common DoS shape)

---

## 6. Path notes

- `src/save.rs` is NOT split by S1 — `validate_save` lives there permanently. The path stays stable.
- `from_save_v1` moves from `src/world.rs:1047-1167` to `src/world/save_v1.rs` per the S1 module map. The S12 wire-up touches the *post-S1* path. If S12 lands before S1 (PR-3 order: S1 already in PR-1), this is moot — by PR-3 the file exists.
- No other paths change. No new public API on `WorldHandle`. JS surface unchanged.

---

## 7. Determinism impact

**None.** `validate_save` is read-only on `&SaveV1` and runs once per load. It cannot affect any tick, any RNG draw, or any hash input. Goldens unchanged. This is why S12 sits in PR-3 as standalone — it does NOT contribute to the PR-3 regen batch (which is driven by S7 + S8).

---

## 8. Risk register

### (a) `HashSet<u32>` use must be R6-lint-compatible

S9 (same PR-3) adds a clippy.toml rule forbidding `HashMap::iter`/`into_iter` and `HashSet::iter`/`into_iter` in sim-critical files (`src/world*.rs`, `src/snapshot_hash.rs`, `src/species.rs`, `src/creature.rs`, and new `src/world/*.rs`). **`src/save.rs` is NOT in that lint scope** — `validate_save` is a load-path one-shot, not a per-tick path. But for safety and forward-compat, this plan uses `HashSet::contains` ONLY; the set is built once via `iter().map(...).collect()` (which iterates the `Vec<Species>` — deterministic — not the HashSet) and queried via `.contains(&id)`. No iteration of the HashSet itself happens. If S9's lint expands later to also cover `save.rs`, this code remains compliant.

### (b) 100k cap must not break legitimate large saves

The cap is 100,000 creatures. The current sim balance produces populations <2k (per perf-5 report and the `peak_population` field's typical observed values). 100k is a 50× safety margin. The implementer should `grep -rn "peak_population" tests/ src/` to confirm no test asserts a population near the cap; if any test population approaches even 10k, raise this in the cross-review.

### (c) `f32::is_finite()` semantics — no legitimate ±∞ in live saves

`is_finite()` rejects `±∞` and NaN, accepts subnormals and signed zero. The sim never produces ±∞ (no `f32::INFINITY` literals in `src/`; confirm with `grep -rn "INFINITY\|f32::infinity\|f32::NAN" src/` before implementing). `decode_action`'s NaN handling (S5, same PR-3) is a runtime defense against logits-NaN — `validate_save` rejects any save *containing* NaN/∞ in persisted fields, which is a stricter rule that no real `SaveV1::from_world` output can violate. Pairs cleanly with S5 (different layer: load-time vs tick-time).

### (d) HashSet allocation cost

`validate_save` allocates one `HashSet<u32>` sized to `species.list.len()` — typically <100, max bounded by the (uncapped today) species count. This is a one-shot O(s + n) where s = species count, n = creature count. With the 100k cap from rule (g) and a typical s <1000, worst case is ~100k contains() calls on a tiny set — under 1 ms in any realistic browser. No perf budget; not in the per-tick path. Documented in the function's doc-comment: "One-shot load-path validation. Allocates a HashSet sized to species count. Not on any hot path."

---

## 9. Pair-with notes

- **S39** (test piece in same PR-3) adds `save_load_hash_equal_immediately_after_load`. That test runs `World::new(seed).snapshot()` → save → load → re-hash, asserting equality before any `step()`. S12's validator MUST pass for every legitimate save the test produces — if `validate_save` rejects any output of `to_save_v1`, S39 fails immediately. This is the integration canary that catches "validator too strict" regressions.
- **S5** (same PR-3) adds NaN propagation defense in `decode_action`. S12 rejects NaN at load time; S5 rejects NaN at tick time. Both defenses ship together. Neither replaces the other (NaN can arise mid-sim from numerics, not only from saves).
- **S1** (PR-1, already landed by PR-3 start) moved `from_save_v1` to `src/world/save_v1.rs`. The wire-up in §4 step 3 references the post-S1 path.
- **S34** (PR-1) added `impl std::error::Error for LoadError`. No interaction with S12 — but if downstream code uses `?` over `LoadError`, the trait impl is now there.

---

## 10. Acceptance criteria

All must hold at PR-3 merge:

1. JS-side load of any crafted payload that violates any of rules (a)–(g) produces a `structural:...` error string and routes to the schema-mismatch modal in `web/src/main.ts:197-204`. NO wasm panic. (Manual test: hand-craft one of the seven failing JSON payloads, paste into devtools `localStorage`/IndexedDB, reload.)
2. All 7 negative tests pass: `cargo test --lib save::tests::validate_save_rejects_*`.
3. Positive test passes: `cargo test --lib save::tests::validate_save_accepts_real_world_save`.
4. All existing `f26_*` round-trip tests in `src/save.rs:259-447` still pass unchanged.
5. `cargo clippy --all-targets -- -D warnings` clean.
6. `cargo clippy --all-targets --features threads -- -D warnings` clean.
7. `cargo test --release --test acceptance` and `cargo test --release --features threads --test acceptance` — both goldens unchanged (S12 has no determinism impact).
8. `pnpm typecheck` + `pnpm build` clean.

---

## 11. Locked scope (do not expand)

- Do NOT refactor `from_save_v1` beyond adding the `validate_save(&save)?` call (replacing the existing `validate_soa_lengths` call site).
- Do NOT add a perf budget — validation is one-shot at load.
- Do NOT change `LoadError` variants beyond using the existing `StructuralError(String)`. Do not add new variants.
- Do NOT widen `validate_save` to also validate `Brain.nn_mutation_rate` finiteness if it isn't already in scope — the briefing's "every brain weight" line is the authoritative list for brains. (Implementer judgement: if `nn_mutation_rate` is trivially included in the per-genome finite sweep, fine; otherwise leave it.)
- Do NOT touch JS error-handling code; the existing catch-all in `web/src/main.ts:197-204` already covers `structural:` strings via the same modal path.
- Do NOT add range checks beyond x/y position — energy/upkeep ranges are open-ended by design and clamping them at load would risk rejecting legitimate saves from future sim balance changes.

---

## Review feedback

**Verdict: APPROVE-WITH-FIXES.** The plan is structurally sound: it implements all 7 master-plan rules, the ordering is well-justified, the JS propagation path is correct, the HashSet-membership-only usage is R6-lint-safe, and the test design covers each rule with a clean substring-match harness. Two non-blocking gaps and one minor ambiguity should be tightened before implementation; none invalidate the design.

### Blocking issues (count: 0)

None.

### Non-blocking issues

#### N1 — `Brain.nn_mutation_rate` finite check (severity: low-medium)

`src/brain.rs:36` declares `pub nn_mutation_rate: f32`. Section §11 ("Locked scope") leaves this to "implementer judgement". That is too loose — the field flows into `(parent.nn_mutation_rate * multiplier).clamp(0.0, 1.0)` at `src/brain.rs:170`. A NaN value survives `clamp` (NaN propagates through `f32::clamp`'s comparisons) and contaminates future mutation draws. Decision: **require** `brain.nn_mutation_rate.is_finite()` as part of rule (e), alongside `brain.weights[w]`. Add the message form `"creature {i} brain.nn_mutation_rate non-finite"` and update §11 to reflect this is now in scope. Test: add one line to `validate_save_rejects_non_finite_position` (rename if desired): `save.creatures.brains[0].nn_mutation_rate = f32::NAN;`.

#### N2 — `DevSliders` and tick/RNG sanity (severity: low)

`DevSliders` carries 5 `f32` fields (`base_sun_rate`, `mutation_rate_multiplier`, `sun_gradient_strength`, `mouth_tax`, `nn_mutation_sigma`) per `src/world.rs:27-32`. None are validated. A NaN slider in a crafted save propagates straight into upkeep / sun-refill math and breaks goldens silently (no panic, but bad state). Add a small rule (h): every `f32` in `save.sliders` must `is_finite()`. Same one-shot cost, same message style. Also worth a one-liner: `save.tick` is `u32` so no overflow, but `save.next_creature_id: u64` is unchecked — a save claiming `next_creature_id < max(creatures.id)` would mint duplicate ids on the next birth. Cheap fix: `if save.next_creature_id <= save.creatures.id.iter().copied().max().unwrap_or(0) { Err(...) }`. Note: planner explicitly limited additional rules; if rule (h) is added, rules (i) tick/next_id checks should be considered too. Recommendation: include rule (h) (sliders finite); rule (i) (next_id) optional but cheap.

#### N3 — `parent_species_id` sentinel question is decisively answered (severity: informational)

Confirmed via grep: every site sets `parent_species_id` to a valid existing species id. Founders set it to `founder_species` (id 0, always in the list); non-speciation births copy the parent's `species_id`; speciation births set it to `parent_species` (a valid id). **There is no sentinel.** The plan's default branch ("no sentinel; must be in set") is correct. The `u32::MAX` fallback branch in the plan §2(c) snippet is dead code given today's schema and should be **removed** — it tempts implementers to permit a value that no legitimate save will contain, weakening the check. Replace the conditional with the unconditional `if !species_ids.contains(psid) { ... }` form. Document explicitly in the function doc-comment: "Schema today has no sentinel; every `parent_species_id` must be in the species list. If a future schema adds an `Option<u32>` wrapper, update this rule."

### Findings against the critique checklist

- **Rule coverage:** All 7 master-plan rules covered. Suggested additions: N1 (brain.nn_mutation_rate) and N2 (sliders/next_id).
- **`parent_species_id` semantics:** Confirmed no sentinel; see N3.
- **`max_id` site discrepancy:** Confirmed via `src/world.rs:1118-1119`. The master plan's citation `creatures.max_id` is a slip; the actual site is `save.species.list.iter().map(|s| s.id).max()`. The plan correctly implements against the real site and calls out the discrepancy in §2(f). Good.
- **HashSet vs R6 lint:** `validate_save` uses `HashSet::contains` only; the set is built by `iter().map(...).collect()` (which iterates the `Vec<Species>`, not the HashSet). S9's lint targets `HashSet::iter` / `HashSet::into_iter` — neither is used. Furthermore, `src/save.rs` is not in S9's listed lint scope (`src/world*.rs`, `src/snapshot_hash.rs`, `src/species.rs`, `src/creature.rs`). Safe today and forward-compat. Confirmed against the S9 briefing in master-plan §6 PR-3 inlined items.
- **Test completeness:** 7 negative + 1 positive is good. Add one negative for `nn_mutation_rate` (per N1) and one for slider NaN (per N2). Consider an additional positive: a `t=10000` world save (matches the acceptance test workload) to catch validator-too-strict regressions against realistic populations.
- **Validation order:** §5's ordering (cap → SoA lengths → sun lengths → max_id overflow → build set → membership → carrion → per-creature finite) is sound: cheapest constant-time checks first, per-creature O(n) checks last, fail-fast on the most common attack shape. Note that putting (g) cap before SoA-length means the cap message surfaces even when columns are mismatched — that's the right call.
- **Performance:** One-shot at load, O(n + s + c) where n≤100k, s≤1k, c≤a few thousand. Worst case well under 100 ms on any reasonable browser. No concern.
- **JS-side propagation:** Confirmed. `src/wasm_api.rs:321` wraps `StructuralError(s)` into `"structural:{s}"`; `web/src/main.ts:197-204` matches only `"schema-mismatch:"` and falls through to the same modal for any other error string (including `"structural:..."`). The catch-all path is exactly what we want — no JS change required.

### Other minor notes

- §2(d) error message: planner writes `"carrion {k} sun_cell {} >= ..."`. The test for rule (d) asserts substring `"carrion 0 sun_cell 9999"` which matches `"carrion 0 sun_cell 9999 >= SUN_DIM..."`. Good.
- §5 helper `minimal_valid_save()` calls `World::new("s12-validate").to_save_v1()`. Confirm `to_save_v1` is on `World` (it is, `src/world.rs:1041`). No issue.
- The `assert_structural` helper takes `Result<(), LoadError>` but `LoadError` doesn't derive `Debug` — wait, it does (`src/save.rs:112` `#[derive(Debug)]`). Good.
- §11 says "Do NOT widen ... if it isn't already in scope — the briefing's 'every brain weight' line is the authoritative list for brains." N1 above formally widens the brain scope to include `nn_mutation_rate`; update §11 to reflect.

