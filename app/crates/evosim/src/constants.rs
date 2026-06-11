//! All sim-wide magic numbers, in one place.

// ---- World shape ----
// v2.0 Wave 1a: the world is now *runtime-sized*. `world_size` is a construction
// setting on `DevSliders` (default `WORLD_SIZE_DEFAULT`), not a compile-time
// constant. Every bounds/clamp/wrap/spawn site reads the runtime value off the
// world (`World::world_size()` / `WorldDims`). `wrap_world` (also a construction
// setting) selects torus vs walled behavior. The constants below are *defaults*
// and per-axis cell sizes — the per-axis cell *counts* (`grass_dim`, `hash_dim`)
// are computed at construction from `world_size` and carried in `WorldDims`.
/// Default world size in world-units. v2.0: replaces the old `WORLD_SIZE = 1200`
/// compile-time constant. 9600u at the 5u grass cell → a 1920² grass grid.
pub const WORLD_SIZE_DEFAULT: f32 = 9600.0;
/// Default for the `wrap_world` construction setting. On ⇒ toroidal world
/// (positions wrap to `[0, world_size)`, no wall NN inputs); off ⇒ walled world
/// (positions clamp to `[0, world_size]`, 4 wall-proximity NN inputs present).
pub const WRAP_WORLD_DEFAULT: bool = true;
/// Creature body radius scale in world-units.
pub const BODY_RADIUS_PER_SIZE: f32 = 1.0;

// ---- Spatial hash ----
// HASH_CELL is the only compile-time knob; the per-axis cell count
// `hash_dim = ceil(world_size / HASH_CELL)` is computed at construction
// (`WorldDims::from_world_size`) and no longer divides evenly in general.
//
// v2.0 Wave 1a retune: HASH_CELL bumped 2.5 → 10.0. The world is now 64× larger
// in area (1200²→9600²) at the same creature counts, so creature density per
// world-unit is ~64× sparser. Coarser cells keep the per-tick `hash_dim²`
// rebuild cheap while still bounding neighbor candidates per cell in the sparse
// regime.
//
// v2.0.x perf retune: HASH_CELL 10.0 → 20.0. Native profiling (benches/tick_profile)
// showed the spatial-grid rebuild is the dominant serial tick cost — ~900µs/rebuild,
// run up to 3× per tick — and it is `O(hash_dim²)`, NOT `O(creatures)`: at the 9600u
// default a 960² grid holds ~1500 creatures (0.0016/cell), so the rebuild is pure
// empty-cell array sweeping. Matching the cell to `PROXIMITY_RANGE = 20u` (the widest
// grid query) shrinks the grid to 480² — a 4× cheaper rebuild — while keeping every
// proximity query inside a tight ±1-cell box. The sparse regime keeps per-cell
// occupancy ≪ 1 so the coarser cell does not inflate neighbor-candidate scans.
pub const HASH_CELL: f32 = 20.0;

// ---- Energy economy upkeep ----
pub const UPKEEP_BASE: f32 = 0.05;
pub const UPKEEP_MOUTH_DEFAULT: f32 = 0.05;
pub const UPKEEP_GUT: f32 = 0.02;
pub const UPKEEP_NN_FIXED: f32 = 0.05;

// ---- One-time costs ----
pub const COST_MOVE_PER_DIST: f32 = 0.02;
pub const SPLIT_GIFT_MAX_DEFAULT: f32 = 30.0;
pub const SPLIT_THRESHOLD_DEFAULT: f32 = 99.0;

// ---- Attack ----
/// Default per-bite energy transfer fraction. An attacker removes
/// `eat_bite_fraction * victim.energy` per bite.
pub const EAT_BITE_FRACTION_DEFAULT: f32 = 0.5;
pub const DIGESTION_COOLDOWN_TICKS: u32 = 0;

// ---- Past-lifespan penalty ----
pub const PAST_LIFESPAN_MULT: f32 = 4.0; // per 1000 ticks past max_age

