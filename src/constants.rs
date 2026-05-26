//! All v5+v6 magic numbers, in one place. Reference each with the spec
//! section number so future readers can audit against the pitch.

// ---- World shape (v5 §3.3, v6 §D) ----
pub const WORLD_SIZE: f32 = 600.0;
/// Creature body radius in world-units per genome.size unit. (S3 — moved from world/mod.rs)
pub const BODY_RADIUS_PER_SIZE: f32 = 1.0;

// ---- Spatial hash (v5 §3.3) ----
pub const HASH_CELL: f32 = 5.0;
pub const HASH_DIM: usize = (WORLD_SIZE / HASH_CELL) as usize; // 120

// ---- Energy economy upkeep (v5 §7) ----
pub const UPKEEP_BASE: f32 = 0.05;
pub const UPKEEP_SIZE_PER_UNIT: f32 = 0.04;
pub const UPKEEP_MOBILITY_FLAG: f32 = 0.10;
pub const UPKEEP_MOVE_SPEED_PER_UNIT: f32 = 0.02;
pub const UPKEEP_PER_EYE: f32 = 0.02;
pub const UPKEEP_VISION_COEFF: f32 = 0.001;
pub const UPKEEP_MOUTH_DEFAULT: f32 = 0.05; // slider in v6 §K
pub const UPKEEP_GUT: f32 = 0.02;
pub const UPKEEP_ARMOR_PER_UNIT: f32 = 0.03;
pub const UPKEEP_LIFESPAN_PER_1K: f32 = 0.01;
pub const UPKEEP_NN_FIXED: f32 = 0.05;

// ---- One-time costs (v5 §7) ----
pub const COST_EAT_ATTEMPT: f32 = 0.3;
pub const COST_SCAVENGE_ATTEMPT: f32 = 0.1;
pub const COST_MOVE_PER_DIST: f32 = 0.02;
#[allow(dead_code)]
pub const COST_SPLIT: f32 = 50.0;
pub const SPLIT_GIFT_MAX: f32 = 30.0;
pub const SPLIT_THRESHOLD: f32 = 50.0;

// ---- Eat / scavenge (v5 §3.5) ----
/// Default per-bite energy transfer fraction. Predator removes
/// `eat_bite_fraction * prey.energy * (1 - prey.armor)` per bite.
/// Live-tunable via DevSliders.eat_bite_fraction (P3b adds slider).
pub const EAT_BITE_FRACTION_DEFAULT: f32 = 0.5;
pub const SCAVENGE_GAIN_COEFF: f32 = 1.5;
pub const DIGESTION_COOLDOWN_TICKS: u32 = 50;

// ---- Death + carrion (v5 §8) ----
pub const CARRION_POOL_COEFF: f32 = 0.3; // pool = coeff × cumulative_upkeep
pub const CARRION_POOL_CAP: f32 = 60.0;
pub const CARRION_MAX_AGE: u32 = 100;

// ---- Past-lifespan penalty (v5 §7) ----
pub const PAST_LIFESPAN_MULT: f32 = 4.0; // per 1000 ticks past max_age

// ---- Brain (v1.3 §D9) ----
// Layout: [0..5) self-state, [5..77) vision 24×3, [77..80) last_action 3,
//         [80..105) grass-patch 5×5, [105..112) padding zeros.
pub const NN_INPUTS: usize = 112; // v1.3: 5 self-state + 72 vision + 3 last_action + 25 grass-patch + 7 pad
pub const NN_HIDDEN: usize = 24;
pub const NN_OUTPUTS: usize = 5; // out[0]=vx, out[1]=vy, out[2..5]=action logits (Graze/Eat/Split)
pub const NN_WEIGHT_COUNT: usize = NN_INPUTS * NN_HIDDEN + NN_HIDDEN * NN_OUTPUTS;
pub const NN_MUT_RATE_DEFAULT: f32 = 0.02;
pub const NN_MUT_SIGMA_DEFAULT: f32 = 0.02;
pub const NN_INIT_RANGE: f32 = 0.3; // uniform random weight initialisation range

// ---- Body mutation (v5 §6) ----
pub const BODY_MUT_RATE_DEFAULT: f32 = 0.03;
pub const MUT_RATE_OF_RATES: f32 = 0.005;
pub const MUT_RATE_JITTER: f32 = 0.20;

