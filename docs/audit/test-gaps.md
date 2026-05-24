# Test coverage gaps

Audit of `tests/acceptance.rs` plus all `#[cfg(test)]` modules in `src/*.rs`,
ordered by risk. Gaps are concrete and actionable; each item names a
suggested test and what it should verify. "Have" lines summarize current
coverage so we don't re-add what already exists.

Conventions: file paths absolute; test names use the existing
`snake_case` / `module_prefix_` style already in the codebase.

---

## P0 — determinism & equivalence (load-bearing for the whole project)

### `tests/acceptance.rs` — cross-feature determinism

Have: single-threaded 10k golden; threaded 10k golden; profiler-on hash
equality; one save/load round-trip at tick 1000+500.

Missing:

- `acceptance_threaded_matches_sequential_t10000`
  Run the sequential world and the `threads`-feature world from the same
  seed; assert `snapshot_hash` byte-identical at t=10_000 (or document why
  not). The current dual-golden setup tolerates divergence but never
  asserts equivalence; that's a real risk given the comment in
  `acceptance.rs:165` ("parallel f32 reduction order across chunks").
  If they legitimately differ, add a test that asserts the divergence is
  ONLY in `Brain::forward` reductions by patching the threaded path to use
  `forward_scalar` and re-running.
- `acceptance_chunk_count_invariant`
  Run at `N_CHUNKS=1`, `N_CHUNKS=8`, `N_CHUNKS=17` (vary via env or wrapper)
  and assert hash is invariant. Today `chunked_tick_deterministic`
  (`world.rs:1652`) only checks same seed → same tick count, not hash, and
  not across chunk counts.
- `save_load_round_trip_at_multiple_phases`
  Save/load currently tested once. Add a parametrized variant that saves
  at ticks {0, 1, 500, 5000} and asserts `ref_hash == loaded_hash` for
  each. Tick 0 catches "initial state not serialized fully", tick 5000
  catches "extinction bookkeeping diverges after reload" — both of which
  the existing test misses.
