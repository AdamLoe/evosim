# Audit S8 — Replace `serde_json::to_vec(&w.rng)` with direct hash of 4 xoshiro u64s

**Status:** plan
**Date:** 2026-05-24
**PR:** PR-3 (determinism + correctness + regen)
**Pair:** lands in **one commit with S7** (`docs/plans/audit-s7-snapshot-hash-coverage.md`)
**Source items:** triage `S8`; master plan §4 S8; `docs/audit/determinism.md` R4; watchlist (b)

> **Precondition:** S37 (twox-hash 1→2) bootstrap must complete in PR-1 with byte-identical goldens. If S37 drifts, this plan's regen is folded with S37's into the PR-3 ceremony (no other change).

---

## 1. Summary

Replace the current `serde_json::to_vec(&w.rng)` byte stream at
`src/snapshot_hash.rs:75-77` with four direct `h.write_u64(s_i)` calls reading
the xoshiro256++ internal state `[u64; 4]` straight out of `SimRng`. This:

- Removes serde_json (and its formatting policy) from the snapshot-hash data
  path. Today's hash is silently coupled to `serde_json`'s decimal-u64 output
  shape: any patch bump that emits the same numbers as e.g. a JSON array
  instead of an object would flip both goldens with **no source change** here.
  (`determinism.md` R4.)
- Makes the RNG hash byte sequence explicit and code-reviewable (4 × u64 LE,
  documented order `s0..s3`).
- Reduces a per-hash heap allocation (`serde_json::to_vec` returns
  `Vec<u8>`) to four pointer-free `write_u64` calls.

Determinism impact: **regen** (intended). Piggybacks the S7 regen ceremony;
**no separate regen for S8**.

---

## 2. RNG state access — the chosen approach

`rand_xoshiro = "0.6"` (`Cargo.toml:18`) does **not** expose a public
`get_state` / `from_state` API on `Xoshiro256PlusPlus`. Verified by reading
`~/.cargo/registry/.../rand_xoshiro-0.6.0/src/xoshiro256plusplus.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature="serde1", derive(Serialize, Deserialize))]
pub struct Xoshiro256PlusPlus {
    s: [u64; 4],   // PRIVATE
}
```

The field `s` is private. `impl SeedableRng` exposes `from_seed([u8; 32])` and
`seed_from_u64(u64)`, and `RngCore` exposes `next_u64` / `fill_bytes` — but
none of these let us **observe** current state without advancing it.

### Options considered

| # | Option | Verdict |
|---|---|---|
| (a) | `unsafe` `transmute` of `&Xoshiro256PlusPlus` to `&[u64; 4]` | **Rejected.** UB-adjacent. Layout is repr-Rust; not guaranteed stable. |
| (b) | Clone rng + `next_u64()` × 4 to "read" state | **Wrong.** Returns *future outputs*, not current state. |
| (c) | `serde_json::to_value(&rng)["s"]` one-shot extraction | **Chosen (primary).** ~10 LOC. The output is four parsed `u64` values — independent of serde_json's output format. See trade-off note below. |
| (d) | Custom `serde::Serializer` that intercepts the `[u64; 4]` field | **Future work.** Correct and dep-free, but 80–120 LOC of boilerplate stubs. Switch to this if `serde_json` is ever removed from the dep graph entirely. |
| (e) | Add `pub(crate) fn state(&self) -> [u64; 4]` on `SimRng` backed by (c) or (d) | **Chosen as the public accessor shape** regardless of backing. Keeps the extraction logic out of `snapshot_hash.rs`. |
| (f) | Hash `rand_core`'s `fill_bytes(&mut [u8; 32])` on a **clone** | **Rejected.** `fill_bytes` is implemented via `next_u64`, advancing the clone's state. Same problem as (b). |

### Chosen accessor design

In `src/rng.rs`, add:

```rust
impl SimRng {
    /// Return the four `u64`s of the xoshiro256++ internal state.
    ///
    /// Used by `snapshot_hash` to fold the RNG into the canonical hash
    /// without going through `serde_json`'s byte stream. The field
    /// `Xoshiro256PlusPlus::s` is private upstream; this accessor extracts
    /// it via `serde_json::to_value` as a one-shot field extractor, then
    /// parses the four `u64` values. The hash byte stream is independent of
    /// `serde_json`'s output format because we hash the parsed `u64` values,
    /// not the JSON bytes.
    ///
    /// Stable: hinges on `rand_xoshiro 0.6`'s `Serialize` derive emitting
    /// the four state words in the field `"s"`. The pinned version is `0.6`
    /// (`Cargo.toml`). If `rand_xoshiro` ever bumps and changes the field
    /// name or shape, this accessor must be updated.
    ///
    /// Future: if `serde_json` is ever removed from the dep graph, replace
    /// this body with a custom `serde::Serializer` (option (d) above).
    pub(crate) fn state(&self) -> [u64; 4] {
        let v = serde_json::to_value(&self.0)
            .expect("Xoshiro256PlusPlus serialization is infallible");
        let arr = v["s"].as_array()
            .expect("rand_xoshiro Serialize derive must emit field 's' as array");
        debug_assert_eq!(arr.len(), 4, "xoshiro256++ has exactly 4 state words");
        [
            arr[0].as_u64().expect("state word 0 must be u64"),
            arr[1].as_u64().expect("state word 1 must be u64"),
            arr[2].as_u64().expect("state word 2 must be u64"),
            arr[3].as_u64().expect("state word 3 must be u64"),
        ]
    }
}
```

Approximate size: ~15 LOC. No new dep — `serde_json` is already in the dep
graph for save/load and wasm-api JSON paths.

### Trade-off note (why this is acceptable despite using serde_json)

S8's goal was to remove `serde_json` from the **hash byte stream** — the
sequence of bytes that feeds `XxHash64`. That goal is fully achieved:

- The old path: `serde_json::to_vec(&rng)` → raw JSON bytes → `h.write(&bytes)`.
  A `serde_json` formatting bump (e.g. reordering keys, changing number repr)
  would change the hash even with no source change.
- The new path: `serde_json::to_value(&rng)["s"]` → four parsed `u64` values
  → `h.write_u64(s0); ... h.write_u64(s3)`. A `serde_json` formatting bump
  cannot change the numeric value of a parsed `u64`. **Determinism is
  preserved against serde_json version bumps.**

The residual dependency: `serde_json` is still used at *accessor call time*
(once per `snapshot_hash` call, a heap-allocating but infrequent path). This
is acceptable because:

1. `serde_json` stays in `Cargo.toml` regardless (save/load, wasm-api).
2. The hash byte stream itself is serde_json-independent.
3. If `serde_json` is ever removed, the accessor body becomes the ~100 LOC
   custom `serde::Serializer` (option (d)); the call site in `snapshot_hash.rs`
   is unchanged.

---

## 3. Hash byte order

Both targets are little-endian (`x86_64-unknown-linux-gnu` for native test
runs, `wasm32-unknown-unknown` for browser builds).

**Endianness of twox-hash 1.6:** `twox-hash 1.6.3` provides only
`fn write(&mut self, bytes: &[u8])` explicitly in its `Hasher` impl. The
numeric writers `write_u32` / `write_u64` are **inherited from
`std::hash::Hasher`'s default impls**, which use **`to_ne_bytes()`** (native
endian), not `to_le_bytes()`. Both build targets (x86_64-linux,
wasm32-unknown) are LE, so the byte stream is LE on all real runtime
targets — but the mechanism is native-endian, not an explicit LE guarantee.

After S37 bumps to twox-hash 2.x, the implementer of S37 should verify
whether v2 overrides the numeric writers; if so, the inline comment can be
updated to whatever v2 guarantees. For S7+S8, the LE-on-LE-host conclusion
is unchanged.

**Convention used in this codebase** (`src/snapshot_hash.rs:25-37`): the
existing hash code already uses `h.write_u32(w.tick)`, `h.write_u64(id)`,
etc. — never explicit `to_le_bytes`. S8 follows the same convention for
visual consistency.

Replacement code in `src/snapshot_hash.rs:75-77` becomes exactly:

```rust
// (6) RNG state — four u64s of xoshiro256++ internal state, in stored order.
// Hashed via Hasher::write_u64 (native-endian → LE on both real targets).
// Replaces serde_json byte stream (v1.0 approach; see DECISIONS audit v1.1).
let state = w.rng.state();
h.write_u64(state[0]);
h.write_u64(state[1]);
h.write_u64(state[2]);
h.write_u64(state[3]);
```

The order `s[0], s[1], s[2], s[3]` matches the storage order in
`Xoshiro256PlusPlus::s: [u64; 4]` and is the same order `from_seed`
populates via `read_u64_into(&seed, &mut state)`. Stable across versions
within `rand_xoshiro 0.6.x`.