// ---- Brain ----
// NN input semantic layout — see src/world/nn.rs::build_nn_input + NnInputLayout.
//
// BiomeDir(4) + CurrCellPenalty(1) were replaced by a single
//   CurrBiomeType(1) — the raw biome id of the current cell (Plains=0.0,
//   Water=0.5, Desert=1.0). Net change: -4 slots per layout.
//
//   Active layout:
//   SelfMemory(8) + [WallProximity(4)] + CreatureSectors(8/16) + GrassSectors(8)
//   + [GrassBandsFar(8)] + CurrBiomeType(1) + CurrGrass(1) + Bias(1)
//
// Width table (all 8 configs, real → padded):
//   Single-band (grass_multisight=false):
//     wrap/no-species:  27 → 32
//     walled/no-species: 31 → 32
//     wrap/species:     35 → 40
//     walled/species:   39 → 40
//   Multi-band (+8 GrassBandsFar, grass_multisight=true):
//     wrap/no-species:  35 → 40
//     walled/no-species: 39 → 40
//     wrap/species:     43 → 48
//     walled/species:   47 → 48  ← fits now! (was 51→56 before P2)
//
// Max padded width = 48 = MAX_NN_INPUTS. The walled+species+multisight fallback
// is no longer needed: 47 real ≤ 48.
//
// Legacy width-32 slot order (the `NnInputLayout::legacy()` anchor):
//   [0..8)   self/memory (hunger, age_frac, prev_vx, prev_vy, is_last_graze,
//            is_last_eat, ticks_since_split_norm, cooldown_ready)
//   [8..12)  wall_proximity N/S/E/W (range WALL_PROXIMITY_RANGE)
//   [12..20) creature_proximity × 8 world-aligned sectors (range PROXIMITY_RANGE)
//   [20..28) grass_density × 8 sectors (range GRASS_PROXIMITY_RANGE)
//   [28]     padding (0.0) — reserved
//   [29]     curr_grass_density (cells under the body)
//   [30]     1.0 (bias-learning constant)
//   [31]     padding (0.0) — SIMD alignment to 32 = 4 × 8
/// Legacy/default NN input width. Both v2.0 default layouts (26 wrap-on and 30
/// wrap-off real slots) pad up to this SIMD-aligned 32. v2.0: input width is a
/// *runtime* field of `NnTopology` (`brain.rs`), seeded from the active
/// `build_nn_input` group set; this const is the default value and the
/// calibration anchor for the drift-guard test.
pub const NN_INPUTS: usize = 32;
/// Hard upper bound on the runtime NN input width. The per-chunk stack input
/// buffer is sized to this ceiling; the active width (a multiple of 8 in
/// `[8, MAX_NN_INPUTS]`) feeds the first matmul as `fan_in`. Future input
/// groups (wrap walls, biome, 16 creature sectors) widen the active layout up
/// to this cap without changing the buffer size or the SIMD invariants.
pub const MAX_NN_INPUTS: usize = 48;
const _: () = assert!(MAX_NN_INPUTS >= NN_INPUTS);
const _: () = assert!(MAX_NN_INPUTS.is_multiple_of(8));
pub const NN_OUTPUTS: usize = 5; // out[0]=vx, out[1]=vy, out[2..5]=action logits (Graze/Eat/Split)
                                 // v1.12: NN_HIDDEN_1/2 + NN_WEIGHT_COUNT were compile-time constants in
                                 // the legacy 32→48→24→5 topology. They're now derived from the
                                 // runtime `Brain.layer_sizes` (`src/brain.rs`). Legacy default lives in
                                 // `LEGACY_NN_TOPOLOGY` below.
/// Hard upper bound on a single hidden-layer width. Drives the size of the
/// per-chunk stack scratch buffers in the forward pass; the UI clamps width
/// to this value too.
pub const NN_MAX_HIDDEN_WIDTH: usize = 256;
/// Hard upper bound on hidden-layer count. Together with `NN_MAX_HIDDEN_WIDTH`
/// it bounds the per-tick stack scratch and the per-layer profiler counters.
pub const NN_MAX_HIDDEN_LAYERS: usize = 8;
/// `NN_MAX_HIDDEN_LAYERS + 1` matmuls (one per hidden + one for output).
pub const NN_MAX_MATMULS: usize = NN_MAX_HIDDEN_LAYERS + 1;
/// Legacy 32→48→24→5 topology. Used as the default when no
/// `nn_topology` rides the boot payload and as a calibration anchor for the
/// SIMD-vs-scalar drift-guard test.
pub const LEGACY_HIDDEN_SIZES: &[usize] = &[48, 24];
/// Number of angular sectors for creature_proximity and grass_density inputs.
/// World-aligned: 0 = N (0°), 1 = NE (45°), …, 7 = NW (315°).
pub const NN_SECTORS: usize = 8;
/// Range used by creature_proximity and grass_density sector inputs (world-units).
pub const PROXIMITY_RANGE: f32 = 20.0;
/// Range used by the 8 grass-density sector NN inputs (world-units).
///
/// v2.0 Wave 1a: raised 8.0 → 20.0. At the new 5u grass cell (was 1.25u) the old
/// 8u range spanned only ~1.6 cells per sector — too coarse to give each of the
/// 8 sectors a meaningful gradient. 20u spans `ceil(20/5) = 4` grass cells per
/// axis (so each sector sees ≥3 cells), matching `PROXIMITY_RANGE`. The sector
/// LUT radius (`LUT_RADIUS`) is computed from this range and the 5u cell.
pub const GRASS_PROXIMITY_RANGE: f32 = 20.0;
/// Range used by wall_proximity inputs (world-units).
pub const WALL_PROXIMITY_RANGE: f32 = 50.0;
// v1.12: legacy per-layer NN_INIT_RANGE_L1/L2/L3 constants removed. Founder
// init computes He-uniform `r = sqrt(6 / fan_in)` per layer at runtime
// in `Brain::founder`.

// ---- Brain mutation (v1.12 buckets) ----
/// Fixed cap on mutation buckets. SAB-stable; under-used slots have weight=0.
pub const MUTATION_BUCKET_COUNT: usize = 8;

// ---- Trait-range constants still used as named values ----
pub const MOVE_SPEED_MAX: f32 = 5.0;

// ---- Physics ----
pub const REPULSION_K: f32 = 2.0;
pub const REPULSION_MAX: f32 = 0.1;

