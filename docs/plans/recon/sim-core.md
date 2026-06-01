# Recon: Rust sim core — current-state map (for v2.0.0)

Read-only recon. Anchors are `file.rs:line`. Source-of-truth precedence
(stated and followed): **`src/` Rust (`constants.rs`, `wasm_api.rs` SAB layout)
> generated `web/wasm/*.d.ts` > `web/` TS > tests > docs.** Where TS and Rust
disagree on a number, the Rust value below is authoritative.

---

## 1. NN / brain (`src/brain.rs`)

- **`NN_INPUTS = 32`** — `constants.rs:49`. `NN_OUTPUTS = 5` — `constants.rs:50`
  (`out[0]=vx, out[1]=vy, out[2..5]=action logits Graze/Eat/Split`).
- **Hard compile asserts** — `brain.rs:27-32`: `NN_INPUTS == 32`,
  `NN_OUTPUTS == 5`, `NN_INPUTS.is_multiple_of(8)`.
- **`Brain` struct** — `brain.rs:307-311`: `pub weights: Vec<f32>` (one
  contiguous row-major SoA, hidden-unit-contiguous per matmul) + `pub topology:
  NnTopology`. `Serialize/Deserialize/Clone`.
- **`Brain::forward`** — `brain.rs:360-367`. Sig: `(&self, input: &[f32;
  NN_INPUTS], output: &mut [f32; NN_OUTPUTS], scratch_a: &mut [f32], scratch_b:
  &mut [f32], timings: Option<&mut PickTimings>)`. Ping-pong scratch (each
  `>= topology.max_width()`). Output projection: `tanh` on slots 0,1 only
  (`brain.rs:426-427`); action logits 2..5 stay raw.
- **`Brain::founder`** — `brain.rs:317-335`. Per-layer **He-uniform**: `r =
  sqrt(6 / fan_in)`, weights drawn `rng.uniform(-r, r)`. `fan_in` starts at
  `NN_INPUTS`, walks `hidden_sizes ++ [NN_OUTPUTS]` (`brain.rs:321-326`). Takes
  `(rng, topology)`.
- **`weight_count`** — `NnTopology::weight_count` `brain.rs:176-184` = Σ
  `fan_in*fan_out` per matmul (`+ fan_in*NN_OUTPUTS`). Legacy = `32*48 + 48*24 +
  24*5 = 2808` (test `brain.rs:567`).
- **`NnTopology`** — `brain.rs:108-112`: `hidden_sizes: Vec<usize>` + `activations:
  Vec<Activation>`. **Inputs/outputs are implicit; only hidden layers are
  runtime** (doc `brain.rs:100-107`). Validation `NnTopology::new` `brain.rs:118-150`:
  ≥1 hidden, ≤ `NN_MAX_HIDDEN_LAYERS`, each width multiple-of-8 in
  `[8, NN_MAX_HIDDEN_WIDTH]`, `activations.len() == hidden_sizes.len()`.
  `legacy()` = `[48,24]` + 2×LReLU (`brain.rs:154-160`).
- **MAX_* ceilings** (`constants.rs`): `NN_MAX_HIDDEN_WIDTH = 256` (`:58`),
  `NN_MAX_HIDDEN_LAYERS = 8` (`:61`), `NN_MAX_MATMULS = 9` (`:63`).
- **SIMD forward path** — `matmul_tiled` (4× output-tile, `wide::f32x8`)
  `brain.rs:201-237` for hidden→hidden; `matmul_output` (NN_OUTPUTS=5 hardcoded)
  `brain.rs:244-258`. Scalar reference `forward_scalar` `brain.rs:445-494`
  (`#[cfg(test)]`).
- **SIMD-vs-scalar drift test** — `forward_pass_matches_scalar_reference_multiple_topologies`
  `brain.rs:713-755`. Asserts `rel_err < 1e-5` across 3 topologies (legacy,
  `[64,32,16]`, `[8]`). **Critical anchor for any topology/SIMD change.**
- **`Activation`** enum — `brain.rs:37-44`: `LReLU, ReLU, Tanh, Sigmoid, Linear`.
  `parse`/`as_str` `brain.rs:46-65`. LReLU slope 0.01 (`brain.rs:69`).