// ---- Genome bounds (v5 §4) ----
pub const SIZE_MIN: f32 = 1.0;
pub const SIZE_MAX: f32 = 10.0;
pub const MAX_AGE_MIN: u32 = 100;
pub const MAX_AGE_MAX: u32 = 50_000;
pub const GRAZE_EFF_MIN: f32 = 0.0;
pub const GRAZE_EFF_MAX: f32 = 2.0;
pub const EAT_EFF_MIN: f32 = 0.0;
pub const EAT_EFF_MAX: f32 = 2.0;
pub const SCAVENGE_EFF_MIN: f32 = 0.0;
pub const SCAVENGE_EFF_MAX: f32 = 2.0;
pub const MOVE_SPEED_MIN: f32 = 0.0;
pub const MOVE_SPEED_MAX: f32 = 5.0;
pub const VISION_RANGE_MIN: f32 = 0.0;
pub const VISION_RANGE_MAX: f32 = 80.0;
pub const ARMOR_MIN: f32 = 0.0;
pub const ARMOR_MAX: f32 = 1.0;
pub const BITE_REACH_MIN: f32 = 1.0;
pub const BITE_REACH_MAX: f32 = 3.0;
pub const EYE_SLOTS: usize = 24;
pub const EYE_VALID: [u8; 8] = [0, 2, 3, 4, 6, 8, 12, 24];
pub const NOSE_VALID: [u8; 6] = [0, 1, 2, 3, 4, 5];

// ---- Physics (v6 §D) ----
pub const REPULSION_K: f32 = 2.0;
pub const REPULSION_MAX: f32 = 5.0;

// ---- Founder (Milestone B default; mutation kicks in C) ----
pub const FOUNDER_ENERGY: f32 = 200.0;
pub const FOUNDER_SIZE: f32 = 1.0;
pub const FOUNDER_MAX_AGE: u32 = 5000;
pub const FOUNDER_GRAZE_EFF: f32 = 1.0;
pub const FOUNDER_BITE_REACH: f32 = 1.0;
pub const FOUNDER_PIGMENT_R: f32 = 0.30;
pub const FOUNDER_PIGMENT_G: f32 = 0.75;
pub const FOUNDER_PIGMENT_B: f32 = 0.35;
pub const FOUNDER_SPLIT_JITTER: f32 = 50.0;

// ---- Species detection (v6 §H) ----
pub const SPECIES_W_BODY: f32 = 3.0;
pub const SPECIES_W_BRAIN: f32 = 1.0;
pub const SPECIES_THRESHOLD: f32 = 4.0; // v6 §H default; v1.1 raised to 6.0 for sun-driven fast drift, v1.2 reverts (slower grass-driven drift won't trigger 6.0 in 10k ticks). See DECISIONS v1.2 PR-1 regen.
pub const SPECIES_EYE_JUMP_COST: f32 = 1.5;

// ---- NN input normalization (v6 §E, §3; see DECISIONS for unspecified bases) ----
/// Normalization base for carrion_overlap_norm NN input (v6 §3; no canonical base in spec).
/// 4 overlapping corpses ≈ "corpse-rich zone". DECISIONS: carrion_overlap_norm base.
pub const CARRION_OVERLAP_NORM_BASE: f32 = 4.0;

// ---- Vision (v5 §10, v6 §E, Milestone C.12) ----
/// Fixed radius used when a ray hits a carrion blob (not in spec — DECISIONS).
pub const CARRION_RADIUS_FOR_VISION: f32 = 1.5;
/// Maximum grid cells walked per ray; bounds worst-case DDA cost (v5 §3.6).
/// vision_range_max = 80u; HASH_CELL = 5u → ~16 cells along a ray axis.
/// Worst diagonal ≈ 32; double for slack.
pub const MAX_CELLS_PER_RAY: usize = 64;

// ---- Deterministic chunking (v6 §J) ----
/// Fixed number of chunks for deterministic parallelism. Results identical
/// regardless of actual core count (v6 §J). Pinned at 8 (matches typical
/// core counts; v6 §J example value).
pub const N_CHUNKS: usize = 8;

// ---- Population milestones (v5 §11, E.25.a) ----
/// Thresholds that fire a PopulationMilestone event once per threshold ever (v5 §11).
pub const POPULATION_MILESTONES: [u32; 6] = [10, 50, 100, 500, 1000, 2000];

// ---- Vision geometry (v5 §3.5, v6 §E; perf-1 sector sin/cos cache) ----
/// Number of vision sectors per creature. Matches EYE_SLOTS; kept in both
/// vision.rs (re-export) and constants.rs (source of truth for creature.rs).
pub const SECTORS: usize = 24;
/// Lookup: index = position of eye_count in EYE_VALID; value = sector stride.
/// EYE_VALID = [0, 2, 3, 4, 6, 8, 12, 24] → strides [-, 12, 8, 6, 4, 3, 2, 1].
pub const EYE_STRIDE: [u8; 8] = [0, 12, 8, 6, 4, 3, 2, 1];

