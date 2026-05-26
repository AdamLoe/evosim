# S1 — Split `src/world.rs` (2,291 LOC) into `src/world/{mod,tick,nn,save_v1}.rs`

**Status:** plan
**Author:** opus (planner)
**Audit master plan:** `docs/plans/audit-master.md` §4 → S1
**Audit triage:** `docs/plans/audit-triage.md` S1
**Depends on:** none
**Blocks (PR-1, PR-3, PR-4):** S3, S4, S5, S6, S7, S9, S10, S11, S12, S17, S18, S20, S21, S23, S24, S25, S26, S27, S28, S29, S30, S31, S33, S35, S39

---

## 1. Summary

Mechanical split of the monolithic `src/world.rs` (2,291 LOC) into a four-file submodule layout: `src/world/mod.rs`, `src/world/tick.rs`, `src/world/nn.rs`, `src/world/save_v1.rs`. **Zero behavior change.** Every existing test must pass under both default and `--features threads` builds. Both goldens (`tests/golden_snapshot_t10000.txt` and `tests/golden_snapshot_t10000_threaded.txt`, both currently `0xb76e907c6221f7f5`) must remain byte-identical. The split exists to unblock the other 25+ SHIP items that touch `world.rs` paths; its only deliverable beyond the code rearrangement is the **path-translation table** in §6, which every downstream planner cites to rewrite `src/world.rs:NNN` references into `src/world/<sub>.rs:???` references.

---

## 2. File layout

Four files plus one test module per submodule (no shared `tests.rs`; tests follow the functions they cover so each submodule remains self-contained).

### 2.1 `src/world/mod.rs` — entry, types, lifecycle, simple getters, ownership of free helpers used by both NN paths

| Item | Kind | Current location (`src/world.rs`) | Visibility (post-split) | Notes |
|---|---|---|---|---|
| Module-level doc comment | doc | 1-6 | n/a | Copy verbatim to `src/world/mod.rs` head |
| `BODY_RADIUS_PER_SIZE` const | const | 22-24 (doc + decl) | `pub` (unchanged) | Stays in `mod.rs` for this S1; S3 moves it to `constants.rs` in a separate commit. **Do not move in S1.** Re-exported automatically via `pub` in `mod.rs`. |
| `DevSliders` struct | type | 26-33 | `pub` (unchanged) | Referenced by `src/save.rs:14`. |
| `impl Default for DevSliders` | impl | 35-45 | n/a (Default) | Stays with the type. |
| `World` struct (all fields) | type | 47-115 | `pub` (unchanged); fields adjusted (see §3) | The struct definition stays in `mod.rs`. Field-by-field visibility changes are listed in §3. |
| `impl World { new }` | fn | 117-188 | `pub` (unchanged) | Constructor; stays in `mod.rs`. |
| `impl World { population }` | fn | 190-192 | `pub` (unchanged) | Tiny getter. |
| `impl World { step }` | fn | 194-340 | `pub` (unchanged) | Orchestrator that calls `tick.rs` and `nn.rs` methods. Stays in `mod.rs` because it is the canonical entry point. All calls are `self.<method>()`; visibility of callees handled by the standard rule that `impl T` blocks in any module file can call private methods on `T` if those methods are declared `pub(crate)` (see §3). |
| `impl World { finalize_extinctions }` | fn | 1007-1032 | `pub` (unchanged) | Called by `tick_once` and externally by `wasm_api.rs` step pattern; keep in `mod.rs` since it's a lifecycle method paired with `step`. |
| `impl World { tick_once }` | fn | 1034-1038 | `pub` (unchanged) | Convenience wrapper around `step + finalize_extinctions`; stays in `mod.rs`. |
| `impl World { handle_births }` | fn | 926-1005 | `pub(crate)` (unchanged) | Already `pub(crate)`. Conceptually a "tick step" but it is also called directly by the `eye_trig_recomputed_on_birth` test. Stays in `mod.rs` to keep birth-related state (anchors, registry, RNG) co-located with the constructor. |
| `impl World { run_vision_pass }` | fn | 1220-1237 | `pub(crate)` (was private `fn`) | Becomes `pub(crate)` to be callable from `step` if `step` ever moves; safe to upgrade and immediately needed because the `impl World` block in `mod.rs` and any future `impl` blocks in `tick.rs` both call it. |
| `impl World { count_carrion_overlap }` | fn | 1170-1202 | `pub(crate)` (was private `fn`) | Called by NN-sequential path (in `nn.rs`); must be `pub(crate)` for cross-file access. Keep the existing `#[cfg_attr(feature = "threads", allow(dead_code))]` attribute. (S11 will later make this `pub(crate) fn` a free function shared by both NN paths.) |
| `impl World { compute_is_at_wall }` | fn | 1204-1218 | `pub(crate)` (was private `fn`) | Same as above. |
| `#[cfg(test)] mod tests` | mod | 1374-2291 (split per submodule — see §7) | private | Tests for items that LIVE in `mod.rs` move here. Tests for items in `tick.rs`/`nn.rs`/`save_v1.rs` move to those submodules' `#[cfg(test)] mod tests` blocks (see §7). |
| Submodule declarations | decl | (new) | `pub(crate) mod tick; pub(crate) mod nn; pub(crate) mod save_v1;` | Added at the top of `mod.rs`. |
| Re-exports from submodules | decl | (new) | none required | All callers (`wasm_api.rs`, `save.rs`, `snapshot_hash.rs`, `tests/acceptance.rs`) use `crate::world::World`, `crate::world::DevSliders`, `crate::world::BODY_RADIUS_PER_SIZE` — the existing `pub` items on `World`/`DevSliders`/the const remain reachable via `crate::world::*` because they live in `mod.rs`. Functions defined inside `impl World { ... }` blocks in other submodule files are reachable as `world.method()` from outside the crate without re-export, because they are `pub` (or `pub(crate)` for crate-internal use). |

### 2.2 `src/world/tick.rs` — per-tick step bodies (steps 4, 5, 6, 8, 9, 10)

