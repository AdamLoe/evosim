//! All v5+v6 magic numbers, in one place. Reference each with the spec
//! section number so future readers can audit against the pitch.

// ---- World shape (v5 §3.3, v6 §D) ----
pub const WORLD_SIZE: f32 = 600.0;
/// Creature body radius scale in world-units (D3: constant; S3 — moved from world/mod.rs).
pub const BODY_RADIUS_PER_SIZE: f32 = 1.0;

// ---- Spatial hash (v5 §3.3) ----
pub const HASH_CELL: f32 = 5.0;
pub const HASH_DIM: usize = (WORLD_SIZE / HASH_CELL) as usize; // 120

// ---- Energy economy upkeep (v5 §7; D3: per-trait terms removed) ----
pub const UPKEEP_BASE: f32 = 0.05;
pub const UPKEEP_MOUTH_DEFAULT: f32 = 0.05; // slider in v6 §K
pub const UPKEEP_GUT: f32 = 0.02;
pub const UPKEEP_NN_FIXED: f32 = 0.05;

// ---- One-time costs (v5 §7) ----
pub const COST_EAT_ATTEMPT: f32 = 0.0;
pub const COST_MOVE_PER_DIST: f32 = 0.02;
pub const SPLIT_GIFT_MAX: f32 = 30.0;
pub const SPLIT_THRESHOLD: f32 = 50.0;

// ---- Eat (v5 §3.5; scavenge action removed in D9) ----
/// Default per-bite energy transfer fraction. Predator removes
/// `eat_bite_fraction * prey.energy * (1 - prey.armor)` per bite.
/// Live-tunable via DevSliders.eat_bite_fraction (P3b adds slider).
pub const EAT_BITE_FRACTION_DEFAULT: f32 = 0.5;
pub const DIGESTION_COOLDOWN_TICKS: u32 = 50;

// ---- Past-lifespan penalty (v5 §7) ----
pub const PAST_LIFESPAN_MULT: f32 = 4.0; // per 1000 ticks past max_age

// ---- Curriculum (v1.5 Step 4) ----
// Cost factor = MIN_FACTOR + (1 - MIN_FACTOR) * smoothstep(min_pop, max_pop, pop).
// Floor defaults to 0.0 (user-tunable up to 1.0); below min_pop costs vanish so
// fragile early populations stop bleeding upkeep before selection has anything
// to grip. User-exposed slider lets us crank floor toward 1.0 once a regime stabilises.
pub const CURRICULUM_MIN_FACTOR_DEFAULT: f32 = 0.0;
pub const CURRICULUM_MIN_POP_DEFAULT: u32 = 1000;
pub const CURRICULUM_MAX_POP_DEFAULT: u32 = 2000;

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

// ---- Brain mutation (v5 §6; body mutation removed in D3) ----
pub const MUT_RATE_OF_RATES: f32 = 0.005;
pub const MUT_RATE_JITTER: f32 = 0.20;

// ---- Trait-range constants still used as named values ----
pub const MOVE_SPEED_MAX: f32 = 5.0;
pub const VISION_RANGE_MAX: f32 = 80.0;
/// Founder bite reach (legacy name). Inspector dropped the reported field
/// in v1.5 S3 — constant retained for tests / future use.
#[allow(dead_code)]
pub const BITE_REACH_MIN: f32 = 1.0;

// ---- Physics (v6 §D) ----
pub const REPULSION_K: f32 = 2.0;
pub const REPULSION_MAX: f32 = 5.0;

// ---- Founder / constant body traits (D3: genome removed; all creatures share these) ----
pub const FOUNDER_ENERGY: f32 = 200.0;
pub const FOUNDER_SIZE: f32 = 1.0;
pub const FOUNDER_MAX_AGE: u32 = 5000;
/// Founder graze efficiency (legacy name). Inspector dropped the reported
/// field in v1.5 S3 — constant retained for tests / future use.
#[allow(dead_code)]
pub const FOUNDER_GRAZE_EFF: f32 = 5.0;
pub const FOUNDER_BITE_REACH: f32 = 0.0;
pub const FOUNDER_SPLIT_JITTER: f32 = 50.0;
/// Default number of founders seeded at world init (multi-founder v1.5).
pub const FOUNDER_COUNT_DEFAULT: u32 = 8;