// ---- Persistence (v5 §13, F.26/F.28) ----
/// Ticks per "day" — used by resume prompt + eulogy card day count (F.26, F.28).
/// At sim 1× (30 ticks/s), one "day" ≈ 33 wall-seconds. v5 §11 + F.26 DECISIONS.
pub const DAY_TICKS: u32 = 1000;

// ---- Grass grid (v1.2 grass mechanic) ----
/// Cell size in world-units. World is 600u → 240 cells per axis. See v1.2 grass
/// mechanic brief §Grass storage.
pub const GRASS_CELL_SIZE: f32 = 2.5;
/// Number of grass cells per axis (240). See v1.2 grass mechanic brief.
pub const GRASS_GRID_DIM: usize = (WORLD_SIZE / GRASS_CELL_SIZE) as usize; // 240
/// Total grass cells (57_600). See v1.2 grass mechanic brief.
pub const GRASS_CELL_COUNT: usize = GRASS_GRID_DIM * GRASS_GRID_DIM; // 57_600
/// Maximum density per cell (clamped post-step). See v1.2 grass mechanic brief.
pub const GRASS_MAX: f32 = 1.0;
/// Default in-cell logistic growth rate slider. See v1.2 grass mechanic brief §Grass dynamics.
/// Raised 0.005 → 0.01 (P1h lever 3) → 0.05 (P2h tuning) — PR-2 founders don't auto-Graze,
/// so grass must spread fast enough that random-walking creatures encounter grass cells.
pub const GRASS_IN_CELL_GROWTH_R_DEFAULT: f32 = 0.05;
/// Default cross-kernel propagation rate slider. See v1.2 grass mechanic brief §Grass dynamics.
/// Raised 0.05 → 0.1 (P2h tuning) for faster propagation across the world.
pub const GRASS_PROPAGATION_RATE_K_DEFAULT: f32 = 0.1;
/// Default number of cells seeded at world init. See v1.2 grass mechanic brief §Initial grass seed.
/// Raised from 8 to 50 (P1h ladder lever 1) → 100 (P2h ladder lever 2) — PR-2 founders
/// no longer benefit from the old NN_FOUNDER_PHOTO_BIAS-style Graze bias (F.30 rewrite),
/// so the lineage needs proportionally more initial food to survive random-walk-to-grass.
pub const GRASS_INITIAL_SEED_COUNT_DEFAULT: u32 = 100;
/// Per-cell, per-tick cap on the delta a single creature can drain from a single grass cell
/// (before graze_efficiency multiplier). P1h regen iterations:
///   - 0.1 (initial lock): extinct tick 358 (gain 0.10/tick < upkeep 0.15/tick).
///   - 0.25 (§A.1 escape clause): extinct tick 5740 (sustainable until compound past-lifespan
///     upkeep cliff hits and population can't replace through splits fast enough).
///   - 0.5 (P1h iteration): targets pop>0 at T=10000 by giving splits enough buffer to
///     outrun the past-lifespan upkeep cliff.
pub const GRAZE_MAX_PER_TICK: f32 = 0.4;

// ---- NN input layout offsets (v1.3 D9) ----
/// Self-state block length: [energy_frac, age_frac, prev_vx, prev_vy, cooldown_frac].
pub const NN_SELF_STATE_LEN: usize = 5;
/// Vision passthrough block start. 5 self-state slots before.
pub const NN_VISION_OFFSET: usize = 5;
/// Vision block length: 24 sectors × 3 RGB features.
/// NOTE: vision.rs currently emits 24×5=120 floats (Q3 not yet landed).
/// If VISION_LEN==120, slots [77..80) overlap; will be fixed when Q3 lands.
pub const NN_VISION_LEN: usize = 72;
/// Last-action one-hot block start. After vision block (5 + 72 = 77).
pub const NN_LAST_ACTION_OFFSET: usize = 77;
/// Last-action block length: 3 (Graze, Eat, Split).
pub const NN_LAST_ACTION_LEN: usize = 3;
/// Grass-patch 5×5 block start (77 + 3 = 80).
pub const NN_GRASS_PATCH_OFFSET: usize = 80;
/// Grass-patch block length.
pub const NN_GRASS_PATCH_LEN: usize = 25;
/// Grass-patch center sample (dx=dy=0); 5×5 iteration is dy outer, dx inner,
/// offset = (dy+2)*5 + (dx+2). Center: dy=0,dx=0 → 12; global slot 80+12 = 92.
pub const NN_GRASS_PATCH_CENTER_SLOT: usize = NN_GRASS_PATCH_OFFSET + 12; // = 92
// Slots [105..112) are SIMD padding zeros.