// ---- Per-creature defaults (D3: genome removed; all creatures share these) ----
pub const START_ENERGY_DEFAULT: f32 = 200.0;
pub const CREATURE_SIZE: f32 = 1.0;
pub const MAX_AGE_DEFAULT: u32 = 5000;
/// Default split-position jitter — children spawn at parent ± random[-v, v]
/// per axis. Live-tunable via DevSliders.split_jitter.
pub const SPLIT_JITTER_DEFAULT: f32 = 1.0;
/// Default number of creatures seeded at world init (multi-founder).
pub const STARTING_POP_DEFAULT: u32 = 32;

/// Sim-side population cap. The cap is a *simulation* invariant — when
/// births would push pop above this number, `World::handle_births` randomly
/// culls existing creatures back to the cap so the sim never holds more state
/// than the snapshot SAB can carry. The two-slot snapshot SAB derives its
/// per-slot creature region size from this constant (`MAX_POP_FOR_SIM × 32 B`
/// each), so the SAB sizing follows the cap, not the other way around.
///
/// The compile-assert below pins this above `STARTING_POP_DEFAULT × 32`,
/// guaranteeing the cap is never tighter than the starting-pop ceiling.
/// The single source of truth lives here; `src/web/src/sim-bridge.ts` mirrors
/// the same value and the worker handshake-asserts they agree.
pub const MAX_POP_FOR_SIM: usize = 32_000;
const _: () = assert!(MAX_POP_FOR_SIM >= STARTING_POP_DEFAULT as usize * 32);

/// Default soft cap for the `max_population` slider. The TS shell may lower
/// this on first boot for low-core devices (see `main.ts`); the hard sim
/// invariant remains `MAX_POP_FOR_SIM`.
pub const MAX_POPULATION_DEFAULT: u32 = 8_000;

// ---- Deterministic chunking ----
/// Chunk-count clamps. Per-tick count = `clamp(pop/32, MIN, min(MAX, workers))`.
pub const MIN_CHUNKS: usize = 4;
pub const MAX_CHUNKS: usize = 16;

// ---- Grass grid ----
// v2.0 Wave 1a: the grass grid is runtime-sized. `GRASS_CELL_SIZE` (the per-axis
// cell width) is the only compile-time knob; the per-axis cell count
// `grass_dim = round(world_size / GRASS_CELL_SIZE)` and `grass_cell_count =
// grass_dim²` are computed at construction (`WorldDims::from_world_size`). At the
// default 9600u world this gives 1920 cells/axis → 3_686_400 cells.
//
// Cell size raised 1.25 → 5.0: the world grew 8× per axis, so a 5u cell keeps the
// grid at 1920² (vs a ruinous 7680² at the old 1.25u). Sim-internal grass density
// stays f32 (dynamics unchanged); only the *snapshot* grass region quantizes to
// u8 (one byte per cell) — see `wasm_api.rs`.
//
// v2.0.4 S2: `grass_cell_size` is now a CONSTRUCTION-ONLY slider (default 5.0).
// Larger cells → fewer cells → less grass_step work (cell count ∝ 1/size²).
// The active value lives on `DevSliders.grass_cell_size` and is read at world
// construction via `WorldDims::from_world_size_with_cell_size`. This constant
// is kept as the default/fallback value so existing call sites (tests, default
// constructors) remain unchanged. Do NOT use it at live grass-dim read sites;
// read `world.dims.grass_cell_size` instead.
/// Default grass cell size in world-units. v2.0.4 S2: still the default; the
/// runtime value is `DevSliders.grass_cell_size` → `WorldDims.grass_cell_size`.
pub const GRASS_CELL_SIZE: f32 = 5.0;
/// Default for the `grass_cell_size` construction slider. Matches `GRASS_CELL_SIZE`.
pub const GRASS_CELL_SIZE_DEFAULT: f32 = GRASS_CELL_SIZE;
/// Maximum density per cell (clamped post-step).
pub const GRASS_MAX: f32 = 1.0;
/// v2.0.3 Stream 2a: maximum number of pyramid levels (L0 alias + owned L1+
/// buffers). The pyramid builder stops at 1×1 or when it would exceed this cap,
/// whichever comes first. At grass_dim = 1920 (default) repeated halving
/// (ceil-divide) yields 11 levels before reaching 1×1 — well under the cap.
/// Raise this only if you need more levels at larger grid sizes; 16 is enough
/// for a 65536² world. Append-only constant — do not remove.
pub const GRASS_PYRAMID_MAX_LEVELS: usize = 16;