## 2. NN input construction (`src/world/nn.rs` + `src/world/proximity.rs`)

`build_nn_input` — **`world/nn.rs:377-490`** (NOT a separate file; it lives in
`world/nn.rs`). Returns `[f32; 32]`. Slot layout (confirmed against
`constants.rs:39-48` and code):

| slot | meaning | code |
|---|---|---|
| 0 | hunger `= (1 - energy/energy_max).clamp` | `nn.rs:407` |
| 1 | age_frac `= age/max_age` | `nn.rs:409` |
| 2 | `prev_vx / MOVE_SPEED_MAX` | `nn.rs:410` |
| 3 | `prev_vy / MOVE_SPEED_MAX` | `nn.rs:411` |
| 4 | is_last_graze `matches!(la, Action::Graze)` | `nn.rs:413` |
| 5 | **is_last_eat** `matches!(la, Action::Eat)` | `nn.rs:418` |
| 6 | `ticks_since_split / max_age` (clamp) | `nn.rs:419` |
| 7 | cooldown_ready `= (digestion_cooldown == 0)` | `nn.rs:420` |
| 8..12 | wall_proximity N/S/E/W (`compute_wall_proximity`, range `WALL_PROXIMITY_RANGE=50`) | `nn.rs:423-427` |
| 12..20 | creature_proximity ×8 sectors (`compute_creature_proximity_sectors`, range `PROXIMITY_RANGE=20`) | `nn.rs:437-439` |
| 20..28 | grass_density ×8 sectors (`compute_grass_density_sectors`, range `PROXIMITY_RANGE`/LUT) | `nn.rs:448-450` |
| 28 | padding 0.0 (reserved, "predator-color v1.6+") | `nn.rs:459` |
| 29 | curr_grass_density = `grass.bilinear_sample(x,y)` | `nn.rs:462` |
| 30 | bias constant 1.0 | `nn.rs:465` |
| 31 | padding 0.0 (SIMD align) | `nn.rs:468` |

Offset consts (`constants.rs:151-161`): `NN_WALL_OFFSET=8`,
`NN_CREATURE_SECTOR_OFFSET=12`, `NN_GRASS_SECTOR_OFFSET=20`,
`NN_CURR_GRASS_SLOT=29`, `NN_BIAS_SLOT=30`. `NN_SECTORS=8`.
**Note: there is no per-creature "memory" of more than `last_action`** — the
self/memory block is just current self-state + `last_action` + `ticks_since_split`.

- **`decode_action`** — `nn.rs:509-533`: argmax over 3 logits with first-index
  tiebreak, then valid-fallthrough; non-finite → `Graze`.
- **`is_valid_action`** — `nn.rs:494-505`: Graze always; Eat iff `cooldown==0`;
  Split iff `energy >= split_threshold`.
- **Action output encoding** — `pick_action_d` `nn.rs:566-654`: `vx =
  output[0]*MOVE_SPEED_MAX`, `vy = output[1]*MOVE_SPEED_MAX` (`nn.rs:623-624`);
  logits `output[2..5]` (`nn.rs:626`). Returns `(vx, vy, action, argmax_pre:u8)`
  where `argmax_pre` is the pre-fallthrough argmax (drives color-EMA green).
- **`is_last_eat` write** — slot 5 `nn.rs:418` (`matches!(la, Action::Eat)`).
- Proximity internals (`world/proximity.rs`): `proximity_starburst`
  (`:50-83`), `build_sector_lut` 33×33 (`:110-146`), `LUT_RADIUS=16`
  (`:89`), `LUT_DIM=33` (`:91`), sector convention 0=N..7=NW clockwise.
  `GRASS_SECTOR_SAT=4.0` (`:96`).

## 3. Creature SoA (`src/creature.rs`)

- **`Action`** enum — `creature.rs:15-21`: `#[repr(u8)] Graze=0, Eat=1, Split=2`.
  `Action::ALL: [Action;3]` — `creature.rs:24`.
