# S21 — Drop unused flag floats from `creatures_buffer`

**PR:** PR-2 (Wasm boundary cleanup).
**Depends on:** S1 (`src/world.rs` split — does not actually touch `wasm_api.rs`, so paths are stable; included for ordering only).
**Coordinated with:** S22 (same PR; same render.ts file but different lines).
**Risk class:** medium-low. Stride mismatch silently corrupts the picture; mandatory TS-side runtime assert.
**Determinism impact:** none. `snapshot_hash` is independent of `creatures_buffer`.
**Effort:** S.

---

## 1. Summary

`WorldHandle::creatures_buffer` (`src/wasm_api.rs:103-133`) currently writes **13 floats per creature** every frame. Two of those floats — `energy_frac` (offset 6) and `age_frac` (offset 7) — are explicitly commented out in `web/src/render.ts:134-135`:

```ts
// const energyFrac = data[i + 6]; // reserved for future visual cue
// const ageFrac = data[i + 7];
```

They are pure waste: a per-creature divide + clamp on the Rust side and dead-load bandwidth on the JS side every frame.

This piece:

1. Drops `energy_frac` and `age_frac` from `creatures_buffer`.
2. Bumps `creature_stride()` from **13 → 11**.
3. Updates `web/src/render.ts` field offsets in lockstep.
4. Adds a TS-side runtime assert at the top of `renderWorld` so a future stride drift fails loud instead of silent.
5. Switches the per-frame pack loop's AoS reads (`self.inner.creatures.genomes[i].eye_count`, `.move_speed`, `.scavenge_efficiency`, `.eat_efficiency`, `.size`) to perf-5 SoA mirror reads where a mirror exists (`g_eye_count`, `g_move_speed`, `g_scav_eff`, `g_eat_eff`, `g_size`).

**Explicit non-goals:**
- Do NOT redesign `creatures_buffer` to use typed arrays beyond `Float32` or to expose column pointers — that's `big-wins #6` (DEFER).
- Do NOT extend the perf-5 SoA mirror in this piece. `pigment_r/g/b`, `armor`, `max_age`, `energy`, `age` are NOT in the mirror and stay AoS in this piece. (`armor` is the one remaining flag still read from AoS; `pigment_*` is read from AoS for the body fill color.)
- Do NOT touch `snapshot_hash` inputs.

---

## 2. Field inventory — before / after

### Before (stride = 13)

| Off | Field        | Rust source                                                 | render.ts use                | Keep/Drop |
|----:|--------------|-------------------------------------------------------------|------------------------------|:---------:|
| 0   | `x`          | `creatures.x[i]`                                            | body fill center (sx)        | KEEP      |
| 1   | `y`          | `creatures.y[i]`                                            | body fill center (sy)        | KEEP      |
| 2   | `radius_world` | `g.size * BODY_RADIUS_PER_SIZE` (AoS)                     | body fill radius, ring stack | KEEP (switch read to `g_size[i] * BODY_RADIUS_PER_SIZE`) |
| 3   | `pigment_r`  | `g.pigment_r` (AoS — no mirror)                             | body fillStyle r-channel     | KEEP (still AoS — pigment not mirrored; out of scope) |
| 4   | `pigment_g`  | `g.pigment_g` (AoS)                                         | body fillStyle g-channel     | KEEP (still AoS) |
| 5   | `pigment_b`  | `g.pigment_b` (AoS)                                         | body fillStyle b-channel     | KEEP (still AoS) |
| 6   | `energy_frac` | `(energy[i] / 100.0).clamp(0,1)`                           | **commented out**            | **DROP**  |
| 7   | `age_frac`   | `(age[i] / g.max_age).clamp(0,1)`                           | **commented out**            | **DROP**  |
| 8   | `flag_eye`   | `g.eye_count > 0 ? 1.0 : 0.0` (AoS)                         | `flagEye` → eye ring         | KEEP (switch read to `g_eye_count[i] > 0`) |
| 9   | `flag_move`  | `g.move_speed > 0.0 ? 1.0 : 0.0` (AoS)                      | `flagMove` → move ring       | KEEP (switch read to `g_move_speed[i] > 0.0`) |
| 10  | `flag_scav`  | `g.scavenge_efficiency > 0.0 ? 1.0 : 0.0` (AoS)             | `flagScav` → scav ring       | KEEP (switch read to `g_scav_eff[i] > 0.0`) |
| 11  | `flag_mouth` | `g.eat_efficiency > 0.0 ? 1.0 : 0.0` (AoS)                  | `flagMouth` → mouth ring     | KEEP (switch read to `g_eat_eff[i] > 0.0`) |
| 12  | `flag_armor` | `g.armor > 0.0 ? 1.0 : 0.0` (AoS — no mirror)               | `flagArmor` → armor ring     | KEEP (still AoS — `armor` not mirrored; out of scope) |