// ---- Vision (v5 §10, v6 §E, Milestone C.12) ----
/// Maximum grid cells walked per ray; bounds worst-case DDA cost (v5 §3.6).
/// vision_range_max = 80u; HASH_CELL = 5u → ~16 cells along a ray axis.
/// Worst diagonal ≈ 32; double for slack.
pub const MAX_CELLS_PER_RAY: usize = 64;

// ---- Deterministic chunking (v6 §J) ----
/// Chunk-count clamps. Per-tick count = `clamp(pop/32, MIN, min(MAX, workers))`.
pub const MIN_CHUNKS: usize = 4;
pub const MAX_CHUNKS: usize = 16;

// ---- Population milestones (v5 §11, E.25.a) ----
/// Thresholds that fire a PopulationMilestone event once per threshold ever (v5 §11).
pub const POPULATION_MILESTONES: [u32; 6] = [10, 50, 100, 500, 1000, 2000];

// ---- Vision geometry (v5 §3.5, v6 §E; perf-1 sector sin/cos cache) ----
/// Number of vision sectors per creature. Matches EYE_SLOTS; kept in both
/// vision.rs (re-export) and constants.rs (source of truth for creature.rs).
pub const SECTORS: usize = 24;

// ---- Grass grid ----
/// Cell size in world-units. World is 600u → 120 cells per axis.
pub const GRASS_CELL_SIZE: f32 = 5.0;
/// Number of grass cells per axis (120).
pub const GRASS_GRID_DIM: usize = (WORLD_SIZE / GRASS_CELL_SIZE) as usize; // 120
/// Total grass cells (14_400).
pub const GRASS_CELL_COUNT: usize = GRASS_GRID_DIM * GRASS_GRID_DIM; // 14_400
/// Maximum density per cell (clamped post-step).
pub const GRASS_MAX: f32 = 1.0;
/// Default in-cell logistic growth rate slider.
pub const GRASS_IN_CELL_GROWTH_R_DEFAULT: f32 = 0.05;
/// Default cross-kernel propagation rate slider. 100× slower than the
/// earlier 0.1 default — grass now spreads slowly, so ripe cells are scarce.
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

// ---- NN input layout offsets (v1.3 D9) ----
/// Self-state block length: [energy_frac, age_frac, prev_vx, prev_vy, cooldown_frac].
/// Used in layout tests; `#[allow(dead_code)]` because tests are cfg(test)-only.
#[allow(dead_code)]
pub const NN_SELF_STATE_LEN: usize = 5;
/// Vision passthrough block start. 5 self-state slots before.
pub const NN_VISION_OFFSET: usize = 5;
/// Vision block length: 24 sectors × 3 RGB features (Q3: dist+size dropped).
pub const NN_VISION_LEN: usize = 72;
/// Last-action one-hot block start. After vision block (5 + 72 = 77).
pub const NN_LAST_ACTION_OFFSET: usize = 77;
/// Last-action block length: 3 (Graze, Eat, Split).
pub const NN_LAST_ACTION_LEN: usize = 3;
/// Grass-patch 5×5 block start (77 + 3 = 80).
pub const NN_GRASS_PATCH_OFFSET: usize = 80;
/// Grass-patch block length. Used in layout tests; `#[allow(dead_code)]` because tests are cfg(test)-only.
#[allow(dead_code)]
pub const NN_GRASS_PATCH_LEN: usize = 25;
/// One-hot for the action *two* ticks ago. Lets the brain see a 2-step action
/// history (combined with the existing last-action slot at offset 77).
/// Occupies former pad slots [105..108).
pub const NN_LAST2_ACTION_OFFSET: usize = 105;
#[allow(dead_code)]
pub const NN_LAST2_ACTION_LEN: usize = 3;
/// Normalized energy change since the previous NN forward pass:
/// (energy_now - prev_energy) / energy_max. Range roughly [-1, 1].
/// Occupies former pad slot [108..109).
pub const NN_ENERGY_DELTA_OFFSET: usize = 108;
/// Grass-patch center sample (dx=dy=0); 5×5 iteration is dy outer, dx inner,
/// offset = (dy+2)*5 + (dx+2). Center: dy=0,dx=0 → 12; global slot 80+12 = 92.
/// Used in tests to pin the center-slot index; `#[allow(dead_code)]` because tests are cfg(test)-only.
#[allow(dead_code)]
pub const NN_GRASS_PATCH_CENTER_SLOT: usize = NN_GRASS_PATCH_OFFSET + 12; // = 92
                                                                          // Slots [105..112) are SIMD padding zeros.