- **`CreatureSoA` columns** — `creature.rs:31-57` (every column):
  - `id: Vec<u64>`, `x/y: Vec<f32>`, `vx/vy: Vec<f32>`, `energy: Vec<f32>`
  - `age: Vec<u32>`, `digestion_cooldown: Vec<u32>`, `cumulative_upkeep: Vec<f32>`
  - `last_action: Vec<Action>`, `action_this_tick: Vec<Action>`
  - `distance_travelled: Vec<f32>`, `birth_tick: Vec<u32>`, `ticks_since_split: Vec<u32>`
  - `brains: Vec<Brain>` (full per-creature `Brain`, weights inline)
  - **`color_r/color_g/color_b: Vec<f32>`** — action-EMA channels in [0,1],
    init 0.0 at birth (`creature.rs:54-56,118-121`). G=Graze, R=successful bite,
    B=Split (see §4).
- **No genome / no per-creature mutation rate** — D3 deleted genome; mutation
  is policy-driven at birth only (see §5). All body traits are constants.
- `push` sig `creature.rs:95-103`: `(id, x, y, energy, birth_tick, brain)`.
  `remove_indices` swap_remove all columns `creature.rs:128-150`.

## 4. Tick loop (`src/world/mod.rs::step` + `src/world/tick.rs`)

Step order in `World::step` `mod.rs:305-538`, each a `profile_span!`:
1. `tick.grid.rebuild` (`mod.rs:331`) — rebuild spatial hash.
2. `tick.nn` (`mod.rs:341`) — chunked NN forward (`nn_forward_all_chunks`);
   `chunk_ranges(n, dynamic_chunks(n, workers))`.
3. `tick.movement` (`mod.rs:357`) — `apply_movement_and_repulsion`
   (`tick.rs:82-194`): speed-cap `MOVE_SPEED_MAX`, move cost `dist *
   COST_MOVE_PER_DIST * move_cost_multiplier` (`tick.rs:102`), repulsion
   (`REPULSION_K`, clamp `repulsion_max`), wall-clamp to `[ri, WORLD_SIZE-ri]`,
   grid rebuild if `any_moved`.
4. `tick.graze` (`mod.rs:366`) — `graze` (`tick.rs:42-78`), sequential. Uses
   `for_each_cell_overlapping_circle`; per-cell `consume(chunk, per_bite)`.
5. `tick.eat_scavenge` (`mod.rs:372`) — `eat` (`tick.rs:207-326`). **Eat path:**
   parallel scan (`scan_one`) → `EatPick` (`tick.rs:18-29`: `Skip/Miss/Hit{j,transfer}`)
   → sequential apply. Uses `grid.find_first_in_radius(xi, yi, max_range, i, pred)`
   (`tick.rs:267`); `max_range = 2*radius_i`; `transfer = bite_frac *
   prey.energy * (1-armor)`, `armor=0`.
6. `tick.grass_step` (`mod.rs:395`) — `compute_propagation` + `rebuild_row_bitset`
   (+ `grass_step.*` sub-rows).
7. `tick.energy_bookkeeping` (`mod.rs:460`) — `energy_bookkeeping`
   (`tick.rs:328-358`): `up = (UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT +
   mouth_tax) * upkeep_multiplier`; past-lifespan `PAST_LIFESPAN_MULT^(excess/1000)`;
   energy clamped to `energy_max`; cooldown decrement; `age += 1`.
8. `tick.collect_deaths` (`mod.rs:467`) — `energy <= 0` → dead; `remove_indices`
   (also swap_removes `scratch_argmax_pre/got_a_bite/sector_accum`).
9. `tick.handle_births` (`mod.rs:496`) — see §5.
10. `tick.color_ema` (`mod.rs:506`) — `color_ema_update` (`tick.rs:368-395`):
    **G** +0.5 if `argmax_pre==Graze` else −0.05; **B** +1.0 on Split tick else
    −0.01; **R** +1.0 on `got_a_bite` else −0.01; all clamped [0,1]. (Doc
    comment at `tick.rs:361-363` lists stale ±0.01/0.5 values — **code is
    authoritative**.) Display floor 0.15 applied at snapshot write
    (`wasm_api.rs:893-895`).