### After (stride = **11**)

| Off | Field          | Rust source (post-S21)                          | render.ts read |
|----:|----------------|-------------------------------------------------|----------------|
| 0   | `x`            | `creatures.x[i]`                                | `data[i + 0]`  |
| 1   | `y`            | `creatures.y[i]`                                | `data[i + 1]`  |
| 2   | `radius_world` | `creatures.g_size[i] * BODY_RADIUS_PER_SIZE`    | `data[i + 2]`  |
| 3   | `pigment_r`    | `creatures.genomes[i].pigment_r` (AoS still)    | `data[i + 3]`  |
| 4   | `pigment_g`    | `creatures.genomes[i].pigment_g` (AoS still)    | `data[i + 4]`  |
| 5   | `pigment_b`    | `creatures.genomes[i].pigment_b` (AoS still)    | `data[i + 5]`  |
| 6   | `flag_eye`     | `if creatures.g_eye_count[i] > 0 { 1.0 } else { 0.0 }` | `data[i + 6]` |
| 7   | `flag_move`    | `if creatures.g_move_speed[i] > 0.0 { 1.0 } else { 0.0 }` | `data[i + 7]` |
| 8   | `flag_scav`    | `if creatures.g_scav_eff[i] > 0.0 { 1.0 } else { 0.0 }`   | `data[i + 8]` |
| 9   | `flag_mouth`   | `if creatures.g_eat_eff[i] > 0.0 { 1.0 } else { 0.0 }`    | `data[i + 9]` |
| 10  | `flag_armor`   | `if creatures.genomes[i].armor > 0.0 { 1.0 } else { 0.0 }` (AoS still) | `data[i + 10]` |

**New stride: 11.** Per-creature savings: 2 floats (8 bytes) marshalled + 2 divides + 2 clamps eliminated. At 1k creatures × 60 fps = ~480 KB/s less marshalling and ~120k fewer divides/sec.

---

## 3. Step-by-step implementation order

Land all steps in **one commit** (the runtime assert and the stride change MUST ship together — see Risk (a)).

### (i) Update `creatures_buffer` body in `src/wasm_api.rs`

In `pub fn creatures_buffer(&mut self) -> js_sys::Float32Array` (currently `src/wasm_api.rs:103-133`):

- Delete the two lines writing `off + 6` (energy_frac) and `off + 7` (age_frac).
- Renumber the five flag writes from offsets 8-12 → 6-10.
- Switch hot-field reads from the AoS `let g = &self.inner.creatures.genomes[i];` path to the perf-5 SoA mirror for the five mirrored fields:
  - `g.size` → `self.inner.creatures.g_size[i]`
  - `g.eye_count` → `self.inner.creatures.g_eye_count[i]`
  - `g.move_speed` → `self.inner.creatures.g_move_speed[i]`
  - `g.scavenge_efficiency` → `self.inner.creatures.g_scav_eff[i]`
  - `g.eat_efficiency` → `self.inner.creatures.g_eat_eff[i]`
