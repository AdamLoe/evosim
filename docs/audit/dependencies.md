# Dependency Audit

Scope: `Cargo.toml` (+ `Cargo.lock`, 51 crates resolved) and `web/package.json` (+ `pnpm-lock.yaml`).
Method: per-crate grep of `src/` and `tests/` for direct usage, plus `cargo tree --depth 1` for transitive map.

---

## TL;DR

- **One unused top-level crate**: `rand` (declared, never imported directly).
- **One outdated major**: `twox-hash` 1.6 -> 2.x (smaller, no `static_assertions`).
- **One out-of-sync minor**: `wasm-bindgen-rayon` pinned to `1.2`; lock resolves to `1.3.0`. Cosmetic; bump for clarity.
- **Two `default-features` candidates already controlled** (`rand`, `twox-hash`). Add `default-features = false` to `serde_json` to drop the `std`-default footprint? No — `serde_json` requires `std` for its main API; skip.
- **Web side is minimal** (`vite` + `typescript` dev-only). No prod runtime JS deps. Nothing to drop.

Estimated wasm-size delta of the recommended changes: **~30-80 KB** off the `default` (non-threads) wasm, dominated by removing `rand` -> `rand_chacha` + `ppv-lite86` + `zerocopy` (+derive) + `libc` link. Threads build saves the same plus is unaffected by other changes.

---

## Drop list (unused / dead)

| Crate | Where declared | Evidence | Action |
| --- | --- | --- | --- |
| `rand` 0.8 | `Cargo.toml:19` | No `use rand::`, no `rand::` paths, no `extern crate rand` anywhere in `src/` or `tests/`. The only `rand_*` direct usage is `rand_xoshiro` in `src/rng.rs`, which depends on `rand_core` (not `rand`). | **Remove from `[dependencies]`**. Pulls in `rand_chacha`, `ppv-lite86`, `zerocopy`, `zerocopy-derive`, `libc`. |

Verification: `grep -rn -E "^use rand(::\|;\| )\|extern crate rand" src/ tests/` -> 0 hits.

Transitive crates that disappear from the dep graph after dropping `rand` (none are used elsewhere; confirmed by `Cargo.lock`):
- `rand_chacha` (only consumer is `rand`)
- `ppv-lite86` (only consumer is `rand_chacha`)
- `zerocopy`, `zerocopy-derive` (only consumer is `ppv-lite86`)
- `libc` is also pulled by `getrandom` for non-wasm targets, so it stays as a `cfg(unix)` build dep but is no longer in the wasm build path through `rand`.

---

## Downgrade-features list

Current `default-features` posture is already good. Notes:

| Crate | Current | Suggestion |
| --- | --- | --- |
| `serde` | `features = ["derive"]` (default-features on) | Keep. `std` is needed by `serde_json`. |
| `serde_json` | default-features on | Keep. Its defaults are minimal (`std` only); no `preserve_order` / `arbitrary_precision` / `raw_value` in code. |
| `rand_xoshiro` | `features = ["serde1"]` | Keep. `serde1` is required by `snapshot_hash.rs` (`serde_json::to_vec(&w.rng)`). |
| `twox-hash` | `default-features = false` | Already optimal for v1. Migrate to v2 (below). |
| `web-sys` | hand-picked features only | Already optimal. All 7 features (`console`, `Window`, `Document`, `Element`, `HtmlCanvasElement`, `CanvasRenderingContext2d`, `Performance`) are referenced. `Document` and `Element` are referenced only via type chains from `Window::document()` -> needed; do not remove without checking compile. |
| `getrandom` | `features = ["js"]` | Required for wasm; keep. |
| `wasm-bindgen-rayon`, `rayon` | `optional = true` behind `threads` | Already optimal — default build excludes both and all the crossbeam-* / wasm_sync transitive bloat. |

No tokio/async-std in the tree. Nothing to trim there.

---

## Outdated majors / version sync

| Crate | Declared | Resolved | Latest major | Action |
| --- | --- | --- | --- | --- |
| `twox-hash` | `1.6` | `1.6.3` | `2.1.2` | **Bump to `2`**. v2 drops `static_assertions`, modernises API to `XxHash64::oneshot(...)` / streaming `XxHash64::new()` (still works). Smaller code-gen; one less transitive crate. Touches 2 files (`src/rng.rs`, `src/snapshot_hash.rs`); API shift is minor (e.g. `XxHash64::with_seed(s)` becomes `XxHash64::with_seed(s)` in 2.x as well; verify the streaming `Hasher` trait import). |
| `wasm-bindgen-rayon` | `1.2` | `1.3.0` | `1.3.0` | Bump declared version to `1.3` for clarity (no code change; cargo already pulled 1.3). |
| `wasm-bindgen` | `0.2` | `0.2.122` | `0.2.x` current | Up-to-date. |
| `web-sys`, `js-sys` | `0.3` | `0.3.99` | current | Up-to-date. |
| `wide` | `0.7` | `0.7.33` | `0.7.x` | Up-to-date. |
| `rand_xoshiro` | `0.6` | `0.6.0` | `0.7` exists (req `rand_core 0.9`) | Hold. Bumping requires migrating `RngCore`/`SeedableRng` traits; low value. |
| `getrandom` | `0.2` | `0.2.17` | `0.3` exists | Hold; `0.3` changes the `js` feature surface. Not a wasm-size win. |