`#[cfg(target_endian = "little")]` debug-assert is **not** added — both build
targets are LE; an attempt to compile for a BE target would already break
other parts of the codebase (e.g. `creatures_buffer` which exposes `f32`s
to JS as raw bytes assuming LE).

---

## 4. Commit pairing with S7

S7 (`audit-s7-snapshot-hash-coverage.md`) extends snapshot-hash field
coverage; S8 changes the RNG section. Both regen the goldens. Per master
plan watchlist (b): **one commit, one regen.**

Suggested commit message stem (final wording up to implementer):

```
fix(snapshot-hash): extend coverage + replace serde_json rng with direct u64×4

S7: hash digestion_cooldown, species_id, parent_species_id, ... (full list
    in audit-s7-snapshot-hash-coverage.md §1)
S8: hash xoshiro256++ state directly via SimRng::state() instead of
    serde_json::to_vec(&rng); removes invisible coupling to serde_json
    output stability (determinism.md R4).

Both goldens regenerated in this commit per audit-master.md §8.
```

Goldens regen ceremony per master §8 runs **once after** the entire PR-3
batch lands, not within this commit. See "Step-by-step implementation order"
below for the precise sequencing.

---

## 5. Step-by-step implementation order

1. **Add `state()` accessor** in `src/rng.rs`:
   - Add the `pub(crate) fn state(&self) -> [u64; 4]` method on `SimRng`
     using the `serde_json::to_value` one-shot extractor (see §2 code block).
   - No `state_capture` module needed — the accessor is ~15 LOC inline.
   - Add unit test `rng_state_is_4_u64`: construct `SimRng::from_u64(7)`,
     call `state()`, assert array is not `[0; 4]` (xoshiro never has all-zero
     state by construction — `deal_with_zero_seed!` macro guards it). Also
     assert two successive calls without advancing return identical arrays.
   - Add unit test `rng_state_changes_after_next_u64`: capture state, call
     `next_u64()`, capture again, assert the arrays differ.
   - Add unit test `rng_state_round_trip_matches_serde`: serialize the rng
     via the existing serde derive, parse out the `s` array via a separate
     `serde_json::to_value`, compare to `state()`. This guards against the
     extractor drifting from the derive.
2. **Replace the snapshot-hash call** in `src/snapshot_hash.rs`:
   - Delete the `serde_json::to_vec(&w.rng).expect(...)` line and the
     subsequent `h.write(&rng_bytes)`.
   - Insert `let state = w.rng.state();` then the four `h.write_u64(state[i])`
     calls per §3 above.
   - Update the doc comment at the top of `snapshot_hash.rs` (lines 6–13)
     to reflect "(6) RNG state — 4 u64s of xoshiro256++ internal state,
     native-endian (LE on both real targets) via `write_u64`" instead of
     "serde_json bytes of SimRng".
   - Confirm `serde_json` is no longer **directly referenced** in
     `snapshot_hash.rs` (it was only used at line 76 for `to_vec`). Remove
     any `use serde_json` import if present. (Note: `serde_json` stays in
     `Cargo.toml` — still used by save/load, wasm-api JSON, and `rng.state()`.)
3. **Add snapshot-hash unit test** `rng_hash_changes_after_step` (see §6).
4. **Confirm both existing goldens are rejected.** Run
   `cargo test --release --test acceptance` and
   `cargo test --release --features threads --test acceptance` locally
   **without** regen env vars. Both `acceptance_t10000` and
   `acceptance_t10000_threaded` MUST fail with `mismatched hash` (because
   the RNG hash bytes changed). Capture the new hashes printed in stdout
   for the PR-3 cross-reviewer's records. **Do not** regen yet — wait for
   the end-of-PR-3 ceremony so the regen captures S7's changes too.
5. **Wait for PR-3 regen ceremony** per master §8. Both goldens then get
   re-pinned to their new combined-S7+S8 values, and `DECISIONS.md` records
   the change.

---

## 6. Test plan

### New unit tests in `src/snapshot_hash.rs`

**`rng_hash_changes_after_step`** — build a tiny world, hash, advance the
RNG via one `tick_once()`, hash again, assert difference. This proves the
RNG bytes actually feed into the hash function (catches the regression of
"someone deleted the RNG write block by accident"):