/// v2.0.x perf: refresh the grass mip pyramid only every Nth tick instead of
/// every tick. The pyramid is a FULL recompute (`O(grass_cells × 4/3)`) and at
/// large scale (e.g. 3840² = 14.7M cells) costs ~2ms/tick — but it feeds only
/// (a) zoomed-OUT render (the default zoom=1 reads L0 = the live `density`
/// field directly, NOT the pyramid) and (b) far-grass NN sensing at mip L3,
/// neither of which needs single-tick freshness. The pyramid already lags NN by
/// one tick by construction (refresh is step 7b, NN is step 2), so cadence just
/// widens an existing lag to ≤ N ticks. At N=4 this amortizes the ~2ms refresh
/// to ~0.5ms/tick. Tunable; raise for more savings at the cost of staler
/// zoomed-out/far-sense grass.
pub const GRASS_PYRAMID_REFRESH_PERIOD: u32 = 4;
// ---- Biome grass carrying capacity (v2.0 Wave 1) ----
// Per-biome multiplier on `GRASS_MAX` for a cell's grass *carrying capacity*.
// Applied to BOTH the initial seed value and the per-cell logistic/clamp ceiling
// so a cell saturates near `factor * GRASS_MAX` instead of always GRASS_MAX. This
// makes biome the grass-richness selection pressure (a desert looks sparse, a
// water cell is near-barren) without touching the deterministic propagation
// kernel structure. FEEL KNOBS — tune for the desert-as-pressure premise.
//   Plains ×1.0  (== today's behavior; preserves all plains determinism tests)
//   Desert ×0.30 (sparse but grazeable)
//   Water  ×0.04 (near-barren)
/// Grass carrying-capacity factor for Plains cells (× GRASS_MAX). Feel knob.
pub const GRASS_CAPACITY_PLAINS: f32 = 1.0;
/// Grass carrying-capacity factor for Desert cells (× GRASS_MAX). Feel knob.
pub const GRASS_CAPACITY_DESERT: f32 = 0.30;
/// Grass carrying-capacity factor for Water cells (× GRASS_MAX). Feel knob.
pub const GRASS_CAPACITY_WATER: f32 = 0.04;
/// Default in-cell logistic growth rate slider.
pub const GRASS_IN_CELL_GROWTH_R_DEFAULT: f32 = 0.01;
/// Default cross-kernel propagation rate slider.
pub const GRASS_PROPAGATION_RATE_K_DEFAULT: f32 = 0.003;
/// Default number of cells seeded at world init (uniform scatter path, kept for
/// tests that bypass clump seeding). The clump path uses `GRASS_CLUMP_COUNT_DEFAULT`
/// × `GRASS_CLUMP_SIZE_DEFAULT` instead; this constant is the fallback for
/// `full_grass_on_init=false` + `grass_clump_count=0`.
pub const GRASS_INITIAL_SEED_COUNT_DEFAULT: u32 = 8000;
/// Default number of clump centres for seeded-clump grass boot (v2.0.6 S3).
///
/// Budget math (default 9600u world, 5u cells → 1920² = 3,686,400 cells):
///   clump_count=40, clump_size=8 cells → disc area ≈ π×8² ≈ 201 cells/clump
///   Total ≈ 40 × 201 = ~8,040 occupied cells — matches old uniform-scatter
///   budget of 8000 and is well under 1% of the grid so the map looks clumped
///   rather than diffuse. Clumps may overlap; actual count is ≤ 8040.
/// When set to 0 the world boots with the old uniform-scatter behaviour.
pub const GRASS_CLUMP_COUNT_DEFAULT: u32 = 40;
/// Default clump radius (in grass cells) for seeded-clump grass boot (v2.0.6 S3).
/// A disc of radius 8 holds ≈ π×8² ≈ 201 cells, giving ~8,040 total occupied
/// cells across 40 clumps — matching the old 8000-cell uniform-scatter budget.
pub const GRASS_CLUMP_SIZE_DEFAULT: u32 = 8;
/// Default energy gained per successful graze bite. Live-tunable via
/// DevSliders.grass_energy_per_bite.
pub const GRASS_ENERGY_PER_BITE_DEFAULT: f32 = 10.0;
/// Default number of bites required to fully drain a ripe (density == 1.0)
/// grass cell. Density removed per bite = GRASS_MAX / bites_per_block.
/// Live-tunable via DevSliders.grass_bites_per_block.
pub const GRASS_BITES_PER_BLOCK_DEFAULT: u32 = 2;
/// Default for `full_grass_on_init`: when true, world init fills every grass
/// cell to `GRASS_MAX` instead of seeding `grass_initial_seed_count` cells.
pub const FULL_GRASS_ON_INIT_DEFAULT: bool = false;