| Item | Kind | Current location | Visibility | Notes |
|---|---|---|---|---|
| Module-level doc comment | doc | (new) | n/a | "Per-tick step bodies that mutate `World`. All as `impl World` blocks; private to the `crate::world` parent." |
| `impl World { apply_movement_and_repulsion }` | fn | 449-592 | `pub(crate)` (was private `fn`) | Called by `step` (in `mod.rs`) and by test `e25b_first_to_move_fires_on_movement_step` in `mod.rs`/tick tests. |
| `impl World { photosynth_two_pass }` | fn | 594-636 | `pub(crate)` (was private) | Called by `step` and the `energy_conservation_in_photosynth_pass` test. |
| `impl World { eat_and_scavenge }` | fn | 638-760 | `pub(crate)` (was private) | Called by `step`. |
| `impl World { energy_bookkeeping }` | fn | 762-829 | `pub(crate)` (was private) | Called by `step` and the `e25_biggest_ever_tracks_max_size` test. |
| `impl World { collect_deaths }` | fn | 831-907 | `pub(crate)` (was private) | Called by `step` and the `e25_weirdest_requires_500_ticks` / `e25_last_survivor_captures_final_death` tests. |
| `impl World { decay_carrion }` | fn | 909-924 | `pub(crate)` (was private) | Called by `step` and the `carrion_returns_to_sun_on_decay` test. |
| `#[cfg(test)] mod tests` | mod | (new) | private | Holds tests that exercise the tick-step bodies above. See §7 for the list. |

### 2.3 `src/world/nn.rs` — NN forward pass, action decode, chunk partition

| Item | Kind | Current location | Visibility | Notes |
|---|---|---|---|---|
| Module-level doc comment | doc | (new) | n/a | "NN forward pass and action-decode helpers. Both sequential and threaded paths live here." |
| `impl World { nn_forward_all_chunks }` | fn | 342-447 | `pub(crate)` (was private) | Called by `step`. Contains both the `#[cfg(not(feature = "threads"))]` and `#[cfg(feature = "threads")]` arms verbatim. |
| `chunk_ranges` | free fn | 1240-1261 (doc + body) | `pub(crate)` (was private `fn`) | Called by `step` (now in `mod.rs`) and by tests in this submodule. The full doc comment (1240-1251) moves with it. |
| `build_nn_input` | free fn | 1263-1308 | `pub(crate)` (was private) | Called only by `pick_action_d` and by tests. |
| `is_valid_action` | free fn | 1310-1318 | `pub(crate)` (was private) | Called only by `decode_action`. |
| `decode_action` | free fn | 1320-1339 | `pub(crate)` (was private) | Called only by `pick_action_d` and by tests. |
| `pick_action_d` | free fn | 1341-1372 (incl. `#[allow]`) | `pub(crate)` (was private) | Called by `nn_forward_all_chunks` (both arms). Preserve the `#[allow(clippy::too_many_arguments)]` attribute. |
| `#[cfg(test)] mod tests` | mod | (new) | private | NN input layout, action decode, chunk partition tests. See §7. |

### 2.4 `src/world/save_v1.rs` — save/load round-trip

| Item | Kind | Current location | Visibility | Notes |
|---|---|---|---|---|
| Module-level doc comment | doc | (new) | n/a | "SaveV1 ↔ World conversion. Hardening lands in S12; S1 only moves the existing bodies." |
| `impl World { to_save_v1 }` | fn | 1040-1043 | `pub` (unchanged) | Tiny wrapper around `crate::save::SaveV1::from_world(self)`. |
| `impl World { from_save_v1 }` | fn | 1045-1168 | `pub` (unchanged) | Touches `cell_to_carrion` and `pending_extinction_check` directly (lines 1154-1155). Those fields must become `pub(crate)` on `World` (see §3). |
| `#[cfg(test)] mod tests` | mod | (new) | private | No existing test in `world.rs` exercises `to_save_v1` / `from_save_v1` directly — those round-trip tests live in `src/save.rs` and `tests/acceptance.rs`. So **this submodule's tests block is empty (omit the mod block entirely if empty)** unless a placeholder is needed; if needed for symmetry, ship `#[cfg(test)] mod tests { /* round-trip tests live in src/save.rs */ }`. Prefer **omit** to keep the diff minimal. |

---

## 3. Visibility plan

The split forces several previously-private items to become `pub(crate)`. This section is exhaustive — implementer should diff against this list.

### 3.1 `World` struct fields (in `mod.rs`)

| Field | Before | After | Reason |
|---|---|---|---|
| `tick, seed, rng, sun, grid, creatures, carrion, species, events, events_enabled, sliders, next_creature_id, peak_population, peak_species_count, world_ended, live_species_count, first_move_fired, first_eat_fired, population_milestones_fired, biggest_ever, last_survivor, weirdest, weirdest_distance, longest_lived, longest_lived_age, first_mover_snapshot, founder_genome_anchor, founder_brain_anchor, vision` | `pub` | `pub` (unchanged) | All already `pub`; consumers in `save.rs`, `snapshot_hash.rs`, `wasm_api.rs`, tests rely on this. |
| `cell_to_carrion` | private | **`pub(crate)`** | `from_save_v1` in `save_v1.rs` writes `cell_to_carrion: Vec::new()` (line 1154); also `run_vision_pass` in `mod.rs` and the threaded NN path in `nn.rs` read it. Even with `impl World` blocks in submodules, struct *fields* require visibility tightening only for direct field access from outside the defining module. `impl World { ... }` in `tick.rs` / `nn.rs` / `save_v1.rs` IS outside the `mod.rs` file that defines `World`, but Rust's privacy rule treats the entire `crate::world` module tree as "outside" relative to the file where the struct is declared. **Therefore: upgrade to `pub(crate)`** so submodule `impl` blocks can read/write the field directly. |
| `pending_extinction_check` | private | **`pub(crate)`** | Same reason: `from_save_v1` writes `pending_extinction_check: Vec::new()` (line 1155); `collect_deaths` (now in `tick.rs`) and `finalize_extinctions` (now in `mod.rs`) read/write it. |
| `profile` | `pub` | `pub` (unchanged) | |
| `scratch_fx, scratch_fy, scratch_neighbors, scratch_damage, scratch_gain, scratch_cooldown_set, scratch_attempted_eat, scratch_attempted_scavenge, scratch_got_a_bite` | private | **`pub(crate)`** | All accessed from `tick.rs` (`apply_movement_and_repulsion`, `eat_and_scavenge`) and from one test (`scratch_fx_fy_zeroed_at_tick_start`, `scratch_grows_with_population`) that currently lives in `world.rs`. After split, tests move into either `mod.rs` tests or `tick.rs` tests; in either case the fields must be `pub(crate)` because the test module is `#[cfg(test)] mod tests` inside a submodule that is itself outside the `mod.rs` defining the struct. Rust's privacy will reject `w.scratch_fx[i] = 999.0;` if the field remains private. |

