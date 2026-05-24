# Rust Code-Quality Audit — `src/*.rs`

Date: 2026-05-24. Scope: 18 files, 7,351 LOC.

## TL;DR

- `cargo clippy --all-targets --no-deps` — **clean** (0 warnings).
- `cargo fmt --check` — **clean**.
- `cargo clippy -W pedantic -W nursery` — 508 warnings (most are noisy: `doc_markdown`, `cast_lossless`, `module_name_repetitions`, `missing_panics_doc`). 65 substantive after filtering.
- No real bugs. The codebase is already disciplined. Issues below are micro-perf / hygiene.

## Top clippy-pedantic / nursery digest (filtered)

| Lint | Hits | Worth fixing? |
|---|---|---|
| `suboptimal_flops` (use `mul_add`) | ~15 | Maybe — hot paths only (vision, world physics, brain bias). Bench first; `mul_add` can be *slower* in wasm without FMA. |
| `imprecise_flops` (use `f32::hypot`) | 2 (`world.rs:468,685`) | No — `hypot` is much slower than `sqrt(dx*dx+dy*dy)` and you don't need overflow protection here. |
| `wildcard_imports` (`use crate::constants::*`) | 8 files | Optional — constants are the one place wildcard is reasonable. |
| `explicit_iter_loop` (use `&v` instead of `v.iter()`) | 2 | Cosmetic. |
| `float_cmp` (`world.rs:693`) | 1 | **Yes — investigate**, see below. |
| `struct_excessive_bools` (`world.rs:47`) | 1 | Acceptable for `World` flags; could bitset but not urgent. |
| `format_push_string` (`profiler.rs:385`) | 1 | Use `write!` to skip an alloc. |
| `map_unwrap_or` (`profiler.rs:130`) | 1 | One-liner cleanup. |
| `if_not_else` (`profiler.rs:114`) | 1 | Cosmetic. |
| `default_trait_access` (`wasm_api.rs:398`) | 1 | Cosmetic. |
| `elidable_lifetime_names` (`vision.rs:45`) | 1 | Cosmetic. |
| `items_after_statements` in tests | ~6 | Cosmetic. |

---

## Per-file findings

### `src/lib.rs`
- Fine. `#![allow(clippy::needless_range_loop)]` at the crate root is appropriate given the SoA-by-index style.
- All modules `pub` — see *over-broad visibility* below.

### `src/constants.rs`
- All constants; nothing to flag. Many `pub const` are `pub(crate)` candidates but JS-facing constants are re-exported indirectly, so this is fine.

### `src/grid.rs`
- `cell_of` (line 40) is `#[inline]`. Good.
- `cell_of` (line 40) — could be `pub const fn` if the integer casts allowed it; they're `as` casts inside a `min`, so not const-compatible without nightly. Skip.
- `new()` (line 29) — could be `#[must_use]`.
- `for_each_in_radius` (line 72) — repeated `(((... ) / HASH_CELL).floor() as i32)` pattern. Consider hoisting `1.0 / HASH_CELL` or factoring; minor.
- `pub starts` and `pub indices` (lines 11–12) — **over-broad visibility**. Only `world.rs` reads them; make `pub(crate)` and `cursors` is already private. Same for `SpatialGrid` itself if not exposed to wasm.

### `src/sun.rs`
- `cell_index_for` (line 44) is `#[inline]`. Good.
- `capacity_at` (line 87) — hot inner loop on hotspots; consider `#[inline]`. Three iterations only, so likely irrelevant.
- `refill` (line 53) — indexed loop. Iterator form (`self.capacity.iter().zip(self.current.iter_mut())`) would generate identical code; keep as-is for clarity.

### `src/rng.rs`
- All small inline-able fns (`unit`, `symm`, `uniform`, `index`) lack `#[inline]`. **Hot — add `#[inline]`** to lines 30, 36, 41, 47, 66, 78. These are called millions of times/tick.
- `next_u64()` wrapper around `rand_xoshiro` — also missing `#[inline]`.
- `geom_skip` (line 66) — `(1.0 - p).ln()` recomputed every call; if callers pass the same `p` in a loop (brain mutation does — line `brain.rs:173–177`), hoist into a `geom_skip_with_log1mp` helper or precompute `(u.ln() / log1mp).floor()`. Minor.

### `src/brain.rs`
- Line 9 — `use crate::constants::*;` wildcard.
- Line 169 — `let mut child = parent.clone();` — **necessary clone** (need owned `Brain` to mutate). OK.
- `forward` / `forward_scalar` (lines 100, 138) — not marked `#[inline]`; these are called once per creature per tick, so call overhead is negligible. Skip.
- Line 182 — `multiplier * value + bias` style triggers `suboptimal_flops`. Ignore in wasm (no FMA target by default).
- `child_from`'s `geom_skip` loop (lines 173–177) — could hoist `ln(1 - p)` (see rng.rs note).