// ---- Grass SCATTER kernel (v2.0.2 Stream 1b) ----
// Stochastic per-cell scatter propagation that REPLACES the separable blur as the
// live propagation path (the blur is retained behind a selector for the bench).
// Per tick, for each cell that has grass (read from a frozen per-tile source
// snapshot), using a position-addressable `(world_seed, tile, tick)` hash RNG:
//   - DECAY roll: with prob GRASS_DECAY_PCT, subtract GRASS_DECAY_AMOUNT (floor 0).
//   - SPREAD roll: with prob GRASS_SPREAD_PCT, pick ONE target offset from the
//     precomputed DISC table (radial band chosen by the ring1/2/3 weights, angle
//     uniform → round footprint), and add GRASS_SPREAD_AMOUNT to it, clamped to
//     the target cell's biome cap.
// All writes are relaxed load→compute→clamp→store on the AtomicU8 field (cross-
// tile legal; concurrent collisions clobber wholesale — accepted noise).
//
// These are the kernel's working defaults until the six/seven sliders (Stream 1d)
// are wired through DevSliders; the kernel reads slider values when present and
// falls back to these. Tuned per the plan's persistence target: PLAINS
// super-critical (spread-in ≫ decay-out so patches self-sustain), WATER
// sub-critical (cap 0.04 = u8 level 10 throttles reinforcement → thin shoreline).
//
/// Per-tick probability a grass cell rolls a decay event.
/// S4 tuning: raised 0.02 → 0.04. With amount∝density (variant A), dense patch
/// centres reinforce themselves via ring-1-dominant spread (RING1_PCT=0.70), so
/// the field can sustain more living-noise without global monotonic collapse.
/// This value is FEEL-TUNABLE — the persistence invariant (plains super-critical,
/// water sub-critical) is the guard, not the exact number. Adjust freely.
pub const GRASS_DECAY_PCT_DEFAULT: f32 = 0.04;
/// Density subtracted (f32 grass-amount domain) on a decay event; floored at 0.
/// 1/255 ≈ 0.0039 is the u8 quantization floor; this is ~2 bytes so decay is
/// always visible (never stalls sub-quantum).
pub const GRASS_DECAY_AMOUNT_DEFAULT: f32 = 0.008;
/// Per-tick probability a grass cell rolls a spread event onto one disc target.
pub const GRASS_SPREAD_PCT_DEFAULT: f32 = 0.55;
/// Density added to the chosen spread target (f32 grass-amount domain), clamped
/// to the target's biome cap. Larger than the decay amount so plains stays
/// super-critical (a patch refills its own grazed/decayed gaps).
pub const GRASS_SPREAD_AMOUNT_DEFAULT: f32 = 0.05;
/// Spread distance radial-band weights: probability mass for ring 1 / 2 / 3 of the
/// disc table (need not sum to 1 — normalized at table build). Ring 1 dominant so
/// spread is mostly local (tight patches), with a light long tail.
pub const GRASS_SPREAD_RING1_PCT_DEFAULT: f32 = 0.70;
pub const GRASS_SPREAD_RING2_PCT_DEFAULT: f32 = 0.22;
pub const GRASS_SPREAD_RING3_PCT_DEFAULT: f32 = 0.08;
/// Spread reach in cells = the disc table radius. Must be ≤ GRASS_TILE_SIZE so a
/// cross-tile spread lands in an immediate fringe neighbour (the fringe rule keeps
/// that neighbour active next tick). The three radial bands split [1, radius].
pub const GRASS_SPREAD_RADIUS: i32 = 3;

// ---- Multi-band grass sight (v2.0.4 S6) ----
//
// `GrassBandsFar` extends the single near band (GRASS_PROXIMITY_RANGE) with
// a second far band sampled from the LOD pyramid at a coarser mip level.
// Each band still uses NN_SECTORS (8) directional sectors.
//
// Band design:
//   Near: radius = GRASS_PROXIMITY_RANGE (20u) → mip level 0 (raw L0, 5u/cell)
//   Far:  radius = GRASS_FAR_SIGHT_RADIUS (160u) → mip level GRASS_FAR_MIP_LEVEL (3)
//         At L3 the effective cell size is 5u × 2³ = 40u; 4 cells = 160u radius.
//         O(1) per tap — single `sample_clamped` read on the pyramid level.
//
// Default ON (multi-band); single-band variant retained via DevSliders toggle.
// The toggle is construction-scoped (changes the NN input width and discards brains).
//
// MAX_NN_INPUTS constraint: adding 8 extra far-band slots pushes walled+species to
// 51 real → 56 padded, exceeding MAX_NN_INPUTS=48. The multi-band layout therefore
// falls back to single-band when walled + species_mode is on. See nn.rs for the
// 4-way layout table.

/// Far grass sensing radius (world-units). At the default 5u cell and mip level 3
/// (40u/cell), 4 cells out = 160u. This gives creatures far-field awareness of
/// grass density that the near band (20u) completely misses.
pub const GRASS_FAR_SIGHT_RADIUS: f32 = 160.0;

/// Pyramid mip level for the far grass band. Level 3 has effective cell size
/// 5u × 2³ = 40u; sampling 4 cells from the creature gives 160u coverage.
/// This is O(1) per tap regardless of actual cell coverage.
pub const GRASS_FAR_MIP_LEVEL: usize = 3;

/// Default for the `grass_multisight` construction setting. True = multi-band
/// (near+far); false = single-band (legacy near only). Default ON.
pub const GRASS_MULTISIGHT_DEFAULT: bool = true;

// ---- Legacy NN input layout offsets (test-only calibration anchors) ----
// v2.0 Wave 1a: these pin the *legacy* (walled, ReservedPredator-present)
// width-32 slot order. The *active* default layout is now driven by
// construction settings (see `NnInputLayout::for_settings`): the
// `ReservedPredator` group is dropped and `WallProximity` is present only when
// `wrap_world == false`. These constants are referenced only by the
// `NnInputLayout::legacy()` calibration anchor and the nn.rs layout tests, so
// they are gated behind `#[cfg(test)]`.
/// Wall proximity block start (after 8 self-state slots).
#[cfg(test)]
pub const NN_WALL_OFFSET: usize = 8;
/// Creature-proximity sector block start.
#[cfg(test)]
pub const NN_CREATURE_SECTOR_OFFSET: usize = 12;
/// Grass-density sector block start.
#[cfg(test)]
pub const NN_GRASS_SECTOR_OFFSET: usize = 20;
/// Slot for `curr_grass_density` (the density under the creature's body).
#[cfg(test)]
pub const NN_CURR_GRASS_SLOT: usize = 29;
/// Slot pinned to constant 1.0 (bias-learning).
#[cfg(test)]
pub const NN_BIAS_SLOT: usize = 30;