```rust
#[test]
fn rng_hash_changes_after_step() {
    let mut w = World::new("rng-hash-changes");
    let h_before = snapshot_hash(&w);
    w.tick_once();
    let h_after = snapshot_hash(&w);
    assert_ne!(
        h_before, h_after,
        "snapshot hash must reflect RNG state after a step"
    );
}
```

Note: a single `tick_once()` advances multiple sim fields, not just RNG —
this test asserts the *aggregate* hash changes, which is the meaningful
guarantee for the snapshot consumer. A tighter test "only RNG changed →
only hash bytes from RNG block differ" requires constructing a world with
no mutation/birth/death, which is fragile to set up. The current test is
sufficient.

### New unit tests in `src/rng.rs`

**`rng_state_is_4_u64`** — capture state on a fresh `SimRng`, assert it's
populated (not `[0; 4]`).

**`rng_state_changes_after_next_u64`** — capture, advance, capture again,
assert difference.

**`rng_state_round_trip_matches_serde`** — for paranoia: serialize the rng
via the existing serde derive, parse out the `s` array via `serde_json`,
compare to `state()`. This is a one-off test guarding against the
custom serializer drifting from the derive. (Implementer may skip this if
the custom serializer code is small enough to eyeball.)

### Existing tests that must continue to pass

- `snapshot_hash_is_deterministic` (`src/snapshot_hash.rs:131`).
- `snapshot_hash_same_seed_same_hash` (`src/snapshot_hash.rs:146`).
- All `src/rng.rs` tests (`same_seed_same_stream`, `unit_in_range`,
  `normal_finite`, `geom_skip_edges`).

### Acceptance tests

- `tests/acceptance.rs` `acceptance_t10000` — **will fail before regen**
  (this is intended; it's the regen signal).
- `tests/acceptance.rs` `acceptance_t10000_threaded` — same.

After the PR-3 regen ceremony, all four acceptance variants pass against
the new hashes.

---

## 7. Path notes

S1 (world split) does **not** move `src/snapshot_hash.rs` or `src/rng.rs` —
both stay at their current top-level paths. No path translation needed for
S8.

---

## 8. Determinism impact

**Regen.** Intended. Same regen event as S7. The byte sequence fed to
`XxHash64` changes from `serde_json::to_vec(&rng).unwrap()` (a JSON object
like `{"0":{"s":[123,456,789,012]}}` with some bracketing — exact bytes
serde_json-version-specific) to four 8-byte LE words concatenated. New
hashes will differ from `0xb76e907c6221f7f5`.

Both sequential and threaded goldens will move. If they move to the **same**
new value, that's the expected outcome (sequential and threaded sims are
bit-identical today, per `determinism.md`); if they diverge, that's covered
by watchlist (e) — record under `DECISIONS.md` as "audit v1.1 — sequential
and threaded snapshot_hash diverge post-S7/S8".

The old `DECISIONS.md` line "RNG state — serde_json bytes of SimRng" in the
hash-input doc comment is updated to "4 u64s of xoshiro256++ internal state,
native-endian (LE on both real targets) via `write_u64`."

---

## 9. Risk register

(a) **`rand_xoshiro` does not expose state.** Confirmed — `s: [u64; 4]` is
private (`xoshiro256plusplus.rs:25`). Mitigation: `serde_json::to_value`
one-shot extractor (primary), or custom `serde::Serializer` if `serde_json`
is later removed. The accessor is opaque to call sites; if upstream ever adds
`pub fn get_state(&self) -> [u64; 4]`, the accessor body becomes a one-liner.

(b) **Endianness portability.** Both build targets (`x86_64-linux-gnu`
native test + `wasm32-unknown-unknown` browser) are little-endian. A
hypothetical BE target would already break `creatures_buffer` (which
ships raw `f32` bytes to JS assuming LE). No `cfg(target_endian = "big")`
guard is added.

(c) **Order of the 4 u64s.** Stable. We hash `s[0], s[1], s[2], s[3]` in
storage order — the same order `from_seed` populates via
`read_u64_into(&seed, &mut state)`. Documented in the comment block and
the accessor's doc-comment.

(d) **`serde_json::to_vec(&w.rng)` may have been hashing more than just
the state.** Verified by reading `xoshiro256plusplus.rs`: the
`Xoshiro256PlusPlus` struct has exactly one field, `s: [u64; 4]`. The
serde derive emits a single struct field; no version tag, no rng-stream
metadata, nothing else. **Hashing only the state is byte-equivalent in
information content.** The old hash bytes differ from the new ones only in
JSON formatting overhead (`{"s":[...]}` brackets, decimal digit reprs of
the u64s), not in determinism content. This is why we don't need to add
any other RNG field to the hash to preserve coverage.

