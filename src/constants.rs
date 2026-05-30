//! All sim-wide magic numbers, in one place.

// ---- World shape ----
pub const WORLD_SIZE: f32 = 1200.0;
/// Creature body radius scale in world-units.
pub const BODY_RADIUS_PER_SIZE: f32 = 1.0;

// ---- Spatial hash ----
// HASH_CELL is the only knob; HASH_DIM is derived. Constraint: WORLD_SIZE / HASH_CELL
// must be an exact integer (e.g. 2.5 → 480, 2.0 → 600, 5.0 → 240).
// Smaller cells = fewer creatures per cell = cheaper neighbor queries in clumps,
// at the cost of larger `starts`/`cursors` allocations (HASH_DIM² entries).
// Picked 2.5u so eat/repulsion (radius 2u) hit ~1-4 cells with ~4× fewer candidates
// per cell than the legacy 5u grid.
pub const HASH_CELL: f32 = 2.5;
pub const HASH_DIM: usize = (WORLD_SIZE / HASH_CELL) as usize; // 480

// ---- Energy economy upkeep ----
pub const UPKEEP_BASE: f32 = 0.05;
pub const UPKEEP_MOUTH_DEFAULT: f32 = 0.05;
pub const UPKEEP_GUT: f32 = 0.02;
pub const UPKEEP_NN_FIXED: f32 = 0.05;

// ---- One-time costs ----
pub const COST_MOVE_PER_DIST: f32 = 0.02;
pub const SPLIT_GIFT_MAX_DEFAULT: f32 = 30.0;
pub const SPLIT_THRESHOLD_DEFAULT: f32 = 50.0;

// ---- Eat ----
/// Default per-bite energy transfer fraction. Predator removes
/// `eat_bite_fraction * prey.energy * (1 - prey.armor)` per bite.
pub const EAT_BITE_FRACTION_DEFAULT: f32 = 0.5;
pub const DIGESTION_COOLDOWN_TICKS: u32 = 50;

// ---- Past-lifespan penalty ----
pub const PAST_LIFESPAN_MULT: f32 = 4.0; // per 1000 ticks past max_age

// ---- Curriculum ----
// Cost factor = MIN_FACTOR + (1 - MIN_FACTOR) * smoothstep(min_pop, max_pop, pop).
// Floor defaults to 0.0 (user-tunable up to 1.0); below min_pop costs vanish so
// fragile early populations stop bleeding upkeep before selection has anything
// to grip.
pub const CURRICULUM_MIN_FACTOR_DEFAULT: f32 = 0.0;
pub const CURRICULUM_MIN_POP_DEFAULT: u32 = 1000;
pub const CURRICULUM_MAX_POP_DEFAULT: u32 = 2000;

// ---- Brain ----
// 32-input semantic layout — see src/world/nn.rs::build_nn_input.
//   [0..8)   self/memory (hunger, age_frac, prev_vx, prev_vy, is_last_graze,
//            is_last_eat, ticks_since_split_norm, cooldown_ready)
//   [8..12)  wall_proximity N/S/E/W (range WALL_PROXIMITY_RANGE)
//   [12..20) creature_proximity × 8 world-aligned sectors (range PROXIMITY_RANGE)
//   [20..28) grass_density × 8 sectors (range PROXIMITY_RANGE)
//   [28]     padding (0.0) — reserved
//   [29]     curr_grass_density (cells under the body)
//   [30]     1.0 (bias-learning constant)
//   [31]     padding (0.0) — SIMD alignment to 32 = 4 × 8
pub const NN_INPUTS: usize = 32;
pub const NN_HIDDEN_1: usize = 48;
pub const NN_HIDDEN_2: usize = 24;
pub const NN_OUTPUTS: usize = 5; // out[0]=vx, out[1]=vy, out[2..5]=action logits (Graze/Eat/Split)
pub const NN_WEIGHT_COUNT: usize =
    NN_INPUTS * NN_HIDDEN_1 + NN_HIDDEN_1 * NN_HIDDEN_2 + NN_HIDDEN_2 * NN_OUTPUTS;