// ---- Biome (v2.0 Wave 1b) ----
/// Per-cell biome tag stored in the sim-side biome grid and per-slot snapshot
/// biome window (one u8 per grass cell). v2.0 Wave 1b generates the grid from
/// `world_seed` (a few large water/desert blobs over a Plains background — see
/// `src/world/biome.rs`). The enum is `#[repr(u8)]` so snapshot bytes map
/// directly.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    Plains = 0,
    Water = 1,
    Desert = 2,
}

// ---- Biome (v2.1 P2) ----
//
// Biomes now do exactly ONE thing: gate grass density via carrying-capacity caps
// (plains ×1.0, desert ×0.30, water ×0.04). All movement/energy penalty effects
// (K_BIOME_SPEED, K_BIOME_COST, K_BIOME_UPKEEP, water/desert penalty sliders)
// have been removed. Creatures learn to avoid dead-grass zones via grass sensing.
//
// The NN biome input is a single compact raw biome-type scalar (`CurrBiomeType`,
// 1 slot) — the normalized biome id of the creature's current cell: Plains=0.0,
// Water=0.5, Desert=1.0. This lets brains directly learn "this cell is barren"
// without any genome modulation.

// ---- Species + sexual mating (v2.0 Wave 3a) ----
//
// `species_mode` is a construction-time toggle (default OFF). OFF = today's
// single-pool asexual `Split` sim, fully unchanged. ON = 10 seeded species +
// sexual `Mate` (same-species, contact-radius, initiator-gated cooldown) +
// Attack-refuses-same-species + 16-sector creature proximity + per-species
// color. Everything below is inert when `species_mode` is off.

/// Default for the `species_mode` construction toggle. OFF ⇒ single-pool asexual.
pub const SPECIES_MODE_DEFAULT: bool = false;

/// Per-sector creature-proximity count when `species_mode` is ON: 8 same-species
/// sectors + 8 other-species sectors = 16. (Single-pool stays at `NN_SECTORS`=8
/// unified.) Plugged into the `CreatureSectors` NN input group width.
pub const NN_CREATURE_SECTORS_SPECIES: usize = 2 * NN_SECTORS;

/// Default number of seeded species in `species_mode`.
pub const STARTING_SPECIES_COUNT_DEFAULT: u32 = 10;
/// Default founders per species (`starting_species_member_count`).
pub const STARTING_SPECIES_MEMBER_COUNT_DEFAULT: u32 = 10;
/// Default per-founder spread multiplier (`starting_species_member_variance`).
/// Applies to per-founder brain spread and lineage-hue drift around a species
/// anchor.
pub const STARTING_SPECIES_MEMBER_VARIANCE_DEFAULT: f32 = 3.0;

/// Default initiator-only post-mating cooldown in ticks. Live-tunable.
///
/// With the mating energy cost = `split_gift` (30) and founders booting at
/// 100 energy, a 200-tick initiator cooldown caps per-initiator birth rate
/// hard enough that the safety population cap should not bind in a default
/// species run. Live-tunable.
pub const MATING_COOLDOWN_TICKS_DEFAULT: u32 = 200;

// ---- Species seeding ----
//
// N-anchor species seeding. All of
// seeding — anchors, canonical hue, founder spread, founder brains — is
// deterministic from `world_seed` via a DEDICATED setup PRNG
// (`SimRng::from_u64(world_seed ^ SEEDING_PRNG_SALT)`), a distinct stream from
// both the biome generator (raw `world_seed` SplitMix64) and the sim RNG
// (string-seeded xoshiro). So the same `world_seed` reproduces the same tick-0
// species layout while the run still varies per launch (sim RNG independent).

/// Salt Xored into `world_seed` to seed the species-seeding PRNG. Picked to be a
/// distinct stream from the biome SplitMix64 (which uses raw `world_seed`) so the
/// two deterministic-from-`world_seed` systems never share a draw sequence.
pub const SEEDING_PRNG_SALT: u64 = 0x5EED_0F00_D1CE_A11E;

/// Minimum spacing between species anchors, as a fraction of `world_size`.
/// Anchors are rejection-sampled to stay at least this far apart so the 10
/// founder clumps occupy visibly distinct patches of the map. Relaxed
/// automatically if too many species are requested to fit (see
/// `pick_anchors`). FEEL KNOB (visual spread of factions).
pub const ANCHOR_MIN_SPACING_FRAC: f32 = 0.12;

/// Founder-cluster radius as a fraction of `world_size`. Non-canonical founders
/// scatter within this radius of their anchor so the clump is contact-mating
/// reachable at tick 0 (a clump wider than mating contact range can't bootstrap).
/// Floored at a few move-steps so a tiny world still clusters. FEEL/BALANCE KNOB.
pub const FOUNDER_CLUSTER_RADIUS_FRAC: f32 = 0.01;

/// Contact-radius padding for mating. `Mate` finds a same-species partner within
/// `(r_initiator + r_partner) * MATING_CONTACT_RADIUS_FACTOR` — essentially
/// touching (summed body radii), NOT the 20u perception range. A factor of 1.0
/// is "bodies overlap"; 1.5 gives a little slack so a clustered founder group can
/// actually reach a partner. Balance knob.
pub const MATING_CONTACT_RADIUS_FACTOR: f32 = 1.5;