### 3.2 `World` methods (in `mod.rs` `impl World` block, but defined across submodule files)

Rust allows multiple `impl T` blocks across files **only when each block lives inside a module that has access to `T`'s name**. The submodules (`tick`, `nn`, `save_v1`) all `use crate::world::World;` (or write `impl super::World`). Methods declared inside an `impl World` block in `src/world/tick.rs` are addressable as `self.foo()` from `step()` in `mod.rs` only if they are `pub(crate)` (or wider). Therefore:

| Method | Before | After | Reason |
|---|---|---|---|
| `new, population, step, finalize_extinctions, tick_once, to_save_v1, from_save_v1` | `pub` | `pub` (unchanged) | External callers (wasm_api, tests) require these. |
| `handle_births` | `pub(crate)` | `pub(crate)` (unchanged) | Already `pub(crate)`. Called by `step` and by test `eye_trig_recomputed_on_birth`. |
| `apply_movement_and_repulsion` | private `fn` | **`pub(crate) fn`** | Called by `step` in `mod.rs` (cross-file from `tick.rs`); also by test `e25b_first_to_move_fires_on_movement_step`. |
| `photosynth_two_pass` | private | **`pub(crate)`** | Called by `step`; also by test `energy_conservation_in_photosynth_pass`. |
| `eat_and_scavenge` | private | **`pub(crate)`** | Called by `step`. |
| `energy_bookkeeping` | private | **`pub(crate)`** | Called by `step`; also by test `e25_biggest_ever_tracks_max_size`. |
| `collect_deaths` | private | **`pub(crate)`** | Called by `step`; also by tests `e25_weirdest_requires_500_ticks`, `e25_last_survivor_captures_final_death`. |
| `decay_carrion` | private | **`pub(crate)`** | Called by `step`; also by test `carrion_returns_to_sun_on_decay`. |
| `nn_forward_all_chunks` | private | **`pub(crate)`** | Called by `step` (cross-file from `nn.rs`). |
| `run_vision_pass` | private | **`pub(crate)`** | Called by `step` (`mod.rs` → `mod.rs`, technically same file, but uplift for future-proofing against later `step` movement; cost is one keyword and helps S11/S23). |
| `count_carrion_overlap` | private | **`pub(crate)`** | Called by `pick_action_d` site in `nn.rs` (cross-file). Keep `#[cfg_attr(feature = "threads", allow(dead_code))]`. |
| `compute_is_at_wall` | private | **`pub(crate)`** | Same as above. |

### 3.3 Free functions in `nn.rs`

| Function | Before | After | Reason |
|---|---|---|---|
| `chunk_ranges` | private | **`pub(crate)`** | Called by `step` (in `mod.rs`, cross-file from `nn.rs`). |
| `build_nn_input` | private | **`pub(crate)`** | Called by `pick_action_d` (same file) and by 3 tests; tests are in the same submodule so visibility could remain private, but uplift to `pub(crate)` for consistency with the rest of the `nn.rs` surface and because S11 will later use it cross-file. **Decision: `pub(crate)`.** |
| `is_valid_action` | private | `pub(crate)` | Same reasoning; cheap consistency. |
| `decode_action` | private | **`pub(crate)`** | Called by `pick_action_d` (same file) and by 5 tests in `nn.rs`'s test module. Visibility could remain private — but S5 (PR-3) will modify it and may want to add a `debug_assert!` callable from `tick.rs`. **Decision: `pub(crate)`.** |
| `pick_action_d` | private | **`pub(crate)`** | Called by `nn_forward_all_chunks` (same file). Could remain private. **Decision: `pub(crate)`** for consistency. |

### 3.4 Consumer-side check (must remain satisfied)

| Consumer | Imports | Post-split status |
|---|---|---|
| `src/wasm_api.rs:6` | `use crate::world::{World, BODY_RADIUS_PER_SIZE};` | OK — both `pub` in `mod.rs`. |
| `src/wasm_api.rs:589` | `crate::world::World::from_save_v1(save)` | OK — `from_save_v1` is `pub` (defined in `save_v1.rs` inside `impl World`, addressable through `World`). |
| `src/save.rs:14, :262` | `use crate::world::{DevSliders, World};` | OK. |
| `src/snapshot_hash.rs:16, :127` | `use crate::world::World;` | OK. |
| `src/vision.rs:340` | `pub const BODY_RADIUS_PER_SIZE: f32 = crate::world::BODY_RADIUS_PER_SIZE;` | OK — still `pub` in `mod.rs`. |
| `src/creature.rs:416` (test) | `use crate::world::World;` | OK. |
| `tests/acceptance.rs:10` | `use evosim::world::World;` | OK — `pub mod world;` in `lib.rs` keeps `World` exported. |

No consumer needs a re-export from a submodule; everything they touch is on `World`, `DevSliders`, or `BODY_RADIUS_PER_SIZE`, all of which live in `mod.rs`.

---

## 4. Module declaration

### 4.1 `src/lib.rs`

**No change to `lib.rs`.** The existing line `pub mod world;` already declares the module. When `src/world.rs` is replaced by `src/world/mod.rs`, Rust picks up the directory automatically. Verify by `ls src/world/` after the split and confirm `src/world.rs` is removed.

### 4.2 `src/world/mod.rs` (top of file, after the file-level doc comment)

