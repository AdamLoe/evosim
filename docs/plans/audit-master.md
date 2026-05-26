# Audit master plan

**Date:** 2026-05-24
**Scope:** One orchestration pass executing the 38 SHIP items from `docs/plans/audit-triage.md` as **four PRs** with **one** golden regen at PR-3, plus one **plan-only** WebGL2 renderer design doc written in parallel with PR-4. No code lands for WebGL2 in this pass.
**Sources of truth:** `docs/plans/audit-triage.md` (SHIP/DEFER/REJECT buckets — LOCKED) and `docs/archive/ORCHESTRATOR.md` (subagent pattern). This master plan does not re-triage.

---

## 1. Locked decisions recap (do not re-litigate)

- **In scope:** the 38 SHIP items in `audit-triage.md`, structured as **4 PRs** (the triage suggested 5; the optional PR-5 is merged into PR-4 contingency notes).
- **Golden regen:** exactly **one** regen event, at the end of PR-3. Both `tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt` are regen'd together. The two files are currently both `0xb76e907c6221f7f5`; if PR-3 breaks that equality that is acceptable, but must be recorded in `DECISIONS.md`.
- **Plus one plan-only doc:** `docs/plans/webgl2-renderer-design.md`. Written in parallel with PR-4 by a dedicated planner. **No WebGL2 code lands.**
- **Out of scope for this pass (do not let any subagent drag these in):**
  - NEAT brain (`big-wins #3`)
  - sparse-substrate NN
  - capability traits as NN inputs
  - F.30 hardwiring fixes (founder NN init bias)
  - mitosis / split mechanic changes
  - photosynth / upkeep balance retuning
  - any "grass mechanic"
  - implementations of `big-wins #1` (WebGL2 — plan-only doc lands, not code), `#3`, `#8` (quadtree), `#9` (replay scrubber)
  - Events subsystem RESTORE (the *delete*-side via S14/S19 is in scope; bringing the panel back is not)
- All brain / balance / gameplay / capability / NEAT / replay design belongs to the **NEXT** orchestration pass; do not mention it inside per-piece plans except by linking to this section.

---

## 2. PR table

| PR | Name | SHIP items | Golden regen | Parallelization | Effort | Depends on |
|---|---|---|---|---|---|---|
| **PR-1** | Foundation + free safety | S1 (first), then S2, S3, S13, S14, S15, S16, S33, S34, S35, S36, S37, S38 | **No** | S1 sequential blocker; remainder in any order, mostly parallel | L (S1 dominates: ~half day) | — |
| **PR-2** | Wasm boundary cleanup | S17, S18, S19, S20, S21, S22 | **No** | S19 depends on S14 (already in PR-1); rest parallel; integration test on real browser at end | M | PR-1 |
| **PR-3** | Determinism + correctness + **regen** | S4, S5, S6, S7, S8, S9, S10, S11, S12, S24, S39 | **Yes — one regen at end** | S7→S8 must land together; S11 must be referenced by both sequential and threaded NN; rest parallel | M-L | PR-1 (for `world/*.rs` paths) |
| **PR-4** | Per-tick perf followups | S23, S25, S26, S27, S28, S29, S30, S31, S32 | **No** (partition-stable) | S26→S24-touchpoints; S28 depends on S27; rest parallel | M | PR-3 (goldens pinned) |
| **Plan-only** | WebGL2 renderer design | (none — doc only) | n/a | Runs in parallel with PR-4 | S | PR-1 (knows the post-split module map) |

After PR-4 lands: re-run the §16 acceptance test on both feature sets; both goldens must still match the values pinned at the end of PR-3.

---

## 3. Per-piece-vs-inline classification

**Rule of thumb:** an item gets its own per-piece plan doc in `docs/plans/<name>.md` if it (a) touches more than ~30 LOC, (b) changes an API surface, (c) requires careful determinism reasoning, or (d) crosses module boundaries. Trivial 1–3 LOC items are inlined in §6 below with enough detail for a sonnet implementer to execute without follow-up questions.

### Per-piece-planned (12 plan docs + 1 plan-only design doc)

| S# | Plan doc filename | Reason |
|---|---|---|
| S1 | `docs/plans/audit-s1-world-split.md` | THE big one. ~2,291 LOC split across 4 files. Crosses every other PR. |
| S7 | `docs/plans/audit-s7-snapshot-hash-coverage.md` | New hash inputs across genome / species / SoA fields; canonical NaN handling; piggybacks S8. |
| S8 | `docs/plans/audit-s8-rng-hash-direct.md` | Removes serde_json from hash path; must land with S7. |
| S11 | `docs/plans/audit-s11-extract-overlap-wall.md` | Free fns used by sequential AND threaded NN; both call sites updated. |
| S12 | `docs/plans/audit-s12-validate-save.md` | New public function, 6+ validation rules, DoS-relevant. ~100 LOC. |
| S17 | `docs/plans/audit-s17-typed-set-slider.md` | Wasm API change; per-slider methods or Result<_, JsValue>. TS callers updated. |
| S18 | `docs/plans/audit-s18-stable-id-creature-at.md` | API surface change + TS inspector refactor; opens lineage-viz door. |
| S21 | `docs/plans/audit-s21-drop-unused-flag-floats.md` | Touches wasm_api.rs + render.ts + creature_stride; per-frame cache impact. |
| S23 | `docs/plans/audit-s23-threaded-nn-par-chunks-mut.md` | Threaded determinism + SoA direct-write; per-chunk borrow dance. |
| S26 | `docs/plans/audit-s26-cell-to-carrion-csr.md` | Data-layout change in vision.rs; touches every caller of cell_to_carrion. |
| S39 | `docs/plans/audit-s39-equivalence-and-save-load-tests.md` | 3 new tests; one crosses feature gates. |
| S24 | `docs/plans/audit-s24-scavenge-cell-to-carrion.md` | Determinism verify-then-decide; possible regen-batch inclusion. |
| **WebGL2** | `docs/plans/webgl2-renderer-design.md` | Plan-only design doc; no implementation. |

### Inlined (26 items — see §6 paragraphs)

S2, S3, S4, S5, S6, S9, S10, S13, S14, S15, S16, S19, S20, S22, S25, S27, S28, S29, S30, S31, S32, S33, S34, S35, S36, S37, S38.

---

## 4. Per-piece plan list (briefing each planner)

For each per-piece-planned item, this section gives the planner subagent everything it needs to produce its `docs/plans/<name>.md`. Each planner is opus.