11. `tick.bookkeeping_tail` (`mod.rs:511`) — promote `last_action ←
    action_this_tick` (memcpy `mod.rs:516`); bump/reset `ticks_since_split`;
    `tick += 1`; world-end check.

Dead-world thin path: `mod.rs:311-327` (grass-only).
`CREATURE_SIZE`/`MOVE_SPEED_MAX`/`COST_MOVE_PER_DIST` used in tick.rs as above;
`energy_cap = sliders.energy_max`.

## 5. Mutation — 8-bucket `MutationPolicy` (`src/brain.rs`)

- Lives in **`src/brain.rs`**, carried on `DevSliders.mutation_policy`
  (`world/mod.rs:28`).
- **`Bucket`** — `brain.rs:265-270`: `{ weight: f32, rate: f32, sigma: f32 }`,
  `Bucket::zero()` `brain.rs:273-279`.
- **`MutationPolicy`** — `brain.rs:285-288`: `buckets: [Bucket;
  MUTATION_BUCKET_COUNT]` where `MUTATION_BUCKET_COUNT = 8` (`constants.rs:82`).
  **`Default`** `brain.rs:290-298`: bucket0 `{1.0, 0.0, 0.0}` (no-mut), bucket1
  `{1.0, 0.05, 0.05}`, bucket2 `{1.0, 0.30, 0.20}`, buckets 3..7 all-zero
  reserves. (Doc string `brain.rs:282-284` says "legacy (0.02,0.02)" — **stale;
  code default differs**.)
- **`Brain::child_from`** — `brain.rs:508-542`. Per birth:
  `total = Σ weight.max(0)`; if `total <= 0` → bit-identical clone (freeze).
  Else weighted draw `u = rng.unit()*total`, cumulative scan picks bucket
  (`brain.rs:520-529`). `p = (chosen.rate * multiplier).clamp(0,1)`,
  `sigma = chosen.sigma`. If both > 0, geometric-skip Gaussian: walk weight
  indices via `rng.geom_skip(p)`, `child.weights[i] += rng.normal()*sigma`
  (`brain.rs:532-540`). `multiplier` = `mutation_rate_multiplier` slider.
- **Birth wiring** — `handle_births` `mod.rs:540-699`: collect splitters
  (`Action::Split` & `energy >= split_threshold`); decide cull vs `active_cap =
  min(max_population, MAX_POP_FOR_SIM)` over virtual pop `n + n_splitters`
  (`mod.rs:571-593`); pre-roll one seed/splitter from `self.rng`
  (`mod.rs:599-604`); **parallel** `Brain::child_from(parent, SimRng::from_u64(seed),
  policy, mut_mult)` (`mod.rs:624-631`); sequential apply pays `split_threshold`,
  gift `clamp(0, split_gift)`, jitter `rng.symm()*split_jitter`, clamp position,
  `push` (`mod.rs:655-670`).
- **Founder seeding (Wave 4 relevant)**: `Brain::founder(rng, topology.clone())`
  per founder in `new_with_sliders_topology` `mod.rs:246-255`. Founders are NOT
  mutated; each gets an independent He-uniform draw from the shared world RNG.

## 6. Constants (`src/constants.rs`) — exact values

All below are `pub const` (compile-time) unless noted.

- `WORLD_SIZE = 1200.0` (`:4`); `BODY_RADIUS_PER_SIZE = 1.0` (`:6`).
- Spatial hash: `HASH_CELL = 2.5` (`:15`), `HASH_DIM = 480` (`:16`, derived).
  (Doc comments in `grid.rs:1-3,17` saying 5u/240/57_600 are **stale** — actual
  is 2.5u / 480 / 230_400 cells.)
- Grass grid: `GRASS_CELL_SIZE = 1.25` (`:127`), `GRASS_GRID_DIM = 960` (`:129`),
  `GRASS_CELL_COUNT = 921_600` (`:131`), `GRASS_MAX = 1.0` (`:133`).