```rust
//! World — owns SoA + sun + carrion + species + RNG + tick orchestration.
//!
//! [... existing doc comment from src/world.rs lines 1-6, copied verbatim ...]

pub(crate) mod tick;
pub(crate) mod nn;
pub(crate) mod save_v1;

// All other use statements + items follow (see §5 step-by-step).
```

`pub(crate)` (not `pub`) for the submodule declarations — they hold crate-internal implementation; no external caller addresses `crate::world::tick::...` directly.

### 4.3 Each submodule's `use` statements

Each submodule needs its own `use` block, since `use crate::constants::*;` in `mod.rs` does not propagate. Implementer should derive each submodule's imports from the items inside it. As a starting point:

**`src/world/tick.rs`:**
```rust
use super::World;
use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::Action;
use crate::events::{Event, EventKind};
use crate::hof::HallOfFame;
use crate::species::species_distance;
use crate::sun::SunMap;
```

**`src/world/nn.rs`:**
```rust
use super::World;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::genome::Genome;
use crate::vision::VisionBuf;
#[cfg(feature = "threads")]
use crate::vision::VisionBuf; // already imported; thread-only items below
#[cfg(feature = "threads")]
use rayon::prelude::*;
use super::BODY_RADIUS_PER_SIZE;
```
(Implementer: confirm `BODY_RADIUS_PER_SIZE` is only needed under `cfg(feature = "threads")`; if so, gate the import.)

**`src/world/save_v1.rs`:**
```rust
use super::World;
use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grid::SpatialGrid;
use crate::save::{rehydrate_event_log, validate_soa_lengths, LoadError, SaveV1, SCHEMA_VERSION};
use crate::species::SpeciesRegistry;
use crate::sun::SunMap;
use crate::vision::{VisionBuf, VISION_LEN};
```

Implementer must run `cargo build` after each move and let the compiler tell them which `use` lines are missing — do NOT pre-write the entire `use` block from this plan, because that risks adding unused imports that fail clippy.

---

## 5. Step-by-step implementation order

Each step is a single commit (or single editor batch with one `cargo check` before the next step).

1. **Create the directory + empty submodule files.**
   - `mkdir src/world/`
   - Create `src/world/tick.rs`, `src/world/nn.rs`, `src/world/save_v1.rs` with just a doc comment header in each.
   - **Do not yet** move `src/world.rs` to `src/world/mod.rs`.

2. **Move `src/world.rs` → `src/world/mod.rs` verbatim.** Single `git mv` (or equivalent). At this point the build must still pass: `cargo build && cargo build --features threads`.

3. **Add submodule declarations** to `src/world/mod.rs`:
   ```rust
   pub(crate) mod tick;
   pub(crate) mod nn;
   pub(crate) mod save_v1;
   ```
   Build: `cargo build && cargo build --features threads`.

4. **Move `from_save_v1` and `to_save_v1` to `src/world/save_v1.rs`.** This is the easiest extraction because both functions are at the end of their `impl World` block and reference only `World`'s public fields plus the two fields-to-be-`pub(crate)`-d.
   - Add `pub(crate)` to `World.cell_to_carrion` and `World.pending_extinction_check` in `mod.rs`.
   - In `save_v1.rs`, write:
     ```rust
     use super::World;
     // ... other use lines per §4.3 ...
     impl World {
         pub fn to_save_v1(&self) -> crate::save::SaveV1 { /* moved body */ }
         pub fn from_save_v1(save: crate::save::SaveV1) -> Result<Self, crate::save::LoadError> { /* moved body */ }
     }
     ```
   - Delete the original `to_save_v1` and `from_save_v1` from `mod.rs`.
   - Build: `cargo build && cargo build --features threads`.
   - Run: `cargo test world::save_v1` plus the existing save round-trip in `cargo test --release --test acceptance`.

5. **Move the NN free functions to `src/world/nn.rs`.** Order within this step:
   1. Move `chunk_ranges` (with its full doc comment 1240-1251). Uplift to `pub(crate)`. Add `use crate::constants::N_CHUNKS;` in `nn.rs`. Update `mod.rs:224` call site `chunk_ranges(n)` to `nn::chunk_ranges(n)` OR add `use self::nn::chunk_ranges;` at the top of `mod.rs`'s `impl World` block (the latter is less churn — **prefer the `use` import**). Build.
   2. Move `build_nn_input` (1263-1308). Uplift to `pub(crate)`. Build.
   3. Move `is_valid_action` (1310-1318). Uplift to `pub(crate)`. Build.
   4. Move `decode_action` (1320-1339). Uplift to `pub(crate)`. Build.
   5. Move `pick_action_d` (1341-1372). Uplift to `pub(crate)`. Build.

6. **Move `nn_forward_all_chunks` to `src/world/nn.rs`** as an `impl World` block:
   ```rust
   impl World {
       pub(crate) fn nn_forward_all_chunks(&mut self, ranges: &[(usize, usize); N_CHUNKS], n: usize) {
           // moved body verbatim, including both #[cfg] arms
       }
   }
   ```
   In `mod.rs`, uplift the method's caller site (`self.nn_forward_all_chunks(&ranges, n)` at line 225) — no change needed because it's `self.<method>` and visibility is now `pub(crate)`. Build under both feature sets.

7. **Move the tick-step `impl World` methods to `src/world/tick.rs`**, one at a time, building between each:
   1. `apply_movement_and_repulsion` (449-592). Uplift to `pub(crate)`. Build.
   2. `photosynth_two_pass` (594-636). Uplift. Build.
   3. `eat_and_scavenge` (638-760). Uplift. Build.
   4. `energy_bookkeeping` (762-829). Uplift. Build.
   5. `collect_deaths` (831-907). Uplift. Build.
   6. `decay_carrion` (909-924). Uplift. Build.

   Implementer **must** keep each method's body byte-identical (whitespace included). The only edit is moving the method header from `fn name(...)` to `pub(crate) fn name(...)`.

8. **Uplift `run_vision_pass`, `count_carrion_overlap`, `compute_is_at_wall` in `mod.rs`** from `fn` to `pub(crate) fn`. (They stay in `mod.rs`; only the visibility changes.) Build.