const _: () = assert!(NN_WEIGHT_COUNT == 2808);
/// Number of angular sectors for creature_proximity and grass_density inputs.
/// World-aligned: 0 = N (0°), 1 = NE (45°), …, 7 = NW (315°).
pub const NN_SECTORS: usize = 8;
/// Range used by creature_proximity and grass_density sector inputs (world-units).
pub const PROXIMITY_RANGE: f32 = 20.0;
pub const GRASS_PROXIMITY_RANGE: f32 = 8.0;
/// Range used by wall_proximity inputs (world-units).
pub const WALL_PROXIMITY_RANGE: f32 = 50.0;
pub const NN_MUT_RATE_DEFAULT: f32 = 0.02;
pub const NN_MUT_SIGMA_DEFAULT: f32 = 0.02;
// Per-layer He-uniform init ranges r = sqrt(6/fan_in).
pub const NN_INIT_RANGE_L1: f32 = 0.433;
pub const NN_INIT_RANGE_L2: f32 = 0.354;
pub const NN_INIT_RANGE_L3: f32 = 0.500;

// ---- Brain mutation ----
pub const MUT_RATE_OF_RATES: f32 = 0.005;
pub const MUT_RATE_JITTER: f32 = 0.20;

// ---- Trait-range constants still used as named values ----
pub const MOVE_SPEED_MAX: f32 = 5.0;

// ---- Physics ----
pub const REPULSION_K: f32 = 2.0;
pub const REPULSION_MAX: f32 = 5.0;

// ---- Per-creature defaults (D3: genome removed; all creatures share these) ----
pub const START_ENERGY_DEFAULT: f32 = 200.0;
pub const CREATURE_SIZE: f32 = 1.0;
pub const MAX_AGE_DEFAULT: u32 = 5000;
/// Default split-position jitter — children spawn at parent ± random[-v, v]
/// per axis. Live-tunable via DevSliders.split_jitter.
pub const SPLIT_JITTER_DEFAULT: f32 = 50.0;
/// Default number of creatures seeded at world init (multi-founder).
pub const STARTING_POP_DEFAULT: u32 = 8;

/// Sim-side population cap. The cap is a *simulation* invariant — when
/// births would push pop above this number, `World::handle_births` randomly
/// culls existing creatures back to the cap so the sim never holds more state
/// than the snapshot SAB can carry. The two-slot snapshot SAB derives its
/// per-slot creature region size from this constant (`MAX_POP_FOR_SIM × 32 B`
/// each), so the SAB sizing follows the cap, not the other way around.
///
/// The compile-assert below pins this above `STARTING_POP_DEFAULT × 32`,
/// guaranteeing the cap is never tighter than the starting-pop ceiling.
/// The single source of truth lives here; `web/src/sim-bridge.ts` mirrors
/// the same value and the worker handshake-asserts they agree.
pub const MAX_POP_FOR_SIM: usize = 32_000;
const _: () = assert!(MAX_POP_FOR_SIM >= STARTING_POP_DEFAULT as usize * 32);

// ---- Deterministic chunking ----
/// Chunk-count clamps. Per-tick count = `clamp(pop/32, MIN, min(MAX, workers))`.
pub const MIN_CHUNKS: usize = 4;
pub const MAX_CHUNKS: usize = 16;

// ---- Grass grid ----
/// Cell size in world-units. World is 1200u → 960 cells per axis.
pub const GRASS_CELL_SIZE: f32 = 1.25;
/// Number of grass cells per axis (960).
pub const GRASS_GRID_DIM: usize = (WORLD_SIZE / GRASS_CELL_SIZE) as usize; // 960
/// Total grass cells (921_600).
pub const GRASS_CELL_COUNT: usize = GRASS_GRID_DIM * GRASS_GRID_DIM; // 921_600
/// Maximum density per cell (clamped post-step).
pub const GRASS_MAX: f32 = 1.0;
/// Default in-cell logistic growth rate slider.
pub const GRASS_IN_CELL_GROWTH_R_DEFAULT: f32 = 0.05;
/// Default cross-kernel propagation rate slider.
pub const GRASS_PROPAGATION_RATE_K_DEFAULT: f32 = 0.001;
/// Default number of cells seeded at world init.
pub const GRASS_INITIAL_SEED_COUNT_DEFAULT: u32 = 100;
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

// ---- NN input layout offsets ----
/// Wall proximity block start (after 8 self-state slots).
pub const NN_WALL_OFFSET: usize = 8;
/// Creature-proximity sector block start.
pub const NN_CREATURE_SECTOR_OFFSET: usize = 12;
/// Grass-density sector block start.
pub const NN_GRASS_SECTOR_OFFSET: usize = 20;
/// Slot for `curr_grass_density` (the density under the creature's body).
pub const NN_CURR_GRASS_SLOT: usize = 29;
/// Slot pinned to constant 1.0 (bias-learning).
pub const NN_BIAS_SLOT: usize = 30;
