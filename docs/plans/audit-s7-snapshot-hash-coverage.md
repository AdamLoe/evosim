# S7 — Extend `snapshot_hash` coverage + canonical NaN handling

**Owner:** PR-3 (audit cleanup pass v1.1).
**Pairs with:** S8 (`docs/plans/audit-s8-rng-hash-direct.md`) — **same commit**.
**Depends on:** S1 path translation (snapshot_hash.rs itself is unchanged by S1, but the `use crate::world::World` import target is now `crate::world::World` re-exported from `src/world/mod.rs`).
**Determinism impact:** **regen** (intended). Both goldens flip.
**Effort:** M.

> **Precondition:** S37 (twox-hash 1→2) bootstrap must complete in PR-1 with byte-identical goldens. If S37 drifts, this plan's regen is folded with S37's into the PR-3 ceremony (no other change).

---

## 1. Summary

The current `snapshot_hash` (`src/snapshot_hash.rs:21-80`) does not cover every
sim-determining field in `CreatureSoA`, `Carrion`, or `Species`. A bug that
flipped `digestion_cooldown` by one tick — or that mutated `species_id` — would
not be caught by the golden. Per `docs/audit/correctness-bugs.md` C4, this is a
real divergence-detection gap.

This piece (a) **appends** the missing fields to the existing per-entity loops
in struct-declaration order, (b) replaces `write_f32` with a NaN-canonical
helper so any NaN payload hashes to a single canonical quiet-NaN bit pattern
(`0x7fc0_0000`) regardless of platform-produced NaN bits, and (c) adds
`Species.born_tick` and `SpeciesRegistry.next_id` to the species coverage.