### S1 — Split `src/world.rs` (2,291 LOC)
- **Target file:** `docs/plans/audit-s1-world-split.md`
- **Summary:** Mechanical split of the monolithic `src/world.rs` into `src/world/{mod,tick,nn,save_v1}.rs`. **Zero behavior change.** Every `cargo test` (default + `--features threads`) must pass with byte-identical goldens.
- **Depends on:** none.
- **Determinism impact:** none (must be verified by both goldens unchanged).
- **Effort:** L.
- **Briefing for planner:**
  - Read `src/world.rs` end-to-end (signatures inventory: see this master plan §3 dependency-graph notes).
  - Proposed module map:
    - `src/world/mod.rs` — `World` struct, `DevSliders`, `new`, `population`, `step`, `tick_once`, `finalize_extinctions`, `handle_births`, `run_vision_pass`, `count_carrion_overlap`, `compute_is_at_wall`. The `pub` surface used by `wasm_api.rs` stays here. Re-export `BODY_RADIUS_PER_SIZE` after S3 moves it to `constants.rs`.
    - `src/world/tick.rs` — `apply_movement_and_repulsion`, `photosynth_two_pass`, `eat_and_scavenge`, `energy_bookkeeping`, `collect_deaths`, `decay_carrion`. All as `impl World` blocks in this file (`impl crate::world::World { ... }`) or `pub(crate)` free fns that take `&mut World`.
    - `src/world/nn.rs` — `nn_forward_all_chunks`, `chunk_ranges`, `build_nn_input`, `is_valid_action`, `decode_action`, `pick_action_d`, plus the `N_CHUNKS` const. Free fns or `impl World` as appropriate.
    - `src/world/save_v1.rs` — `to_save_v1`, `from_save_v1`. Currently `from_save_v1` is `~120` LOC (`src/world.rs:1047-1167`) and references private fields (`cell_to_carrion`, `pending_extinction_check`) — those fields must become `pub(crate)` (queue for S35).
  - All tests in `src/world.rs:1379-end` move to `src/world/tests.rs` or are split per the function they cover; the simplest approach is one `#[cfg(test)] mod tests` per submodule with the existing tests rehomed by topic.
  - Update `src/lib.rs` to declare `pub mod world;` (with submodule visibility) instead of `pub mod world;` over a single file. Confirm `wasm_api.rs` only uses re-exported items.
  - Run, in order: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo clippy --all-targets --features threads -- -D warnings`, `cargo test`, `cargo test --features threads`, `cargo test --release --test acceptance`, `cargo test --release --features threads --test acceptance`. **Goldens must be unchanged.**
  - **Pattern reference:** none in-repo (this is the first split). The closest pattern is how `src/vision.rs` separates `VisionPass` struct + free fns; mimic that style.
- **Cross-piece note:** every other PR-1/3/4 piece references the *new* paths after this lands. The planner must publish a "path translation table" (`src/world.rs:X` → `src/world/<sub>.rs:Y` for each function) at the end of its plan doc so downstream planners cite the right paths.

### S7 — Extend `snapshot_hash` coverage + canonical NaN
- **Target file:** `docs/plans/audit-s7-snapshot-hash-coverage.md`
- **Summary:** Add hash inputs for `digestion_cooldown`, `species_id`, `parent_species_id`, `cumulative_upkeep`, `last_action`, `action_this_tick`, `max_size_reached`, `distance_travelled`, `birth_tick`, `carrion.sun_cell`, `carrion.id`, and species `name`, `parent_id`, `depth`, `died_tick`, `child_count`, `anchor_brain_weights`. Canonicalize NaN bits in `write_f32` (`v.to_bits() | 0x7fc0_0000` when `v.is_nan()`).
- **Depends on:** S1 (paths).
- **Determinism impact:** **regen** (the hash byte sequence changes intentionally).
- **Effort:** M.
- **Briefing for planner:**
  - Read `src/snapshot_hash.rs` (159 LOC, fully reproduced in this plan author's context for reference) and `src/creature.rs` for SoA field list, `src/carrion.rs` for `Carrion`, `src/species.rs` for `Species`.
  - Hash-input ordering rule: **append** new fields at the end of each per-entity loop. Don't reorder existing fields — keeps the diff bisectable.
  - For `anchor_brain_weights`: hash by element, not by length-prefixed slice (already in `Species.anchor_brain_weights: Vec<f32>` — confirm field name during planning).
  - For NaN canonicalization: introduce `write_f32_canonical(h, v)` helper; map all NaN bit patterns to a single canonical quiet-NaN. Update `write_f32` call sites OR replace `write_f32` entirely (planner's call — document the choice in the plan).
  - Add a unit test: `nan_canonicalization_hashes_equal` that hashes two worlds where one creature has `f32::NAN` and another has `f32::from_bits(0xffc00001)` in the same field, and asserts equal hash.
  - This piece DOES NOT regen the goldens. The regen ceremony in §9 of this master plan happens **once** after S8 also lands.
- **Pattern reference:** `src/snapshot_hash.rs` itself (the existing `hash_genome` helper); keep the same per-entity-loop style.

### S8 — Replace `serde_json::to_vec(&w.rng)` with direct LE hash of 4 xoshiro u64s
- **Target file:** `docs/plans/audit-s8-rng-hash-direct.md`
- **Summary:** At `src/snapshot_hash.rs:75-77`, replace the serde_json byte stream with `h.write_u64(s0); h.write_u64(s1); h.write_u64(s2); h.write_u64(s3);` reading directly from `SimRng` internals.
- **Depends on:** S1, S7 (lands in the same commit as S7).
- **Determinism impact:** **regen** (piggybacks the S7 regen).
- **Effort:** S.
- **Briefing for planner:**
  - Inspect `src/rng.rs` for the field layout of `SimRng` / `Xoshiro256PlusPlus`. The `rand_xoshiro` crate's `Xoshiro256PlusPlus` exposes state via `get_state` / `from_state` in recent versions; if not exposed, add a `pub(crate) fn state(&self) -> [u64; 4]` accessor on `SimRng`.
  - The hash byte order must be deterministic and documented. Use little-endian explicit (`h.write(&v.to_le_bytes())`) rather than `h.write_u64` if endian portability matters for native vs wasm32 (both are LE, so `write_u64` is fine — note this in the plan).
  - Add a unit test: `rng_hash_changes_after_step` that hashes the world twice with one `tick_once()` between, asserts the hash differs only by RNG bits (use a controlled test world).
  - Combine commits with S7 so the regen happens once.
- **Pattern reference:** the existing `h.write_u32(w.tick)` style at the top of `snapshot_hash`.

### S11 — Extract `count_carrion_overlap` and `compute_is_at_wall`
- **Target file:** `docs/plans/audit-s11-extract-overlap-wall.md`
- **Summary:** These two helpers already exist as private methods on `World` at `src/world.rs:1174` and `:1208` (post-S1: `src/world/mod.rs:???`). The sequential NN path (`build_nn_input`, `src/world/nn.rs`) calls them; the threaded NN path inlines the same logic at `src/world.rs:394-425` (post-S1: `src/world/tick.rs` or `src/world/nn.rs`). The extraction is to ensure the threaded path calls the same helper, removing the `cfg-allow dead-code` annotation.
- **Depends on:** S1.
- **Determinism impact:** **regen-verify** — refactor should be byte-identical; verify via bootstrap before pinning. If byte-identical, regen still happens once at PR-3 end alongside S7+S8 (the goldens get new bytes from S7 anyway).
- **Effort:** S.
- **Briefing for planner:**
  - Read `src/world.rs:385-446` (the threaded inline carrion-overlap and wall-clamp loop) and compare to the sequential helpers at `:1174-1220`. Confirm they compute identical values for the same input.
  - The plan must update BOTH call sites in one commit.
  - Add a unit test: `sequential_and_threaded_use_same_helpers` that constructs a tiny World, runs one tick under both feature builds, and asserts identical NN input vectors for creature 0.
- **Pattern reference:** how `src/vision.rs` factored `ray_circle_hit` as a free fn used by both sequential and parallel paths.

### S12 — `validate_save(&SaveV1) -> Result<(), LoadError>`
- **Target file:** `docs/plans/audit-s12-validate-save.md`
- **Summary:** Hardens `from_save_v1` (currently `src/world.rs:1047-1167`, post-S1: `src/world/save_v1.rs`) against malicious or corrupt saves. Adds a pure validation pass before any `creatures.push` happens.
- **Depends on:** S1.
- **Determinism impact:** none.
- **Effort:** M.
- **Briefing for planner:**
  - Read `src/save.rs` (`pub enum LoadError` at `:113`, `validate_soa_lengths` at `:207`) and the existing `from_save_v1` at `src/world.rs:1047-1167`.
  - Required checks (all return `LoadError::StructuralError(reason)` on failure):
    1. `save.sun.capacity.len() == SUN_DIM * SUN_DIM`; same for `current` and `demand`.
    2. Every `creatures.species_id[i]` is in `save.species.list` (build a `HashSet<u32>` once; do NOT iterate it — set-membership only, safe for R6).
    3. `parent_species_id[i]` similarly.
    4. For each `carrion`: `sun_cell < SUN_DIM * SUN_DIM`.
    5. For each creature `i`: `x[i]`, `y[i]`, `vx[i]`, `vy[i]`, `energy[i]`, `cumulative_upkeep[i]`, `distance_travelled[i]`, `max_size_reached[i]`, all `genome` floats, all brain weights — must be `is_finite()` AND `x/y` must be in `[0, WORLD_SIZE)`.
    6. `max_id.checked_add(1)` must not overflow (today's `max_id + 1` at line ~1126 can wrap).
    7. Hard cap: `n <= 100_000`. (Mirrors `big-wins #2` partial mitigation per triage REJECT R12.)
  - Wire as: `validate_save(&save)?` at the top of `from_save_v1` after the schema check. JS already surfaces `LoadError`.
  - Add a unit test `validate_save_rejects_crafted_payloads` covering each rule with a minimal failing payload.
  - **Do not** add a perf check; validation is one-shot at load.