- `GRASS_PROXIMITY_RANGE = 8.0` (`:73`); `PROXIMITY_RANGE = 20.0` (`:72`, the
  "perception range");`WALL_PROXIMITY_RANGE = 50.0` (`:75`).
- `LUT_RADIUS = 16` (in `proximity.rs:89`, not constants.rs).
- `CREATURE_SIZE = 1.0` (`:93`); `MOVE_SPEED_MAX = 5.0` (`:85`).
- **`CREATURE_STRIDE`**: no Rust const by that name. Rust byte stride is
  `SNAPSHOT_CREATURE_STRIDE = 32` (`wasm_api.rs:104`); TS `CREATURE_STRIDE = 8`
  is the **f32-lane** count (`sim-bridge.ts:88`). 8 lanes × 4B = 32B — consistent.
- `MAX_POP_FOR_SIM = 32_000` (`:112`); compile-assert `>= STARTING_POP_DEFAULT*32`
  (`:113`). Soft cap default `MAX_POPULATION_DEFAULT = 8_000` (`:118`).
- Split/energy: `SPLIT_THRESHOLD_DEFAULT = 99.0` (`:27`), `SPLIT_GIFT_MAX_DEFAULT
  = 30.0` (`:26`), `SPLIT_JITTER_DEFAULT = 1.0` (`:97`), `START_ENERGY_DEFAULT =
  200.0` (`:92`), `MAX_AGE_DEFAULT = 5000` (`:94`), `STARTING_POP_DEFAULT = 32`
  (`:99`). DevSliders default `energy_max = 100.0` (`mod.rs:96`).
- Upkeep: `UPKEEP_BASE=0.05`, `UPKEEP_MOUTH_DEFAULT=0.05`, `UPKEEP_GUT=0.02`,
  `UPKEEP_NN_FIXED=0.05` (`:19-22`); `COST_MOVE_PER_DIST=0.02` (`:25`).
- Eat: `EAT_BITE_FRACTION_DEFAULT=1.0` (`:32`) — **note DevSliders default is
  0.5** (`tick.rs` test `p3a`, set via `apply_eat_bite_fraction`; the const is
  unused for the slider default). `DIGESTION_COOLDOWN_TICKS=0` (`:33`).
- Physics: `REPULSION_K=2.0`, `REPULSION_MAX=0.1` (`:88-89`).
- Chunking: `MIN_CHUNKS=4`, `MAX_CHUNKS=16` (`:122-123`).
- Grass defaults: `GRASS_IN_CELL_GROWTH_R_DEFAULT=0.01`,
  `GRASS_PROPAGATION_RATE_K_DEFAULT=0.003`, `GRASS_INITIAL_SEED_COUNT_DEFAULT=8000`,
  `GRASS_ENERGY_PER_BITE_DEFAULT=10.0`, `GRASS_BITES_PER_BLOCK_DEFAULT=2`,
  `FULL_GRASS_ON_INIT_DEFAULT=false` (`:135-149`).

## 7. SpatialGrid (`src/grid.rs`)

- **`SpatialGrid`** `grid.rs:7-25`: `starts: Vec<u32>` (len `HASH_DIM²+1`),
  `indices: Vec<u32>`, `cursors: Vec<u32>`, `cells: Vec<u32>` (per-creature
  cached cell). `new()` `grid.rs:34-43` allocs `HASH_DIM*HASH_DIM+1` starts.
- **`cell_of`** `grid.rs:46-54`: **clamps** ix/iy to `[0, HASH_DIM-1]` (walled,
  no wrap). **`rebuild`** `grid.rs:59-83`: count → prefix-sum → scatter, caches
  `cells[k]`.
- **Queries**: `find_first_in_radius(x,y,radius,rotate_seed,pred)`
  `grid.rs:96-140` (used by eat; rotates start within each cell);
  `for_each_in_radius(x,y,radius,f)` `grid.rs:147-174`. Both **walled /
  non-wrapping** — cells `< 0 || >= dim` are `continue`d (`grid.rs:114,118,
  158,162`). `debug_assert!(radius < WORLD_SIZE*0.5)`.