(e) **`serde_json::to_value` extractor depends on the derive's field name.** If
`rand_xoshiro` 0.7+ renames the field `"s"` to something else, the
`v["s"].as_array()` call panics. Mitigation: the accessor `expect()` message
is explicit; `rng_state_round_trip_matches_serde` unit test will catch the
mismatch immediately. `rand_xoshiro` is pinned to `0.6` in `Cargo.toml`,
so no silent drift can occur.

(f) **PR-3 cross-pieces also regen.** S7 changes the rest of the hash too;
the combined regen captures both. S4/S5/S6/S11 (defensive bytes-identical
items in PR-3) will ride the same regen with no extra ceremony.

---

## 10. Acceptance criteria

This piece is "done" when **all** of the following hold:

- [ ] `serde_json` is no longer referenced anywhere in `src/snapshot_hash.rs`
      (`grep -n serde_json src/snapshot_hash.rs` returns nothing).
- [ ] `SimRng::state(&self) -> [u64; 4]` exists in `src/rng.rs` with
      `pub(crate)` visibility, implemented via `serde_json::to_value` one-shot
      extractor (≈15 LOC, no `state_capture` module needed).
- [ ] The block `// (6) RNG state` in `src/snapshot_hash.rs` consists of
      four `h.write_u64(state[i])` calls in the order `i = 0..=3` with a
      doc comment noting "native-endian → LE on both real targets".
- [ ] New unit tests pass: `rng_hash_changes_after_step` (in
      `snapshot_hash.rs`), `rng_state_is_4_u64` and
      `rng_state_changes_after_next_u64` (in `rng.rs`).
- [ ] All existing tests pass under both default and `--features threads`
      builds.
- [ ] `cargo clippy --all-targets -- -D warnings` and
      `cargo clippy --all-targets --features threads -- -D warnings` clean.
- [ ] Acceptance tests `acceptance_t10000` and
      `acceptance_t10000_threaded` **fail with mismatched hash before the
      PR-3 regen** (this is the expected, recorded signal).
- [ ] After the PR-3 regen ceremony per master §8: both goldens re-pinned;
      all four acceptance test invocations pass; `DECISIONS.md` updated.

---

## 11. Locked out of scope

(per the briefing — do **not** broaden the plan to these areas in
implementation)

- The rest of the hash function — S7 owns that.
- Golden regen — ceremony is end-of-PR-3, not in this commit.
- RNG semantics or seeding changes — `SimRng::from_string`, `next_u64`,
  `normal`, `geom_skip` etc. are untouched.
- Removing `serde_json` from `Cargo.toml` — it is still used by save/load.
- Replacing `serde_json` elsewhere in the codebase (e.g. `wasm_api.rs`'s
  `snapshot_json`).
- Switching the hash function away from `XxHash64`. (S37 in PR-1 bumps the
  crate version but not the algorithm.)
- Cross-platform endianness work — both real targets are LE.

---

## 12. Pattern reference

The existing `h.write_u32(w.tick)` and `h.write_u64(w.creatures.id[i])`
style at `src/snapshot_hash.rs:25-37`. Same idiom: numeric primitive →
`write_<width>` on the hasher, no intermediate buffer.

---

## Review feedback (pair S7+S8)

Reviewer: opus, 2026-05-24. Cross-reference: `docs/plans/audit-s7-snapshot-hash-coverage.md` review block.

### Verdict
**APPROVE WITH RECOMMENDED SIMPLIFICATION.** Plan is correct in shape; the
goal (remove `serde_json` from the hash byte stream) is well-motivated and
the regen-pairing with S7 is right. One blocking issue (endianness wording)
and one strong recommendation (drop the custom Serializer for the fallback
path).

### Blocking issues (1)