- **Pattern reference:** `validate_soa_lengths` at `src/save.rs:207-241`.

### S17 — Typed `set_slider`
- **Target file:** `docs/plans/audit-s17-typed-set-slider.md`
- **Summary:** Replaces the silent-ignore `set_slider(name: &str, value: f32)` at `src/wasm_api.rs:171-183` with either per-slider methods (`set_base_sun_rate(f32)`, etc.) OR `set_slider(name, value) -> Result<(), JsValue>`.
- **Depends on:** S1.
- **Determinism impact:** none.
- **Effort:** S.
- **Briefing for planner:**
  - Inventory the current matched slider names in `set_slider` and list every caller in `web/src/main.ts` and the console-exposed `window.world.set_slider(...)` use mentioned in BUILD-REPORT Known Issue #4.
  - Recommendation: ship BOTH per-slider typed methods (`set_base_sun_rate`, `set_mutation_rate_multiplier`, etc.) AND keep the string form returning `Result<(), JsValue>` for the dev-console use case. JS callers migrate to typed methods; console retains string form.
  - Add an `Err(JsValue::from_str(format!("unknown slider: {name}")))` branch.
  - Smoke test: a wasm-bindgen-test (or a lib test if wasm-bindgen-test is not wired) asserting that an unknown name returns Err.
- **Pattern reference:** typed wasm-bindgen methods elsewhere in `src/wasm_api.rs` (e.g. the explicit `creature_at(world_x, world_y, tolerance_world)` signature).

### S18 — Stable-id `creature_at` + `creature_idx_by_id`
- **Target file:** `docs/plans/audit-s18-stable-id-creature-at.md`
- **Summary:** Change `creature_at` (`src/wasm_api.rs:226-238`) to return `Option<u64>` (the stable creature id) instead of `Option<u32>` (the SoA index). Add `creature_idx_by_id(id: u64) -> Option<u32>` so callers that still need the index for SoA-direct calls have a path. Drop the TS-side per-frame linear scan of the ids buffer in the inspector.
- **Depends on:** S1; pairs naturally with S20 (grid-backed `creature_at`).
- **Determinism impact:** none.
- **Effort:** M.
- **Briefing for planner:**
  - Read `web/src/rail/inspector.ts` (the current per-frame scan referenced by `web-wasm:1.7`).
  - Add `pub fn creature_idx_by_id(&self, id: u64) -> Option<u32>` to `WorldHandle`. Implementation: linear scan over `creatures.id` for v1 (population <2k); document that the scan is acceptable and a future hashmap-backed lookup is deferred.
  - Update the TS inspector to: on creature-click → store the returned `u64` id → each frame call `creature_idx_by_id(stored_id)` once; if `None`, the creature died (show 2-second placeholder per existing DECISIONS).
  - Coordinate with S20 (grid-backed `creature_at`): the two pieces touch the same wasm API method body. Either piece can land first, but they should be reviewed together — note in the plan.
- **Pattern reference:** how the existing `creature_inspect_json(idx)` returns a serde-json string; same idiom for any helper additions.

### S21 — Drop unused flag floats from `creatures_buffer`
- **Target file:** `docs/plans/audit-s21-drop-unused-flag-floats.md`
- **Summary:** Currently `creatures_buffer` (in `src/wasm_api.rs`) writes 13 floats per creature; `energy_frac` and `age_frac` are commented-out in `web/src/render.ts`. Drop those fields, drop any flag-bit reads that go through AoS `genomes[i]` instead of the perf-5 SoA mirror, and reduce `creature_stride()` accordingly.
- **Depends on:** S1; coordinated with S22.
- **Determinism impact:** none (snapshot_hash is independent of `creatures_buffer`).
- **Effort:** S.
- **Briefing for planner:**
  - Read `web/src/render.ts` and identify the field offsets actually consumed.
  - Read `src/wasm_api.rs` for `creatures_buffer` and `creature_stride()`.
  - Bump `creature_stride()` to the new value. Update `render.ts` field offsets in lockstep. Add a TS-side runtime assert at the top of `renderWorld` that `stride === <expected>` (defensive).
  - Confirm that the SoA mirror fields added in perf-5 (`docs/research/perf+ui-final-report.md`) are the source of truth for `eye_count` etc. — switch any AoS reads `creatures.genomes[i].eye_count` to the SoA mirror reads.