- **Wrap-awareness would touch**: `cell_of` (clamp→wrap), both query loops
  (skip→wrap index), and every displacement `dx/dy` computed at the call sites
  in `proximity.rs` / `tick.rs::eat`/`apply_movement_and_repulsion` (currently
  raw Euclidean, "D7 walled"). Tests `for_each_in_radius_no_seam_wrap`
  (`grid.rs:303-319`) and others explicitly assert NO seam wrap.

## 8. Grass mechanic (`src/grass.rs`)

- **`GrassGrid`** `grass.rs:23-59`: `density: Vec<f32>` (len 921_600), `scratch`,
  `row_has_density: Vec<u64>` (per-row "any non-empty" bitset, `GRASS_GRID_DIM/64`
  u64s), plus per-tick atomic timers.
- **`compute_propagation(r_in_cell, k_propagate)`** `grass.rs:162-268`: reads
  `density`, writes `scratch`, swaps. Per cell: logistic `r*v*(1-v/MAX)` +
  `k * max(N,S,E,W)` (**max-of-neighbors spill**, `grass.rs:213-218`), ghost-zero
  boundary, clamp `[0,MAX]`. Threaded `par_chunks_mut` (`grass.rs:229-251`) /
  sequential (`:252-265`).
- **`rebuild_row_bitset`** `grass.rs:105-122`; `row_nonempty` `grass.rs:126-132`.
- **`consume(cell, density_chunk, energy_per_bite)`** `grass.rs:318-326`:
  partial-bite — drains `min(density, chunk)`, awards `energy_per_bite *
  taken/chunk`. `density_chunk = GRASS_MAX / bites_per_block` (`tick.rs:50`).
  **This is the "bite-by-density"** (proportional energy, not all-or-nothing).
- **`bilinear_sample`** `grass.rs:274-310` (clamped/ghost-zero).
- **u8 quantization: REMOVED** (v1.11). Grass now ships as raw **f32** per cell
  in the snapshot (`wasm_api.rs:106-108,425-438`); renderer uploads R32F.
- **Dirty tracking: NONE for upload.** The snapshot grass write is a **full**
  contiguous `copy_from_slice` of all 921_600 f32 every tick
  (`wasm_api.rs:434-438`). Only intra-sim sparsity opt is the per-row bitset
  (used by the NN grass-sector scan), not by the SAB upload. → A dirty-region
  grass upload does not exist yet.

## 9. SAB layout (`src/wasm_api.rs` + `src/control_sab.rs`)

**Control SAB** (`control_sab.rs`) — i32 slot indices:
`CTRL_CURRENT_SLOT=0, CTRL_SEQ=1, CTRL_FUTEX=2, CTRL_PAUSED=3,
CTRL_TARGET_TPS_BITS=4, CTRL_CONTROL_EPOCH=5, CTRL_PROFILE_CLEAR_EPOCH=6,
CTRL_RESET_JANK_EPOCH=7, CTRL_PROFILE_WINDOW_MS=8` (`:34-57`);
`CTRL_SLIDERS_BASE=16` (`:63`); inspect req block `64..71`
(`EPOCH=64,KIND=65,WX=66,WY=67,TOL=68,ID_LO=69,ID_HI=70`, `:66-78`); inspect resp
`72/73/74` (`:81-86`); profile report `80/81` (`:89-91`); nn-stats `88/89`
(`:94-96`); `CTRL_I32_REGION_LEN=256` (`:100`). Byte buffers:
`INSPECT_RESP_OFFSET=1024, INSPECT_RESP_CAP=8KB` (`:105-107`);
`PROFILE_REPORT_CAP=16KB` (`:112`); `NN_STATS_CAP=4KB` (`:117`);
`CONTROL_SAB_BYTES = NN_STATS_OFFSET + 4KB + 1024` (`:121`). Compile-asserts
`:125-136`.