9. **Uplift scratch fields** in the `World` struct definition (in `mod.rs`) from private to `pub(crate)`. Build under both feature sets.

10. **Move tests.** Per §7 below. For each test, cut its full body (including `#[test]` attribute and any preceding doc comment) from `mod.rs`'s `#[cfg(test)] mod tests { ... }` block, and paste it into the destination submodule's new `#[cfg(test)] mod tests { use super::*; use crate::...; ... }` block. The `use super::*;` in the test module brings the parent submodule's items into scope; cross-submodule imports (e.g., a `tick.rs` test that calls `decode_action` from `nn`) need `use crate::world::nn::decode_action;` or `use super::super::nn::decode_action;`.
    Build + `cargo test` after each test-batch move.

11. **Run the full acceptance suite (DEFAULT build):**
    ```bash
    cargo fmt
    cargo clippy --all-targets -- -D warnings
    cargo test
    cargo test --release --test acceptance
    ```

12. **Run the full acceptance suite (THREADS build):**
    ```bash
    cargo clippy --all-targets --features threads -- -D warnings
    cargo test --features threads
    cargo test --release --features threads --test acceptance
    ```

13. **Rebuild the wasm bundle and TS layer:**
    ```bash
    wasm-pack build --target web --out-dir web/wasm
    cd web && pnpm typecheck && pnpm build
    ```

14. **Confirm both golden files are byte-identical to pre-split** (see §8).

---

## 6. Path-translation table

This is the load-bearing artifact for every other PR-1/PR-3/PR-4 piece. **All downstream planners cite this table when rewriting `src/world.rs:NNN` references.**

Line numbers in the "Post-split file:lines" column are **approximate target ranges** — the final exact line numbers depend on the boilerplate the implementer adds (use statements, blank lines). Use the column to know the *destination file*; rely on `git blame` or `grep <function-name>` to find the exact line.

| Pre-split `src/world.rs:lines` | Item | Post-split file | Post-split approx. lines |
|---|---|---|---|
| 1-6 | File-level doc comment | `src/world/mod.rs` | 1-6 |
| 8-20 | `use` statements | `src/world/mod.rs` | 8-22 (subset; rest move to submodules — see §4.3) |
| 22-24 | `BODY_RADIUS_PER_SIZE` const | `src/world/mod.rs` | ~25-27 |
| 26-33 | `DevSliders` struct | `src/world/mod.rs` | ~30-37 |
| 35-45 | `impl Default for DevSliders` | `src/world/mod.rs` | ~40-50 |
| 47-115 | `World` struct definition | `src/world/mod.rs` | ~55-125 |
| 117-188 | `impl World { new }` | `src/world/mod.rs` | ~130-200 |
| 190-192 | `impl World { population }` | `src/world/mod.rs` | ~205-207 |
| 194-340 | `impl World { step }` | `src/world/mod.rs` | ~215-360 |
| 342-447 | `impl World { nn_forward_all_chunks }` | `src/world/nn.rs` | ~30-140 |
| 449-592 | `impl World { apply_movement_and_repulsion }` | `src/world/tick.rs` | ~20-160 |
| 594-636 | `impl World { photosynth_two_pass }` | `src/world/tick.rs` | ~165-210 |
| 638-760 | `impl World { eat_and_scavenge }` | `src/world/tick.rs` | ~215-340 |
| 762-829 | `impl World { energy_bookkeeping }` | `src/world/tick.rs` | ~345-415 |
| 831-907 | `impl World { collect_deaths }` | `src/world/tick.rs` | ~420-500 |
| 909-924 | `impl World { decay_carrion }` | `src/world/tick.rs` | ~505-520 |
| 926-1005 | `impl World { handle_births }` | `src/world/mod.rs` | ~370-450 |
| 1007-1032 | `impl World { finalize_extinctions }` | `src/world/mod.rs` | ~455-480 |
| 1034-1038 | `impl World { tick_once }` | `src/world/mod.rs` | ~485-490 |
| 1040-1043 | `impl World { to_save_v1 }` | `src/world/save_v1.rs` | ~15-20 |
| 1045-1168 | `impl World { from_save_v1 }` | `src/world/save_v1.rs` | ~25-150 |
| 1170-1202 | `impl World { count_carrion_overlap }` | `src/world/mod.rs` | ~495-530 |
| 1204-1218 | `impl World { compute_is_at_wall }` | `src/world/mod.rs` | ~535-550 |
| 1220-1237 | `impl World { run_vision_pass }` | `src/world/mod.rs` | ~555-575 |
| 1240-1261 | `chunk_ranges` (free fn + doc) | `src/world/nn.rs` | ~145-170 |
| 1263-1308 | `build_nn_input` (free fn) | `src/world/nn.rs` | ~175-225 |
| 1310-1318 | `is_valid_action` (free fn) | `src/world/nn.rs` | ~230-240 |
| 1320-1339 | `decode_action` (free fn) | `src/world/nn.rs` | ~245-265 |
| 1341-1372 | `pick_action_d` (free fn) | `src/world/nn.rs` | ~270-305 |
| 1374-2291 | `#[cfg(test)] mod tests` | **distributed** — see §7 | — |

**Field-level translations (struct fields whose visibility changes — referenced by other plans):**

| Field on `World` | Pre-split file:line | Post-split file:line | Visibility change |
|---|---|---|---|
| `cell_to_carrion` | `src/world.rs:94` | `src/world/mod.rs:~104` | private → `pub(crate)` |
| `pending_extinction_check` | `src/world.rs:97` | `src/world/mod.rs:~107` | private → `pub(crate)` |
| `scratch_fx, scratch_fy, scratch_neighbors, scratch_damage, scratch_gain, scratch_cooldown_set, scratch_attempted_eat, scratch_attempted_scavenge, scratch_got_a_bite` | `src/world.rs:106-114` | `src/world/mod.rs:~116-124` | private → `pub(crate)` |

---

## 7. Test plan

