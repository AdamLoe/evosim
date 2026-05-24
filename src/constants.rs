//! All v5+v6 magic numbers, in one place. Reference each with the spec
//! section number so future readers can audit against the pitch.

// ---- World shape (v5 §3.3, v6 §D) ----
pub const WORLD_SIZE: f32 = 600.0;

// ---- Spatial hash (v5 §3.3) ----
pub const HASH_CELL: f32 = 5.0;
pub const HASH_DIM: usize = (WORLD_SIZE / HASH_CELL) as usize; // 120

// ---- Sun map (v5 §3.3, v6 §D) ----
pub const SUN_CELL: f32 = 30.0;
pub const SUN_DIM: usize = (WORLD_SIZE / SUN_CELL) as usize; // 20
pub const SUN_REFILL_RATE: f32 = 0.08; // R, default — slider in v6 §K
pub const SUN_GRADIENT_WEST: f32 = 3.0;
pub const SUN_GRADIENT_EAST: f32 = 1.0;
pub const HOTSPOT_COUNT: usize = 3;
pub const HOTSPOT_SIGMA: f32 = 80.0;
pub const HOTSPOT_PEAK: f32 = 4.0;
pub const HOTSPOT_MIN_SPACING: f32 = 200.0;
pub const HOTSPOT_MIN_WALL: f32 = 60.0;

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
pub const COST_SPLIT: f32 = 50.0;
pub const SPLIT_GIFT_MAX: f32 = 30.0;
pub const SPLIT_THRESHOLD: f32 = 50.0;

// ---- Eat / scavenge (v5 §3.5) ----
pub const PHOTO_GAIN_COEFF: f32 = 0.5; // payout = coeff × eff × size × factor
pub const EAT_DAMAGE_COEFF: f32 = 1.0; // damage = coeff × attacker.size
pub const EAT_GAIN_COEFF: f32 = 1.5; // gain = coeff × eat_efficiency
pub const SCAVENGE_GAIN_COEFF: f32 = 1.5;
pub const DIGESTION_COOLDOWN_TICKS: u32 = 50;

// ---- Death + carrion (v5 §8) ----
pub const CARRION_POOL_COEFF: f32 = 0.3; // pool = coeff × cumulative_upkeep
pub const CARRION_POOL_CAP: f32 = 60.0;
pub const CARRION_MAX_AGE: u32 = 100;

// ---- Past-lifespan penalty (v5 §7) ----
pub const PAST_LIFESPAN_MULT: f32 = 4.0; // per 1000 ticks past max_age

// ---- Brain (v5 §5, v6 §E) ----
pub const NN_INPUTS: usize = 136;
pub const NN_HIDDEN: usize = 24;
pub const NN_OUTPUTS: usize = 8;
pub const NN_WEIGHT_COUNT: usize = NN_INPUTS * NN_HIDDEN + NN_HIDDEN * NN_OUTPUTS;
pub const NN_MUT_RATE_DEFAULT: f32 = 0.02;
pub const NN_MUT_SIGMA_DEFAULT: f32 = 0.02;
pub const NN_INIT_RANGE: f32 = 0.3;

// ---- Body mutation (v5 §6) ----
pub const BODY_MUT_RATE_DEFAULT: f32 = 0.03;
pub const MUT_RATE_OF_RATES: f32 = 0.005;
pub const MUT_RATE_JITTER: f32 = 0.20;

// ---- Genome bounds (v5 §4) ----
pub const SIZE_MIN: f32 = 1.0;
pub const SIZE_MAX: f32 = 10.0;
pub const MAX_AGE_MIN: u32 = 100;
pub const MAX_AGE_MAX: u32 = 50_000;
pub const PHOTO_EFF_MIN: f32 = 0.0;
pub const PHOTO_EFF_MAX: f32 = 2.0;
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

// ---- Physics (v6 §D) ----
pub const REPULSION_K: f32 = 2.0;
pub const REPULSION_MAX: f32 = 5.0;
pub const WALL_THRESHOLD_PAD: f32 = 5.0; // is_at_wall iff dist < size + pad

// ---- Founder (Milestone B default; mutation kicks in C) ----
pub const FOUNDER_ENERGY: f32 = 200.0;
pub const FOUNDER_SIZE: f32 = 1.0;
pub const FOUNDER_MAX_AGE: u32 = 5000;
pub const FOUNDER_PHOTO_EFF: f32 = 1.0;
pub const FOUNDER_BITE_REACH: f32 = 1.0;
pub const FOUNDER_PIGMENT_R: f32 = 0.30;
pub const FOUNDER_PIGMENT_G: f32 = 0.75;
pub const FOUNDER_PIGMENT_B: f32 = 0.35;
pub const FOUNDER_SPLIT_JITTER: f32 = 1.0;

// ---- Species detection (v6 §H) ----
pub const SPECIES_W_BODY: f32 = 3.0;
pub const SPECIES_W_BRAIN: f32 = 1.0;
pub const SPECIES_THRESHOLD: f32 = 4.0;
pub const SPECIES_EYE_JUMP_COST: f32 = 1.5;

// ---- Vision (v5 §10, v6 §E, Milestone C.12) ----
/// Fixed radius used when a ray hits a carrion blob (not in spec — DECISIONS).
pub const CARRION_RADIUS_FOR_VISION: f32 = 1.5;
/// Maximum grid cells walked per ray; bounds worst-case DDA cost (v5 §3.6).
/// vision_range_max = 80u; HASH_CELL = 5u → ~16 cells along a ray axis.
/// Worst diagonal ≈ 32; double for slack.
pub const MAX_CELLS_PER_RAY: usize = 64;

// ---- Acceptance test (v5 §16) ----
pub const ACCEPTANCE_TICKS: u32 = 10_000;
pub const ACCEPTANCE_SEED: &str = "evosim-test-001";