**Snapshot SAB** (`wasm_api.rs`): `SNAPSHOT_HEADER_BYTES=32` (`:102`, 20 payload
+12 pad), `SNAPSHOT_CREATURE_STRIDE=32` (`:104`),
`SNAPSHOT_CREATURE_BYTES=MAX_POP*32` (`:106`), `SNAPSHOT_GRASS_BYTES=cells*4`
(`:108`), `SNAPSHOT_SLOT_BYTES` (`:110`), 2 slots (`:113`).
- **Header fields** (LE, slot off 0; `wasm_api.rs:418-423`): `[0..4) tick`,
  `[4..8) pop`, `[8..12) world_ended`, `[12..16) tps_bits`, `[16..20) jank_count`,
  `[20..32) pad`.
- **Creature lane order** (`fill_creature_bytes` `wasm_api.rs:881-912`, 32B/rec):
  `x, y, body_radius, color_r, color_g, color_b, id_lo(u32 bits), id_hi(u32 bits)`.
  Color floored at 0.15 (`:893-895`). Matches TS doc `sim-bridge.ts:84`.
- **Grass region**: raw f32 density, contiguous `copy_from_slice`
  (`wasm_api.rs:434-438`).
- **`boot_ready` payload** (`SimReplyBootReady` `sim-bridge.ts:211-235`, built
  `sim-worker.ts:230-243`): `{world_size, grass_dim, threads, rayon_ok,
  max_pop_for_sim, wasm_memory, snapshot_buf_byte_offset, snapshot_buf_byte_len,
  control_sab, sliders_defaults_json}`. Boot **request** carries
  `nn_topology_json` (`sim-bridge.ts:197-200`).
- **Slider names + `try_set_slider`** (`wasm_api.rs:41-75`, `SLIDER_COUNT=47`):
  indices 0..22 are scalar sliders incl. **`_reserved_legacy_nn_mutation_sigma`(1)**,
  `_reserved_curriculum_*` (17-20), `_reserved_auto_curriculum`(20),
  `full_grass_on_init`(21), `max_population`(22). Indices **23..47 = 8 buckets ×
  {weight,rate,sigma}** (`SLIDER_BUCKET_BASE=23`, `:78`). `try_set_slider`
  `wasm_api.rs:579-587` → `apply_slider_by_index` `:592-625` (bucket dispatch
  `:617-622`). Note: `founder_count` is slider 16 but `grass_initial_seed_count`
  is in `sliders_defaults_json` yet NOT in `SLIDER_NAMES` (construction-only via
  boot payload).
- **Polled reports**: worker writes JSON into byte windows + bumps epoch/len.
  `nn_worker_stats_json` (`wasm_api.rs:864-867`) → `CTRL_NN_STATS_LEN/EPOCH`
  (`sim-worker.ts:359-364`); `profile_report_json` (`wasm_api.rs:794-796`) →
  `CTRL_PROFILE_REPORT_LEN/EPOCH` (`sim-worker.ts:341-353`); inspector
  `creature_inspect_json` (`wasm_api.rs:697-737`) → `CTRL_INSPECT_RESP_LEN/EPOCH`
  (`sim-worker.ts:322-333`). `INSPECT_RESP_CAP = 8KB` (`control_sab.rs:107`).
- **`bindings_in_sync` test** — `src/bin/gen_bindings.rs:132-154`. Compares
  on-disk `web/src/generated/{slider-ids,control-sab}.ts` against the generator
  output (`render_slider_ids` / `render_control_sab`). Covers SLIDER_NAMES,
  SLIDER_COUNT, all CTRL_* slot indices, byte offsets/caps. **Does NOT cover the
  snapshot SAB layout** (CREATURE_STRIDE / header) — those are hand-mirrored in
  `sim-bridge.ts` and guarded only by `max_pop_for_sim` parity + the native
  `write_snapshot_to_layout_matches_stride` test (`wasm_api.rs:1027`).

## 10. World seeding / RNG (`src/rng.rs`, `src/world/mod.rs`)

- **`SimRng`** = xoshiro256++ (`rng.rs:11`); `from_string` hashes via XxHash64
  (`rng.rs:18-22`), `from_u64` via splitmix seed (`rng.rs:14-16`).
- **World RNG seed** is set once from the **string seed** in
  `new_with_sliders_topology` `mod.rs:231-232` (`SimRng::from_string(&seed)`).
  The seed string itself comes from the `WorldHandle::new*` arg; empty → random
  `seed-{hex}` via getrandom (`wasm_api.rs:144-152, 183-190`).
