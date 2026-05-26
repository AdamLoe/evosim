# Audit triage

Date: 2026-05-24. Triages every finding in `docs/audit/` (16 docs).
Buckets: SHIP (do this pass), DEFER (v1.2+ candidate), REJECT
(out of scope / duplicate / wrong / shipped / v5 §14 deferred).

Hard rules honored:
- v5 §14 deferred items (WebGPU/WebGL renderer, NEAT, lineage viz, NN viz,
  follow-cam, signaling, sexual repro, Tauri, trait histograms) are NOT in SHIP.
  big-wins #1 and #3 are flagged "needs user approval".
- Items duplicating already-shipped perf-1..5, perf-4 threads, ui-stats /
  ui-inspector / ui-perf, perf-timing → REJECT with "already shipped".
- sim-balance.md is gameplay tuning; whole doc DEFER except §8's
  per-tick biggest_ever bug (perf+correctness — SHIP).
- F.30 founder NN-init hardwiring is an accepted spec deviation; do NOT
  propose fixes to it absent NEW evidence (sim-balance §5 IS a new tuning
  observation but it's still tuning — DEFER).
- Items requiring golden regen are grouped so we regen once.

## Summary counts

| Bucket | Count |
|---|---|
| SHIP | 38 |
| DEFER | 30 |
| REJECT | 25 |
| Needs user approval | 4 (big-wins #1, #3, #8 quadtree, #9 replay) |

---

## SHIP

Grouped by suggested commit ordering. Each group is one logical PR.

### Group A — Foundational refactor (do FIRST; nothing else in this pass should land before A1)

#### S1. Split `src/world.rs` (2 291 LOC) into `src/world/{mod,tick,nn,save_v1}.rs` — `architecture:A1`
**What:** Mechanical move; no behavior change.
**Why ship:** Unblocks every other SHIP item that touches `world.rs` (S5, S7, S8, S10–S14, S17, S22, S24, S27); removes the biggest merge-conflict surface.
**Effort:** L (half day, mostly mechanical).
**Determinism impact:** none (no logic change).
**Depends on:** none.

#### S2. Always-on panic hook (release too) — `architecture:B7`
**What:** Move `console_error_panic_hook::set_once()` out from `cfg(debug_assertions)`.
**Why ship:** 3 lines; release wasm currently silently aborts.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** none.

#### S3. Move `BODY_RADIUS_PER_SIZE` to `constants.rs`; delete vision re-export — `architecture:A6` / `architecture:D1`
**What:** Single home for the constant.
**Why ship:** 5 min, removes a half-resolved seam.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** none.

### Group B — Determinism / correctness (golden regen batch — do SECOND, regen sequential + threaded goldens ONCE at end)

#### S4. C1: `place_hotspots` fallback never resets `attempts` — `correctness-bugs:C1` — `src/sun.rs:114-139`
**What:** Use `else if attempts > 2000`; today can OOB-panic if hot-spot config changes.
**Why ship:** trivial fix, real latent bug.
**Effort:** XS.
**Determinism impact:** **regen golden** (changes RNG draws on init? no — current config never hits the bug, so likely safe; verify with bootstrap). Group with golden regen batch defensively.
**Depends on:** S1.

#### S5. C8: `decode_action` NaN propagation — `correctness-bugs:C8` — `src/world.rs:1322-1339`
**What:** Detect NaN in logits before sort; force `Rest` + zero velocity. Add debug-only `is_finite` assert in tick hot path.
**Why ship:** silences a real silent-corruption path (NaN can persist into position).
**Effort:** S.
**Determinism impact:** **regen golden** (no NaN today, so identical, but verify).
**Depends on:** S1.

#### S6. C9: `mutate_f32` / `mutate_u32` NaN defense — `correctness-bugs:C9` — `src/genome.rs:262-275`
**What:** `if next.is_finite() { ... } else { *value }`.
**Why ship:** trivial defense in depth, pairs with S5.
**Effort:** XS.
**Determinism impact:** none today (rng.normal cannot produce NaN); but defensive, group with B-batch.
**Depends on:** S1.

#### S7. C4 (partial) + extend snapshot_hash to cover all sim-determining state — `correctness-bugs:C4` — `src/snapshot_hash.rs`
**What:** Add `digestion_cooldown`, `species_id`, `parent_species_id`, `cumulative_upkeep`, `last_action`, `action_this_tick`, `max_size_reached`, `distance_travelled`, `birth_tick`, `carrion.sun_cell`, `carrion.id`, species `name`/`parent_id`/`depth`/`died_tick`/`child_count`/`anchor_brain_weights` to the hash. Also: canonicalize NaN bits.
**Why ship:** real divergence-detection gap; the golden currently misses bugs in these fields.
**Effort:** M.
**Determinism impact:** **regen golden** (new hash). The CENTRAL item of group B; everything else here piggybacks the regen.
**Depends on:** S1.

#### S8. R4: replace `serde_json::to_vec(&w.rng)` with direct LE hash of 4 xoshiro u64s — `determinism:R4` — `src/snapshot_hash.rs:76`
**What:** Hash `(s0,s1,s2,s3): [u64;4]` directly; eliminate serde_json dep in the hash path.
**Why ship:** removes invisible coupling to serde_json output stability; pairs with S7.
**Effort:** S.
**Determinism impact:** **regen golden** (hash bytes differ); batches with S7.
**Depends on:** S7.

#### S9. R6: lint rule forbidding `HashMap::iter`/`into_iter` in sim-critical files — `determinism:R6`
**What:** grep-gate in CI, or `#[deny(...)]` attribute set + custom clippy.toml lint, on `world.rs`, `snapshot_hash.rs`, `species.rs`, `creature.rs`, new `world/*.rs`.
**Why ship:** prevents a class of silent golden-flip bugs.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1.

#### S10. R1 / C10: tighten `chunk_ranges` vs `par_chunks_mut` partition; debug-assert chunk count — `determinism:R1` / `correctness-bugs:C10` / `architecture:F2` — `src/vision.rs:74`, `src/world.rs:1247`
**What:** Add unit test asserting both partitions agree for `n ∈ {0,1,7,8,9,100,1500}`; debug_assert chunk count post-partition.
**Why ship:** closes a thread-determinism foot-gun.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** S1.

#### S11. D2 / F1: extract `count_carrion_overlap` and `compute_is_at_wall` from threaded inline — `architecture:D2` / `architecture:F1` — `src/world.rs:394-425, :1174`
**What:** Hoist to free functions; both paths call them. Removes cfg-allow dead-code annotation.
**Why ship:** removes the biggest remaining sequential/threaded code-duplication drift risk.
**Effort:** S.
**Determinism impact:** **regen golden** (refactor only; should be byte-identical, verify with bootstrap and only re-pin if hash matches old or intentionally changes).
**Depends on:** S1.

### Group C — Security hardening (do anytime after A; the H1 work is independent of golden regen)

#### S12. H1: `validate_save(&SaveV1) -> Result<(), LoadError>` — `security:H1` / `correctness-bugs:C5` — `src/world.rs:1047-1167`
**What:** Validate sun array lengths, `species_id` range, `carrion.sun_cell < SUN_DIM²`, `x/y` finite + in `[0,WORLD_SIZE)`, `max_id` overflow via `checked_add`, hard cap on creature count (e.g. 100k). Return `LoadError::StructuralError`; JS already handles.
**Why ship:** prevents wasm-panic DoS from crafted save (the #1 security finding).
**Effort:** M.
**Determinism impact:** none.
**Depends on:** S1.

#### S13. M3: Content-Security-Policy header — `security:M3` — `web/public/_headers`
**What:** Add CSP with `default-src 'self'`, `script-src 'self' 'wasm-unsafe-eval'`, etc.
**Why ship:** one-line config; defense-in-depth.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** none.

#### S14. M5: delete `appendEventRow` / `formatEvent` innerHTML sink — `security:M5` / `web-wasm:S4` / `wasm-api:S4` — `web/src/rail/events.ts`
**What:** Events panel is `display:none` and dead-coded; delete the file + remove imports. Future re-enable in v1.2 rebuilds it with `textContent`.
**Why ship:** removes a latent XSS sink; aligns with big-wins #7's "decide" call (we're picking "delete for now").
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** none.

#### S15. L2: cap URL `seed` param length to 128 chars in JS — `security:L2` — `web/src/main.ts:215`
**What:** `seedParam.slice(0, 128)` before `new WorldHandle(seedParam)`.
**Why ship:** prevents 10MB-URL IndexedDB pollution.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** none.

#### S16. L1: replace `expect("save serialization is infallible")` with `unwrap_or_else(|_| "{}".into())` — `security:L1` / `web-wasm:7.7` — `src/wasm_api.rs:301`
**What:** Consistent with other JSON paths.
**Why ship:** trivial, removes the one panic-path-on-OOM in the wasm boundary.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** none.

### Group D — Wasm boundary cleanup (cheap wins, no golden impact)

#### S17. B1 / wasm-api:8.2: type `WorldHandle::set_slider` — `architecture:B1` / `wasm-api:8.2` / `web-wasm:8.2` — `src/wasm_api.rs:171-183`
**What:** Either per-slider methods (`set_base_sun_rate(f32)`) or return `Result<(), JsValue>` on unknown name.
**Why ship:** silent-ignore failure mode is a real footgun; 20 LOC.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1.

#### S18. B2 / S9: stable-id `creature_at`; add `creature_idx_by_id(id) -> Option<u32>` — `architecture:B2` / `wasm-api:S9,M2` / `web-wasm:1.7`
**What:** Return stable `Option<u64>` from `creature_at`; add `creature_idx_by_id`. TS-side inspector drops per-frame ids-buffer scan.
**Why ship:** removes the per-frame linear scan; tiny code surface; unblocks future lineage viz.
**Effort:** M.
**Determinism impact:** none.
**Depends on:** S1.

#### S19. Delete dead wasm-api: `recent_events_json`, `events_total_count` — `wasm-api:S4,S5` / `web-wasm:S4,S5`
**What:** Plus delete the TS-side `EvKind`/`EvEvent` re-exports (`architecture:B6`).
**Why ship:** dead code; reduces wasm bundle + JSON-serde footprint.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** S14 (which removes the last TS reference).

#### S20. S8: `creature_at` uses `SpatialGrid` — `wasm-api:S8` / `web-wasm:1.3` — `src/wasm_api.rs:226-238`
**What:** Bound the scan via existing grid.
**Why ship:** free win; protects 100× speed users from multi-ms click freezes at high pop.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1 (or do alongside S18).

#### S21. A5 / web-wasm:1.2 / wasm-api:H1 (smaller variant): drop unused `creatures_buffer` flag floats — `architecture:A5` / `wasm-api:H1` / `web-wasm:1.2`
**What:** `energy_frac` and `age_frac` are commented out in renderer; just drop them. Read flag bits from `g_eye_count` etc. SoA mirrors (perf-5) not from AoS `genomes[i]`. Cut stride 13→reduced; bump `creature_stride()`.
**Why ship:** kills the perf-5 cache regression in the per-frame pack, removes per-frame branchy reads.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1 (touches wasm_api + render.ts).

#### S22. H5/H6/A1/A4: status-bar getter hygiene + DPR cap — `wasm-api:H5,H6,A1,A4` / `web-wasm:4.1` — `web/src/main.ts`
**What:** Cache `world.seed` at boot; hoist `world.world_ended` once per frame; throttle status DOM updates to 5 Hz; cap DPR at 2 for the aquarium canvas.
**Why ship:** trivial, removes per-frame `String` clone + 4 boundary calls + 4K-phone fill-rate cliff.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** none.

### Group E — Per-tick perf (no golden regen — partition-stable; do AFTER B's regen lands)

#### S23. perf-hot-loop #2: threaded NN writes directly into SoA via `par_chunks_mut` — `perf-hot-loop:#2` / `allocations:#1` — `src/world.rs:385-446`
**What:** Eliminate `Vec<(f32,f32,Action)>` flat_map + drain; write `vx`/`vy`/`action_this_tick` chunk slices in-place. Per-piece SoA + chunk_size identical to today's `par_chunks_mut(chunk_size)`.
**Why ship:** removes 9 allocs/tick + N tuple copies; perf-2 pattern; identical partition so threaded golden unchanged.
**Effort:** M.
**Determinism impact:** none (deterministic chunk partition).
**Depends on:** S1, S11.

#### S24. perf-hot-loop #4: `Action::Scavenge` uses `cell_to_carrion` (O(scavengers × cells) not × carrion) — `perf-hot-loop:#4` — `src/world.rs:720-730`
**What:** 3×3 cell sweep instead of linear scan over all carrion.
**Why ship:** future-proofing for carrion-rich states; localized; no determinism risk (order of best-hit unchanged because today's `break` stops at first hit and we replicate same order).
**Effort:** S.
**Determinism impact:** **verify**: today is "first match within full carrion vec"; new is "first match within 9 cells in grid order". If first match differs, regen golden. SHIP this into the Group-B regen batch if hashes differ; otherwise standalone.
**Depends on:** S1.

#### S25. perf-hot-loop #3 / allocations:#1: `scratch_eat_candidates` pool — `perf-hot-loop:#3` / `allocations:cell-1` — `src/world.rs:676` / `correctness-bugs:C13`
**What:** Either pool the Vec like perf-2 did, or inline the body inside `for_each_in_radius` (preferred). Eliminate per-Eat-per-tick alloc.
**Why ship:** perf-2 pattern, completes the scratch-pool sweep.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1.

#### S26. allocations:#5 / perf-hot-loop:#7: `cell_to_carrion` → CSR layout — `allocations:vision.rs:329-331` / `perf-hot-loop:#7` — `src/vision.rs`
**What:** Replace `Vec<Vec<u32>>` (14 400 inner vecs, 345 KB headers) with `starts: Vec<u32>` + `indices: Vec<u32>` like `SpatialGrid`.
**Why ship:** biggest memory-footprint win + cache win; same pattern as perf-3.
**Effort:** M.
**Determinism impact:** none (same per-cell membership).
**Depends on:** S1, S24 (which already reads `cell_to_carrion`).

#### S27. allocations:#2, #3: `scratch_dead` and `scratch_species_lost` pools — `allocations:#2,#3` / `perf-hot-loop:#16` — `src/world.rs:831-834`
**What:** Promote both Vecs to `World` fields; collapse `species_lost` into `pending_extinction_check` directly.
**Why ship:** pattern consistency with perf-2.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1.

#### S28. allocations:#3 / perf-hot-loop:#17: replace `HashSet` in `finalize_extinctions` with `Vec<bool>` — `allocations:#3` / `perf-hot-loop:#17` — `src/world.rs:1008`
**What:** dense bool indexed by species_id; resize + clear each call.
**Why ship:** removes HashMap allocator churn + RandomState seeding (also closes a latent R6 risk).
**Effort:** S.
**Determinism impact:** none (HashSet was only used for `contains`, never iterated).
**Depends on:** S1, S27.

#### S29. perf-hot-loop:#15 + sim-balance:§8: gate `biggest_ever` update to `handle_births` (size only changes at birth) — `perf-hot-loop:#15` / `sim-balance:§8` — `src/world.rs:805-821`
**What:** Move the per-tick O(N) `biggest_ever` check out of `energy_bookkeeping` into `handle_births`; check newborn against record.
**Why ship:** sim-balance flags it as a perf+correctness item (per brief); this is the only sim-balance item that ships.
**Effort:** S.
**Determinism impact:** none (HoF state observed only at run-end; the captured snapshot is still the largest creature, identical timing for the comparison).
**Depends on:** S1.

#### S30. perf-hot-loop:#12: swap `last_action` / `action_this_tick` instead of copy — `perf-hot-loop:#12` — `src/world.rs:294-296`
**What:** `std::mem::swap` end-of-tick; saves an N-element memcpy.
**Why ship:** O(1) instead of O(N); trivial.
**Effort:** XS.
**Determinism impact:** none (`action_this_tick` is overwritten by nn_forward before read).
**Depends on:** S1.

#### S31. perf-hot-loop:#10: skip wall-clamp grid rebuild iff no wall fired — `perf-hot-loop:#10` — `src/world.rs:591`
**What:** Track `any_wall: bool` in clamp loop; gate the final `grid.rebuild` on it.
**Why ship:** typical mid-world ticks have zero wall hits; saves one rebuild/tick.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1.

#### S32. perf-hot-loop:#9: cache `cell_of` once in `grid.rebuild` — `perf-hot-loop:#9` — `src/grid.rs:50-66`
**What:** Add `cells: Vec<u32>` scratch to `SpatialGrid`; compute `cell_of` once per creature per rebuild.
**Why ship:** perf-3 pattern; eliminates 6 cell_of calls per creature per tick.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1.

### Group F — Rust hygiene (cheap, no determinism impact)

#### S33. `#[inline]` on hot tiny RNG fns + small accessors — `rust-quality:priority-1` — `src/rng.rs:30-80`, `src/creature.rs:36,133,136`, `src/world.rs:190`, `src/vision.rs:88`, `src/snapshot_hash.rs:84`
**What:** Add `#[inline]` attributes.
**Why ship:** RNG fns are called millions of times/tick; small accessor pattern.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** S1.

#### S34. `impl std::error::Error for LoadError` — `rust-quality:priority-2` — `src/save.rs:115`
**What:** One-line trait impl.
**Why ship:** lets callers use `?` with `Box<dyn Error>`.
**Effort:** XS.
**Determinism impact:** none.
**Depends on:** none.

#### S35. Tighten `pub mod` to `pub(crate)` where appropriate — `rust-quality:over-broad visibility` / `architecture:E1,E2` — `src/lib.rs:5-20`, `src/grid.rs:11-12`
**What:** Internal modules become `pub(crate)`; `SpatialGrid::{starts,indices}` become `pub(crate)`.
**Why ship:** tightens API surface; helps A1's downstream module split.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S1.

#### S36. Remove `mod heapless` confusion in genome.rs — `rust-quality:genome.rs:284-325` / `architecture:D4`
**What:** Replace 40-LOC in-house heapless with `[Option<u8>; 2]` + count, or two branches.
**Why ship:** comment claims overkill but code still uses it; ~5 LOC after.
**Effort:** XS.
**Determinism impact:** none (identical semantics).
**Depends on:** none.

### Group G — Build / tooling (cheap, biased SHIP per brief)

#### S37. Drop unused `rand` crate; bump `twox-hash` 1→2; sync `wasm-bindgen-rayon` to 1.3 — `dependencies` / `build-tooling:1,2` — `Cargo.toml`
**What:** Three small Cargo.toml edits + `XxHash64::with_seed` API confirmation (algorithm byte-identical so goldens survive; verify via test).
**Why ship:** ~30–70 KB smaller wasm; cleaner dep graph; no API surface change.
**Effort:** S.
**Determinism impact:** none expected (XXH64 algorithm is version-stable; verify with existing acceptance run).
**Depends on:** none.

#### S38. Add wasm-opt metadata block + pin Rust toolchain + drop sourcemap in prod — `build-tooling:#1,#6,#7` / `web-wasm:6.1` — `Cargo.toml`, `rust-toolchain.toml`, `web/vite.config.ts`
**What:** `[package.metadata.wasm-pack.profile.release]` with `wasm-opt = ["-O4", ...flags]`. `rust-toolchain.toml` pinning stable + nightly-for-wasm. Vite `sourcemap: process.env.CI ? "hidden" : true`.
**Why ship:** ~25% wasm-size win, repro guard, removes shipped 147KB sourcemap.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** none.

---

## DEFER (v1.2+ candidate)

One-line per item. Most reasons: requires golden regen for marginal gain,
needs design input, or is too large to fit a cleanup pass.

- D1. C2 `geom_skip` off-by-one — `correctness-bugs:C2` — needs spec call on Bernoulli semantics + golden regen for a tiny stats bias.
- D2. C7 eye-offset mutation of newly-active slots — `correctness-bugs:C7` — evolutionary-fairness only, no panic/divergence.
- D3. C11 species-distance anchor disambiguation — `correctness-bugs:C11` — spec call (anchor vs founder); flag for design review.
- D4. C12 `EventLog::default` ring_cap=0 — `correctness-bugs:C12` — superseded by S14 if we delete the events module entirely; revisit only if events return in v1.2.
- D5. C6 threaded NN result-ordering invariant comment — `correctness-bugs:C6` — captured by S10's debug-assert; full `(usize, vx, vy, action)` rewrite is the wrong direction (S23 supersedes).
- D6. M1 `Float32Array::view` lifetime contract — `security:M1` — superseded by wasm-api M1 (DEFER) and S23 reducing surface; today the use is safe.
- D7. M2 esbuild dev-server CORS — `security:M2` — dev-only; waits for Vite-6 upgrade.
- D8. M4 profiler `unsafe` raw ptr tightening — `security:M4` — local soundness arg holds; `PhantomData<*const ()>` polish for later.
- D9. L3, L5 — `security:L3,L5` — latent inspector sink (textContent-safe today); `cargo audit` in CI (covered partially by S38).
- D10. R3 `RAYON_NUM_THREADS` matrix CI — `determinism:R3` — wanted, but adds CI matrix complexity; defer to v1.2.
- D11. R7 libm vs platform for `f32::ln` — `determinism:R7` — only matters cross-platform; we ship wasm + Linux CI today.
- D12. R8 wide-crate reduce_add lane-order assertion — `determinism:R8` — pinned `wide = "0.7"` and explicit equality test is fine for v1.x.
- D13. A2 / B6 `events_enabled` removal — `architecture:A2,B6` — superseded by S14 (delete TS) + S19 (delete API); the Rust `events_enabled` field stays as a one-line revert switch.
- D14. A3 `TickScratch` substruct — `architecture:A3` — nice cleanup, no functional win; defer until scratch fields grow further.
- D15. A4 `genomes` accessor encapsulation — `architecture:A4` — needed eventually for safety; ~20 call-site edits is too much for this pass.
- D16. A7 data-driven `Action::is_valid` — `architecture:A7` — only matters when adding a 7th action.
- D17. B3 inspector JSON binary path — `architecture:B3` / `wasm-api:H7` / `web-wasm:1.8` — DEFER aggressive rewrite; partial mitigation via 5 Hz throttle (D18).
- D18. wasm-api:H7 / web-wasm:9.3: throttle `creature_inspect_json` to 5 Hz — DEFER (touches inspector widget; do alongside the next inspector UI iteration).
- D19. B5 split `main.ts` into boot/autosave/loop modules — `architecture:B5` — pure cleanup; doesn't block other SHIP work.
- D20. C1 / C2 / C3 / C4 / D5 module merges — `architecture:C1-C4,D4,D5` — pure cleanup; do alongside A1 if budget allows, otherwise punt.
- D21. C5 (mutation-rate term in species_distance) — `architecture:C5` — spec call; trait-histogram work bundles this.
- D22. perf-hot-loop:#1, #18, #20 — NN weight transpose / SIMD ray-vs-circle / aligned weights — `perf-hot-loop:#1,#18,#20` / `big-wins:#5` — high-value but require save schema change + dual golden regen + restructured `Brain`; the next perf pass after this one.
- D23. perf-hot-loop:#5, #6 — partition-by-action + merged photosynth two-pass — `perf-hot-loop:#5,#6` — moderate code churn; do after Group E settles to re-profile.
- D24. perf-hot-loop:#8, #11, #13, #14, #19 — tiny optimisations — `perf-hot-loop:#8,#11,#13,#14,#19` — each <1%; bundle into a future micro-perf PR.
- D25. allocations:cell-4 (Brain weight pool) — `allocations:brain.rs:169` / `big-wins:#5` — 13.5 KB per birth alloc; defer until births dominate per profiling (currently rayon/vision dominates).
- D26. observability Tier A sub-spans + TPS + jank counter — `observability:Tier-A` — high-value but spans the wasm boundary and the perf widget; design it as its own pass.
- D27. observability Tier B (counters_json, p50/p95/p99, profiler_node_count) — `observability:Tier-B` — depends on Tier A.
- D28. test-gaps P1, P2, P3 (selective) — `test-gaps:P1,P2,P3` — selectively SHIP the threaded-equiv test (S39 below); rest defer.
- D29. test-gaps P4–P8 — `test-gaps:P4-P8` — broad coverage gaps; defer to dedicated test-hardening pass.
- D30. wasm-api H1/H2/H3/H4 full ptr-based rewrite + M1 batched snapshot — `wasm-api:H1-H4,M1` / `web-wasm:1.x` / `big-wins:#6` — large structural change; defer until after S21 measures the easy win.
- D31. web-wasm 1.10–1.12 Path2D batching / highlight set / fillStyle string alloc — `web-wasm:1.10-1.12` — render-perf wins; defer until a render-pass orchestration.
- D32. sim-balance.md whole doc except §8 — gameplay tuning, separate orchestration concern.
- D33. plans-coherence cleanup actions — `plans-coherence:1-6` — archive shipped plans, refresh stale headers; useful housekeeping, low priority.
- D34. big-wins #2 (MAX_POPULATION pre-alloc), #4 (memory budget + binary save), #5 (Brain slab), #6 (frame_snapshot), #7 (delete events — partially S14), #10 (drop sequential codepath) — DEFER; each is a multi-day structural shift. #7 partially ships via S14/S19/S26.

Insert into test backlog (one S-item promoted, the rest D):

#### S39. test-gaps P0+P1: `acceptance_threaded_matches_sequential_t10000` + `save_load_hash_equal_immediately_after_load` + chunk-count invariant — `test-gaps:P0,P1`
**What:** Three new tests; assert threaded == sequential; assert save→load hash matches BEFORE stepping; assert chunk-count partition invariant.
**Why ship:** P0 items per brief; closes load-bearing equivalence assertions today's golden does not enforce.
**Effort:** S.
**Determinism impact:** none.
**Depends on:** S10, S11, S12.

(S39 lives in Group B since it pairs with the golden-regen work.)

---

## REJECT

One-line each. Reasons: already shipped, duplicates of S-items, v5 §14 deferred, or wrong.

- R1. perf-1 sector trig — `perf-hot-loop overlaps` — **already shipped** (`cbb410e`).
- R2. perf-2 scratch pool — **already shipped** (`c382916`).
- R3. perf-3 grid cursor — **already shipped** (`1844f78`).
- R4. perf-5 genome SoA — **already shipped** (`8f8d202`).
- R5. perf-4 threads + dual golden — **already shipped** (`41a91a5`).
- R6. ui-stats top-left box — **already shipped** (`0d9b726`).
- R7. ui-inspector top-right popup — **already shipped** (`642c7e7` + `2422437`).
- R8. ui-perf bottom-left box — **already shipped** (`c9e446f`).
- R9. perf-timing profiler — **already shipped**; `plans-coherence:1` confirms.
- R10. big-wins #1 WebGL/WebGPU renderer — **v5 §14 deferred**; needs user approval.
- R11. big-wins #3 NEAT brain — **v5 §14 deferred** + invalidates F.30 founder hardwiring; needs user approval.
- R12. big-wins #2 MAX_POPULATION (full) — out of scope this pass; see D34. Partial mitigation via S12's hard cap in save validation.
- R13. big-wins #4 memory budget + binary save — out of scope; see D34.
- R14. big-wins #5 Brain slab — out of scope; see D34/D25.
- R15. big-wins #6 batched frame_snapshot — out of scope; see D30.
- R16. big-wins #8 quadtree spatial index — needs user approval (architectural).
- R17. big-wins #9 deterministic replay/scrubber — needs user approval (product call).
- R18. big-wins #10 ship threads-only — REJECT this pass; the dual-codepath protects WSL2 devs and SAB-less browsers. Revisit when WSL2 perf is fixed.
- R19. Architecture B3 inspector typed-array rewrite — wrong target right now; throttle (D18) is the cheap mitigation.
- R20. observability Tier C (perf budget regression test, event-firehose hardening, wasm-memory readouts) — needs its own plan; out of scope.
- R21. sim-balance §1–§7, §9, §10 — gameplay tuning; not code-fix-able by orchestrator. Per brief, "DEFER as a whole except §8" — categorized REJECT-for-this-pass because the user explicitly scoped balance separately.
- R22. test-gaps P5–P8 — too broad for a cleanup pass; bundle as a separate testing-hardening orchestration.
- R23. clippy `mul_add` / `hypot` suggestions across hot paths — `rust-quality:suboptimal_flops` — wasm32 has no FMA target; clippy is wrong here.
- R24. clippy `imprecise_flops` (use `hypot`) — `rust-quality:imprecise_flops` — `hypot` is much slower than `sqrt(dx²+dy²)`; clippy wrong for our use.
- R25. cargo deny / cargo audit / criterion bench harness / perf budget regression test / ESLint / pre-commit hooks / web-wasm A11y full pass — `build-tooling:#3,#4,#5,#8` / `web-wasm:§5,§7` — valuable but too large for this pass; bundle as separate orchestrations. (S38 takes the cheap subset.)

---

## Items needing user approval before planning

- **big-wins #1 (WebGL/WebGPU renderer)** — v5 §14 deferred to v1.1+. Largest single payoff in the audit set, but spec-deferred per brief.
- **big-wins #3 (NEAT brain)** — v5 §14 deferred to v1.1+. Would invalidate the F.30 founder NN-init hardwiring (currently a known spec deviation). Multi-week effort.
- **big-wins #8 (quadtree / sweep-and-prune spatial index)** — large architectural shift; would invalidate determinism comments and force a new partition scheme.
- **big-wins #9 (deterministic replay/scrubber)** — product shape question (event stream + UI). Engagement-multiplier candidate per the audit.

Anything in this list should be its own orchestration with explicit user sign-off
before a plan exists.

---

## Recommended pass shape

Slice the 38 SHIP items into **5 PRs**, in this order:

1. **PR-1 "world.rs split + free safety"** — S1, S2, S3, S13 (CSP), S14 (delete events panel), S15 (URL cap), S16 (JSON expect), S33 (`#[inline]`), S34 (Error trait), S35 (vis tighten), S36 (heapless), S37 (deps), S38 (wasm-opt + sourcemap + toolchain pin). Everything that does NOT touch sim arithmetic or wasm-bindgen API. Big mechanical foundation; reviewer should sanity-check golden hash still passes (it must) and `pnpm build` + `pnpm typecheck` clean. **No golden regen.**

2. **PR-2 "wasm boundary cleanup"** — S17 (typed slider), S18 (stable-id `creature_at` + `creature_idx_by_id`), S19 (delete dead APIs), S20 (grid-backed `creature_at`), S21 (drop unused creatures_buffer flags), S22 (status getter hygiene + DPR cap). Touches `wasm_api.rs`, TS-side inspector, render.ts. Reviewer should manually open the app, click a creature, watch the inspector update; cycle through the 3 speed buttons. **No golden regen.**

3. **PR-3 "determinism + correctness + golden regen"** — S4 (place_hotspots), S5 (NaN decode), S6 (NaN mutate), S7 (snapshot_hash coverage), S8 (RNG hash direct), S9 (HashMap lint), S10 (chunk invariant), S11 (extract overlap/wall helpers), S12 (validate_save), S24 (cell_to_carrion scavenge — verify hash drift first), S39 (cross-feature equivalence tests). **One golden regen at end of PR-3 (both sequential + threaded files).** Reviewer should run `EVOSIM_WRITE_GOLDEN=1` and `EVOSIM_WRITE_GOLDEN_THREADED=1`, then re-run acceptance on both feature sets and confirm 4/4 pass. Manually test a hand-crafted corrupt save to confirm `StructuralError` path.

4. **PR-4 "per-tick perf followups"** — S23 (threaded NN par_chunks_mut), S25 (scratch_eat_candidates), S26 (cell_to_carrion CSR), S27 (scratch_dead/species_lost), S28 (bool vec for alive species), S29 (biggest_ever to handle_births), S30 (last_action swap), S31 (skip wall rebuild), S32 (grid cell cache). All partition-stable; reviewer should run sequential + threaded acceptance once and confirm both goldens still match the PR-3-pinned values. **No further regen.** Note in-app `Perf` widget delta if measurable.

5. **PR-5 (optional, can be folded into PR-4)** — if PR-4 trips a golden flip on S24 or S31, bundle the regen here separately and call it out.

**Things for the user to sanity-check before approving the pass plan:**

- Sequential and threaded golden hashes will change after PR-3 (S7 + S8 alone guarantee this; the rest are likely no-ops). One regen, well documented in a new `DECISIONS.md` block ("v1.1 audit — snapshot_hash coverage extended"). All downstream PRs keep the new hashes pinned.
- S14 deletes the events TypeScript module entirely. The Rust `EventLog` stays (one-liner to re-enable in v1.2); but the TS-side `EvKind`/`EvEvent`/`appendEventRow`/`formatEvent` paths go. Confirm before merge.
- S37 bumps `twox-hash` 1→2. The XXH64 algorithm is byte-stable across versions, so existing goldens should survive — but the bootstrap test must be re-run before PR-3 to confirm; if it doesn't, fold S37 into the PR-3 regen batch.
- The 4 user-approval items (WebGL/NEAT/quadtree/replay) are NOT planned by this pass. If the user wants any of them, they become their own orchestration.
- sim-balance.md (whole doc except §8) is intentionally NOT touched. If the user wants a balance pass, it's a separate orchestration (gameplay-tuning, not code-fix).
- A1 (split world.rs) is sequenced FIRST. If A1 is rejected (e.g. user prefers to keep the monolith), most SHIP items still apply but each PR-3/4 commit's diff will be larger and harder to review. Strongly recommend keeping A1.

End state after the 5 PRs: 38 cleanups landed; one golden-regen event;
zero v5 §14 violations; zero new dependencies; wasm bundle ~30–70 KB
smaller; security DoS surface closed; threaded/sequential equivalence
asserted by test; cleanest version of `world.rs` since milestone D.
