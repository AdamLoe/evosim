# S17 — Typed `WorldHandle::set_slider`

**Status:** PLAN (not implemented)
**Owner role:** opus (plan + implement)
**Depends on:** S1 (`world.rs` decomposition — `DevSliders` moves to `src/world/mod.rs`).
**Determinism impact:** none.
**Effort:** S (~30–50 LOC Rust + 0 LOC TS today).
**Bundled in:** PR-2 "Wasm boundary cleanup" (with S18, S19, S20, S21, S22).

---

## 1. Summary

`WorldHandle::set_slider(name: &str, value: f32)` at `src/wasm_api.rs:171-183`
performs a string match on the five slider names and **silently drops unknown
names** via the `_ => {}` arm. The audit (`architecture:B1`, `wasm-api:8.2`,
`web-wasm:8.2`) flags this as a footgun: a JS-side typo (e.g.
`world.set_slider("base_son_rate", 0.5)`) is undetectable at the boundary
and produces no error, warning, or visible effect.

**Resolution (per audit-master S17 briefing):** ship **both** flavours.

1. **Per-slider typed methods** on `WorldHandle`
   (`set_base_sun_rate(f32)`, `set_mutation_rate_multiplier(f32)`,
   `set_sun_gradient_strength(f32)`, `set_mouth_tax(f32)`,
   `set_nn_mutation_sigma(f32)`). These are the static call surface for
   future TS dev-panel UI; wasm-bindgen gives JS typed signatures and
   the TS `.d.ts` enumerates them.
2. **Keep the string form**, but change its signature to
   `set_slider(name: &str, value: f32) -> Result<(), JsValue>`. Returns
   `Err(JsValue::from_str(format!("unknown slider: {name}")))` on unknown
   name. This path is preserved because BUILD-REPORT Known Issue #4
   documents `world.set_slider("base_sun_rate", 0.5)` as the **current
   user-facing way to operate sliders from the JS console** while no
   dev-panel UI exists. Removing the string form would break that
   documented workflow.

Both paths share one implementation: the string form's match arms delegate
to the per-slider helpers. There is exactly one place that mutates each
`DevSliders` field.

---

## 2. Slider inventory

Pulled from `src/world.rs:27-45` (`DevSliders` struct + `Default`) and
`src/wasm_api.rs:171-183` (current match). v6 §K is the spec source for
the canonical slider set; per-slider min/max ranges are not encoded in
Rust today (no `Sliders::clamp` helper exists) and are not in scope for
S17 (locked scope: do not change ranges/defaults).

| Slider name (string)        | Field on `DevSliders`        | Type | Default constant            | Default value | Notes |
|-----------------------------|------------------------------|------|-----------------------------|---------------|-------|
| `base_sun_rate`             | `base_sun_rate`              | f32  | `SUN_REFILL_RATE`           | 0.30          | Mirrors to `self.sun.refill_rate` each tick at `world.rs:200`. |
| `mutation_rate_multiplier`  | `mutation_rate_multiplier`   | f32  | (literal `1.0`)             | 1.0           | Consumed at `world.rs:944,949`. |
| `sun_gradient_strength`     | `sun_gradient_strength`      | f32  | (literal `1.0`)             | 1.0           | **Side effect:** also calls `self.inner.sun.recompute_capacity(value)` in the current matcher. Per-slider helper must preserve this. |
| `mouth_tax`                 | `mouth_tax`                  | f32  | `UPKEEP_MOUTH_DEFAULT`      | 0.05          | Consumed at `world.rs:764`. |
| `nn_mutation_sigma`         | `nn_mutation_sigma`          | f32  | `NN_MUT_SIGMA_DEFAULT`      | 0.02          | Consumed at `world.rs:948`. |

No other fields exist on `DevSliders`. No new sliders are added by S17.

---

## 3. API design

### 3.1 Per-slider methods (new)

All five live on `impl WorldHandle` in `src/wasm_api.rs`, each marked
`#[wasm_bindgen]`. Signatures:

```rust
#[wasm_bindgen]
pub fn set_base_sun_rate(&mut self, value: f32) { /* delegate */ }

#[wasm_bindgen]
pub fn set_mutation_rate_multiplier(&mut self, value: f32) { /* delegate */ }

#[wasm_bindgen]
pub fn set_sun_gradient_strength(&mut self, value: f32) { /* delegate */ }

#[wasm_bindgen]
pub fn set_mouth_tax(&mut self, value: f32) { /* delegate */ }

#[wasm_bindgen]
pub fn set_nn_mutation_sigma(&mut self, value: f32) { /* delegate */ }
```

Each calls a private (non-`wasm_bindgen`) helper that mutates the field
and runs any necessary side effects:

```rust
impl WorldHandle {
    fn apply_base_sun_rate(&mut self, value: f32) {
        self.inner.sliders.base_sun_rate = value;
    }
    fn apply_mutation_rate_multiplier(&mut self, value: f32) {
        self.inner.sliders.mutation_rate_multiplier = value;
    }
    fn apply_sun_gradient_strength(&mut self, value: f32) {
        self.inner.sliders.sun_gradient_strength = value;
        self.inner.sun.recompute_capacity(value);
    }
    fn apply_mouth_tax(&mut self, value: f32) {
        self.inner.sliders.mouth_tax = value;
    }
    fn apply_nn_mutation_sigma(&mut self, value: f32) {
        self.inner.sliders.nn_mutation_sigma = value;
    }
}
```

The public `#[wasm_bindgen]` setters are one-liners that call the matching
`apply_*` helper. Keeping helpers private (not `pub`) avoids inflating the
non-wasm Rust call surface; if Rust callers need to mutate the field they
already do so directly (e.g. `world.rs:2157-2160` in tests).

### 3.2 String form (existing, retyped)

```rust
/// Apply a dev-panel slider live by name. JS console workflow
/// (BUILD-REPORT Known Issue #4). Returns `Err` on unknown name so a
/// console typo is visible instead of silently ignored.
#[wasm_bindgen]
pub fn set_slider(&mut self, name: &str, value: f32) -> Result<(), JsValue> {
    match name {
        "base_sun_rate" => self.apply_base_sun_rate(value),
        "mutation_rate_multiplier" => self.apply_mutation_rate_multiplier(value),
        "sun_gradient_strength" => self.apply_sun_gradient_strength(value),
        "mouth_tax" => self.apply_mouth_tax(value),
        "nn_mutation_sigma" => self.apply_nn_mutation_sigma(value),
        _ => return Err(JsValue::from_str(&format!("unknown slider: {name}"))),
    }
    Ok(())
}
```

JS-side semantics: returning `Err(JsValue)` from a `wasm_bindgen` method
causes the generated TS wrapper to **throw**. The string form is only
called from the dev console, where an uncaught throw surfaces in the
console — exactly the intended UX for a typo. (See Risk 6.1.)

### 3.3 Why not a Rust enum at the boundary

Audit B1 mentions a "`SliderId` JS type via `#[wasm_bindgen]` enum" as an
alternative. Rejected for S17: five sliders, the per-slider methods are
strictly more discoverable (TS autocomplete on `world.set_*`) and avoid
introducing a new JS type the dev-panel UI would need to import. The
enum approach can be revisited if the slider count grows past ~10.

---

## 4. TS call-site migration

**Current state:** there are **zero** `set_slider` call sites in
`web/src/**/*.ts` (confirmed by `grep -rn 'set_slider\|setSlider'
web/src/`). The only existing caller is the JS console invocation
documented in `BUILD-REPORT.md:53`. There is therefore no TS migration
work in S17 today.

**Implication for future dev-panel UI:** when the `~`-hotkey dev panel
(v5 §11 / v6 §K) is implemented, it MUST use the per-slider typed
methods (`world.set_base_sun_rate(v)`, etc.), not the string form. Note
this in the dev-panel plan when it is written; S17 itself does not
modify any TS file.

**Console pattern remains:**
`window.world.set_slider("base_sun_rate", 0.5)` — unchanged in shape,
but now throws on typo instead of silently doing nothing.

---

## 5. Path notes

- `src/wasm_api.rs:171-183` — sole edit location for S17.
- `DevSliders` (currently `src/world.rs:27-45`) moves to `src/world/mod.rs`
  under S1's decomposition. S17 references the struct via
  `self.inner.sliders.<field>` regardless of which module file
  `DevSliders` lives in; no S17 edit touches `world.rs`/`world/mod.rs`.