- **Pattern reference:** the perf-5 SoA mirror pattern from `docs/plans/perf-5-genome-soa.md`.

### S23 — Threaded NN writes directly into SoA via `par_chunks_mut`
- **Target file:** `docs/plans/audit-s23-threaded-nn-par-chunks-mut.md`
- **Summary:** At `src/world.rs:385-446` (post-S1: `src/world/nn.rs`), eliminate the `Vec<(f32,f32,Action)>` flat_map + drain pattern. Per-chunk, do a `par_chunks_mut(chunk_size)` over `(vx, vy, action_this_tick)` slices in tandem (use multiple `par_chunks_mut`s zipped, or a single struct-of-slices borrow pattern) and write outputs in-place.
- **Depends on:** S1, S11.
- **Determinism impact:** none (chunk partition identical to today's `par_chunks_mut(chunk_size)`).
- **Effort:** M.
- **Briefing for planner:**
  - Read the current threaded NN code at `src/world.rs:345-447`. Note the `chunk_ranges` partition contract.
  - Use the pattern from perf-2: `let mut neighbors = std::mem::take(&mut self.scratch_neighbors); /* work */ self.scratch_neighbors = neighbors;` to dodge split-borrow.
  - Multiple parallel mutable slices: `vx.par_chunks_mut(chunk).zip(vy.par_chunks_mut(chunk)).zip(action_this_tick.par_chunks_mut(chunk))` — confirm rayon supports this; if not, do a single Vec<(&mut [f32], &mut [f32], &mut [u8])> via a helper and `par_iter_mut`.
  - Reuse the helpers from S11 (`count_carrion_overlap`, `compute_is_at_wall`) — both sequential and threaded paths must use them.
  - Add a unit test `threaded_nn_in_place_writes_match_sequential` (gated `#[cfg(feature="threads")]`).
- **Pattern reference:** perf-2 `mem::take` recipe and the existing `par_chunks_mut` in `src/vision.rs` from perf-4.

### S26 — `cell_to_carrion` → CSR layout
- **Target file:** `docs/plans/audit-s26-cell-to-carrion-csr.md`
- **Summary:** Replace `Vec<Vec<u32>>` (14,400 inner vecs, ~345 KB header overhead) with `starts: Vec<u32>` + `indices: Vec<u32>`, mirroring `SpatialGrid`.
- **Depends on:** S1; touches `src/vision.rs:323-345` (`build_cell_to_carrion`) and every reader (`src/world.rs:382, 394-425`; `src/vision.rs:235`).
- **Determinism impact:** none (per-cell membership unchanged).
- **Effort:** M.
- **Briefing for planner:**
  - Read `src/grid.rs:50-66` for the CSR build pattern. Mirror it exactly: count-pass then write-pass with `cursors`.
  - The CSR type becomes a struct `CarrionIndex { starts: Vec<u32>, indices: Vec<u32> }` exported from `vision.rs`; readers iterate with `&indices[starts[cell] as usize .. starts[cell+1] as usize]`.
  - **Cross-piece coordination with S24:** S24 (in PR-3) reads `cell_to_carrion` using the OLD `Vec<Vec<u32>>` layout. S26 lands in PR-4 and changes the layout. The planner must NOT change S24's call site — that happens in PR-4's S26 implementation step. Document this in the plan: "S26 includes a follow-up edit to the S24 reader site."
- **Pattern reference:** `src/grid.rs` `SpatialGrid::rebuild`.

### S39 — Threaded=sequential + save/load-hash-equal + chunk invariant tests
- **Target file:** `docs/plans/audit-s39-equivalence-and-save-load-tests.md`
- **Summary:** Three new tests:
  1. `acceptance_threaded_matches_sequential_t10000` — run sequential AND threaded paths, hash both, assert equal. **Requires `--features threads` to actually exercise the threaded path.** Note the test only meaningfully asserts under that feature build; without it, the test should be `#[ignore]`d (planner's call to decide marker style).
  2. `save_load_hash_equal_immediately_after_load` — for `n in {0, 1, 200, 2000}`, run world, save, load, hash both before any tick — assert equal.
  3. Chunk-count invariant — debug-assert in `chunk_ranges` that the partition covers `[0, n)` exactly and the returned chunk count is `<= N_CHUNKS`; expose as a `#[test]` covering `n ∈ {0,1,7,8,9,100,1500,N_CHUNKS, N_CHUNKS+1}`.
- **Depends on:** S10 (chunk invariant), S11 (helpers), S12 (validate_save — so the load path is hard).
- **Determinism impact:** none (tests only).
- **Effort:** S.
- **Briefing for planner:**
  - Read `tests/acceptance.rs` and the existing threaded acceptance test.
  - Both goldens currently match — `0xb76e907c6221f7f5`. Use `assert_eq!(seq_hash, thr_hash, "threaded must match sequential at T=10000")`.
  - For test 2, the planner must call `WorldHandle::from_json(&world.snapshot_json())` and re-hash immediately, before any `step()`.

### S24 — Action::Scavenge uses `cell_to_carrion`
- **Target file:** `docs/plans/audit-s24-scavenge-cell-to-carrion.md`
- **Summary:** At `src/world.rs:720-730` (post-S1: `src/world/tick.rs`), replace linear scan over all carrion with a 3×3-cell sweep via `cell_to_carrion`.
- **Depends on:** S1.
- **Determinism impact:** **verify-then-decide.** Today's behavior is "first match in full carrion vec"; new behavior is "first match within 9 cells in grid order". If the bootstrap shows different hash, fold this into the PR-3 regen batch; if not, it can ship in PR-3 alongside (no regen needed).
- **Effort:** S.
- **Briefing for planner:**
  - The plan must include a **bootstrap-first** workflow: implement the change locally, run `cargo test --release --test acceptance` WITHOUT regen, capture stdout, and report the diff to the orchestrator BEFORE any further commits.
  - Iteration order must be deterministic: iterate cells `cy-1..=cy+1`, `cx-1..=cx+1` (y-outer, x-inner) and within each cell iterate the CSR `indices` slice in stored order.
  - **Layout note:** uses the OLD `Vec<Vec<u32>>` layout in PR-3. S26 in PR-4 will translate this site to the CSR layout.

### WebGL2 renderer design (plan-only)
- **Target file:** `docs/plans/webgl2-renderer-design.md`
- **Summary:** Design-only doc for a future WebGL2 instanced-rings renderer to replace the current Canvas2D `renderWorld`. **No code lands.** The doc is the deliverable; a future orchestration may schedule the implementation.
- **Depends on:** PR-1 (for awareness of the post-split module map, though no Rust code is involved).
- **Effort:** S (planner-only).
- **Briefing for planner (opus):**
  - Read `web/src/render.ts` end-to-end. Understand RING_COLORS, PX_PER_SIZE, camera transform, screenToWorld, the per-creature ring stack.
  - Required doc contents: data flow from `creatures_buffer` (post-S21 shape) to GPU instanced draw; ring-stack tessellation strategy (single quad with fragment shader vs N-quad-per-ring); zoom and DPR considerations; how to keep the click→`creature_at` integration working with WebGL coordinates; fallback when WebGL2 unavailable; perf target (60fps at 5000 creatures); progressive migration plan (parallel canvas + flag-flip).
  - Cite `big-wins #1` from `docs/audit/big-wins.md` for the original motivation.
  - **Explicitly out of scope of the doc:** WebGPU (deferred), shader-side culling (defer to v1.2 impl pass).
  - Output: a single markdown doc, ~300-600 lines. No code blocks longer than illustrative shader snippets.

---

## 5. Inlined items (briefings for sonnet implementers)

Items are grouped by PR. Each paragraph contains everything a sonnet implementer needs.

### PR-1 inlined items

**S2 — Always-on panic hook.** At `src/wasm_api.rs` (search for `console_error_panic_hook::set_once`), move the `set_once()` call out from `#[cfg(debug_assertions)]` so it runs in release builds too. This is 3 lines (1 deletion of `#[cfg(...)]` and the surrounding brace). No new dep — `console_error_panic_hook` is already in the dep graph. Test: a manual `pnpm build` + browser-load; no automated test needed (panic surfacing is hard to assert in Rust unit tests). No golden impact.

**S3 — Move `BODY_RADIUS_PER_SIZE` to `constants.rs`.** Currently at `src/world.rs:24` (post-S1: `src/world/mod.rs`). Move the `pub const BODY_RADIUS_PER_SIZE: f32 = 1.0;` line to `src/constants.rs`. Delete the re-export from `src/vision.rs` (search for `pub use crate::world::BODY_RADIUS_PER_SIZE` or similar). Update any `use crate::world::BODY_RADIUS_PER_SIZE` to `use crate::constants::BODY_RADIUS_PER_SIZE`. Test: `cargo test`. No golden impact.

**S13 — CSP header.** Edit `web/public/_headers`. Add under the `/*` block: `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'`. Note: `'unsafe-inline'` for style is required because some inline styles ship in `index.html`; document this in a comment if the planner wants to tighten later. Test: manually deploy or `pnpm build && pnpm preview` and check DevTools → Network → response headers. No golden impact.

**S14 — Delete `appendEventRow` / `formatEvent`.** Delete `web/src/rail/events.ts` (97 LOC) entirely. Remove every import of it from `web/src/rail/index.ts` and from `web/src/main.ts`. Confirm no other file imports it via `grep -rn "from.*rail/events" web/src/`. The `#rail-events` DOM section can remain (display:none per existing DECISIONS) so v1.2 can re-attach. Run `pnpm typecheck` and `pnpm build`. **Pairs with S19** (which deletes the Rust-side `recent_events_json` and `events_total_count` and the TS re-exports of `EvKind`/`EvEvent`).

**S15 — Cap URL `seed` param length.** In `web/src/main.ts` near line 215 where `seedParam` is read from `URLSearchParams`, change to `const seedParam = (params.get('seed') ?? '').slice(0, 128);`. Test: pass a `?seed=` of length 10000 in the URL bar and confirm the world boots cleanly. No automated test required. No golden impact.

**S16 — JSON `expect` → graceful.** At `src/wasm_api.rs:301`, change `.expect("save serialization is infallible")` to `.unwrap_or_else(|_| "{}".into())`. Sister change: search for other `.expect(...)` calls on `serde_json::to_string` / `serde_json::to_vec` in `wasm_api.rs` and apply the same graceful pattern (planner's call which ones; safe default is "all serializations of borrow-only data"). Test: no behavioral change in happy path; `cargo test` plus `cargo clippy`. No golden impact.

**S33 — `#[inline]` on hot tiny RNG fns + accessors.** Add `#[inline]` attributes at the following sites (post-S1 paths in parens where they move):
- `src/rng.rs:30-80` — every small RNG helper (`gen`, `gen_u32`, `next_u64`, `normal`, etc.).
- `src/creature.rs:36, 133, 136` — the accessors mentioned at those lines (`len`, push helper, etc.).
- `src/world.rs:190` (post-S1: `src/world/mod.rs`) — the `pub fn population(&self) -> u32` getter.
- `src/vision.rs:88` — `fill_one` if small enough.
- `src/snapshot_hash.rs:84` — `hash_genome` (already `#[inline]` candidate).
Run `cargo build --release` before and after; binary should shrink slightly or stay flat. No golden impact.

**S34 — `impl std::error::Error for LoadError`.** At `src/save.rs:115` (right after the `impl Display for LoadError`), add `impl std::error::Error for LoadError {}`. The default impl is sufficient since `Display` is implemented. Test: add a one-line lib test that confirms `let _: Box<dyn std::error::Error> = Box::new(LoadError::StructuralError("x".into()));` compiles. No golden impact.

**S35 — Tighten `pub mod` to `pub(crate)`.** In `src/lib.rs:5-20`, change internal `pub mod` to `pub(crate) mod` for any module NOT consumed by `wasm_api.rs`'s public API. Likely candidates: `snapshot_hash`, `profiler`, `events`, `hof`, `species`, `vision` (verify each by `grep -rn "use evosim::<mod>" tests/`). The acceptance test in `tests/acceptance.rs` uses `evosim::snapshot_hash` — keep that one `pub`. At `src/grid.rs:11-12`, change `SpatialGrid::{starts, indices}` from `pub` to `pub(crate)`. After S1 lands, also `pub(crate)` the world submodule helpers that are now cross-file but not cross-crate. Test: `cargo build` + `cargo test` + `cargo test --features threads`. No golden impact. **Sequence after S1** because the split changes which items need `pub(crate)`.

**S36 — Remove `mod heapless` confusion in `genome.rs`.** At `src/genome.rs:284-325`, the `mod heapless` block exists alongside a comment saying the in-house heapless is overkill. Replace usage with `[Option<u8>; 2]` + a count, OR with two explicit branches if only 1–2 elements are ever stored. Delete the `mod heapless` definition. Confirm identical semantics by running the existing tests in `genome.rs`. No golden impact.

**S37 — Drop `rand`; bump `twox-hash` 1→2; sync `wasm-bindgen-rayon` 1.2→1.3.** Edit `Cargo.toml`:
- Delete the `rand = { version = "0.8", ... }` line (verify no usage with `grep -rn "use rand::"` in `src/` — `rand_xoshiro` stays).
- Change `twox-hash = { version = "1.6", default-features = false }` to `twox-hash = { version = "2", default-features = false }`. The `XxHash64::with_seed(0)` API is preserved in v2; confirm with a doc check.
- Change `wasm-bindgen-rayon = "1.2"` to `"1.3"`.
**Cross-piece risk:** if the `twox-hash` 1→2 bump changes hash bytes, both goldens flip. The plan must include a bootstrap check BEFORE PR-3: run `cargo test --release --test acceptance` after the dep bump, confirm the existing golden still matches. If it does NOT match, S37 moves from PR-1 to PR-3 (folded into the regen batch). The orchestrator must run this bootstrap and report the result before kicking off PR-3 planners. No golden impact if XXH64 byte-stable.

**S38 — wasm-opt metadata + rust-toolchain.toml + .nvmrc + hidden prod sourcemap.** Three independent files:
1. `Cargo.toml`: add `[package.metadata.wasm-pack.profile.release]` with `wasm-opt = ["-O4", "--enable-bulk-memory", "--enable-mutable-globals"]`. Confirm wasm-pack respects this metadata (it does as of 0.12+).
2. `rust-toolchain.toml` at repo root: pin a stable channel (e.g. `1.78.0`) for default builds AND document the nightly required for `--features threads` wasm builds (the existing `.cargo/config.toml` flag pattern; see DECISIONS Threads block).
3. `.nvmrc` at repo root: `20` (or whatever the user runs; check `node --version`).
4. `web/vite.config.ts`: change `sourcemap: true` to `sourcemap: process.env.CI ? "hidden" : true`. "hidden" emits the map but does not reference it from the JS (so it's not shipped to users via dev-tools but is available for sentry-style upload).
Test: `wasm-pack build --release --target web --out-dir web/wasm` and `pnpm build`; verify `web/dist/assets/*.js.map` is NOT generated (or is and is not referenced from `*.js`) in CI mode. No golden impact.

### PR-2 inlined items

**S19 — Delete dead wasm-api: `recent_events_json`, `events_total_count`.** At `src/wasm_api.rs:187` and `:194`, delete both functions. Also delete the corresponding wasm-bindgen `#[wasm_bindgen]` attribute lines. In `web/src/main.ts` and any rail consumers, delete the `EvKind` / `EvEvent` TS re-exports and any remaining call sites. **Depends on S14** (which deletes the last TS reference). Run `cargo build`, `cargo test`, `pnpm typecheck`, `pnpm build`. No golden impact (functions were not in the hash path).

**S20 — `creature_at` uses `SpatialGrid`.** At `src/wasm_api.rs:226-238`, replace the linear scan with a `grid.for_each_in_radius(wx, wy, tolerance + max_creature_radius)` (need a slightly larger radius to cover edge-touching creatures since the existing scan checks distance to creature center). Iterate the grid cells and apply the same distance-to-creature filter the current code uses. Care: the grid is rebuilt at tick-step-1 from start-of-tick positions; if `creature_at` is called mid-frame, the grid is already current for THIS tick. Pairs with S18; both touch `creature_at`. Test: existing `creature_at_finds_founder` test (`src/wasm_api.rs:490`) and the additional click-on-edge case. No golden impact.

**S22 — Status-bar getter hygiene + DPR cap.** Edits all in `web/src/main.ts`:
- Cache `world.seed` at boot (one call per world lifetime) — currently called per-frame.
- Hoist `world.world_ended()` to once per RAF frame; do not call inside per-system polls.
- Throttle status DOM updates (the seed pill, day counter, pop counter) to 5 Hz via a wall-clock gate (`if (now - lastStatusUpdate > 200) { ... }`).
- For the aquarium canvas, cap `devicePixelRatio` at 2: `const dpr = Math.min(window.devicePixelRatio || 1, 2);` and use `dpr` in canvas sizing. Touches `web/src/render.ts` only if DPR is set there.
Test: manual; no golden impact.

### PR-3 inlined items

**S4 — `place_hotspots` fallback never resets `attempts`.** At `src/sun.rs:114-139`, change the inner control flow so `attempts > 2000` triggers an `else if` (or short-circuit `break`) instead of falling through. Concretely: today the code does `if <conflict> { regenerate; attempts += 1; } if attempts > 2000 { fallback }` — the fallback path is reached but the outer loop's `attempts` variable is not bounded if the fallback also conflicts. The fix is `else if attempts > 2000 { fallback; break; }`. Add a unit test `place_hotspots_terminates_under_dense_obstruction` constructing a world where hotspots have no valid placement (mock or extreme size) and asserting `SunMap::new` returns within bounded time. **Determinism:** today's config never hits the bug, so bootstrap-verify: if hash unchanged, the change rides the S7+S8 regen anyway; if hash changes, document the new value.

**S5 — `decode_action` NaN propagation.** At `src/world.rs:1322-1339` (post-S1: `src/world/nn.rs`), before the sort-by-logit, check `if logits.iter().any(|v| !v.is_finite()) { return Action::Rest; }` AND in the caller (`pick_action_d`) zero the resulting velocity if NaN was detected (track a `let had_nan = bool` flag). Add a `debug_assert!(logits.iter().all(|v| v.is_finite()), "NaN in NN logits at tick {}", w.tick)` in the tick hot path. Test: a unit test feeding NaN logits and asserting `Rest` output + zero velocity. **Determinism:** no NaN in current goldens, so byte-identical; rides the S7+S8 regen.

**S6 — `mutate_f32` / `mutate_u32` NaN defense.** At `src/genome.rs:262-275`, wrap each mutator in `let next = ...; if next.is_finite() { *value = next.clamp(min, max); } else { /* no-op */ }`. For `mutate_u32`, no NaN is possible but range-check defensively. Add a unit test injecting an RNG that returns NaN (or `f32::INFINITY`) and asserting the value is unchanged. **Determinism:** `rng.normal()` cannot produce NaN today; identical bytes expected; rides S7+S8 regen.

**S9 — HashMap lint rule.** Add a `clippy.toml` at repo root with `disallowed-methods = ["std::collections::HashMap::iter", "std::collections::HashMap::iter_mut", "std::collections::HashMap::into_iter", "std::collections::HashSet::iter", "std::collections::HashSet::into_iter"]`. Apply via crate-level `#![cfg_attr(test, allow(clippy::disallowed_methods))]` if needed for tests. Alternative: a CI grep gate (`! grep -rE 'HashMap.*\\.(into_)?iter|HashSet.*\\.(into_)?iter' src/world*.rs src/snapshot_hash.rs src/species.rs src/creature.rs`). Implementer's call between clippy.toml vs grep; prefer clippy.toml. Test: deliberately add a forbidden call and confirm `cargo clippy -- -D warnings` fails. No golden impact.

**S10 — Chunk-partition invariant test.** At `src/world.rs:1252` (post-S1: `src/world/nn.rs`) where `chunk_ranges` lives, add a `debug_assert!` post-partition that the returned ranges cover `[0, n)` exactly with no overlaps and that the count is `<= N_CHUNKS`. Add a unit test exercising `n ∈ {0, 1, 7, 8, 9, 100, 1500}`. The existing `chunk_ranges_partition` and `chunk_ranges_small_population` tests at `src/world.rs:1613, 1634` provide most of this; the new test asserts equivalence with `vision.rs:74`'s partition for the same `n` set. No golden impact.

### PR-4 inlined items

**S25 — `scratch_eat_candidates` pool.** At `src/world.rs:676` (post-S1: `src/world/tick.rs`), the Eat-handling loop allocates a fresh `Vec` per-Eat-per-tick. Prefer the inline-in-callback option: rewrite the body so the candidates are processed inside `grid.for_each_in_radius`'s callback without a buffer (one creature handled per callback invocation). If that's not feasible, promote `scratch_eat_candidates: Vec<usize>` to a `World` field (matching perf-2's pattern at `src/world.rs:106-114`) and `mem::take` to dodge borrow. Test: `scratch_grows_with_population` style test confirming the pool reuses memory. Reference C13 in `correctness-bugs.md`. No golden impact.

**S27 — `scratch_dead` and `scratch_species_lost` pools.** At `src/world.rs:831-834` (post-S1: `src/world/tick.rs` in `collect_deaths`), promote both `Vec`s to `World` fields and fill/clear per call. Additionally collapse `species_lost` into a direct `pending_extinction_check` write: instead of `let mut species_lost = vec![]; ... species_lost.push(sid);` then later draining into `pending_extinction_check`, push directly. Test: extend existing extinction tests. No golden impact.

**S28 — Dense-bool alive-species.** At `src/world.rs:1008` (post-S1: `src/world/mod.rs` in `finalize_extinctions`), replace the `HashSet<u32>` of alive species ids with `Vec<bool>` indexed by species_id; resize to `max_species_id + 1` and clear per call. Confirm HashSet was only used for `contains` (it was per the triage). **Depends on S27** (both touch `finalize_extinctions` adjacent code). No golden impact.

**S29 — Gate `biggest_ever` to `handle_births`.** At `src/world.rs:805-821` (post-S1: `src/world/tick.rs` in `energy_bookkeeping`), remove the per-tick O(N) `biggest_ever` scan. Move the equivalent check into `handle_births` (post-S1: `src/world/mod.rs`): after each newborn is pushed, compare its size against the current `biggest_ever` and update if larger. Size only changes at birth, so this is equivalent. Test: the existing `e25_biggest_ever_tracks_max_size` test at `src/world.rs:1894` must still pass. No golden impact (HoF state is observation-only and never read mid-sim).

**S30 — `last_action` swap.** At `src/world.rs:294-296` (post-S1: end of `step` in `src/world/mod.rs` or `src/world/tick.rs`), replace the per-creature copy loop with `std::mem::swap(&mut self.creatures.last_action, &mut self.creatures.action_this_tick);`. This works because `action_this_tick` is overwritten by NN forward at the next tick's step 3 before any read. Add a unit test asserting that after `swap`, `last_action` contains the previous tick's action for every creature. No golden impact.

**S31 — Skip wall-rebuild iff no wall fired.** At `src/world.rs:591` (post-S1: `src/world/tick.rs` end of `apply_movement_and_repulsion`), wrap the final `grid.rebuild` in `if any_wall { ... }`. Track `let mut any_wall = false;` in the clamp loop; set true on any wall-clamp event. Confirm: mid-world ticks without wall hits skip the rebuild; the grid stays valid because positions only changed for clamped creatures, and if `any_wall == false` no clamp happened. Test: a unit test with a creature far from the wall confirming rebuild is skipped (mock or assert via call count if a counter is added). No golden impact.

**S32 — Cache `cell_of` in `grid.rebuild`.** At `src/grid.rs:50-66`, add `cells: Vec<u32>` to `SpatialGrid`. In `rebuild`, compute `cell_of(x, y)` once per creature and store in `cells[i]`. Downstream consumers that currently re-call `cell_of` (search for sites) read from `grid.cells[i]` instead. Test: existing grid tests + add `cell_of_cache_matches_recompute`. No golden impact.

---

## 6. Dependency graph (within each PR)

**PR-1:**
```
S1 (world.rs split)  ──┬─→ S3 (BODY_RADIUS_PER_SIZE move — references new src/world/ path)
                       ├─→ S35 (pub(crate) tightening — needs post-split surface)
                       └─→ (S33 #[inline] on post-split functions)
S37 (twox-hash 1→2) → BOOTSTRAP-CHECK before PR-3
   ↓ (if hash drifts)
   move S37 to PR-3 regen batch
S38, S2, S13, S14, S15, S16, S34, S36 — independent of S1
S19 depends on S14
```

**PR-3:**
```
S7 ──→ S8   (S8 piggybacks the regen; same commit)
S11 ──→ S23 (S23 in PR-4 uses extracted helpers; both NN paths reference them)
S10 — independent
S12 — independent (validate_save standalone)
S4, S5, S6 — independent (defensive; ride regen)
S9 — independent (lint only)
S24 — bootstrap-verify first; either rides regen or stands alone
S39 — depends on S10, S11, S12 (uses their guarantees)
```

**PR-4:**
```
S27 ──→ S28 (S28 sits in the same code area; refactor pairs cleanly)
S26 ──→ touches S24's call site in tick.rs (re-rewrites the scavenge reader to CSR)
S23 ──→ depends on S11 already landed in PR-3 (helpers in place)
S25, S29, S30, S31, S32 — independent
```

**Cross-PR (already noted in PR table):**
- S1 blocks every PR-1/3/4 piece that touches `world.rs` paths.
- S35 best done after S1 (visibility surface depends on the split).

---

## 7. Cross-piece-conflict watchlist

The cross-review subagent (one opus, end of each PR) must check **at least** these items:

(a) **World.rs split path-translation:** every per-piece plan that cites a `src/world.rs:NNN` line must be re-checked against the post-S1 paths (`src/world/{mod,tick,nn,save_v1}.rs:???`). The S1 planner must publish a path translation table; reviewer confirms every downstream plan uses the new paths.

(b) **S7 + S8 commit-pairing:** both must land in **one commit**. The cross-reviewer rejects any sequence where S7 lands first, regen happens, and S8 lands second — that's two regens. Enforce one regen ceremony.

(c) **S11 helpers referenced by both NN paths:** the cross-reviewer must `grep -nE 'count_carrion_overlap|compute_is_at_wall' src/world*.rs src/world/` and confirm BOTH the sequential `build_nn_input` site (post-S1: `src/world/nn.rs`) and the threaded inline site (post-S1: `src/world/nn.rs` or `src/world/tick.rs`) call the extracted helpers, not inlined logic. If S11 lands but the threaded path still inlines, S11 has regressed.

(d) **S24 hash-drift verification:** before PR-3 freezes, the implementer must run `cargo test --release --test acceptance` with S24 applied **but without regen** and report the diff to the orchestrator. If hash matches old `0xb76e907c6221f7f5`, S24 rides PR-3 standalone. If it differs, S24 is folded into the regen batch and **removed from PR-4 planning**. The plan for S24 documents both branches.

(e) **Dual-golden equality is currently `0xb76e907c6221f7f5`.** PR-3's S7 + S8 will change both. If they change to DIFFERENT values for sequential vs threaded (because the new hash inputs interact with the threaded path's RNG ordering differently), that is acceptable — but the cross-reviewer must add a DECISIONS line:
  `audit v1.1 — sequential and threaded snapshot_hash diverge post-S7/S8; sequential = 0xXXX, threaded = 0xYYY; both deterministic against themselves.`

(f) **S37 `twox-hash` 1→2 bootstrap:** before PR-3 begins, the orchestrator must run the dep bump locally and confirm byte-identity. If broken, S37 moves to PR-3's regen batch (and PR-1 ships without it). The planner brief for S37 must include the explicit bootstrap-check step at the top.

(g) **S21 stride math:** changing `creature_stride()` while `render.ts` reads stale offsets will silently corrupt the picture. The cross-reviewer must verify the TS-side runtime assert (`stride === <new value>`) lands in the same commit as the Rust change.

(h) **S26 CSR vs S24 reader:** S24 lands in PR-3 using the OLD `Vec<Vec<u32>>` layout. S26 in PR-4 changes the layout AND must update the S24-introduced reader site in the same commit. Cross-reviewer confirms.

---

## 8. Golden-regen ceremony (end of PR-3)

Run exactly once after every other PR-3 commit has landed and `cargo test` (default and `--features threads`) is green except for the goldens.

**Note (post-review update):** S39 test (a) — the threaded-equals-sequential equivalence assertion — lands as the FINAL PR-3 commit, AFTER this regen ceremony, because it asserts against the newly pinned golden values. Tests (b) and (c) from S39 land before the regen (their assertions are golden-independent).

```bash
# 1. Regen sequential
cd /home/adamg/evosim
EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance acceptance_t10000

# 2. Regen threaded
EVOSIM_WRITE_GOLDEN_THREADED=1 cargo test --release --features threads --test acceptance acceptance_t10000_threaded

# 3. Re-run both, confirm pass (no env vars; just verify)
cargo test --release --test acceptance
cargo test --release --features threads --test acceptance

# 4. Confirm all 4 tests pass (3 default + 1 threaded as of post-perf-4)
```

Read the new hashes from `tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt`. Append a `DECISIONS.md` line under a new `## Audit v1.1 (2026-05)` heading:

```
v1.1 audit — snapshot_hash coverage extended (S7), RNG hash format changed to direct LE u64×4 (S8)[, scavenge cell-to-carrion reorder (S24 if drift)]; new sequential hash 0xXXXXXXXXXXXXXXXX, new threaded hash 0xYYYYYYYYYYYYYYYY.
```

If sequential == threaded post-regen, note that explicitly. If they diverge, note that explicitly (see watchlist item e).

**PR-4 sanity:** at the end of PR-4, re-run the same 4 acceptance tests. **Both goldens must still match the values pinned at the end of PR-3.** If they don't, PR-4 has introduced a non-partition-stable change; bisect and revert.

---

## 9. Subagent assignment table

| S# / piece | Planner | Reviewer | Implementer | Code-reviewer |
|---|---|---|---|---|
| S1 (world split) | opus | opus | sonnet (retry opus if weak) | opus |
| S2 | — (inline) | — | sonnet | (rolled into PR-1 code-review) |
| S3 | — (inline) | — | sonnet | (rolled in) |
| S4 | — (inline) | — | sonnet | (rolled into PR-3 code-review) |
| S5 | — (inline) | — | sonnet | (rolled in) |
| S6 | — (inline) | — | sonnet | (rolled in) |
| S7 | opus | opus | sonnet | opus |
| S8 | opus | opus | sonnet (same commit as S7) | opus (combined w/ S7) |
| S9 | — (inline) | — | sonnet | (rolled in) |
| S10 | — (inline) | — | sonnet | (rolled in) |
| S11 | opus | opus | sonnet | opus |
| S12 | opus | opus | sonnet (retry opus) | opus |
| S13 | — (inline) | — | sonnet | (rolled in) |
| S14 | — (inline) | — | sonnet | (rolled in) |
| S15 | — (inline) | — | sonnet | (rolled in) |
| S16 | — (inline) | — | sonnet | (rolled in) |
| S17 | opus | opus | sonnet | opus |
| S18 | opus | opus | sonnet | opus |
| S19 | — (inline) | — | sonnet | (rolled in) |
| S20 | — (inline) | — | sonnet | (rolled in) |
| S21 | opus | opus | sonnet | opus |
| S22 | — (inline) | — | sonnet | (rolled in) |
| S23 | opus | opus | sonnet (retry opus) | opus |
| S24 | opus | opus | sonnet | opus |
| S25 | — (inline) | — | sonnet | (rolled into PR-4 code-review) |
| S26 | opus | opus | sonnet | opus |
| S27 | — (inline) | — | sonnet | (rolled in) |
| S28 | — (inline) | — | sonnet | (rolled in) |
| S29 | — (inline) | — | sonnet | (rolled in) |
| S30 | — (inline) | — | sonnet | (rolled in) |
| S31 | — (inline) | — | sonnet | (rolled in) |
| S32 | — (inline) | — | sonnet | (rolled in) |
| S33 | — (inline) | — | sonnet | (rolled in) |
| S34 | — (inline) | — | sonnet | (rolled in) |
| S35 | — (inline) | — | sonnet | (rolled in) |
| S36 | — (inline) | — | sonnet | (rolled in) |
| S37 | — (inline; bootstrap-checked) | — | sonnet | (rolled in) |
| S38 | — (inline) | — | sonnet | (rolled in) |
| S39 | opus | opus | sonnet | opus |
| WebGL2 design doc | opus | opus | — (no implementer) | — (no code-review) |

Per-PR cross-review: one opus pass at end of each PR (PR-1, PR-2, PR-3, PR-4) running the watchlist in §7.

---

## 10. Acceptance gates (must hold after every commit)

- `cargo fmt -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo clippy --all-targets --features threads -- -D warnings`
- `cargo test`
- `cargo test --features threads`
- `cargo test --release --test acceptance` (3 tests, all pass; goldens match)
- `cargo test --release --features threads --test acceptance` (1 test, passes; threaded golden matches)
- `pnpm typecheck` (clean)
- `pnpm build` (clean — pre-existing `evosim.js` static+dynamic-import warning is acceptable)
- Conventional commit messages (`feat:`, `fix:`, `refactor:`, `perf:`, `chore:`, `docs:`, `test:`).

After every PR: dev-server smoke check per `docs/dev-server-prompt.md` (port 47821).

---

## 11. Out-of-scope reminder

Brain (NEAT, sparse-substrate, capability-trait NN inputs), balance (sun rate, upkeep, mitosis thresholds, founder NN hardwiring per BUILD-REPORT F.30 known issue, FOUNDER_SPLIT_JITTER, SPECIES_THRESHOLD), gameplay (grass mechanic, signaling, sexual repro), bigger architectural bets (quadtree, deterministic replay, WebGL2/WebGPU implementation, batched frame_snapshot, NN weight transpose with save schema bump), and the Events subsystem RESTORE are **explicitly deferred to the NEXT orchestration pass.** Do not let any per-piece planner expand scope into these areas. The WebGL2 *design doc* is the only future-bet artifact landing this pass.