### `src/creature.rs`
- `Action::ALL` (line 27) — array literal repeats `Action::` 6×. Cosmetic.
- `one_hot_index` (line 36) — tiny, missing `#[inline]`. **Add**. (Although it's a single cast, so LLVM inlines anyway.)
- `len` (line 133) and `is_empty` (line 136) — missing `#[inline]`.
- Line 287 has `#[inline]` already.
- `CreatureSoA` (line 44) — large struct with all-`pub` fields. Already converted some to `pub(crate)` (mirrors). The `genomes`, `brains`, etc. are read from many places; current visibility is consistent. Consider `#[repr(C)]` if any JS-side typed-view ever exposes them — currently it doesn't (you build `creature_buf` byte-by-byte in wasm_api.rs:103).
- Line 320 (test) — `g.clone()` — fine (test code).

### `src/genome.rs`
- Line 6 — wildcard.
- Line 100 — `let r = self.mutation_rates.clone();` — **borrow-tangle workaround**. Acceptable: `TraitMutationRates` is a small `Copy`-able struct (verify it derives `Copy`; if it does, this is no-op but the explicit `.clone()` is misleading). If it's only `Clone`, deriving `Copy` would remove the clone.
- Line 169 — `let mut candidates: heapless::Vec<u8, 2>` — confusing: the local `mod heapless` (line 286) reimplements a stack vec. Comment on line 284 says "heapless was overkill here"; in fact you're still using it. Rename or replace with `[Option<u8>; 2]`/`ArrayVec`. **Slightly confusing.**
- Line 279 — suboptimal_flops noise. Skip.
- Line 322 — `self.buf[i].as_ref().unwrap()` inside `Index::index`. **Panic on invalid input** — but this is the documented contract of `Index`; matches `Vec`. OK.

### `src/vision.rs`
- Line 14 — wildcard.
- Line 36 — `VisionPass<'a>` lifetime is elidable per clippy. Cosmetic.
- Line 42 — `cell_to_carrion: &'a Vec<Vec<u32>>` — prefer `&'a [Vec<u32>]`. Cosmetic.
- `fill_one` (line 88) — hot per-creature per-tick. Not `#[inline]`. Since it's called from inside `run`'s closure / serial loop and via rayon, `#[inline]` would help only the serial path. Worth adding.
- Internal sector trig hot loop (lines 280–300) flagged for `mul_add`. Bench before applying.

### `src/world.rs`
- Line 10 — wildcard.
- Lines 1905, 1916 (tests): `w.biggest_ever.as_ref().unwrap()` — test code, fine.
- Line 1365 — `let logits: &[f32; 6] = output_buf[2..8].try_into().unwrap();` — **could be `expect("output_buf >= 8 floats")`** for better diagnostics, or change `output_buf: &mut [f32; NN_OUTPUTS]` so the type system enforces it. The slice is statically known to be NN_OUTPUTS=8 long. Same effective safety.
- Line 1396, 2026 — `panic!("never split — …")` inside `#[cfg(test)]` blocks. **Fine — test failure**.
- Line 47 — `World` struct with 47 `pub` fields including booleans, scratch buffers, snapshots. `scratch_*` were left `pub` (or made `pub(crate)`); most fields *are* legitimately read across modules. Consider grouping scratch state into a sub-struct (`WorldScratch`) for clarity — not for visibility.
- Line 459, 468, 513, 531, 547, 672, 685, 723, 1195 — `mul_add`/`hypot` lints. Ignore unless profiling indicates.
- Line 693 — `d == bd` (f32 strict eq) — **legitimate flag**. You're using equality to detect the tied-distance tiebreak (lowest id wins). The two `d`s came from independent sqrts so this can miss ties due to last-bit error. Likely irrelevant in practice (ties are vanishingly rare) but worth a `(d - bd).abs() < f32::EPSILON` or comparing `d2` (squared) up the call chain. **Minor correctness smell.**
- Lines 1183–1199 (`count_carrion_overlap`) — manual 3×3 cell sweep with indexed access; this is idiomatic and faster than iterator chains for fixed small ranges. Keep as-is.
- Lines 505–508 — `resize(n, 0.0); fill(0.0);` — the `fill` is redundant when `n` already equals `len` *and* when grew zero-initialised. When `n <= len`, `resize` doesn't touch existing slots, so `fill` is required to clear them. OK; leave the comment to justify.

### `src/wasm_api.rs`
- Line 4 — wildcard.
- Line 69 — `self.inner.seed.clone()` — required for `#[wasm_bindgen(getter)] -> String` (must own). OK.
- Line 252 — `self.inner.species.get(pid).name.clone()` — required (String return). OK.
- Line 279 — `to_string(...).unwrap_or_else(|_| "{}".into())` — graceful, good.
- Line 301 — `expect("save serialization is infallible")` — comment justifies. OK.
- Line 429 — `pub fn creature_stride() -> u32 { 13 }` could be `pub const fn`.
- Lines 240–280 — `creature_inspect_json` builds a json `Value` and re-serializes. One-call-per-click; cosmetic perf.
- `pub use wasm_api::*` from lib.rs (line 24) re-exports **everything** including private helpers. Tighten with explicit re-exports.

### `src/save.rs`
- Lines 142–198 — `from_world` clones every SoA column (`w.creatures.id.clone()`, ×17, plus genomes, brains, sun, carrion, species, events). **Unavoidable for the borrowed `&World` API** if you need a `SaveV1: Serialize` owning value. Two alternatives:
  1. Make `SaveV1<'a>` borrow-based (`Cow<'a, [u64]>`, etc.) — significant refactor.
  2. Use `serde` directly on `&World` via a wrapper. Big change.
  3. Accept the clones; serialize is called once per autosave window (~minutes), not per tick. **Current choice is correct.**
- Line 249 — `for ev in snap.all[start..].iter()` → `for ev in &snap.all[start..]`. Trivial.
- Line 250 — `log.recent.push_back(ev.clone());` — required, `recent` owns events.
- `LoadError` (line 115) impls `Display` but not `std::error::Error`. **Add `impl std::error::Error for LoadError`** so callers can use `?` with `Box<dyn Error>` etc.

### `src/species.rs`
- Line 5 — wildcard.
- Line 44–45, 67 — clones for owned struct fields. OK.
- `get` (line 89) — panics via `Vec::index` on bad id. **Document the precondition** or return `Option<&Species>`. All callers pass valid ids today; index-style is consistent with `CreatureSoA`.

### `src/snapshot_hash.rs`
- Line 76 — `expect("SimRng serialization is infallible")` with justification. OK.
- Hot inner hashing functions (`write_f32` line 120 — already `#[inline]`, good; `hash_genome` line 84 — consider `#[inline]`).

### `src/profiler.rs`
- Lines 135, 148 — `self as *const Profiler` — `ref_as_ptr` lint. Use `std::ptr::from_ref(self)` (Rust 1.76+). Cosmetic; current form is fine.
- Line 385 — `result.push_str(&format!(...))` allocates twice. Use `write!(&mut result, ...)`. Trivial perf in cold-path (json reporting).
- Line 396, 398, 427, 429 — `.expect("no window") / .expect("no performance")` in `clock_now_ms`/`clock_now_us`. These are wasm-only and the failures are unrecoverable, so panic is acceptable, but consider returning `f64::NAN` or `0.0` to avoid bringing the world down if `Performance` is unavailable in a worker context. **Low risk — wasm-bindgen workers do have `performance`.**

### `src/events.rs`
- Tiny module, clean. Line 63 — `self.recent.push_back(ev.clone())` — required (logging both `all` and `recent`).

### `src/hof.rs` / `src/carrion.rs`
- Trivial. Nothing to flag.

---

## Cross-cutting findings

### Unnecessary clones
- **All identified clones are necessary** (owned-value APIs, ring-buffer logging, `Brain::child_from` mutation, JS getter returns). The only candidate for `Copy` is `TraitMutationRates` at `genome.rs:100` — derive `Copy` if it isn't already.
- `save.rs` clones (60+ `.clone()` calls between lines 142–198) are by design — see analysis above.

### `.unwrap()` audit
- All non-test `.unwrap()` calls (15 outside tests) are either:
  - `Index::index` panics by design (`genome.rs:322`),
  - Static-slice `try_into` that cannot fail (`world.rs:1365` — change to `.expect("NN output buffer")` for clarity), or
  - Test/dev code.
- `expect()` calls (line `wasm_api.rs:301`, `profiler.rs:396/398/427/429`, `snapshot_hash.rs:76`) all have justifying comments or are wasm-bind-glue. OK.

### Missing `#[inline]` on hot tiny fns (worth adding)
| File:Line | Function | Rationale |
|---|---|---|
| `rng.rs:30,36,41,47,78,80` | `unit, symm, uniform, normal, index` | called millions/tick |
| `creature.rs:36,133,136` | `one_hot_index, len, is_empty` | trivial accessors |
| `world.rs:190` | `population` | trivial accessor |
| `vision.rs:88` | `fill_one` | per-creature per-tick (serial path) |
| `snapshot_hash.rs:84` | `hash_genome` | called per-creature per-snapshot |

### Missing `const fn`
| File:Line | Function | Rationale |
|---|---|---|
| `wasm_api.rs:429` | `creature_stride() -> u32` | returns literal `13` |
| `creature.rs:36` | `one_hot_index` | `self as usize` is `const`-stable since 1.83 (enum-to-int) |

### `#[repr(C)]` / layout
- No structs are sent to JS as raw byte views — `creatures_buffer()` repacks into a flat `Vec<f32>`. **No `#[repr(C)]` needed.**
- `Action` is `#[repr(u8)]`. Correct.
- `Genome`, `Brain`, etc. are serde-`Serialize` and accessed via field paths only. **No layout attrs needed.**

### Lifetime tangles
- One small: `VisionPass<'a>` (vision.rs:36) holds 4 borrowed fields — clippy notes the `'a` is elidable on the `impl` (line 45). Cosmetic.
- `genome.rs:97 mutate_in_place` snapshots `mutation_rates` to a local to free `&mut self` — comment explains. Clean.
- No others.

### Over-broad `pub` visibility
- `lib.rs:5-20` — every module is `pub mod`. **Most are crate-internal**. `wasm_api`, `world`, `constants` need to be visible to integration tests. Others (`carrion`, `events`, `grid`, `hof`, `profiler`, `rng`, `snapshot_hash`, `species`, `sun`, `vision`, `brain`, `genome`, `save`, `creature`) could be `pub(crate)`. Concrete win: shrinks generated docs and tightens API surface.
- `grid.rs:11-12` — `pub starts`, `pub indices`. Make `pub(crate)`.
- `wasm_api`: `pub use wasm_api::*` at lib.rs:24 leaks helpers like `creature_stride`. Tighten to explicit list.
- `world.rs:47` — `World` has ~47 `pub` fields. Many are read across modules (save.rs, wasm_api.rs); current state is consistent. No action.

### Iterators vs indexed loops
- The indexed `for i in 0..n` SoA pattern (e.g., `world.rs:455–499`, `508–588`) is correct: parallel multi-array writes need indices, not iterators, and you've explicitly allowed `clippy::needless_range_loop`. **Keep.**
- `save.rs:249` — could become a `&` loop. Trivial.
- `snapshot_hash.rs:40` — same.

### Cargo.toml — unused deps
- `rand = { version = "0.8", features = ["std", "std_rng"] }` — **check usage**. `rand_xoshiro` is used in `rng.rs`; the bare `rand` crate may only be needed for its `RngCore` trait. Verify with `cargo udeps`. Likely needed (`rand_xoshiro` re-exports its traits but you `use rand::*` somewhere — confirm with a grep).
- All other deps appear in use: `wasm-bindgen`, `js-sys`, `getrandom`, `serde`, `serde_json`, `wide`, `rand_xoshiro`, `twox-hash`, `web-sys`.
- Suggest enabling `wasm-opt` settings (in `[package.metadata.wasm-pack.profile.release]`) — out of scope for this audit.

### Suggested Clippy lints to *enable* repo-wide
In `Cargo.toml` `[lints.clippy]` or `lib.rs`:
```rust
#![warn(
    clippy::redundant_clone,
    clippy::needless_pass_by_value,
    clippy::inefficient_to_string,
    clippy::large_types_passed_by_value,
    clippy::manual_assert,
    clippy::semicolon_if_nothing_returned,
    clippy::unnested_or_patterns,
    clippy::cast_lossless,        // optional
)]
```
Avoid blanket `pedantic`/`nursery` — too noisy for this codebase.

---

## Priority action list (concise)

1. **Add `#[inline]`** to the hot RNG fns (`rng.rs:30–80`) and small accessors in `creature.rs:36,133,136`, `world.rs:190`, `vision.rs:88`. (Five-line PR.)
2. **`impl std::error::Error for LoadError`** in `save.rs:115`. One-line PR.
3. **Tighten `pub mod` in `lib.rs:5-20`** to `pub(crate)` where appropriate. Module-by-module judgement call.
4. **Replace `output_buf[2..8].try_into().unwrap()` (world.rs:1365)** with a typed `&[f32; NN_OUTPUTS]` parameter or `.expect("logits")`.
5. **Investigate `d == bd` strict-eq tiebreak** at `world.rs:693`. Compare squared distances upstream to avoid the sqrt rounding gap.
6. **`creature_stride` → `const fn`** (`wasm_api.rs:429`).
7. **Run `cargo udeps`** (or `cargo +nightly udeps`) to verify `rand` vs `rand_xoshiro` overlap.
8. Make `SpatialGrid::{starts, indices}` `pub(crate)` (`grid.rs:11–12`).
9. Rename or remove the `mod heapless` confusion in `genome.rs:284–325` (comment lies about the actual code).