- S17 ordering vs. S1: implement S17 **after** S1 lands so the helper
  bodies don't churn during the S1 module move. If S17 lands first, the
  S1 patch is trivially mechanical here (no logic change).

---

## 6. Step-by-step implementation order

1. **Add the five private `apply_*` helpers** on `impl WorldHandle` (one
   small `impl` block, non-`#[wasm_bindgen]`). Each helper mirrors the
   current match arm verbatim, including the
   `sun_gradient_strength` → `sun.recompute_capacity(value)` side effect.
2. **Add the five `#[wasm_bindgen]` per-slider setter methods** that each
   call the matching `apply_*` helper. Place them next to (and in the
   same order as) the existing `set_slider` for reviewer locality.
3. **Refactor `set_slider`** to delegate to the helpers and return
   `Result<(), JsValue>` with the unknown-name `Err` branch.
4. **Write Rust unit tests** (see §7). Run `cargo test`.
5. **TS validation:** run `pnpm typecheck` and `pnpm build` in `web/`.
   Both should be no-ops API-wise (no TS callers), but the regenerated
   `pkg/evosim.d.ts` should now show six methods (five typed +
   `set_slider`) where previously there was one.

---

## 7. Test plan

All tests live in `src/wasm_api.rs` (or whatever module owns
`WorldHandle` post-S1) as `#[cfg(test)] mod tests`. Use the existing
`WorldHandle::new(seed)` constructor to spin up a handle. No
`wasm-bindgen-test` is required — the helpers and the matcher both run
fine on native Rust because `JsValue::from_str` works under `cfg(test)`
when the crate is built for `wasm32-unknown-unknown`, but the simplest
path is to assert against the public setters (no `JsValue` plumbing)
and add ONE test that exercises the `Err` path using
`set_slider("bogus", 0.0)`.

If `JsValue` is unavailable in the test target (it requires the wasm
target), gate the `set_slider_unknown_returns_err` test behind
`#[cfg(target_arch = "wasm32")]` and add a parallel native test that
asserts the matcher's helper-table coverage by exercising every known
name via the string form, expecting `Ok(())`. (The point is to catch
"helper added but match arm forgotten" regressions.)

### Test list

1. `set_base_sun_rate_mutates_field` — call setter, assert
   `world.sliders().base_sun_rate == new_value`. Requires
   `WorldHandle::sliders(&self) -> &DevSliders` test-helper OR direct
   access via `self.inner.sliders` in an in-module `#[cfg(test)]` block
   (preferred — no public API addition).
2. Same shape × 4 for the other sliders. The
   `sun_gradient_strength` test additionally asserts that
   `world.inner.sun.capacity_for_test()` (or whatever the existing test
   hook is) changes — reuse whatever pattern other sun-recompute tests
   use; if none exists, just assert the field changed and document that
   `recompute_capacity` coverage is tested via `sun` module tests.
3. `set_slider_known_names_return_ok` — loop over the 5 names, assert
   `Ok(())` and assert each field changed.
4. `set_slider_unknown_returns_err` — call `set_slider("bogus", 1.0)`
   and assert the result is `Err`. Body of the `Err` (the JsValue
   string) does not need to be asserted on native; if the test is
   wasm-gated, assert `err.as_string().unwrap().contains("bogus")`.

### Non-Rust gates

- `pnpm --filter web typecheck` — must pass (no TS changes, sanity).
- `pnpm --filter web build` — must pass; eyeball
  `web/pkg/evosim.d.ts` to confirm the five new methods appear with the
  expected `(value: number): void` signatures and that `set_slider`
  now has return type `void` (the wasm-bindgen idiom for
  `Result<(), JsValue>` is "returns void, throws on Err").

---

## 8. Determinism impact

None. Slider mutation is unchanged in semantics; only the **error path**
changes (silent no-op → thrown JS error). Goldens are not affected;
`cargo test` should reproduce existing outputs bit-identically.

---

## 9. Risk register

### 6.1 `Result<(), JsValue>` changes JS call shape