- **There is NO separate `world_seed` numeric concept** — the world seed is the
  string, hashed to u64. One RNG stream drives grass seeding, founder brains +
  placement, and all per-tick stochastic births. (Relevant if v2 wants a numeric
  reproducible-seed surface.)
- **Founders spawned today**: `new_with_sliders_topology` `mod.rs:240-255` —
  `founder_count = clamp(1,32)`, founder energy `min(START_ENERGY_DEFAULT,
  energy_max)`, placement via **Halton(2,3)** low-discrepancy sequence
  (`halton` `mod.rs:114-123`, shift `k+1`), each gets `Brain::founder`. Default
  count `STARTING_POP_DEFAULT=32`; `World::new` test ctor uses 1.

---

## Key risks / notes for v2.0.0

1. **`build_nn_input` lives in `world/nn.rs:377`, not a standalone `nn.rs`** at
   repo root — and its "self/memory" block is *not* a generic memory vector;
   it's current self-state + `last_action` + `ticks_since_split` only. Slots 28
   & 31 are free padding (28 explicitly "reserved for predator-color"). If the
   mission assumes free input slots, **28 and 31 are the only ones**; everything
   else is occupied. Adding inputs means widening `NN_INPUTS` (must stay
   multiple of 8 → next step is 40) and re-baselining `weight_count`, the SIMD
   drift test, and the `nn_input_layout_size_is_32` test (`nn.rs:662`).
2. **`CREATURE_STRIDE` ambiguity**: Rust = 32 (bytes), TS = 8 (f32 lanes). Both
   correct, different units. The 8-lane order `x,y,radius,color_r,color_g,
   color_b,id_lo,id_hi` is full (no spare lanes). Any new per-creature snapshot
   field (e.g. species/genome tag for Wave 2) needs a **stride bump on BOTH
   sides** + the native layout test + the renderer/inspector readers — and there
   is **no `bindings_in_sync` guard** for the snapshot layout, only for the
   control SAB.
3. **Mutation default mismatch with its own doc**: `MutationPolicy::default`
   (`brain.rs:290-298`) is buckets `{1,0,0}/{1,0.05,0.05}/{1,0.30,0.20}`, but the
   doc strings (`brain.rs:25-27,282-284`) still say legacy `(0.02,0.02)`. Trust
   the code. `EAT_BITE_FRACTION_DEFAULT=1.0` const vs DevSliders/UI default 0.5
   is a similar trap — the const is effectively unused for the live default.
4. **Stale spatial-hash docs**: `grid.rs` header + `repository-layout.md` line 36
   say 5u / 240×240 / 57_600. Real values (`constants.rs`): 2.5u / 480×480 /
   230_400. Any plan citing "57,600 cells" or "5u grid" is wrong.
5. **Walled world is pervasive and tested.** Wrap-awareness (if v2 wants a torus)
   touches `grid.rs::cell_of` + both query loops, `compute_wall_proximity`,
   grass ghost-zero boundary, repulsion/eat/move displacement, AND the explicit
   no-wrap tests in `grid.rs`/`grass.rs`/`tick.rs`. Not a localized change.
6. **Grass snapshot upload is full every tick** (921_600 f32 ≈ 3.7 MB) — no
   dirty tracking exists at the SAB boundary. If v2 perf targets bigger grids,
   this is the obvious cost (intra-sim row bitset is for the NN scan only).
7. **`grass_initial_seed_count` + `full_grass_on_init` are construction-only** —
   they ride the boot payload / `set_slider` but only shape the *next* world, not
   the live one. Founder count likewise stored for next construction
   (`apply_founder_count` `wasm_api.rs:538-541`).
8. **Single RNG stream, string-seeded.** No numeric `world_seed`. Founder
   placement is deterministic Halton(2,3); founders are unmutated independent He
   draws. Wave 4 founder-seeding changes should preserve the
   draw-order-determinism (seeds pre-rolled in `handle_births`; founders drawn
   sequentially in `new_with_sliders_topology`).