The existing `#[cfg(test)] mod tests` in `src/world.rs:1374-2290` contains the tests listed below. Each test moves to the submodule whose code it primarily exercises. Where a test exercises items from multiple submodules (e.g., calls `decode_action` from `nn` AND mutates `World` state), it goes to the submodule whose code the *assertion* targets.

| Test fn | Pre-split lines | Target submodule | Cross-submodule imports needed |
|---|---|---|---|
| `world_initializes_with_one_creature` | 1378-1383 | `mod.rs` | none |
| `lone_creature_eventually_splits` | 1385-1397 | `mod.rs` | none |
| `energy_conservation_in_photosynth_pass` | 1399-1415 | `tick.rs` | `use super::super::World;` (already in `super::*`) |
| `carrion_returns_to_sun_on_decay` | 1417-1433 | `tick.rs` | none |
| `world_runs_many_ticks_without_panic` | 1435-1444 | `mod.rs` | none |
| `world_runs_2000_ticks_with_movement` | 1446-1465 | `mod.rs` | none |
| `vision_layout_matches_nn_input_block` | 1467-1475 | `nn.rs` | `use crate::vision::VISION_LEN;` (already) |
| `nn_input_layout_self_state_correct` | 1479-1514 | `nn.rs` | calls `build_nn_input` (same submodule) |
| `nn_input_layout_vision_passthrough` | 1516-1531 | `nn.rs` | same |
| `nn_input_layout_last_action_onehot` | 1533-1550 | `nn.rs` | same |
| `decode_action_valid_fallthrough_split_invalid` | 1552-1571 | `nn.rs` | calls `decode_action` (same submodule) |
| `decode_action_first_index_tiebreak` | 1573-1582 | `nn.rs` | same |
| `decode_action_eat_invalid_in_cooldown` | 1584-1595 | `nn.rs` | same |
| `decode_action_scavenge_invalid_when_zero_eff` | 1597-1607 | `nn.rs` | same |
| `chunk_ranges_partition` | 1611-1630 | `nn.rs` | calls `chunk_ranges`, `N_CHUNKS` (same submodule) |
| `chunk_ranges_small_population` | 1632-1648 | `nn.rs` | same |
| `chunked_tick_deterministic` | 1650-1685 | `mod.rs` | none (only `World::new`, `tick_once`) |
| `d19_thousand_creatures_thousand_ticks_no_explode` | 1689-1770 | `mod.rs` | broad — uses brain/genome/vision; lives in `mod.rs` because it's an end-to-end smoke test of `step`/`tick_once` |
| `decode_action_rest_always_valid_as_fallback` | 1772-1792 | `nn.rs` | calls `decode_action` |
| `e25_population_milestones_fire_once` | 1796-1843 | `mod.rs` | exercises `step`'s milestone block (lines 308-321) |
| `e25b_first_to_move_fires_on_movement_step` | 1849-1888 | `tick.rs` | calls `apply_movement_and_repulsion` |
| `e25_biggest_ever_tracks_max_size` | 1893-1920 | `tick.rs` | calls `energy_bookkeeping` |
| `e25_weirdest_requires_500_ticks` | 1923-1954 | `tick.rs` | calls `collect_deaths` |
| `e25_last_survivor_captures_final_death` | 1957-1998 | `tick.rs` | calls `collect_deaths` |
| `e20_tiny_mutation_keeps_species` | 2004-2027 | `mod.rs` | end-to-end `tick_once` loop |
| `e20_synthetic_large_drift_creates_new_species` | 2030-2068 | `mod.rs` | exercises `species.speciate` directly; not a tick-step test |
| `e20_speciation_event_lands_in_log_when_threshold_crossed` | 2073-2187 | `mod.rs` | end-to-end + synthetic speciation; broad |
| `eye_trig_recomputed_on_birth` | 2191-2229 | `mod.rs` | calls `handle_births` (which lives in `mod.rs`) |
| `scratch_fx_fy_zeroed_at_tick_start` | 2236-2267 | `tick.rs` | mutates `w.scratch_fx`, `w.scratch_fy` — those fields are now `pub(crate)` on `World`; test runs against `World`'s tick path |
| `scratch_grows_with_population` | 2270-2290 | `tick.rs` | same — exercises scratch-Vec lifecycle across ticks |

**Test count by destination:**
- `mod.rs` tests: 14
- `tick.rs` tests: 8
- `nn.rs` tests: 9
- `save_v1.rs` tests: 0 (round-trip lives in `src/save.rs` and `tests/acceptance.rs`)

**Total: 31 tests.** The implementer must verify `cargo test 2>&1 | grep -c 'test world::'` matches 31 (or equivalent count via `cargo test -- --list`) before and after the split.

---

## 8. Determinism verification recipe

### 8.1 Pre-split baseline (capture once before any edits)

```bash
cd /home/adamg/evosim
cargo test --release --test acceptance 2>&1 | tee /tmp/s1-pre-default.log
cargo test --release --features threads --test acceptance 2>&1 | tee /tmp/s1-pre-threads.log
cat tests/golden_snapshot_t10000.txt          # expect: 0xb76e907c6221f7f5
cat tests/golden_snapshot_t10000_threaded.txt # expect: 0xb76e907c6221f7f5
```

### 8.2 Post-split verification (after step 12 in §5)

```bash
cd /home/adamg/evosim
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features threads -- -D warnings
cargo test
cargo test --features threads
cargo test --release --test acceptance 2>&1 | tee /tmp/s1-post-default.log
cargo test --release --features threads --test acceptance 2>&1 | tee /tmp/s1-post-threads.log
diff /tmp/s1-pre-default.log /tmp/s1-post-default.log    # acceptable: only timing diffs
diff /tmp/s1-pre-threads.log /tmp/s1-post-threads.log    # acceptable: only timing diffs
cat tests/golden_snapshot_t10000.txt          # MUST still be: 0xb76e907c6221f7f5
cat tests/golden_snapshot_t10000_threaded.txt # MUST still be: 0xb76e907c6221f7f5
```

### 8.3 Wasm + TS verification

```bash
wasm-pack build --target web --out-dir web/wasm
cd web && pnpm typecheck && pnpm build
```