/// Default live multiplier on the mating contact radius. The accepted
/// partner distance is `(r_i + r_j) * MATING_CONTACT_RADIUS_FACTOR * this`, so
/// 1.0 reproduces the touching-bodies default and higher values let creatures
/// mate from further apart (toward the perception range). Live-tunable.
pub const MATE_REACH_MULTIPLIER_DEFAULT: f32 = 1.0;

/// Default founder-only multiplier on the Graze (`out[0]`) output-layer weight
/// row. Applied once to founder brains at construction (NOT per birth, which
/// would compound). >1.0 inflates the graze output's variance so it wins the
/// action argmax more often, biasing brand-new creatures toward foraging; 1.0
/// is a no-op. Construction-scoped.
pub const INIT_GRAZE_BOOST_DEFAULT: f32 = 1.0;

/// Default founder-only multiplier on the Split/Mate (`out[2]`) output-layer
/// weight row (same logit slot decodes to Split in single-pool and Mate in
/// species mode). Applied once to founder brains at construction. >1.0 biases
/// new creatures toward splitting/mating; 1.0 is a no-op. Construction-scoped.
pub const INIT_SPLIT_BOOST_DEFAULT: f32 = 1.0;

/// Crossover policy for sexual reproduction (`species_mode` ON). Applied
/// to brain weights before mutation. Default `FiftyFifty` preserves variance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum CrossoverMode {
    /// Per-slot elementwise midpoint of the two parents.
    Average = 0,
    /// Per-slot 50/50 random pick from the two parents.
    FiftyFifty = 1,
}

impl CrossoverMode {
    /// Construction-slider encoding (carried as f32 over the SAB): 0 = average,
    /// non-zero = fifty_fifty. Default is FiftyFifty.
    #[inline]
    pub fn from_slider(value: f32) -> Self {
        if value == 0.0 {
            CrossoverMode::Average
        } else {
            CrossoverMode::FiftyFifty
        }
    }
    /// Slider value (f32) for this mode.
    #[inline]
    pub fn to_slider(self) -> f32 {
        match self {
            CrossoverMode::Average => 0.0,
            CrossoverMode::FiftyFifty => 1.0,
        }
    }
}

/// Default crossover mode (construction setting).
pub const CROSSOVER_MODE_DEFAULT: CrossoverMode = CrossoverMode::FiftyFifty;

// ---- v2.1 P3: food-limited equilibrium knobs ----
//
// Five complementary mechanisms that together replace the random cull as the
// primary population regulator:
//
//   1. CROWDING MORTALITY — per-tick extra energy upkeep proportional to the
//      number of neighbours within `CROWDING_RADIUS`.  Uses the existing
//      spatial-hash lookup; no second O(n²) pass.
//   2. STARVATION DRAIN — below `STARVATION_THRESHOLD` energy, creatures pay an
//      extra `STARVATION_DRAIN_RATE` per tick on top of idle upkeep.  Death still
//      occurs at energy ≤ 0.  Selects against bad foragers.
//   3. GRASS CARRYING-CAPACITY SCALE — a live multiplier on every cell's capacity
//      cap (all biomes).  Scaling down permanently caps total food and therefore
//      population at equilibrium.
//   4. GRASS REGROWTH RATE — multiplier on both scatter-spread probability and
//      in-cell growth rate.  Dialled down to limit total food throughput.
//   5. SPLIT GIFT — the existing birth-energy cost (`split_gift`) is surfaced
//      prominently in the Equilibrium category; its constant lives above at
//      SPLIT_GIFT_MAX_DEFAULT.
//
// These default values were chosen via the headless equilibrium harness
// (world/equilibrium_tests.rs).  K=4 seeds × T=10_000 ticks; pop settled in
// the band [180, 2 800], mean ≈ 840; foraging proxy (avg energy-per-lifespan)
// rose ~25 % from first to last generation window; backstop cull did NOT fire.

/// Extra energy drain per neighbour per tick when a creature is crowded.
/// Crowding cost = crowding_strength × neighbour_count (within crowding_radius).
///
/// Tuned so that a creature surrounded by ~10 neighbours pays 0.01×10 = 0.10
/// extra upkeep per tick — roughly equal to 60 % of the base idle upkeep
/// (0.17/tick) — meaningfully penalising crowded patches without instantly
/// killing lightly-clustered creatures.
pub const CROWDING_STRENGTH_DEFAULT: f32 = 0.010;
/// Spatial radius (world-units) within which neighbours are counted for the
/// crowding upkeep.  Reuses the existing spatial-hash; must be ≤ PROXIMITY_RANGE
/// so the hash-cell size (HASH_CELL = 20u) is the only cap on candidate count.
pub const CROWDING_RADIUS_DEFAULT: f32 = 20.0;
/// Energy level below which `STARVATION_DRAIN_RATE` kicks in.  Creatures whose
/// energy falls below this threshold pay extra drain each tick.
pub const STARVATION_THRESHOLD_DEFAULT: f32 = 15.0;
/// Extra per-tick energy drain applied when a creature's energy is below
/// `STARVATION_THRESHOLD`.  Stacks on top of idle upkeep; does NOT itself kill —
/// energy ≤ 0 triggers death as usual.
///
/// At 0.30/tick, a creature with no food drops from 15 to 0 in 50 ticks
/// (on top of idle upkeep), giving a clear selection window where good
/// foragers recover and poor ones starve.
pub const STARVATION_DRAIN_RATE_DEFAULT: f32 = 0.30;
/// Live multiplier on every grass cell's carrying-capacity cap.  At 1.0 the
/// caps are unchanged (Plains=1.0, Desert=0.30, Water=0.04).  Scaling down
/// permanently limits total food and therefore equilibrium population.
///
/// 0.7 reduces all caps by 30 %, tightening the food ceiling so equilibrium
/// pop settles comfortably below the `max_population` slider.
pub const GRASS_CAPACITY_SCALE_DEFAULT: f32 = 0.7;
/// Live multiplier on both the scatter-spread probability (`grass_spread_pct`)
/// and the in-cell growth rate (`grass_in_cell_growth_r`).  Values < 1.0 slow
/// regrowth and tighten the food ceiling; values > 1.0 enrich the world.
pub const GRASS_REGROWTH_RATE_DEFAULT: f32 = 1.0;

