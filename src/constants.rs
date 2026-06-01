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
// rebuild cheap (960² vs a hypothetical 3840² at the old 2.5u) while still
// bounding neighbor candidates per cell in the sparse regime. Flagged for later
// profiler measurement — the optimal cell size at the new density is unverified.
pub const HASH_CELL: f32 = 10.0;

// ---- Energy economy upkeep ----
pub const UPKEEP_BASE: f32 = 0.05;
pub const UPKEEP_MOUTH_DEFAULT: f32 = 0.05;
pub const UPKEEP_GUT: f32 = 0.02;
pub const UPKEEP_NN_FIXED: f32 = 0.05;

// ---- One-time costs ----
pub const COST_MOVE_PER_DIST: f32 = 0.02;
pub const SPLIT_GIFT_MAX_DEFAULT: f32 = 30.0;
pub const SPLIT_THRESHOLD_DEFAULT: f32 = 99.0;

// ---- Eat ----
/// Default per-bite energy transfer fraction. Predator removes
/// `eat_bite_fraction * prey.energy * (1 - prey.armor)` per bite.
pub const EAT_BITE_FRACTION_DEFAULT: f32 = 1.0;
pub const DIGESTION_COOLDOWN_TICKS: u32 = 0;

// ---- Past-lifespan penalty ----
pub const PAST_LIFESPAN_MULT: f32 = 4.0; // per 1000 ticks past max_age

// ---- Brain ----
// NN input semantic layout — see src/world/nn.rs::build_nn_input + NnInputLayout.
//
// v2.0 Wave 1a: the active layout is driven by construction settings.
//   * `ReservedPredator` group dropped entirely.
//   * `WallProximity` (4 slots) present ONLY when `wrap_world == false`.
//
// Default (wrap on)  — SelfMemory(8) + CreatureSectors(8) + GrassSectors(8)
//                      + CurrGrass(1) + Bias(1) = 26 real → padded to 32.
// Walled (wrap off)  — + WallProximity(4) = 30 real → padded to 32.
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
/// The single source of truth lives here; `web/src/sim-bridge.ts` mirrors
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
/// Grass cell size in world-units.
pub const GRASS_CELL_SIZE: f32 = 5.0;
/// Maximum density per cell (clamped post-step).
pub const GRASS_MAX: f32 = 1.0;
/// Default in-cell logistic growth rate slider.
pub const GRASS_IN_CELL_GROWTH_R_DEFAULT: f32 = 0.01;
/// Default cross-kernel propagation rate slider.
pub const GRASS_PROPAGATION_RATE_K_DEFAULT: f32 = 0.003;
/// Default number of cells seeded at world init.
pub const GRASS_INITIAL_SEED_COUNT_DEFAULT: u32 = 8000;
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

// ---- Biome (v2.0 Wave 1a placeholder; populated by Wave 1b) ----
/// Per-cell biome tag stored in the dedicated `biomeSab` (one u8 per grass
/// cell). v2.0 Wave 1a fills the whole grid with `Plains` (0) as a placeholder —
/// real biome generation (water/desert regions seeded from `world_seed`) lands
/// in Wave 1b. The enum is `#[repr(u8)]` so the SAB byte maps directly.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Water/Desert are populated by Wave 1b biome generation; Wave 1a only uses
// `Plains` (the placeholder fill), so the other variants are not yet constructed.
#[allow(dead_code)]
pub enum Biome {
    Plains = 0,
    Water = 1,
    Desert = 2,
}

/// Runtime world dimensions, computed once at construction from `world_size`.
///
/// v2.0 Wave 1a: replaces the old compile-time `WORLD_SIZE` / `HASH_DIM` /
/// `GRASS_GRID_DIM` / `GRASS_CELL_COUNT` constants. A small `Copy` value carried
/// on `World`, `GrassGrid`, and `SpatialGrid` so every bounds/clamp/wrap/spawn
/// site reads the same derived dims. The snapshot grass region (u8) and the
/// `biomeSab` (u8) are both sized from `grass_cell_count`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldDims {
    /// World extent in world-units (square world, `[0, world_size]`).
    pub world_size: f32,
    /// Whether the world is toroidal (wrap) or walled (clamp).
    pub wrap_world: bool,
    /// Grass cells per axis: `round(world_size / GRASS_CELL_SIZE)`.
    pub grass_dim: usize,
    /// Total grass cells: `grass_dim²`.
    pub grass_cell_count: usize,
    /// Spatial-hash cells per axis: `ceil(world_size / HASH_CELL)`.
    pub hash_dim: usize,
}

impl WorldDims {
    /// Compute runtime dims from a world size + wrap flag. Clamps `world_size`
    /// to a sane floor so a degenerate construction setting can't produce a
    /// zero-dim grid.
    pub fn from_world_size(world_size: f32, wrap_world: bool) -> Self {
        // Floor at one grass cell + one hash cell so dims are always ≥ 1.
        let ws = world_size.max(GRASS_CELL_SIZE.max(HASH_CELL));
        let grass_dim = (ws / GRASS_CELL_SIZE).round() as usize;
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
        assert_eq!(d.hash_dim, 960); // ceil(9600 / 10)
    }
}