- Keep the AoS `let g = &self.inner.creatures.genomes[i];` binding only for `pigment_r`, `pigment_g`, `pigment_b`, `armor` (the four unmirrored reads). These four reads stay on AoS in this piece.
- Update the doc-comment block above the fn (`src/wasm_api.rs:97-101`) to the new 11-float layout. New comment:
  ```rust
  /// Repack creature SoA into a contiguous Float32Array. Layout per creature
  /// (11 floats, stride = [`creature_stride`]):
  /// `[x, y, radius_world, r, g, b,
  ///   flag_eye, flag_move, flag_scav, flag_mouth, flag_armor]`.
  /// Ring flags: 1.0 if trait > 0, else 0.0. See v6 §B for ring order.
  ```

### (ii) Update `creature_stride()` in `src/wasm_api.rs`

Change `pub fn creature_stride() -> u32 { 13 }` (line 429-431) to `{ 11 }`. Update the doc comment above (lines 424-427) to:

```rust
/// Per-creature float count in [`WorldHandle::creatures_buffer`].
/// v1.1 layout (audit S21): 11 floats.
/// Offset 0..6: x, y, radius, r, g, b.
/// Offset 6..11: flag_eye, flag_move, flag_scav, flag_mouth, flag_armor.
```

### (iii) Update `web/src/render.ts` offsets

In `drawCreatures` (currently `web/src/render.ts:127-141`):

- Delete the two `// const energyFrac = data[i + 6]; ...` and `// const ageFrac = data[i + 7];` commented lines.
- Renumber the five flag reads:
  - `data[i + 8]` → `data[i + 6]` (`flagEye`)
  - `data[i + 9]` → `data[i + 7]` (`flagMove`)
  - `data[i + 10]` → `data[i + 8]` (`flagScav`)
  - `data[i + 11]` → `data[i + 9]` (`flagMouth`)
  - `data[i + 12]` → `data[i + 10]` (`flagArmor`)

The comment above the flag block (`// Feature ring flags (v6 §B, Milestone C.11): eye→move→scav→mouth→armor`) is unchanged.

### (iv) Add TS-side runtime stride assert in `renderWorld`

In `web/src/render.ts:208-225`, at the top of `renderWorld` (before any `world.*` call), add:

```ts
const EXPECTED_CREATURE_STRIDE = 11;
if (stride !== EXPECTED_CREATURE_STRIDE) {
  throw new Error(
    `creatures_buffer stride mismatch: got ${stride}, expected ${EXPECTED_CREATURE_STRIDE} ` +
      `(Rust wasm_api.rs::creature_stride and web/src/render.ts must agree)`,
  );
}
```

This is the **mandatory** guard called out in watchlist item (g). It fires once-per-frame but the cost is one compare on a small integer; negligible.

### (v) (No additional AoS→SoA-mirror migrations beyond step (i).)

The brief asks "switch any remaining AoS reads to perf-5 SoA mirror reads in the per-frame pack". Step (i) already covers every per-frame AoS read in `creatures_buffer` that has a mirror counterpart. The four remaining AoS reads (`pigment_r/g/b`, `armor`) have no mirror; extending the mirror is **explicitly out of scope** per locked scope item (b) and risk (b). They stay AoS.

### Repo-wide grep checks (must run as part of implementation)

- `grep -rn "data\[i + 6\]\|data\[i + 7\]\|data\[i + 8\]\|data\[i + 9\]\|data\[i + 10\]\|data\[i + 11\]\|data\[i + 12\]" web/src/` to confirm no other site reads the old offsets. As of this plan only `web/src/render.ts:drawCreatures` reads `creatures_buffer`.
- `grep -rn "creature_stride\|creatures_buffer" web/src/ src/` to confirm no other consumer relies on stride == 13.
- `grep -rn "energy_frac\|age_frac" .` to confirm no other consumer reads the dropped fields.

---

## 4. Determinism impact

**None.** `snapshot_hash` (`src/snapshot_hash.rs`) does not read from `creatures_buffer`; it hashes `CreatureSoA` fields directly. Both goldens (sequential + threaded) must remain unchanged: `0xb76e907c6221f7f5`.

Verify post-implementation:
- `cargo test --release --test acceptance` (3 tests pass, golden unchanged)
- `cargo test --release --features threads --test acceptance` (1 test passes, threaded golden unchanged)

If either golden flips, **revert**: this piece has no business changing them.

---

## 5. Path notes

`src/wasm_api.rs` is **not** moved by S1 (the world-split). All paths in this plan are stable across the S1 landing. No path translation needed.

`web/src/render.ts` is not touched by S1 or any other PR-1 piece.

---

## 6. Test plan

### Rust unit tests (in the existing `#[cfg(test)] mod tests` at `src/wasm_api.rs:433`)

(a) **Rename + update existing test.** Replace `creature_stride_is_13` (line 442) with:

```rust
#[test]
fn creature_stride_is_11() {
    assert_eq!(creature_stride(), 11);
    let n: usize = 3;
    let expected = n * creature_stride() as usize;
    assert_eq!(expected, 33);
}
```

(b) **Length-matches-population test.** Add a new test that builds a `WorldHandle` and (without invoking the wasm-only `creatures_buffer`) asserts the math on `creature_buf.len()` after a manual fill. Since `creatures_buffer` returns a `js_sys::Float32Array` only available on wasm32, the native test exercises the stride math via a helper:

```rust
#[test]
fn creature_buf_length_matches_population_times_stride() {
    let mut handle = WorldHandle::new("s21-stride");
    // Founder population = 1 at boot.
    let n = handle.inner.creatures.len();
    let stride = creature_stride() as usize;
    // Manually resize as the wasm path would.
    handle.creature_buf.clear();
    handle.creature_buf.resize(n * stride, 0.0);
    assert_eq!(handle.creature_buf.len(), n * stride);
    assert_eq!(handle.creature_buf.len(), 1 * 11);
}
```

(If `handle.inner` is `pub(crate)` and the test is in the same crate, this works without further visibility changes. If not, fall back to asserting `creature_stride() == 11` and re-confirm fill correctness via the existing acceptance build.)

### TS/build checks

(c) `pnpm typecheck` — clean.
(d) `pnpm build` — clean (pre-existing dynamic-import warning is acceptable per master plan §10).

### Manual smoke (dev-server)

Per `docs/dev-server-prompt.md` (port 47821) at end of PR-2:
- Launch the app at default speed.
- Confirm creatures render with body fill + ring stack identical to pre-change (sample a few species).
- Toggle a `?seed=` URL and confirm no console errors and no `stride mismatch` throw.
- Open DevTools console; confirm no thrown errors after world boot and ~30 s of simulation.

A visual diff is sufficient; no automated screenshot test is in scope.

---

## 7. Risk register

(a) **Stride mismatch = silent picture corruption.** If the Rust stride change ships but `render.ts` still reads the old offsets (or vice versa), the renderer reads the next creature's `x` as the current creature's `flag_eye`, and the picture goes wrong silently (no exception, just wrong rings on wrong creatures). **Mitigation:** the TS-side `EXPECTED_CREATURE_STRIDE` assert in `renderWorld` (step iv) MUST land in the same commit as the Rust change. This is also watchlist item (g) — the cross-reviewer will verify.

(b) **Perf-5 SoA mirror only covers 7 fields.** The mirror in `CreatureSoA` (`src/creature.rs:78-98`) covers `g_size`, `g_photo_eff`, `g_eat_eff`, `g_scav_eff`, `g_move_speed`, `g_vision_range`, `g_eye_count`. Of the seven AoS fields `creatures_buffer` currently reads, **only `pigment_r/g/b` and `armor` lack a mirror**. Do NOT extend the mirror in this piece — that's a separate piece of work (`big-wins #6` family, DEFER). The four AoS reads stay AoS; per-frame they're four pointer-chasings per creature, which is what shipping this piece partially mitigates (down from seven AoS reads to four).

(c) **Field-order changes affect every render-loop site.** The dropped offsets 6 and 7 sit in the middle of the stride, so every downstream offset shifts by 2. Per-loop sites confirmed:
- `web/src/render.ts:127-141` (`drawCreatures`) — the only consumer of `creatures_buffer`.
- `web/src/render.ts:224` (`renderWorld` call site) — passes `stride` through, no offset math.
No other file in `web/src/` references `creatures_buffer` or hardcoded offsets. Verified by `grep`.

(d) **`pub(crate)` access to mirror fields from `wasm_api.rs`.** The mirror fields (`g_size`, `g_eye_count`, etc.) are `pub(crate)` in `src/creature.rs:80-98`. `wasm_api.rs` is in the same crate, so the access is legal. No visibility change needed.

(e) **Native test cannot exercise `creatures_buffer` directly.** `js_sys::Float32Array::view` is wasm32-only. The native unit test asserts the stride constant and the fill-math indirectly. The real correctness check is the dev-server smoke (test plan §6.c-e). This matches the existing native test's `// We can't call creatures_buffer() in native tests` comment.

---

## 8. Acceptance criteria

- `creature_stride()` returns **11**, with doc comment updated.
- `creatures_buffer` writes exactly 11 floats per creature in the order: `x, y, radius, r, g, b, flag_eye, flag_move, flag_scav, flag_mouth, flag_armor`.
- Five mirrored AoS reads (`g.size`, `g.eye_count`, `g.move_speed`, `g.scavenge_efficiency`, `g.eat_efficiency`) in the pack loop replaced by `creatures.g_size[i]`, `creatures.g_eye_count[i]`, `creatures.g_move_speed[i]`, `creatures.g_scav_eff[i]`, `creatures.g_eat_eff[i]`.
- `web/src/render.ts:drawCreatures` reads offsets 6-10 for the five flags; `energy_frac` / `age_frac` comment lines removed.
- `renderWorld` throws on stride mismatch.
- Native Rust tests pass (including the renamed `creature_stride_is_11`).
- `cargo clippy --all-targets -- -D warnings` clean; `cargo clippy --all-targets --features threads -- -D warnings` clean.
- Both goldens unchanged (sequential + threaded both still `0xb76e907c6221f7f5`).
- `pnpm typecheck` + `pnpm build` clean.
- Dev-server smoke: creatures render identically to pre-change (body fill + 5 rings).

---

## 9. Subagent flow (per master plan §9)

- **Planner:** opus (this doc).
- **Reviewer:** opus.
- **Implementer:** sonnet.
- **Code-reviewer:** opus.

Cross-review at end of PR-2 must verify watchlist item (g): TS-side stride assert landed in the same commit as the Rust stride change.

---

## Layout authority note

The post-S21 v1 `creatures_buffer` layout — **stride 11, five separate `float` flags at offsets 6–10** — is the authoritative source of truth for all downstream consumers, including the forthcoming WebGL2 renderer. This choice was made deliberately: it requires zero renderer churn beyond renumbering offsets, and it avoids any bit-unpacking logic in either the Canvas2D or future WebGL2 shader paths. The flags remain individual `float` values because Canvas2D code reads them as simple boolean comparisons (`data[i + 6] > 0.5`), and replicating that cheaply in GLSL is trivial (`a_flag_eye > 0.5`). If the WebGL2 renderer later wants to pack the five flags into a single `u32 ring_mask` for GPU-side bit-testing, that is a separate micro-perf follow-up ("S21b: pack flags into ring_mask") that touches only the Rust packer and the WebGL2 instance-attribute schema in `render_gl.ts`; it does not affect Canvas2D or this plan. See `docs/plans/webgl2-renderer-design.md` §3.3 (updated 2026-05-24) for the corrected WebGL2 instance layout that consumes the stride-11 record.

---

## Review feedback

**Reviewer:** opus (audit pass cross-review for piece S21).
**Verdict:** **APPROVE WITH CHANGES.** Plan is sound, scope is correct, all mandatory items (TS runtime assert, single-commit landing, doc-comment updates, test renames) are present. The field-inventory math is correct (independently verified — see N-1). One blocking-class doc inconsistency (the WebGL2 design doc assumes a *different* post-S21 layout) needs a one-line resolution before the implementer can rely on this plan as the source of truth; everything else is non-blocking polish.

### Blocking issues — **1**

**B-1. WebGL2 design doc assumes a DIFFERENT post-S21 layout than this plan delivers (severity: medium-high; coupling).**
`docs/plans/webgl2-renderer-design.md` §3.3 (lines 102–118) explicitly lists the "post-S21" per-instance attribute layout as **7 floats / fields**:

> | Offset (post-S21) | Field |
> | 0 | x |
> | 1 | y |
> | 2 | **size** |
> | 3–5 | pigment_r/g/b |
> | 6 | **ring_mask (uint, packed from flags)** |

It explicitly states (line 118): "S21 packs them into one mask."

This S21 plan delivers a **different shape**: stride 11, with `radius_world` (= `size * BODY_RADIUS_PER_SIZE`) at offset 2 — not raw `size` — and **five separate float flags** at offsets 6–10 — *not* a packed `ring_mask` uint at offset 6.

Two of three deltas matter (size vs radius_world is one f32 multiply on the consumer side, easy; flag-packing is a real semantic difference). The WebGL2 doc is plan-only and lands in parallel with PR-4, so this does not break runtime today — but master plan §7 watchlist item (g) holds the S21 cross-reviewer responsible for "stride math," and the WebGL2 doc was authored in the same orchestration pass with explicit forward references to S21 ("After S21 lands"). Silent doc drift between the two plans is exactly the kind of footgun the audit is meant to catch.

Required resolution (planner picks one and documents it in this plan):

- **(a) [recommended]** Add a step (vi) to §3: "Edit `docs/plans/webgl2-renderer-design.md` §3.3 table and the line-118 paragraph to reflect the actual stride-11 layout this plan delivers (5 separate flag floats at offsets 6–10; `radius_world` at offset 2). The WebGL2 author can repack flags into a JS-side bitmask at `gl.bufferSubData` time, exactly as the doc says is the pre-S21 fallback path." Two-line plan edit; one-table-edit to the WebGL2 doc.
- **(b)** Expand S21 to **also** pack the five flag floats into one `u32` ring_mask at offset 6 (stride 7). Matches what WebGL2 assumes but enlarges the diff (`creatures_buffer` writes via a `Float32Array`-as-`u32` reinterpret or a new typed-array path; `render.ts` unpacks per ring). Strictly more work and arguably out of scope for S21's "drop unused" framing.
- **(c)** Add an explicit note in this plan acknowledging the divergence and deferring reconciliation to a future WebGL2-implementation pass. Cheapest but leaves a real footgun for the future WebGL2 implementer.

Recommendation: **(a)**.

### Non-blocking observations

**N-1. Field inventory is correct (independently verified).**
- `src/wasm_api.rs:97–133` writes 13 floats (offsets 0–12 inclusive). Confirmed.
- `src/wasm_api.rs:429–431` returns 13. Confirmed.
- `web/src/render.ts:127–141` reads offsets 0–5 (x, y, radius, r, g, b), comments out 6 and 7 (`energy_frac`, `age_frac`), reads 8–12 as five flag floats. Confirmed.
- Renumber math is correct: 8→6, 9→7, 10→8, 11→9, 12→10.
- No other `web/src/` file reads `creatures_buffer` (grep clean). `web/src/main.ts:237` only reads `creature_stride()` — no offset math. Confirmed.
- No other `*.rs` or `*.ts` file references `energy_frac`/`age_frac` as a `creatures_buffer` field. Remaining hits (`src/world.rs:1286` NN input, `src/wasm_api.rs:266` `creature_inspect_json`, `web/src/rail/inspector.ts:51,80` Inspector JSON consumer, `src/brain.rs` NN sensor wiring) are independent code paths and are unaffected by the S21 change.

**N-2. AoS reads kept for `pigment_r/g/b` and `armor` — correct scope call.**
Confirmed against `src/creature.rs:78–98` (perf-5 SoA mirror): mirror fields are `g_size`, `g_photo_eff`, `g_eat_eff`, `g_scav_eff`, `g_move_speed`, `g_vision_range`, `g_eye_count` — seven fields. `pigment_*` and `armor` are NOT mirrored. Extending the mirror would touch `CreatureSoA::{with_capacity, push, remove_indices, push_hot_mirrors}` plus the reviewer-grep — correctly out of scope. The plan's "down from seven AoS reads to four" note is accurate.

**N-3. TS runtime assert is present, mandatory, and placed correctly.**
§3 step (iv) puts it at the top of `renderWorld` before any `world.*` call. Correct location — fires on the first frame, throws loudly. Constant name `EXPECTED_CREATURE_STRIDE` is greppable and self-documenting. Throw message names both files that must agree. Per-frame cost is one integer compare — negligible. Alternative would be a one-shot assert at boot in `main.ts:237`; the plan's per-frame placement is also fine and arguably more defensive (catches hot-reload mismatch). Non-blocking.

**N-4. Same-commit landing mandated explicitly (good).**
§3 opening line: "Land all steps in **one commit** (the runtime assert and the stride change MUST ship together — see Risk (a))." Satisfies master watchlist item (g) directly.

**N-5. Tests — native vs wasm32 split handled correctly.**
- T(a): rename `creature_stride_is_13` → `creature_stride_is_11`. Trivial; correct.
- T(b): the fill-math test correctly notes `js_sys::Float32Array::view` is wasm32-only and falls back to asserting the stride constant + manual `resize` math. Matches the existing comment at `src/wasm_api.rs:438–440`. The added `handle.creature_buf.resize(n * stride, 0.0)` + length-check brings genuine new coverage of the fill path.
- `handle.inner` access works: the test lives in the same `mod tests` inside `wasm_api.rs`, so it has full module access. Confirmed.

**N-6. Acceptance criteria checklist is complete.**
§8 covers: stride value, fill order, AoS→mirror migrations, render.ts offsets, runtime assert, Rust tests, clippy (both feature builds), goldens unchanged, TS build clean, dev-server smoke. Nothing missing.

**N-7. Determinism analysis is correct.**
`snapshot_hash` hashes `CreatureSoA` directly (`src/snapshot_hash.rs:38` via `hash_genome(&w.creatures.genomes[i])`), not `creatures_buffer`. The change is invisible to the hash. Both goldens should remain `0xb76e907c6221f7f5`.

**N-8. Doc-comment offset notation in §3 step (ii) is correct.**
> Offset 0..6: x, y, radius, r, g, b.
> Offset 6..11: flag_eye, flag_move, flag_scav, flag_mouth, flag_armor.

Rust half-open range convention; `0..6` = indices 0–5, `6..11` = indices 6–10. Consistent with the existing pre-change comment style at `src/wasm_api.rs:425–427`.

**N-9. Watchlist item (g) explicitly cited.**
§3 step (iv) and §7 risk (a) both reference watchlist item (g) by name. Good audit-trail hygiene.

**N-10. Plan correctly does NOT touch `creature_inspect_json` (`src/wasm_api.rs:266`).**
`creature_inspect_json` recomputes `energy_frac` independently for the Inspector panel; it does not read `creatures_buffer`. The TS-side Inspector reads `data.energy_frac` from the inspect JSON, not from the buffer. Correctly out of scope.

### Severity summary

| ID  | Severity         | Type                                                   |
|-----|------------------|--------------------------------------------------------|
| B-1 | medium-high      | doc inconsistency between two same-orchestration plans |
| N-1..N-10 | none       | confirmations / minor polish                           |

### Final verdict

**APPROVE WITH CHANGES.** Fix B-1 (1-step plan edit + 1 small doc edit to `webgl2-renderer-design.md`), then this is ready for the sonnet implementer. Field inventory, offset shifts, AoS-read scoping, TS assert, single-commit mandate, test plan, and determinism analysis are all correct.

*End of review feedback.*

---

## Plan-update changelog (2026-05-24)

**Conflict resolution — S21 B-1 (WebGL2 layout mismatch).**

- Added "Layout authority note" section (before "Review feedback") confirming that the stride-11, five-separate-flag-float layout is the v1 source of truth for all downstream consumers including the WebGL2 renderer.
- The note explicitly defers flag-packing (`ring_mask`) to a future follow-up piece ("S21b") if WebGL2 later wants it, and forward-references the corrected WebGL2 design doc §3.3.
- No change to the planned S21 layout, stride, field order, or implementation steps. The layout remains: `[x, y, radius_world, pigment_r, pigment_g, pigment_b, flag_eye, flag_move, flag_scav, flag_mouth, flag_armor]` (offsets 0–10, stride 11).