It does **not**:
- Touch the RNG hash path (that's S8, same commit).
- Re-pin the goldens (that ceremony happens once at end of PR-3 per
  `docs/plans/audit-master.md §8`).
- Modify `world.rs` field shape (S1 owns that split; this piece consumes the
  already-split paths).

---

## 2. Full list of new hash inputs

All new inputs are **appended** to their respective per-entity loops in the
order listed below. Existing field ordering is **not** changed.

### 2a. CreatureSoA per-creature loop (`src/snapshot_hash.rs:30-44`)

| # | Field | Source | Insert after | Width |
|---|---|---|---|---|
| 1 | `digestion_cooldown[i]` | `creature.rs:52` | `nn_mutation_rate` (line :43) | `u32` (4 bytes) |
| 2 | `cumulative_upkeep[i]` | `creature.rs:53` | (1) | f32-canonical (4) |
| 3 | `species_id[i]` | `creature.rs:54` | (2) | `u32` (4) |
| 4 | `parent_species_id[i]` | `creature.rs:55` | (3) | `u32` (4) |
| 5 | `last_action[i]` | `creature.rs:56` | (4) | `u8` (1) — via `action as u8` |
| 6 | `action_this_tick[i]` | `creature.rs:57` | (5) | `u8` (1) |
| 7 | `max_size_reached[i]` | `creature.rs:58` | (6) | f32-canonical (4) |
| 8 | `distance_travelled[i]` | `creature.rs:59` | (7) | f32-canonical (4) |
| 9 | `birth_tick[i]` | `creature.rs:61` | (8) | `u32` (4) |

`Action` is `#[repr(u8)]` (`creature.rs:16`), so `action as u8` is a stable
cast and trivially hashable.

### 2b. Carrion per-corpse loop (`src/snapshot_hash.rs:57-62`)

| # | Field | Source | Insert after | Width |
|---|---|---|---|---|
| 10 | `cc.id` | `carrion.rs:8` | `age` (line :61) | `u64` (8) |
| 11 | `cc.sun_cell` | `carrion.rs:14` | (10) | `u32` (4) — cast `usize as u32`; document the cast |

`sun_cell` is `usize` (`carrion.rs:14`); on wasm32 and Linux x86_64 both,
`usize` fits in `u32` (sun grid is `SUN_DIM × SUN_DIM = 20 × 20 = 400` cells,
well under `u32::MAX`). Cast explicitly with `cc.sun_cell as u32` and add a
`debug_assert!(cc.sun_cell < u32::MAX as usize)` next to the cast for safety.

**Endianness note:** twox-hash 1.6's `write_u32`/`write_u64` inherit from
`std::hash::Hasher`'s default impls, which use `to_ne_bytes()` (native endian).
Both build targets (x86_64-linux, wasm32-unknown) are LE, so the byte stream is
LE on all real runtime targets. After S37 bumps to twox-hash 2.x, the
implementer of S37 should verify whether v2 overrides the numeric writers. (See
also S8 review B1 / §3 for the same note.)

### 2c. Species per-species loop (`src/snapshot_hash.rs:66-73`)

All new species fields are **appended after** the existing anchor sub-hash
(`h.write_u64(ah.finish())` at line :72) — strict trailing-append, no
interleaving. See §4 for the rationale.

| # | Field | Source | Insert after | Width |
|---|---|---|---|---|
| 12 | `sp.parent_id` | `species.rs:12` | anchor sub-hash (line :72) | `u32` (4) + 1-byte presence — encode `None` as `(0u8, 0u32)` and `Some(v)` as `(1u8, v)` |
| 13 | `sp.name` bytes | `species.rs:13` | (12) | `u32` length-prefix + UTF-8 bytes |
| 13.5 | `sp.born_tick` | `species.rs:16` | (13) | `u32` (4) |
| 14 | `sp.died_tick` | `species.rs:17` | (13.5) | `(u8, u32)` Option encoding same as (12) |
| 15 | `sp.child_count` | `species.rs:19` | (14) | `u32` (4) |
| 16 | `sp.depth` | `species.rs:21` | (15) | `u32` (4) |
| 17 | `sp.anchor_brain_weights` (each f32) | `species.rs:15` | (16) | f32-canonical per element |

### 2d. After the species loop (top-level hash input)

| # | Field | Source | Insert after | Width |
|---|---|---|---|---|
| 18 | `w.species.next_id` | `species.rs:26` | species loop body | `u32` (4) |

`SpeciesRegistry.next_id` is a `pub(crate)` field (private in `species.rs:26`)
that drives all child-species id allocation. It is sim-determining: a bug that
mis-advances `next_id` at speciation would produce a different next species id
even if the `list` slice looks identical. Hash it with a single
`h.write_u32(w.species.next_id)` after the `for sp in &w.species.list { ... }`
loop closes.

Access note: `next_id` is currently private (`species.rs:26`). The
implementer must add `pub(crate) next_id: u32` (or a `pub(crate) fn
next_id(&self) -> u32` accessor) to `SpeciesRegistry` so `snapshot_hash.rs`
can read it without a visibility violation. The field is already read via
`SpeciesRegistry::from_snapshot` at line :95, confirming the value is
meaningful to consumers outside the registry's own impl.

For `name`: write `h.write_u32(name.len() as u32)` then `h.write(name.as_bytes())`.
The length prefix prevents the "`"ab" + "c"` vs `"a" + "bc"`" boundary
collision when two adjacent species names share a prefix.

For `anchor_brain_weights`: hash element-by-element via the canonical helper.
`Brain.weights` is `Vec<f32>` with `length = NN_WEIGHT_COUNT = 3456` per
`src/brain.rs:35`; `Species.anchor_brain_weights` is `brain.weights.clone()`
(`species.rs:45`), so per-element length is spec-fixed at 3456. **Do NOT
length-prefix** (the length is invariant; prefixing would still be correct
but wastes 4 bytes). If a future change makes the length variable, add a
`debug_assert_eq!(sp.anchor_brain_weights.len(), NN_WEIGHT_COUNT)` next to
the loop now to surface the assumption.

### Total new fields added: **18.**

(9 in `CreatureSoA`, 2 in `Carrion`, 6 in per-species loop, 1 top-level
registry counter after the species loop.)

### Explicitly OUT of scope for this piece

- `sun.demand[k]` — per C4 finding "zeroed each tick so usually fine"; it's
  reset before any read, so hashing it would never produce a divergence
  signal. Skip.
- `eye_trig` — derived from genome; would double-count (already noted in
  `creature.rs:69`).
- `g_size` / `g_photo_eff` / … (perf-5 hot mirrors) — derived from `genomes[i]`
  by contract; the existing `hash_genome` call covers the source of truth.
  An invariant test in `creature.rs:357` already enforces equality; the hash
  shouldn't double-count.

---

## 3. NaN canonicalization design

### Replacement helper (drop-in replace existing `write_f32`)

Replace the existing helper at `src/snapshot_hash.rs:119-122` with:

```rust
#[inline]
fn write_f32(h: &mut XxHash64, v: f32) {
    // NaN canonicalization (C4): every NaN bit-pattern hashes identically.
    // Maps all NaN payloads to a single canonical quiet-NaN (0x7fc0_0000).
    // Finite values (including ±0.0, ±Inf, denormals) pass through unchanged.
    let bits = if v.is_nan() { 0x7fc0_0000_u32 } else { v.to_bits() };
    h.write_u32(bits);
}
```

### Recommendation: replace in place, do NOT add a separate helper

Reasoning:
1. There is only one `write_f32` in the file; all 50+ callers want
   canonicalization (we have no use case for a "preserve raw NaN bits"
   path).
2. A second helper invites future drift ("did this call site use the canon
   one?"). One name, one behavior is simpler.
3. The behavior change is invisible to today's goldens (no NaN currently
   appears in any hashed field; S5/S6 are landing in the same PR to keep
   it that way).

### Note on -0.0

Per C4 last paragraph: `-0.0 != 0.0` in bits, so two "identical" worlds
where one path went through `(-0.0_f32).max(0.0)` could diverge. **This
piece does not canonicalize -0.0**, because:
- The sim never deliberately produces -0.0 (no negation of a clamped
  energy path), and
- Conflating -0.0 with +0.0 in the hash would hide a real future bug class
  (silent sign loss).

If we ever ship a code path that legitimately produces -0.0 mid-tick, that's
a sim arithmetic issue; the hash should still distinguish them.

### Why `0x7fc0_0000` specifically

It's the canonical quiet-NaN bit pattern for IEEE-754 binary32 (sign=0,
exponent=all-ones, MSB-of-mantissa=1, rest=0). All wasm32, x86_64, and ARM
runtimes recognize it. `f32::NAN` itself produces this pattern on most
platforms but is not contractually guaranteed to.