Before: `world.set_slider("foo", 0.0)` returns `undefined` unconditionally.
After: same call returns `undefined` on success but **throws** on Err.

- **Caller audit:** only caller is the JS console (BUILD-REPORT KI#4).
  No `try`/`catch` exists anywhere because the previous method couldn't
  fail. An unhandled throw at the console prompt is exactly the desired
  UX. **Risk: nil.**
- **Future TS dev-panel:** the dev-panel will use the typed setters
  (no error path), not the string form. Mitigation: call this out in
  the dev-panel plan when written (see §4).
- **Hot-path concern:** none. Sliders are dev-panel UI, called at human
  rates (≤1 Hz manual drag); a throw cannot crash the sim loop.

### 6.2 API surface growth

Five new wasm-bindgen methods inflate the JS bindings. For v1's 5
sliders this is acceptable (~30 generated TS lines, ~5 wasm exports).
Reassess if/when the slider count exceeds ~10.

### 6.3 Reuse a `DevSliders::set_by_name`-style helper if it exists

`grep -n 'impl DevSliders' src/world.rs` shows only the `Default` impl —
no existing `set_by_name` to reuse. The match table for the string form
must live on `WorldHandle` (because `sun_gradient_strength` has a
side-effect on `self.inner.sun` that `DevSliders` cannot reach).
**Confirmed: no helper to reuse; OK to write the match table fresh.**

### 6.4 Side-effect drift on `sun_gradient_strength`

The current matcher calls `self.inner.sun.recompute_capacity(value)`
after the field write. The per-slider helper
`apply_sun_gradient_strength` MUST preserve this. Add a comment in the
helper body pointing to v6 §D / sun-cap derivation so the side effect
is not stripped by a future refactor.

### 6.5 S1 ordering

If S1 hasn't landed when S17 is implemented, the diff lives entirely
inside `src/wasm_api.rs` and is trivially rebased onto the post-S1
tree. **No blocker.**

---

## 10. Acceptance criteria

- Every slider in `DevSliders` has a corresponding `#[wasm_bindgen]`
  per-slider typed setter on `WorldHandle`.
- `set_slider(name, value) -> Result<(), JsValue>` exists and returns
  `Err` with a message containing the bad name when `name` is not one
  of the 5 known values.
- All 5 per-slider helpers route through a single private
  `apply_<field>` method per slider (no field is mutated in two places).
- `sun_gradient_strength` setter still triggers
  `sun.recompute_capacity(value)`.
- No TS file is modified by S17 (no current callers to migrate).
- `cargo test` clean.
- `pnpm --filter web typecheck` clean.
- `pnpm --filter web build` clean; `evosim.d.ts` shows the five new
  typed methods and a `set_slider` whose generated wrapper throws on Err.

---

## 11. Locked scope (do NOT do in S17)

- Do **not** add new sliders.
- Do **not** change slider min/max ranges or default values.
- Do **not** delete the string form — BUILD-REPORT KI#4 documents it as
  the current console workflow; the audit rejected S6 ("dead code,
  delete") in favour of S17 ("type it properly").
- Do **not** add a `SliderId` wasm-bindgen enum (revisit if slider
  count grows past 10).
- Do **not** build the `~`-hotkey dev panel here — that's a separate
  UI plan and depends on S17 landing first.

---

## Review feedback

**Verdict:** APPROVE WITH MINOR REVISIONS. The plan is tight, well-scoped, and correctly identifies the audit's core resolution (ship typed methods AND a Result-typed string form). It is implementation-ready. Five issues below; only one is non-trivial.

### Issues

1. **[MAJOR] `#[cfg(target_arch = "wasm32")]` gate on the Err-path test is unnecessary and degrades native coverage.**
   §7 proposes gating `set_slider_unknown_returns_err` behind `cfg(target_arch = "wasm32")` because "JsValue may be unavailable in the test target." This is wrong: `wasm-bindgen` provides a `JsValue` stub on native targets (no `wasm-bindgen-test` runtime required), and `src/wasm_api.rs:578-595` already exercises code that uses `JsValue::from_str` under `cargo test`. The Err-path test should just be a plain `#[test]` calling `handle.set_slider("bogus", 0.0).is_err()`. Drop the cfg gate and drop the parallel "native fallback that exercises every name via the string form" — the per-slider tests (#1–2 in §7) plus a single string-form round-trip test cover the same ground.
   *Action:* simplify §7 to one unconditional `#[test]` for Err, and one for known-name round-trip. Removes ~10 lines of plan complexity and avoids a real native coverage hole. Also delete the misleading "JsValue::from_str works under `cfg(test)` when the crate is built for `wasm32-unknown-unknown`" sentence — the truth is simpler: it works under any target.

2. **[MINOR] v6 §K default for `base_sun_rate` is 0.08, not 0.30.**
   The slider inventory in §2 lists `base_sun_rate` default as `0.30` (the value of `SUN_REFILL_RATE`). v6 §K (PITCH-v6.md:118) specifies default `0.08`. This is a pre-existing constant-vs-spec drift, not an S17 concern (§11 correctly locks scope on defaults), but the inventory table should add a footnote: *"Spec default per v6 §K is 0.08; current code uses 0.30 (`SUN_REFILL_RATE`). S17 preserves current code default; reconciliation is out of scope."* Same for §K ranges (`[0, 1.0]`, log-scale): the plan should note that range/clamp enforcement is explicitly deferred and the typed setters accept any `f32` (consistent with the existing matcher).

3. **[MINOR] §6.1 risk register understates the dev-panel migration story.**
   The audit-master row for PR-2 and §4 of the plan both mention the future `~`-hotkey dev panel as the canonical migration target for typed setters. This belongs in the risk register as a *positive* deliverable, not just a side comment: when the dev panel is built (v1.2), the per-slider methods become the contract; the string form remains console-only. Suggest adding §6.6 "Dev-panel rebuild is the canonical migration path" with a one-line cross-reference to the (not-yet-written) dev-panel plan, so future readers don't wonder why the typed methods exist if there are zero TS callers today.

4. **[NIT] Section-numbering: §6.x risk register and §9 are inconsistent.**
   The risks are titled §6.1, §6.2, §6.3, §6.4, §6.5 but live under heading `## 9. Risk register`. Renumber the risk subsections to §9.1–§9.5 (or rename the heading). Editorial only.

5. **[NIT] `wasm_api.rs:171` line range will shift.**
   §1 and §2 cite `src/wasm_api.rs:171-183` as the edit target. Once S1 lands and the file potentially moves/grows, the line numbers will drift. Plan reads as a static spec but is implemented after S1 — recommend changing line cites to method-name anchors (`WorldHandle::set_slider`) with the current line range parenthesized as "(currently L171–183, may shift post-S1)". Mostly applies to §1, §2, §3.2, §5.

### Confirmations (planner's claims verified)

- **Slider inventory is complete.** `grep 'pub.*: f32' src/world.rs` inside `DevSliders` (L27-33) yields exactly the 5 fields listed in §2. No omissions.
- **"Zero TS callers" claim is correct.** Independent `grep -rn 'set_slider\|setSlider' web/src/` returns no matches across all .ts files. The only console-mode caller is documented in `BUILD-REPORT.md:53`. §4's "no TS migration work" is accurate.
- **`Result<(), JsValue>` throws on Err in JS-land.** Confirmed via `web/dist/assets/evosim-*.js` — the existing `fromJson` wrapper emits `if(o[2])throw I(o[1])`. The same codegen will apply to `set_slider`, so a console typo throws (visible) instead of silently no-op'ing. The plan's §6.1 caller audit is correct: there is no `try`/`catch` to break, and a thrown error at the console prompt is the desired UX.
- **Side-effect preservation on `sun_gradient_strength`.** The current matcher's `self.inner.sun.recompute_capacity(value)` is correctly carried into `apply_sun_gradient_strength` (§3.1) and called out in §9.4. Good.
- **No `DevSliders::set_by_name` helper to reuse.** Confirmed; `impl DevSliders` only has the `Default` impl.

### Severity summary
- Blocking: 0
- Major: 1 (item 1 — test-gating choice)
- Minor: 3 (items 2, 3, 4)
- Nit: 1 (item 5)

The plan is safe to implement once item 1 is fixed (drops a real native-coverage hole) and items 2–3 are addressed (documentation hygiene). Items 4–5 are editorial.