---

## Heavy-dep replacement suggestions (wasm-size oriented)

1. **`twox-hash` 1.x -> `twox-hash` 2.x** (preferred) or replace with `xxhash-rust` (`features=["xxh64"]`).
   - Both ship pure-Rust XXH64 with no transitive deps. `xxhash-rust` is the leanest by line count.
   - Hash inputs are deterministic snapshot bytes; any conformant XXH64 will produce identical digests, so golden snapshots survive a swap to `xxhash-rust`. Skipping a hash-impl swap is fine if golden files are seeded against this exact impl already — verify before swapping.

2. **`serde_json` for the small known-shape payloads in `wasm_api.rs`**: most JSON producers there are tiny fixed-shape objects (`{"tick": ..., "alive": ...}`) built via `serde_json::json!`. Replacing those with hand-rolled `String` formatting would let you drop `serde_json` from the wasm code path used at runtime — but `save.rs` (`SaveV1`) and `snapshot_hash.rs` (`to_vec(&rng)`) genuinely need it. So `serde_json` stays; not worth the surgery.

3. **`wide`** is used only for the `f32x8` SIMD MLP forward pass in `brain.rs`. It is small (pulls `bytemuck` + `safe_arch`) and load-bearing for perf. Keep.

4. **Profile tweak**: `profile.release` already has `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`. Consider testing `lto = "fat"` for wasm; typical 5-15% extra size reduction at the cost of link time. Not strictly a dep change.

---

## Redundant deps / overlap

None. Cross-checked:
- `getrandom` (entropy source) vs `rand_xoshiro` (PRNG) — different roles, no overlap.
- `rand` (declared) would be redundant with `rand_xoshiro` for sim PRNG but isn't even used; see drop list.
- `wide` (SIMD MLP) vs anything else — no other SIMD or vector crate present.
- `twox-hash` vs `serde_json` — different jobs (content hashing vs serialization).

---

## Transitive bloat hotspots

Build with `threads` feature off (default v1 ship target):
- Worst single offender today is the `rand`/`rand_chacha`/`ppv-lite86`/`zerocopy*` chain (~5 crates) that is dead code. Dropping `rand` eliminates it.
- `wasm-bindgen-macro-support` -> `syn` -> `proc-macro2`, `quote` are proc-macro build-deps; they do not land in the final wasm. Ignore.
- `futures-util` is pulled by `js-sys` only as a feature surface; verify whether the `default` features of `js-sys` are pulling it unnecessarily — but trimming `js-sys` defaults is fragile and yields little. Skip.

`threads` feature on:
- `rayon` -> `rayon-core`, `crossbeam-channel`, `crossbeam-deque`, `crossbeam-epoch`, `crossbeam-utils`, `wasm_sync`, `either`. All load-bearing for `par_chunks_mut` in `vision.rs` and `par_iter` in `world.rs`. Keep.

---

## Web package audit

`web/package.json` declares only `vite` and `typescript` as `devDependencies`. No runtime npm dependencies; the page imports from `./wasm/...` artifacts only. `pnpm-lock.yaml` is dominated by per-platform `@esbuild/*` and `@rollup/*` optional natives (devDeps; do not ship). Nothing actionable.

---

## Concrete diff to apply (recommended minimum)

```toml
# Cargo.toml

# REMOVE this line (unused):
# rand = { version = "0.8", default-features = false, features = ["std", "std_rng"] }

# BUMP:
twox-hash = { version = "2", default-features = false }   # was "1.6"
# (sync to lock; minor)
[dependencies.wasm-bindgen-rayon]
version = "1.3"   # was "1.2"
optional = true
```

Code touch:
- `src/rng.rs` and `src/snapshot_hash.rs`: adjust `use twox_hash::XxHash64;` and possibly the `Hasher` import for 2.x API; re-run golden snapshot tests to confirm hashes are unchanged (XXH64 algorithm itself is byte-identical across versions, so `tests/acceptance.rs` should pass without regenerating goldens).

## Wasm-size impact estimate

- Drop `rand` + chain: **~25-60 KB** off `.wasm` after LTO + opt-level=3 (`rand_chacha` + `ppv-lite86` SIMD/scalar paths + `zerocopy`). Mostly dead-code today (compiler can prune some), but it still pays linker time and inflates incremental builds.
- `twox-hash` 1 -> 2: **~3-10 KB**, mostly from dropping `static_assertions` and the older const-eval macros.
- `wasm-bindgen-rayon` 1.2 -> 1.3 declaration sync: 0 KB (already resolved to 1.3).

Combined headline: **~30-70 KB** smaller default wasm, **0 KB** regression risk, **no API surface change** outside the two `twox-hash` import sites.