Acceptance: no clippy warnings, no fmt diff, all tests pass under both feature sets, both goldens byte-identical, wasm+pnpm builds clean (pre-existing static+dynamic-import warning for `evosim.js` is acceptable per master plan §10).

---

## 9. Risk register

| # | Risk | Likelihood | Mitigation |
|---|---|---|---|
| R1 | A method moved cross-file misses a `pub(crate)` upgrade → privacy compile error. | High | Step-by-step move in §5 with `cargo build` after each function. Any privacy error surfaces immediately. |
| R2 | A test moved to a submodule loses access to private items in its old home. | Medium | All items the tests touch are upgraded to `pub(crate)` in §3. Run `cargo test` after each test-batch move. |
| R3 | `impl World` block split across files reorders trait/inherent-method resolution (esp. if a method calls another that hasn't yet moved). | Low | Each move is one method at a time with a build between; Rust would error on missing method, not silently mis-resolve. `Drop` is not implemented on `World` (verify with `grep -n 'impl Drop for World' src/world.rs` — none found), so no destructor reordering risk. |
| R4 | Subtle behavior drift from a function move that accidentally changes inlining decisions. | Very low | Cross-module calls in Rust default to "may inline" via LLVM; `#[inline]` is not added or removed here. `cargo build --release` byte-compares trivially impossible to verify, but the goldens act as the integration check. **Note: clippy's `inline_always` is not the right tool here — do not add `#[inline(always)]` annotations during the split; that's S33's scope.** |
| R5 | A `use crate::...::*;` glob import in one submodule shadows an item from another. | Low | §4.3 specifies explicit imports; no glob imports across submodules. The only glob is `use crate::constants::*;` which is harmless. |
| R6 | `from_save_v1`'s direct write to `cell_to_carrion: Vec::new()` and `pending_extinction_check: Vec::new()` will fail to compile after the move until those fields are `pub(crate)`. | High | Field uplift happens BEFORE the function move (step 4 in §5 — implementer must uplift the field in the same edit batch as moving the function). |
| R7 | Cross-feature build skew: a method works under default but not `--features threads` (because the threaded arm has different field accesses). | Medium | Both `cargo build` and `cargo clippy` are run for both feature sets after every major move in §5. |
| R8 | Tests in their new home can't find a free fn because the test forgot `use super::*` or `use crate::world::nn::...`. | Medium | §7 lists the cross-submodule imports each test needs. Implementer adds explicit imports when the body references a non-local item. |
| R9 | `chunk_ranges` is called from `mod.rs` (`step` body) after moving to `nn.rs`. Implementer might miss adding `use self::nn::chunk_ranges;` (or fully qualifying). | High | §5 step 5.1 explicitly calls this out. Also: a missing import is a hard compile error — easy to catch. |
| R10 | Reviewer rejects the diff because the test-move makes `git blame` noisy. | Low | Acceptable trade. The path-translation table (§6) is the recovery mechanism. |
| R11 | The `#[cfg_attr(feature = "threads", allow(dead_code))]` attribute on `count_carrion_overlap` / `compute_is_at_wall` doesn't transfer correctly when the methods move (it's on the `fn` line at 1173 and 1207). | Low | The attribute travels with the function body. Preserve verbatim. **Note:** S11 will later replace these with shared free fns and remove the `dead_code` allow; this S1 piece keeps them as-is. |

---

## 10. Acceptance criteria

All of the following must be true before S1 is marked done:

- [ ] `cargo fmt -- --check` clean.
- [ ] `cargo clippy --all-targets -- -D warnings` clean (default features).
- [ ] `cargo clippy --all-targets --features threads -- -D warnings` clean (threads feature).
- [ ] `cargo test` passes all unit tests (default features) — total test count matches pre-split (31 in `world::*`, plus all other module tests).
- [ ] `cargo test --features threads` passes (threads feature).
- [ ] `cargo test --release --test acceptance` passes; **`tests/golden_snapshot_t10000.txt` is byte-identical to pre-split (`0xb76e907c6221f7f5`)**.
- [ ] `cargo test --release --features threads --test acceptance` passes; **`tests/golden_snapshot_t10000_threaded.txt` is byte-identical to pre-split (`0xb76e907c6221f7f5`)**.
- [ ] `wasm-pack build --target web --out-dir web/wasm` succeeds.
- [ ] `pnpm typecheck` clean.
- [ ] `pnpm build` clean (pre-existing `evosim.js` static+dynamic-import warning is acceptable).
- [ ] `src/world.rs` is removed; `src/world/` directory contains exactly four files: `mod.rs`, `tick.rs`, `nn.rs`, `save_v1.rs`.
- [ ] The path-translation table in §6 has been verified (spot-check 5 random rows) against the actual post-split file contents.
- [ ] No new dependencies added; `Cargo.toml` unchanged.
- [ ] No re-exports added beyond what `pub mod world;` already provides (i.e., no `pub use self::tick::*;` etc. — submodule items are crate-internal).
- [ ] Commit message follows conventional commits: `refactor(world): split src/world.rs into mod/tick/nn/save_v1 submodules`.

---

## Review feedback

**Reviewer:** opus (critical reviewer)
**Date:** 2026-05-24

**Verdict:** APPROVE WITH MINOR REVISIONS

The plan is unusually thorough. The path-translation table is complete, the visibility analysis is correct, the implementation order is staged with build-checks between every move, and the determinism recipe is explicit. A sonnet implementer can execute this front-to-back without questions, modulo the small fixes below. None of the issues block; all are clean-ups to the plan text itself.

### Issues (numbered, severity tagged)