/// Runtime world dimensions, computed once at construction from `world_size`.
///
/// v2.0 Wave 1a: replaces the old compile-time `WORLD_SIZE` / `HASH_DIM` /
/// `GRASS_GRID_DIM` / `GRASS_CELL_COUNT` constants. A small `Copy` value carried
/// on `World`, `GrassGrid`, and `SpatialGrid` so every bounds/clamp/wrap/spawn
/// site reads the same derived dims. The full grass field and sim-side biome
/// grid are both sized from `grass_cell_count`; snapshots carry runtime-sized
/// grass/biome windows.
///
/// v2.0.4 S2: `grass_cell_size` added. The construction slider default is
/// `GRASS_CELL_SIZE_DEFAULT` (5.0); the LOD computation in `wasm_api.rs` and
/// `biome_cell_index` in `world/mod.rs` both read this field so they honour
/// whatever cell size was chosen at construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldDims {
    /// World extent in world-units (square world, `[0, world_size]`).
    pub world_size: f32,
    /// Whether the world is toroidal (wrap) or walled (clamp).
    pub wrap_world: bool,
    /// Grass cells per axis: `round(world_size / grass_cell_size)`.
    pub grass_dim: usize,
    /// Total grass cells: `grass_dim²`.
    pub grass_cell_count: usize,
    /// Spatial-hash cells per axis: `ceil(world_size / HASH_CELL)`.
    pub hash_dim: usize,
    /// Grass cell size in world-units used at construction. Default
    /// `GRASS_CELL_SIZE_DEFAULT` (5.0); adjustable via the `grass_size` slider.
    /// Stored so the LOD computation and biome-cell lookup use the correct value
    /// throughout the world's lifetime.
    pub grass_cell_size: f32,
}

impl WorldDims {
    /// Compute runtime dims from a world size + wrap flag, using the default
    /// `GRASS_CELL_SIZE` (5.0). All existing test/default-construction call
    /// sites use this overload. Clamps `world_size` to a sane floor so a
    /// degenerate construction setting can't produce a zero-dim grid.
    pub fn from_world_size(world_size: f32, wrap_world: bool) -> Self {
        Self::from_world_size_with_cell_size(world_size, wrap_world, GRASS_CELL_SIZE)
    }

    /// Compute runtime dims with an explicit grass cell size. Used by the
    /// `grass_size` construction slider path. `cell_size` is floored at 1.0 to
    /// prevent a zero or negative cell size from producing a degenerate grid.
    pub fn from_world_size_with_cell_size(
        world_size: f32,
        wrap_world: bool,
        cell_size: f32,
    ) -> Self {
        let cs = cell_size.max(1.0);
        // Floor world_size at one grass cell + one hash cell so dims are always ≥ 1.
        let ws = world_size.max(cs.max(HASH_CELL));
        let grass_dim = (ws / cs).round() as usize;
        let grass_dim = grass_dim.max(1);
        let grass_cell_count = grass_dim * grass_dim;
        let hash_dim = (ws / HASH_CELL).ceil() as usize;
        let hash_dim = hash_dim.max(1);
        Self {
            world_size: ws,
            wrap_world,
            grass_dim,
            grass_cell_count,
            hash_dim,
            grass_cell_size: cs,
        }
    }
}

#[cfg(test)]
mod constant_tests {
    use super::*;

    /// The retuned proximity/cell constants the LUT + starburst are built from.
    /// v2.0 Wave 1a: GRASS_PROXIMITY_RANGE=20, GRASS_CELL_SIZE=5 → 4 cells/axis.
    #[test]
    fn grass_proximity_spans_at_least_three_cells() {
        let cells_per_axis = (GRASS_PROXIMITY_RANGE / GRASS_CELL_SIZE).ceil() as usize;
        assert!(
            cells_per_axis >= 3,
            "each grass sector must span ≥3 cells at the 5u cell; got {cells_per_axis}"
        );
        assert_eq!(cells_per_axis, 4, "expected ceil(20/5) = 4 cells per axis");
    }

    /// Default world size derives the documented dims: 9600 → 1920 → 3_686_400.
    #[test]
    fn default_world_dims_match_spec() {
        let d = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
        assert_eq!(d.grass_dim, 1920);
        assert_eq!(d.grass_cell_count, 3_686_400);
        assert_eq!(d.hash_dim, 480); // ceil(9600 / 20)
    }
}