**B1. Endianness wording in §3 is technically wrong for twox-hash 1.6.** §3
asserts: "`twox_hash::XxHash64`'s `write_u64` is implemented as `to_le_bytes`
then `write(&buf)` — on LE hosts it's a no-op on bytes either way." This is
incorrect. `twox-hash 1.6.3` only implements `fn write(&mut self, bytes:
&[u8])` (verified at
`~/.cargo/registry/.../twox-hash-1.6.3/src/sixty_four.rs:275-280`). The
numeric writers `write_u32` / `write_u64` are **inherited from the std
`Hasher` trait's default impls**, which use **`to_ne_bytes`** (native
endian), not `to_le_bytes`. Behavior on LE targets is byte-identical so the
plan's downstream correctness reasoning still holds — but the asserted
mechanism is wrong, and the comment in the inserted code ("four 8-byte LE
words concatenated", §8) would be misleading if pasted verbatim.

Fix: change §3 paragraph 1 to: "twox-hash 1.6's `Hasher` impl provides only
`fn write(&[u8])` explicitly. `write_u64` is inherited from `std::hash::
Hasher`'s default impl, which calls `to_ne_bytes()` — native endian. Both
build targets (x86_64-linux, wasm32-unknown) are LE, so the byte stream is
LE on all real runtime targets." Then the inline comment in the code block
becomes: `// (6) RNG state — four u64s of xoshiro256++ internal state,
written via Hasher::write_u64 (native-endian → LE on both targets).`

**Cross-reference S7 review B2** for the same correction in §3 of that plan.

(After S37 bumps twox-hash to 2.x, the implementer of S37 should verify
whether v2 overrides the numeric writers; if so, the inline comment can be
updated to whatever v2 guarantees. For S7+S8, the LE-on-LE-host conclusion
is unchanged.)

### Strong recommendation (1)

**R1. Default to the `serde_json::to_value(&self.0)["s"]` fallback (§2's
option (c)/fallback paragraph) rather than the custom `serde::Serializer`
(option (d)).** The plan correctly enumerates options and chooses (d) for
purity. Reviewer disagrees on cost/benefit:

- The custom Serializer is **80–120 LOC** of `unreachable!()` stubs across
  the entire `serde::Serializer` trait (>30 methods, plus
  `SerializeStruct`, `SerializeSeq`, `SerializeTuple` impls), plus an
  error type, plus tests to assert the contract. This is the single
  largest LOC block in PR-3 by a wide margin.

- The stated motivation for (d) over the fallback was: "fallback still
  depends on `serde_json` (defeating S8's stated motivation in part)."
  This conflates two different uses of `serde_json`:
  - **Hot-path hashing of serde_json's output bytes** (what S8 removes) —
    the byte stream is sensitive to `serde_json` formatting policy bumps.
  - **One-shot field extraction via `to_value(...)["s"].as_u64()`** — the
    output of this is **four `u64`s**, not bytes. We then hash the four
    `u64`s ourselves via `h.write_u64`. A `serde_json` formatting bump
    cannot change the value of a parsed `u64` — only its on-the-wire
    string form. **Determinism is preserved.**

- The fallback is ~10 LOC, has zero foreign-crate-internals coupling
  beyond "the derive emits `s: [u64; 4]`" (same coupling as option (d)),
  and the heap allocation is per-`snapshot_hash`-call (already infrequent;
  only called by acceptance test + dev save dump).

- Trait-coherence trouble cited as a risk for the custom serializer is
  non-trivial — adding `serde::Serializer` impl to a struct that already
  has trait bounds elsewhere can produce confusing compile errors that
  cost the implementer hours to debug. The fallback has no such risk.

**Recommended new §2 wording:** "Default to (c)/fallback (`to_value(&rng)
["s"]` extraction). If a future S37 dep bump or other refactor removes
`serde_json` from the crate entirely, revisit and add the custom
Serializer (d) at that time." This also de-risks the commit: 10 LOC of
extraction code vs 80–120 LOC of trait stubs makes the S7+S8 combined
diff bisectable.

The accessor method on `SimRng` (§2 "Chosen accessor design") stays the
same — only its body changes. Doc comment becomes: "Uses `serde_json::
to_value` as a one-shot extractor. The hash byte stream is independent of
`serde_json`'s output format because we hash the parsed `u64` values, not
the JSON bytes."

If the implementer feels strongly about option (d), the plan is still
correct — just larger. The reviewer's preference is to default to (c) and
flip to (d) only if (c) hits an unexpected snag.

### Non-blocking notes (3)

**N1. Test plan is precise.** `rng_hash_changes_after_step`,
`rng_state_is_4_u64`, and `rng_state_changes_after_next_u64` are all
crisply specified. A sonnet implementer can write them verbatim from §6.
`rng_state_round_trip_matches_serde` is a nice belt-and-suspenders test;
keep it (especially if option (c) is chosen — it directly validates the
extraction path).

**N2. Commit pairing with S7 (§4) is explicit and matches master §4 / §7 (b)
/ §8.** Confirmed. The commit-message template is clear; only minor request:
if S7 review's B1 (`Species.born_tick`) or B3 (`species.next_id`) land,
update the commit-template's "S7" line accordingly.

**N3. Risk (d) ("hashing only the state is byte-equivalent in information
content") is correctly argued.** `Xoshiro256PlusPlus` has exactly one field
`s: [u64; 4]` (verified at
`~/.cargo/registry/.../rand_xoshiro-0.6.0/src/xoshiro256plusplus.rs:24-26`).
No version tag, no stream metadata. Hashing the four words = hashing the
full state. Good.

### Pair-pacing check

- Master §4 S8 brief says "lands in the same commit as S7." ✓ Plan §4.
- Master §7 (b) watchlist demands one-commit pairing. ✓ Plan §4 and §1.
- Master §8 regen ceremony is deferred to end-of-PR-3. ✓ Plan §5 step 4
  ("do not regen yet — wait for end-of-PR-3 ceremony") and §10 last bullet.

### Required-edit summary for the planner before implementer kicks off

1. Correct §3 endianness paragraph: twox-hash 1.6's numeric writers
   inherit `to_ne_bytes` from std's `Hasher` default (B1). Update inline
   code-block comment in §3 likewise. Cross-link to S7 review B2.
2. Recommend changing default approach in §2 to the `serde_json::to_value`
   one-shot extractor (R1). Document the custom-Serializer as a "if
   `serde_json` is ever removed from the dep graph, swap to this" future-
   work note rather than the primary path. The accessor signature on
   `SimRng` (`pub(crate) fn state(&self) -> [u64; 4]`) is unchanged.
3. If S7 picks up `Species.born_tick` and/or `species.next_id` per S7's
   B1/B3, update §4's commit-message template stem to mention them.

After those edits the plan is implementer-ready. 

---

## Plan-update changelog (2026-05-24)

Applied based on reviewer feedback (pair S7+S8 review block, opus 2026-05-24):

- **[R1] Switched primary accessor approach** from custom `serde::Serializer` (option (d), 80–120 LOC) to `serde_json::to_value(&self.0)["s"]` one-shot extractor (option (c), ~15 LOC). Rewrote §2 accessor design and code block accordingly. Custom serializer demoted to "future work if `serde_json` is removed from dep graph."
- **[R1] Added trade-off note in §2** explaining why the `serde_json` extractor still achieves S8's goal: the hash byte stream is serde_json-independent (we hash parsed `u64` values, not JSON bytes). Three numbered reasons documented (dep stays anyway, format-bump-safe, byte-stream determinism preserved).
- **[B1] Corrected endianness wording in §3.** Removed false claim that "`write_u64` is implemented as `to_le_bytes`." Replaced with: twox-hash 1.6's numeric writers are inherited from `std::hash::Hasher`'s default impls which use `to_ne_bytes()`. Both real targets are LE so the byte stream is LE in practice. Updated inline code-block comment from "four 8-byte LE words" to "native-endian → LE on both real targets."
- **[B1] Added S37 cross-reference** in §3: after S37 bumps to twox-hash 2.x, implementer should verify whether v2 overrides numeric writers.
- **[S37 dep note] Added precondition block** at top of plan header (same text as S7).
- **[§5 / implementation order]** Rewrote step 1 to reflect the simpler accessor (~15 LOC, no `state_capture` module). Added `rng_state_round_trip_matches_serde` test to step 1 test list (reviewer N1 confirmed this is a good belt-and-suspenders test; especially valuable with the `to_value` path).
- **[§9 risk (e)]** Replaced custom-serializer boilerplate risk with extractor field-name stability risk; mitigation is the round-trip test and pinned `rand_xoshiro 0.6`.
- **[§10 acceptance]** Updated criterion for `state()` to note "≈15 LOC, no `state_capture` module." Updated RNG-state block comment criterion to "native-endian → LE on both real targets."
- **[§8]** Added note that the doc-comment in the hash-input header is updated to replace "serde_json bytes" with "4 u64s, native-endian (LE on both real targets)."
- **[N2 — S7 cross-ref / commit template]** S7 B1/B3 landed; §4 commit-message template updated in S7 to mention `born_tick` and `next_id` (S8's commit template references the same commit so no separate update needed here).