---

## 4. Append-only rule

**Hard rule:** new fields are appended at the **end** of their per-entity
loop body. The existing field order is **not** reorganized.

- CreatureSoA additions (#1-#9) go after `write_f32(&mut h, w.creatures.brains[i].nn_mutation_rate);` (`src/snapshot_hash.rs:43`).
- Carrion additions (#10-#11) go after `h.write_u32(cc.age);` (`src/snapshot_hash.rs:61`).
- Species additions (#12-#17): **all go after** `h.write_u64(ah.finish());`
  (line :72 — the existing anchor sub-hash). No interleaving before the
  anchor sub-hash is permitted. The chosen ordering is: `parent_id` → `name`
  → `born_tick` → `died_tick` → `child_count` → `depth` →
  `anchor_brain_weights` (all after the existing anchor sub-hash call).
- Top-level registry counter (#18): `h.write_u32(w.species.next_id)` goes
  after the closing brace of the `for sp in &w.species.list { ... }` loop.

**Why strict trailing-append for species:** keeping all new species fields
after the anchor sub-hash makes the diff a single contiguous block. Two
different implementers following the plan will produce identical byte streams.
The code reviewer has a single grep anchor to verify: "everything after
`ah.finish()`".

Why append-only matters: a single regen ceremony pins NEW byte-stream values
into the goldens. If a later debugging session needs to bisect "which new
field broke determinism", the append-only diff means each field can be
individually un-added by removing one block.

---

## 5. Step-by-step implementation order

The implementer (sonnet per `audit-master.md §9`) executes:

1. **(i) Replace `write_f32`** with the NaN-canonical version from §3.
   Run `cargo test snapshot_hash` — the existing two tests
   (`snapshot_hash_is_deterministic`, `snapshot_hash_same_seed_same_hash`)
   must still pass. They do not depend on a fixed hash value, so they will.

2. **(ii) Add new per-entity fields** in the order listed in §2. After each
   block (creature loop, carrion loop, species loop) run
   `cargo build --release` to make sure the casts (`action as u8`,
   `cc.sun_cell as u32`) compile.

3. **(iii) Add the NaN-canon unit test** (`nan_canonicalization_hashes_equal`,
   spec in §6).

4. **(iv) Run acceptance to capture the NEW hash, but do NOT regen yet:**
   ```bash
   cargo test --release --test acceptance acceptance_t10000 2>&1 | tee /tmp/s7-hash.txt
   ```
   The test will FAIL with a mismatch error containing both the old and
   new hash. Record the new hash in the commit body for posterity
   ("`new sequential hash before S8: 0x...`"), then proceed to S8's
   implementation in the same commit. After S8 lands its byte change, the
   hash will shift again. The PR-3 implementer runs the single regen
   ceremony from `audit-master.md §8` once both S7 and S8 are applied.

5. **(v) Cargo fmt + clippy** before commit:
   ```bash
   cargo fmt
   cargo clippy --all-targets -- -D warnings
   cargo clippy --all-targets --features threads -- -D warnings
   ```

---

## 6. Test plan

### New unit test (lands in `src/snapshot_hash.rs::tests`)

```rust
/// NaN canonicalization: any NaN bit pattern in a hashed field must produce
/// identical world hashes. Prevents platform NaN-payload drift.
#[test]
fn nan_canonicalization_hashes_equal() {
    let mut w1 = World::new("nan-canon-1");
    let mut w2 = World::new("nan-canon-1");
    // Force a NaN into a hashed f32 field on creature 0 in two different ways.
    assert!(!w1.creatures.is_empty(), "founders must exist");
    w1.creatures.max_size_reached[0] = f32::NAN;                       // 0x7fc0_0000 on most platforms
    w2.creatures.max_size_reached[0] = f32::from_bits(0xffc0_0001);    // signaling-ish, alt payload
    // Both NaN — must hash identically.
    assert!(w1.creatures.max_size_reached[0].is_nan());
    assert!(w2.creatures.max_size_reached[0].is_nan());
    assert_eq!(
        snapshot_hash(&w1),
        snapshot_hash(&w2),
        "two NaN payloads must canonicalize to the same hash"
    );
}
```

If `max_size_reached` is not yet wired into the hash at the moment the
implementer drops the test in (test-driven order), the assertion will fail
and the implementer adds the field per §2a step (7). The test passes
without modification once all of §2a lands.

### Existing tests

- `snapshot_hash_is_deterministic` (`src/snapshot_hash.rs:131`) — still passes
  (does not pin a hash value).
- `snapshot_hash_same_seed_same_hash` (`src/snapshot_hash.rs:147`) — still
  passes (compares two same-seed worlds; both get the same new bytes).

**No existing unit tests in `snapshot_hash.rs` pin a specific hash value.**
The two pinned hashes live in `tests/golden_snapshot_t10000.txt` and
`tests/golden_snapshot_t10000_threaded.txt`, which are intentionally regen'd
at end of PR-3. No other test file in `tests/` or `src/` asserts on a
literal `snapshot_hash` return value (verify with
`grep -rE 'snapshot_hash.*==' src/ tests/` before commit).

### Acceptance tests (no regen here)

- `cargo test --release --test acceptance` — expected to FAIL with hash
  mismatch after this piece + S8 land. The failure message itself is the
  artifact that feeds the PR-3 regen ceremony.

---

## 7. Pair-with-S8 note

**This piece lands in the same commit as S8.** Reasoning per
`audit-master.md §7 watchlist item (b)` and `§4 S7/S8` briefings:

- Both S7 and S8 individually flip both goldens.
- One regen ceremony at end of PR-3 is the master-plan rule.
- If S7 ships first and S8 second (two commits, two regens), we've spent
  two regen ceremonies and obscured which fields drove which delta.

The PR-3 implementer (sonnet per §9) drafts BOTH plans' code, builds, runs
acceptance to confirm the hash differs from `0xb76e907c6221f7f5`, then runs
the regen ceremony from `audit-master.md §8` exactly once. Commit message
template:

```
fix(snapshot_hash): extend coverage + canonical NaN + direct RNG hash (S7+S8)

Adds 18 new hashed fields across CreatureSoA, Carrion, and Species (S7),
including Species.born_tick and SpeciesRegistry.next_id.
Replaces serde_json RNG byte stream with direct u64×4 hash of xoshiro state (S8).
Replaces `write_f32` with NaN-canonical variant (S7).

Determinism: both goldens regen'd in this commit per audit-master §8.
New sequential hash: 0x<NEW>
New threaded hash: 0x<NEW> (or note if it diverges from sequential)
```

---

## 8. Path notes

`src/snapshot_hash.rs` itself is **not** moved or split by S1. The S1
world.rs split affects `src/world.rs` only; this file's `use crate::world::World`
continues to work because the re-export at `src/world/mod.rs` preserves the
public path. All line numbers cited above are stable post-S1.

The cross-piece dependencies that matter for path stability:
- `w.creatures.{...}` — `CreatureSoA` lives in `src/creature.rs:44` (unchanged).
- `w.carrion[..]` — `Carrion` lives in `src/carrion.rs:7` (unchanged).
- `w.species.list[..]` — `Species` lives in `src/species.rs:10` (unchanged).
- `w.rng` — `SimRng` in `src/rng.rs` (unchanged; S8 plan owns the access pattern).

---

## 9. Determinism impact

**Regen — intended.** Both goldens flip:
- `tests/golden_snapshot_t10000.txt` (currently `0xb76e907c6221f7f5`).
- `tests/golden_snapshot_t10000_threaded.txt` (currently the same).

The post-S7+S8 hashes go into `DECISIONS.md` per `audit-master.md §8`. If
the sequential and threaded hashes diverge post-regen (acceptable per
watchlist item (e)), both are recorded separately.

This is one of the two source-of-regen pieces in PR-3 (the other is S8).
All other PR-3 pieces (S4, S5, S6, S9, S10, S11, S12, S24, S39) are
expected byte-identical against the OLD hash; the bootstrap on each
verifies that, and the regen at PR-3 end pins the NEW hash for all of them
simultaneously.

---

## 10. Risk register

| # | Risk | Mitigation |
|---|---|---|
| (a) | Accidentally reordering existing fields → diff becomes hostile to bisection. | §4 append-only rule. Code reviewer (opus per §9) checks the diff is a pure append in all three loops. For the species loop, ALL new fields (#12-#18) appear after the existing `ah.finish()` call — reviewer greps for any addition before that line and rejects if found. |
| (b) | Missing a sim-determining field. | Cross-check against `creature.rs:44-99` (every `pub` SoA field), `carrion.rs:7-15` (every `pub` field), `species.rs:10-22` (every `pub` field). The §2 tables enumerate every field; reviewer greps `pub` in each file and ticks them off vs the tables. Out-of-scope items (`sun.demand`, `eye_trig`, perf-5 mirrors) are explicitly listed at the end of §2 with rationale. |
| (c) | NaN canon helper alters hash of currently-finite values. | The helper branches **only** on `is_nan()`. For finite values, `±0.0`, `±Inf`, denormals → the `else` arm runs `v.to_bits()` unchanged. The implementer adds a one-shot debug-only assertion test: `for v in [-0.0_f32, 0.0, f32::MIN_POSITIVE, f32::INFINITY, -f32::INFINITY, 1.5, -2.7] { assert_eq!(canonical_hash_of(v), legacy_hash_of(v)) }` — or simply trusts the branch. Code-reviewer (opus) confirms the branch shape. |
| (d) | `anchor_brain_weights` length is `Vec<f32>` and could be platform-variable. | Per `src/brain.rs:35` comment, length is `NN_WEIGHT_COUNT = 3456` constant. `species.rs:45` clones `brain.weights` directly. Add a `debug_assert_eq!(sp.anchor_brain_weights.len(), w.creatures.brains[0].weights.len())` (or `NN_WEIGHT_COUNT` if imported) immediately inside the species loop to surface any future drift. |
| (e) | The Option encoding for `parent_id` / `died_tick` conflicts with future "stricter" encodings. | Locked to `(u8 presence, u32 value-or-0)` for both. Documented in §2c. Reviewer confirms both fields use the same shape. |
| (f) | `cc.sun_cell as u32` cast clamps on 32-bit-tight platforms. | `SUN_DIM = 20` (confirmed from `src/constants.rs:13`), so max sun_cell = 399, well within `u32`. Debug-assert is in the cast site per §2b. |
| (g) | Forgetting to land S7 + S8 in one commit → two regen ceremonies. | §7 explicitly couples them; commit-message template provided; cross-reviewer watchlist (b) catches this if missed. |

---

## 11. Acceptance criteria

This piece is accepted when ALL of the following hold (verified at the
end of PR-3, in concert with S8):

1. Every field in §2's tables (1)–(18) appears in `src/snapshot_hash.rs` in
   the documented order. `grep -nE 'digestion_cooldown|cumulative_upkeep|species_id|parent_species_id|last_action|action_this_tick|max_size_reached|distance_travelled|birth_tick' src/snapshot_hash.rs` returns one hit per field, all inside the creature loop.
2. `cc.id` and `cc.sun_cell` are hashed in the carrion loop.
3. Every species field listed in §2c is hashed **after** the anchor sub-hash
   (`ah.finish()`), in the order: `parent_id` → `name` → `born_tick` →
   `died_tick` → `child_count` → `depth` → `anchor_brain_weights`.
4. `w.species.next_id` is hashed after the species loop with a single
   `h.write_u32(w.species.next_id)` call. `SpeciesRegistry.next_id` is
   `pub(crate)` (or has a `pub(crate)` accessor) to allow this.
5. `write_f32` is the NaN-canonical version from §3.
6. `nan_canonicalization_hashes_equal` test passes.
7. Existing `snapshot_hash_is_deterministic` and `snapshot_hash_same_seed_same_hash` tests still pass.
8. After S8 also lands (same commit) and the §8 ceremony from
   `audit-master.md` regenerates both files, `tests/golden_snapshot_t10000.txt`
   and `tests/golden_snapshot_t10000_threaded.txt` contain the NEW hashes
   and `cargo test --release --test acceptance` + `cargo test --release --features threads --test acceptance` both pass.
9. `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`, and
   `cargo clippy --all-targets --features threads -- -D warnings` all clean.
10. `DECISIONS.md` has the new audit v1.1 block from `audit-master.md §8` with
   the new sequential and threaded hashes (added by the PR-3 implementer at
   ceremony time, not in this commit).

---

## 12. Locked scope reminders

- **Do NOT** touch the RNG hash path (`src/snapshot_hash.rs:75-77`) — that's S8.
- **Do NOT** regen the goldens here — the regen ceremony lives at the end of
  PR-3 per `audit-master.md §8`.
- **Do NOT** modify `world.rs`'s field shape (or any sim file's field
  layout); this piece only reads.
- **Do NOT** rename `write_f32` (preserves diff focus on the body change,
  not the call sites).
- **Do NOT** reorder the existing hash inputs.

---

## Review feedback (pair S7+S8)

Reviewer: opus, 2026-05-24. Cross-reference: `docs/plans/audit-s8-rng-hash-direct.md` review block.

### Verdict
**APPROVE WITH FIXES.** Plan is correct in shape and well-organized. Three real
issues to address before implementation; one factual error to correct; two
small clarifications. None invalidate the structure.

### Blocking issues (3)

**B1. `Species.born_tick` is missing from §2c.** Audit `src/species.rs:16` lists
`born_tick: u32` as a `pub` field — a genuine sim-determining datum (used by
the species-age UI and the lineage-tree visualization). The §2c table covers
`parent_id`, `name`, `died_tick`, `child_count`, `depth`, and
`anchor_brain_weights` but skips `born_tick`. A bug that mis-sets `born_tick`
at speciation would today be invisible to the snapshot hash AND would still
be invisible after S7. Add as row "(13.5) `sp.born_tick` (u32, 4 bytes) —
insert between `name` and `died_tick`" or append after `anchor_brain_weights`
(append-only is fine). Either way **total new fields becomes 18, not 17.**
Update §11 acceptance criterion (1) accordingly.

**B2. Endianness statement in §3 / pattern reference is silently wrong for
twox-hash 1.6.** The current crate (`twox-hash = "1.6"`) uses the default
`Hasher::write_u32` / `write_u64` provided by `std::hash::Hasher`, which call
`to_ne_bytes()` — **native endian**, not little-endian. (Confirmed by reading
`~/.cargo/registry/.../twox-hash-1.6.3/src/sixty_four.rs:275-280` — only
`fn write(&mut self, bytes: &[u8])` is implemented; the numeric writers fall
through to the default trait impls. The default impls in `std` for primitive
writers use `to_ne_bytes`.) Both build targets are LE today so this is byte-
stable in practice; but the plan's claim ("Both wasm32 and Linux x86_64
write_u64 are LE on byte level") should be amended to: "Both build targets
are LE; the trait's default `write_u64` is `to_ne_bytes`, which equals
`to_le_bytes` on LE hosts." Add a one-line `// NOTE: twox-hash 1.6's
write_u64 uses to_ne_bytes; both real targets are LE so this is byte-stable.`
next to the new `write_u32`/`write_u64` block introduced for new fields, AND
flag the same in S8's plan §3. After S37 (twox-hash 1→2 bump), recheck the
behavior — twox-hash 2.x may or may not override the trait defaults.

**B3. `Species.next_id` is not hashed and not listed as out-of-scope.** The
`SpeciesRegistry { list, next_id: u32 }` carries a private `next_id` counter
(`species.rs:25-26`) that is sim-determining (it drives child-species naming
and identity allocation). It is not in §2c and not in §2's out-of-scope list.
Two acceptable resolutions: (a) add a hash input "after the species loop:
`h.write_u32(w.species.next_id)`" — preferred, deterministic and cheap; (b)
explicitly add it to the out-of-scope list with rationale ("derivable from
`list[-1].id + child_count` chain"). Either is fine, but it must not be left
silent. Recommend (a). If chosen, §11 acceptance criterion gets a check for
this single byte.

### Non-blocking issues (2 corrections + 2 clarifications)

**N1. Factual error: `SUN_DIM` is 20, not 120.** §2b risk-mitigation text
("sun grid is 120×120 = 14400 cells") and §10 row (f) ("SUN_DIM = 120") are
wrong. Real value is `pub const SUN_DIM: usize = 20;` at
`src/constants.rs:13` → 400 cells. The `cc.sun_cell as u32` cast is still
trivially safe (max 399 fits in u32) and the `debug_assert!(cc.sun_cell <
u32::MAX as usize)` is still correct, but the rationale text should be
corrected so a future reader is not confused. (The 120 figure appears to be
`HASH_DIM` confused with `SUN_DIM`.)

**N2. Pick one species-loop ordering and lock it.** §4's "Exception
clarified" paragraph allows the implementer to choose between two orderings
(interleave around anchor sub-hash vs strict append). Two implementers
following the same plan could produce different byte streams, both "correct".
Lock to: "all new species fields (#12–#18 if B1 accepted) go **after** the
existing anchor sub-hash `h.write_u64(ah.finish())` at line :72, in the
order listed in §2c." This makes §2c row "Insert after" columns uniform and
gives the reviewer a single grep to validate.

**N3. NaN canon helper is correctly minimal.** The `if v.is_nan() {
0x7fc0_0000 } else { v.to_bits() }` branch shape is reviewed and correct.
`is_nan()` is true iff the IEEE-754 exponent is all-ones AND the mantissa is
non-zero; the else-branch passes `±0.0`, `±Inf`, denormals, and all finite
values byte-identically. `0x7fc0_0000` matches the canonical quiet-NaN
(`f32::NAN.to_bits() == 0x7fc00000` on every mainstream target). Spot-check
in §6's `nan_canonicalization_hashes_equal` test uses
`f32::from_bits(0xffc0_0001)` (sign=1, exp=all-ones, mantissa MSB=1, payload
=1) which is indeed NaN — confirmed.

**N4. Append-only rule + bisect benefit caveat.** The plan correctly invokes
the append-only discipline. Note for the implementer: after this commit
lands the goldens are pinned to the new value, so "remove one block to
bisect a regression" still works for **future** divergences but does not
help re-bisect the S7 landing itself. That's fine — the S7 landing is
verified by the deterministic-twice and same-seed tests, not by golden
equality.

### Non-issues confirmed

- Test `nan_canonicalization_hashes_equal` is precisely specified and a
  sonnet implementer can write it verbatim from §6.
- Commit pairing with S8 (§7) is explicit and matches master §4/§7 (b).
- Regen deferred to master §8 ceremony (§7, §1, §11.9) — verified.
- All scope exclusions in §2 "Explicitly OUT of scope" are sound
  (`sun.demand`, `eye_trig`, perf-5 mirrors, derived `g_*` fields).
- Cast widths and Option encoding (`(u8, u32)`) are stable choices.
- `Brain.nn_mutation_rate` is already in the loop today (line :43) — not a
  gap.
- `anchor_brain_weights` is correctly hashed element-wise with a length-
  invariant comment; the `debug_assert_eq!(...len() == NN_WEIGHT_COUNT)` per
  risk (d) is good practice.

### Required-edit summary for the planner before implementer kicks off

1. Add `Species.born_tick` to §2c (B1) → bumps "17" to "18" in §2 totals,
   §11 acceptance (1), and the §1 summary's field list.
2. Decide `Species.next_id`: add as a 1-line hash input after the species
   loop, or add to OUT-of-scope with rationale (B3).
3. Correct §2b + §10 row (f) numbers: SUN_DIM = 20, not 120 (N1).
4. Lock species-loop ordering to "append after anchor sub-hash" (N2);
   delete the "either-ordering-acceptable" clause from §4.
5. Soften §3 endianness note to "to_ne_bytes on LE host = to_le_bytes" and
   add cross-reference to S8 review block B2 (B2).

After those edits the plan is implementer-ready. Pair-with-S8 commit
template (§7) is good as-is — minor: append "+ Species.born_tick + Species.
next_id" to the summary line if B1/B3 land. 

---

## Plan-update changelog (2026-05-24)

Applied based on reviewer feedback (pair S7+S8 review block, opus 2026-05-24):

- **[B1] Added `Species.born_tick` to §2c hash inventory** as row 13.5 (between `name` and `died_tick`). Confirmed field exists at `src/species.rs:16` (`pub born_tick: u32`). Total new-fields count bumped from 17 → 18 throughout (§2 totals, §11 criterion 1, §7 commit template).
- **[B3] Added `SpeciesRegistry.next_id` as top-level hash input** (new §2d, row 18). Chose resolution (a): hash it via `h.write_u32(w.species.next_id)` after the species loop. `next_id` is confirmed private at `src/species.rs:26`; implementer must expose it `pub(crate)`. Added access note to §2d explaining the visibility requirement.
- **[N1] Corrected SUN_DIM error**: changed `120×120 = 14400 cells` → `20×20 = 400 cells` in §2b, and `SUN_DIM = 120` → `SUN_DIM = 20` in §10 row (f). Confirmed `pub const SUN_DIM: usize = 20` at `src/constants.rs:13`. The cast safety argument is preserved (399 still fits in u32).
- **[N2] Locked species-loop ordering to strict trailing-append.** Deleted the "Exception clarified / either ordering acceptable" paragraph from §4. All new species fields (#12–#17) are now mandated to go after `ah.finish()` at line :72. §2c header, §4, and §11 criterion 3 updated consistently.
- **[B2] Added endianness nuance note** in §2b next to the cast block: twox-hash 1.6's `write_u32`/`write_u64` uses `to_ne_bytes` (std default), which equals `to_le_bytes` on LE hosts. Cross-references S8 review B1/B2 implicitly.
- **[S37 dep note] Added precondition block** at top of plan header: S37 (twox-hash 1→2) bootstrap must be byte-identical in PR-1; if it drifts, S7 regen folds into S37's PR-3 ceremony.
- **[§7 commit template]** Updated to reference 18 fields and specifically name `Species.born_tick` and `SpeciesRegistry.next_id`.
- **[§10 row (a)]** Removed tolerated-interleave reference; updated to "ALL new fields (#12-#18) appear after `ah.finish()`" as reviewer requirement.
- **[N3/N4 — approved]** NaN canon helper shape and append-only bisect caveat confirmed correct; no changes needed.