1. **[minor] Line numbers in §2 and §6 are off by 2–7 from the live file.** Spot-check against `src/world.rs` at HEAD:
   - `nn_forward_all_chunks` is at **345**, plan says 342.
   - `count_carrion_overlap` is at **1174**, plan says 1170.
   - `compute_is_at_wall` is at **1208**, plan says 1204.
   - `run_vision_pass` is at **1222**, plan says 1220.
   - `chunk_ranges` is at **1252** (doc starts 1240), plan says 1240/1261 inconsistently.
   - `build_nn_input` at **1270** vs plan 1263; `is_valid_action` at **1311** vs 1310; `decode_action` at **1322** vs 1320; `pick_action_d` at **1346** vs 1341; `from_save_v1` at **1047** vs 1045; `to_save_v1` at **1041** vs 1040; `tests` mod at **1374** matches.
   The §6 caveat that line numbers are "approximate" mitigates this, but the *pre-split* column should be exact since downstream planners will grep for those lines. **Fix:** re-run `grep -nE '^(impl|fn|pub fn|pub\(crate\) fn|#\[cfg\(test\)])' src/world.rs` and replace the §2/§6 pre-split line numbers with the actual values. Single editor pass.

2. **[minor] §3.1 omits `vision` from the scratch-field uplift discussion, but `from_save_v1` constructs it via the local `vision` binding (line 1153) — that's fine because `vision` is already `pub`.** Worth a one-liner in §3.1 acknowledging that the struct-literal at lines 1124-1167 writes ALL of the previously-private fields (`cell_to_carrion`, `pending_extinction_check`, **all nine scratch_***), and that the §3.1 uplift list is exhaustive over those. As written, a reader has to cross-reference §3.1's three rows against the actual struct-literal to convince themselves no field is missing. **Fix:** add one sentence at the top of §3.1: "These uplifts are exactly what `from_save_v1`'s struct literal (`src/world.rs:1124-1167`) requires after the move, plus what `tick.rs` tests need." Resolves R6's coverage gap.

3. **[minor] §4.3 `src/world/nn.rs` use-block has a duplicate `use crate::vision::VisionBuf;`** (lines 184 and 186 in the plan). The second one is intended to gate `rayon::prelude::*` but the comment "thread-only items below" appears after a redundant re-import. **Fix:** delete the duplicate `use crate::vision::VisionBuf;` line; keep only the `#[cfg(feature = "threads")] use rayon::prelude::*;`. Also the comment "Implementer: confirm `BODY_RADIUS_PER_SIZE` is only needed under `cfg(feature = "threads")`" undersells: per S3's plan it will move to `constants.rs` *after* S1, so during S1 the import is unconditional from `super::BODY_RADIUS_PER_SIZE`. Reword.

4. **[minor] §2.4's "omit the tests mod entirely" instruction conflicts with the §7 test count ("save_v1.rs tests: 0").** Decision is to omit — restate this in §7 explicitly so the implementer doesn't write a `#[cfg(test)] mod tests {}` block to "match the pattern". **Fix:** in §7 add "(`save_v1.rs`: no `mod tests` block — round-trip tests live in `src/save.rs` and `tests/acceptance.rs`)."

5. **[minor] Master plan §4-S1 says "Either way, sonnet implementer must rehome tests"; this plan's §10 step 5 is split per-method-move and step 10 batches all tests at the end.** The batched-at-end approach is safer (fewer rebuilds during test-move). But §5 step 10 should explicitly say "**after** all production code has moved AND `cargo test` is green on the unsplit tests block in `mod.rs`". As written, an implementer could try to move tests before some methods are uplifted to `pub(crate)`. **Fix:** add "Prerequisite: all of steps 1-9 are complete and `cargo test` is green with all tests still residing in `mod.rs`'s tests block" to the head of step 10.

6. **[minor] §5 step 5.1 says "prefer the `use` import" for `chunk_ranges`** (i.e., `use self::nn::chunk_ranges;` at the top of `mod.rs`'s `impl World` block). `use` statements are not valid inside an `impl` block — they must be at module scope. **Fix:** clarify "add `use self::nn::chunk_ranges;` at the **top of `src/world/mod.rs`** (module scope, alongside other `use` lines)." Minor wording issue; experienced implementer would catch this immediately but the plan is supposed to be question-free.

7. **[nit] §2.1 "Module placement judgment" rationale for keeping `handle_births`, `finalize_extinctions`, `run_vision_pass`, `count_carrion_overlap`, `compute_is_at_wall` in `mod.rs` is sound** and I endorse it:
   - `handle_births` + `finalize_extinctions` bracket the lifecycle alongside `step` and `tick_once` — moving them to `tick.rs` would split the public surface across two files arbitrarily.
   - `run_vision_pass` calls `build_cell_to_carrion` and uses `VisionPass` directly; it is more of a setup helper than a tick-step body. Mod.rs is fine.
   - `count_carrion_overlap` / `compute_is_at_wall` are leaf helpers shared between sequential and threaded NN paths. S11 will refactor them in PR-3; bracketing them now would just create churn. Keeping in `mod.rs` is correct.
   The master plan §4-S1 briefing suggested they live in `mod.rs` and the planner followed that. No change needed.

8. **[nit] Risk register is good but should add R12: "Implementer accidentally adds `pub use self::tick::*;` re-exports in `mod.rs` thinking they're needed for `World` methods to be visible."** Methods defined in `impl World` blocks in submodules ARE addressable as `world.method()` from outside the crate without any re-export, because method resolution follows the *type* `World`, not the module path. Adding `pub use` would needlessly widen visibility. The acceptance criterion already forbids this; mention the rationale in the risk register so the implementer understands *why*.

9. **[nit] §8 determinism recipe is correct but missing one belt-and-suspenders check.** After step 12, also run `cargo test 2>&1 | grep -E '^test world::' | wc -l` (or `cargo test -- --list | grep 'world::' | wc -l`) and assert the count equals the pre-split count of 31. This catches the case where a test-move accidentally drops a `#[test]` attribute. The plan mentions this in §7 ("implementer must verify... 31") but doesn't put it in the §8 recipe.

### Reviewer's confidence

High. The plan is implementable as-is; the minor revisions above are polish, not corrections to substance. The path-translation table in §6 is the load-bearing artifact for the other 25 downstream pieces, and it is complete. The visibility analysis correctly identifies that struct-field privacy is module-scoped (so `impl World` in `tick.rs` cannot touch a private field on `World` defined in `mod.rs`), which is the subtlest correctness point in the split.