- `save_load_hash_equal_immediately_after_load`
  Save world W → load into W'. Assert `snapshot_hash(W) ==
  snapshot_hash(W')` BEFORE stepping. Current
  `save_load_step_preserves_determinism` only compares after additional
  ticks, so a missing field (e.g. `pending_extinction_check`, which is
  reset to empty on load — `world.rs:1155`) could mask divergence.

### `src/snapshot_hash.rs` — coverage of the hash itself

Have: deterministic; same-seed-same-hash.

Missing:

- `hash_differs_when_creature_position_changes`
  Mutate a single `creatures.x[0]` by 1 ULP; assert hash changes. Today
  there is no negative test — a bug zeroing the hash function would still
  pass.
- `hash_includes_carrion_age_and_pool`
  Construct two worlds identical except for `carrion[0].pool`; assert
  different hash. Same for `carrion[i].age`.
- `hash_includes_pending_extinction_check_or_documents_exclusion`
  `snapshot_hash` does NOT hash `pending_extinction_check` or
  `live_species_count`. If that's intentional (transient), add a test
  asserting hash equality across two worlds where those differ; if not,
  add them to the hash.

---

## P1 — threading vs single-threaded equivalence

### `src/world.rs` — parallel NN forward path

Have: `chunk_ranges_partition`, `chunk_ranges_small_population` exercise
the partition; no test exercises the threaded code path (`world.rs:351`
region) against the sequential one.

Missing:

- `nn_forward_threaded_matches_sequential_single_tick`
  Build a 200-creature world, run ONE tick under each feature flag,
  compare `creatures.action_this_tick` and the post-NN `vision[]` /
  `last_action[]` byte-for-byte.
- `nn_forward_chunking_invariant_under_pop_1`
  Population = 1: ensure the `chunk_ranges` partition with N_CHUNKS=8
  produces 7 empty chunks and the parallel code path still runs. Today
  `chunk_ranges_small_population` checks the partition but not the actual
  NN forward dispatch on n<N_CHUNKS.
- `nn_forward_no_torn_writes_into_action_buffer`
  After parallel forward + decode, assert no `Action` value is invalid
  (out of 0..6) and `action_this_tick.len() == creatures.len()`. Catches
  index aliasing bugs in the threaded gather/scatter.

### `src/brain.rs`

Have: SIMD-vs-scalar within 1e-5; ReLU clip; no-NaN on extremes;
determinism; mutation rate scaling.

Missing:

- `forward_pass_zero_input_zero_output_with_arbitrary_weights`
  Currently only tested with zero weights. The dual — zero inputs across
  arbitrary weights — verifies the matmul has no spurious bias.
- `forward_pass_simd_chunk_boundary_correctness`
  Specifically test that the 17-input-chunk × 24-hidden boundary doesn't
  miss the last group: set one weight at index `NN_INPUTS-1` in row 0 to
  1.0, input[NN_INPUTS-1]=1.0, all else zero; assert `hidden[0] == 1.0`.
  Same for the 3-hidden-chunk × 8-output boundary.
- `forward_pass_no_inf_on_huge_weights`
  Set all weights to `f32::MAX / 200.0` (so 136-term sum just stays
  finite). Assert outputs are finite. Catches accumulator-saturation
  regressions if weight init range changes.
- `child_mutation_geometric_skip_expected_count`
  `child_mutation_perturbs_some_weights` asserts 100..800 diffs at
  rate=0.1; tighten to a statistical bound (e.g. Binomial 99% CI) so a
  10× regression in mutation count is caught.

---

## P2 — genome mutation invariants

### `src/genome.rs`

Have: founder in bounds; zero rate → no-op; full rate → drift;
eye_count adjacency.

Missing:

- `mutate_preserves_bounds_under_stress`
  Run `mutate_in_place(rate_multiplier=1000.0)` for 10_000 iterations;
  assert all numeric fields remain within their MIN/MAX constants. The
  existing test only checks one mutation pass.
- `mutate_eye_count_terminal_values_have_one_neighbor`
  When `eye_count` is at `EYE_VALID[0]` (lowest) or `EYE_VALID.last()`,
  mutation must produce only the single valid neighbor — never duplicate,
  never invalid. Currently only the middle case (6 → 4 or 8) is tested.
- `mutate_eye_offsets_circular_modulo`
  Set `eye_offsets[0] = TAU - 0.01`, eye_count=4, repeatedly mutate;
  assert value stays in `[0, TAU)` and wraps correctly. Today the modulo
  logic at `genome.rs:188` is unverified.
- `mutate_eye_offsets_only_active_slots`
  With `eye_count=4`, mutate 100×; assert `eye_offsets[4..]` is
  byte-identical to the pre-mutation value. Catches a future regression
  that mutates inactive slots and silently changes vision behavior on a
  later eye-count increase.
- `mutation_rate_drift_clamped_to_unit_interval`
  Hammer `drift_rate` 100_000× with full multiplier; assert every
  per-trait rate stays in `[0, 1]`. The clamp at `genome.rs:280` is
  untested.
- `mutate_max_age_rounding_no_underflow`
  `mutate_u32` rounds a `f32` delta then clamps; with `max_age = MIN`
  and a large negative delta, ensure no underflow / wraparound.

---

## P3 — grid edge cases

### `src/grid.rs`

Have: rebuild correctness; bounded radius scan; rebuild-twice.

Missing:

- `cell_of_clamps_at_world_boundary`
  `cell_of(WORLD_SIZE, WORLD_SIZE)` — the boundary exactly — should
  clamp to `HASH_DIM - 1`. `cell_of(WORLD_SIZE + 0.01, ...)` (out of
  bounds; can happen mid-tick before clamp) must also not panic. The
  `.min(HASH_DIM - 1)` at `grid.rs:42` is untested.
- `cell_of_handles_negative_input`
  `cell_of(-0.1, -0.1)` — what's the behavior? `(-0.1 / 5.0) as usize`
  underflows to huge usize before `.min()`, which still works but is
  fragile. Add a test pinning current behavior so a refactor catches
  the change.
- `for_each_in_radius_negative_query_origin`
  Calling `for_each_in_radius(-100.0, -100.0, 1.0, ...)` must not panic
  and must return empty (lo_x clamped to 0, hi_x clamped to 0 → empty
  iteration). Today the `.max(0) as usize` cascade is untested for
  negative inputs.
- `for_each_in_radius_at_world_max_corner`
  Query at `(WORLD_SIZE, WORLD_SIZE)` with radius 0.1 — must not
  out-of-bounds the `starts` array (the +1 lookup at `grid.rs:83`).
- `rebuild_empty_population_is_noop`
  `rebuild(&[], &[])` then `for_each_in_radius` — must not panic and
  yield zero callbacks.

There is no world wrap (the world clamps at walls — `world.rs:570`),
so a "wrap" test is N/A; document this in `cell_of_handles_negative_input`
to capture the invariant.

---

## P4 — species speciation thresholds

### `src/species.rs`

Have: lineage naming alternates; identical genomes → distance ~0.

Missing:

- `species_distance_eye_jump_dominates`
  Two identical genomes except `eye_count` (4 vs 8); assert distance
  exceeds `SPECIES_EYE_JUMP_COST`. The categorical jump cost at
  `species.rs:156` is untested.
- `species_distance_brain_only_diff`
  Identical genomes, different brain weights; verify the `brain` term
  contributes proportionally to `||Δw|| / sqrt(N)` and the body term is
  zero.
- `species_distance_eye_offset_circular_min`
  `g1.eye_offsets[0] = 0.0`, `g2.eye_offsets[0] = TAU - 0.01`; the
  circular distance should be ~0.01, NOT ~TAU. Verifies the
  `raw.min(two_pi - raw)` at `species.rs:166`.
- `species_distance_eye_offset_uses_only_shared_indices`
  Set `g1.eye_count=4`, `g2.eye_count=8`, randomize offsets[4..8];
  distance must equal the same call with offsets[4..8] zeroed (only
  shared indices contribute).
- `speciate_threshold_exact_boundary`
  Construct a child with distance == `SPECIES_THRESHOLD` exactly (or
  within 1 ULP). Today `handle_births` uses `dist > SPECIES_THRESHOLD`
  (`world.rs:969`); a test pinning the strict-greater-than semantics
  prevents a future drift to `>=` that changes the golden.
- `speciate_deep_lineage_name_truncates`
  Walk `speciate` to depth 20 with long names; assert the
  `name.truncate(15)` + `…` path fires correctly. Today the
  `>16 chars` path at `species.rs:69` is unreachable in
  `lineage_naming_alternates`.

---

## P5 — hall-of-fame correctness

### `src/world.rs` HoF tracking

Have: biggest_ever tracks max; weirdest requires age 500;
last_survivor captures final death.

Missing:

- `hof_biggest_ever_persists_through_save_load`
  Set biggest_ever in W; save/load → W'; assert `W'.biggest_ever` is
  byte-identical. F.26 round-trip tests assert population but not HoF
  fields specifically.
- `hof_weirdest_picks_max_distance_among_qualifying`
  Two creatures both age >= 500 dying same tick; one has bigger genome
  drift. Assert weirdest is the higher-drift one. Today
  `e25_weirdest_requires_500_ticks` only tests the age gate.
- `hof_longest_lived_overwritten_when_strictly_greater`
  Pin tiebreak: if two creatures die at same age, longest_lived should
  remain the first one (`>` not `>=` at `world.rs:868`). A regression to
  `>=` would silently change which creature gets the eulogy card.
- `hof_first_mover_creature_id_matches_first_to_cross_5u`
  With two creatures both moving, assert `FirstToMove.creature_id`
  belongs to the one that crossed 5.0 first within that movement step
  (not the lower-index one). Today `e25b_first_to_move_fires_on_movement_step`
  only has one mover.

---

## P6 — event emission ordering

### `src/events.rs` + `src/world.rs`

Have: PopulationMilestone fires once; FirstToMove fires;
Speciation event in log; F.26 round-trips event log length.

Missing:

- `event_ordering_within_a_single_tick`
  Construct a tick where multiple event kinds fire (e.g. a birth that
  speciates + a death that extincts the parent species). Assert the
  order in `events.all` matches the step order in `step()`:
  FirstToEat (during eat_and_scavenge) → Speciation (handle_births)
  → Extinction (finalize_extinctions). Today event-order is
  load-bearing for save/load + UI but completely untested.
- `extinction_event_fires_only_once_per_species`
  Drive a species to extinction; tick 100 more times; assert exactly
  one `Extinction { species_id }` event for that id. The
  `died_tick.is_none()` guard at `world.rs:1016` is untested.
- `extinction_event_does_not_fire_when_species_alive`
  Put a species into `pending_extinction_check` while members still
  exist; call `finalize_extinctions`; assert no event fires and
  `died_tick` stays None.
- `events_log_ring_buffer_caps_at_200`
  `EventLog::push` evicts when `recent.len() == ring_cap`. Push 250
  events; assert `recent.len() == 200` and `all.len() == 250`.
  `EventLog` has zero direct tests in the codebase.
- `world_ended_event_fires_exactly_once_at_extinction`
  Run a world to extinction (force-kill the founder); assert
  `WorldEnded` event fires once and `tick_once()` returns false on the
  next call. Untested today.
- `events_suppressed_when_events_enabled_false`
  Default `events_enabled` is false in `World::new`; trigger every
  event kind and assert `events.all` stays empty. Catches a regression
  to "log by default" that would blow up save size.

---

## P7 — numerical stability edge cases

### `src/brain.rs`

- `brain_forward_subnormal_inputs`
  Input filled with `f32::MIN_POSITIVE / 2.0` (subnormals); assert
  output is finite. SIMD subnormals can flush-to-zero on some CPU
  modes; deterministic semantics demand we know.
- `brain_forward_large_negative_hidden_does_not_overflow_relu`
  All input=+1, all w_ih=-1e6; pre-activation ≈ -1.36e8; ReLU should
  produce hidden=0, not propagate. The existing
  `forward_pass_relu_clips_negative_hidden` uses -136; this stresses
  the SIMD reduce_add.

### `src/world.rs` movement & energy

- `movement_does_not_escape_world_at_max_velocity`
  Place creature at `(WORLD_SIZE - 0.01, WORLD_SIZE - 0.01)` with
  `vx=vy=1e6`; run one tick; assert position is clamped to
  `[r, WORLD_SIZE - r]`. The clamp at `world.rs:570-582` is untested
  in isolation.
- `repulsion_force_capped_at_REPULSION_MAX`
  Place 50 creatures within 1.0 of each other; assert
  `|scratch_fx[i]| <= REPULSION_MAX` post-step (currently asserted in
  `scratch_fx_fy_zeroed_at_tick_start` only as a byproduct).
- `energy_never_negative_after_bookkeeping`
  Run 5000 ticks across a stressed world; assert
  `creatures.energy[i] > some_min` for any survivor (or that
  collect_deaths picks up everyone with energy < 0).
- `nn_input_is_at_wall_at_corners`
  Place creature at `(0, 0)`; build NN input; assert `input[5] ==
  1.0`. Today `nn_input_layout_self_state_correct`
  (`world.rs:1481`) only tests center, where `is_at_wall == 0`.

---

## P8 — smaller gaps

### `src/sun.rs`

- `refill_rate_zero_is_noop` — set `refill_rate = 0`, drain a cell,
  refill 1000×; assert no change.
- `hotspot_capacity_higher_than_baseline` — verify hotspots actually
  produce a capacity bump (the gradient test only checks west-east
  trend).

### `src/rng.rs`

- `geom_skip_distribution_matches_geometric` — sample 100k from
  `geom_skip(0.01)`; assert empirical mean ≈ 99. Today
  `geom_skip_edges` only covers `p=0` and `p=1`.
- `serde_round_trip_preserves_stream` — serialize, deserialize, draw
  1000 values; compare to original stream. F.26 depends on this.

### `src/save.rs`

- `save_v2_legacy_load_path` — if there's any v0 → v1 migration path,
  test it. If schema is locked at 1, document with a test that asserts
  any non-1 version is rejected (have `f26_schema_version_mismatch_errs`
  for 999; add tests for 0 and 2).
- `save_creature_count_mismatch_is_rejected` — construct a `SaveV1`
  with `creatures.x.len() != creatures.y.len()`; assert
  `validate_soa_lengths` errs. Hand-edited save files will hit this.

### `src/creature.rs`

- `remove_indices_unsorted_input_panics_or_handles` — pin the contract.
  Today `remove_indices` is called with sorted indices; document /
  test what happens on unsorted input.

### `src/wasm_api.rs`

Has 12 tests already; coverage looks reasonable. Lower priority.

### `src/profiler.rs`

Has 10 tests; profiler is observation-only (verified by
`profile_does_not_change_hash`). Lower priority.

---

## Out of scope (intentionally not adding)

- Coverage tooling itself (`cargo-tarpaulin`, etc.) — not requested.
- Property-based tests (`proptest`) — would be valuable for genome
  mutation invariants and grid edge cases, but adds a dep. Mention as
  future work.
- UI / browser tests — not part of this audit.
