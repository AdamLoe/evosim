//! Thin wasm-bindgen wrapper exposing the World to JS. Stable function
//! shapes so the web shell can iterate independently.

use crate::brain::{Activation, NnTopology};
use crate::constants::*;
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use wasm_bindgen::prelude::*;

/// v1.12: parse the boot payload's `nn_topology_json`. Empty string → legacy
/// 32→48→24→5 default. Otherwise expects
/// `{"hidden_sizes":[…],"activations":[…]}`.
fn parse_nn_topology(json: &str) -> Result<NnTopology, String> {
    if json.is_empty() {
        return Ok(NnTopology::legacy());
    }
    #[derive(serde::Deserialize)]
    struct Raw {
        hidden_sizes: Vec<usize>,
        activations: Vec<String>,
    }
    let raw: Raw = serde_json::from_str(json).map_err(|e| format!("parse: {e}"))?;
    let mut acts = Vec::with_capacity(raw.activations.len());
    for a in &raw.activations {
        acts.push(Activation::parse(a).ok_or_else(|| format!("unknown activation: {a}"))?);
    }
    NnTopology::new(raw.hidden_sizes, acts)
}

/// Budget for "jank" detection: ticks that take longer than this are counted.
pub const JANK_BUDGET_MS: f64 = 16.0;

/// Rolling window size (in ticks) for the TPS average.
const TPS_WINDOW: usize = 10;

const TELEMETRY_SCHEMA_VERSION: u32 = 1;
const TELEMETRY_SAMPLE_PERIOD_TICKS: u32 = 120;
const TELEMETRY_SAMPLE_CAP: usize = 2048;
const TELEMETRY_EVENT_CAP: usize = 256;
const LARGE_JANK_EVENT_MS: f64 = 64.0;

#[derive(Clone, Serialize, Deserialize)]
pub struct WorldConfig {
    pub schema_version: u32,
    pub master_seed: u32,
    pub world: WorldShapeConfig,
    pub grass: GrassConstructionConfig,
    pub population: PopulationConstructionConfig,
    pub species: SpeciesConstructionConfig,
    pub founders: FounderConstructionConfig,
    pub nn_topology: NnTopologyConstructionConfig,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WorldShapeConfig {
    pub size: f32,
    pub wrap: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GrassConstructionConfig {
    pub initial_seed_count: u32,
    pub full_on_init: bool,
    pub cell_size: f32,
    pub clump_count: u32,
    pub clump_size: u32,
    pub multisight: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PopulationConstructionConfig {
    pub founder_count: u32,
    pub energy_max: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SpeciesConstructionConfig {
    pub enabled: bool,
    pub crossover_mode: WorldConfigCrossoverMode,
    pub starting_species_count: u32,
    pub starting_species_member_count: u32,
    pub starting_species_member_variance: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FounderConstructionConfig {
    pub init_graze_boost: f32,
    pub init_split_boost: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NnTopologyConstructionConfig {
    pub hidden_sizes: Vec<usize>,
    pub activations: Vec<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldConfigCrossoverMode {
    Average,
    FiftyFifty,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WorldConfigPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub config: WorldConfig,
}

impl From<CrossoverMode> for WorldConfigCrossoverMode {
    fn from(value: CrossoverMode) -> Self {
        match value {
            CrossoverMode::Average => Self::Average,
            CrossoverMode::FiftyFifty => Self::FiftyFifty,
        }
    }
}

impl From<WorldConfigCrossoverMode> for CrossoverMode {
    fn from(value: WorldConfigCrossoverMode) -> Self {
        match value {
            WorldConfigCrossoverMode::Average => Self::Average,
            WorldConfigCrossoverMode::FiftyFifty => Self::FiftyFifty,
        }
    }
}

impl WorldConfig {
    pub fn defaults() -> Self {
        let sliders = crate::world::DevSliders::default();
        Self {
            schema_version: WORLD_CONFIG_SCHEMA_VERSION,
            master_seed: 0,
            world: WorldShapeConfig {
                size: sliders.world_size,
                wrap: sliders.wrap_world,
            },
            grass: GrassConstructionConfig {
                initial_seed_count: sliders.grass_initial_seed_count,
                full_on_init: sliders.full_grass_on_init,
                cell_size: sliders.grass_cell_size,
                clump_count: sliders.grass_clump_count,
                clump_size: sliders.grass_clump_size,
                multisight: sliders.grass_multisight,
            },
            population: PopulationConstructionConfig {
                founder_count: sliders.founder_count,
                energy_max: sliders.energy_max,
            },
            species: SpeciesConstructionConfig {
                enabled: sliders.species_mode,
                crossover_mode: sliders.crossover_mode.into(),
                starting_species_count: sliders.starting_species_count,
                starting_species_member_count: sliders.starting_species_member_count,
                starting_species_member_variance: sliders.starting_species_member_variance,
            },
            founders: FounderConstructionConfig {
                init_graze_boost: sliders.init_graze_boost,
                init_split_boost: sliders.init_split_boost,
            },
            nn_topology: NnTopologyConstructionConfig {
                hidden_sizes: LEGACY_HIDDEN_SIZES.to_vec(),
                activations: vec!["lrelu".to_string(), "lrelu".to_string()],
            },
        }
    }

    pub fn presets() -> Vec<WorldConfigPreset> {
        let default = Self::defaults();
        let mut compact = default.clone();
        compact.world.size = 4800.0;
        compact.population.founder_count = 24;
        compact.grass.clump_count = 24;

        let mut species = default.clone();
        species.species.enabled = true;
        species.population.founder_count = 100;
        species.founders.init_graze_boost = 1.25;
        species.founders.init_split_boost = 1.15;

        vec![
            WorldConfigPreset {
                id: "default",
                label: "Default",
                description: "Large toroidal single-pool world using canonical defaults.",
                config: default,
            },
            WorldConfigPreset {
                id: "compact",
                label: "Compact",
                description: "Smaller world with lighter boot cost.",
                config: compact,
            },
            WorldConfigPreset {
                id: "species",
                label: "Species",
                description: "Complete species-mode construction config.",
                config: species,
            },
        ]
    }
}

fn random_nonzero_u32() -> u32 {
    let mut bytes = [0u8; 4];
    getrandom::getrandom(&mut bytes).ok();
    let n = u32::from_le_bytes(bytes);
    if n == 0 {
        1
    } else {
        n
    }
}

fn parse_nn_topology_config(config: &NnTopologyConstructionConfig) -> Result<NnTopology, String> {
    let mut acts = Vec::with_capacity(config.activations.len());
    for a in &config.activations {
        acts.push(Activation::parse(a).ok_or_else(|| format!("unknown activation: {a}"))?);
    }
    NnTopology::new(config.hidden_sizes.clone(), acts)
}

#[derive(Clone, Serialize)]
struct TelemetrySample {
    tick_start: u32,
    tick_end: u32,
    at_ms: f64,
    wall_elapsed_ms: f64,
    population: u32,
    tps: f32,
    jank_count_delta: u32,
    jank_count_total: u32,
    live_grass_cell_count: u32,
    total_grass_density: f32,
}

#[derive(Clone, Serialize)]
struct TelemetryEvent {
    seq: u64,
    tick: u32,
    at_ms: f64,
    kind: String,
    payload: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct JankPhaseAttribution {
    phase: String,
    confidence: String,
    reason: String,
}

#[derive(Clone, Serialize)]
struct WorstJank {
    tick: u32,
    duration_ms: f64,
    population: u32,
    phase: JankPhaseAttribution,
}

#[derive(Serialize)]
struct JankSummary {
    budget_ms: f64,
    large_event_threshold_ms: f64,
    count: u32,
    worst: Option<WorstJank>,
}

struct TelemetryState {
    created_at_ms: f64,
    last_sample_tick: u32,
    last_sample_ms: f64,
    last_sample_jank_count: u32,
    samples: VecDeque<TelemetrySample>,
    sample_dropped_count: u64,
    events: VecDeque<TelemetryEvent>,
    event_dropped_count: u64,
    next_event_seq: u64,
    world_end_logged: bool,
    worst_jank: Option<WorstJank>,
}

impl TelemetryState {
    fn new(now_ms: f64) -> Self {
        Self {
            created_at_ms: now_ms,
            last_sample_tick: 0,
            last_sample_ms: now_ms,
            last_sample_jank_count: 0,
            samples: VecDeque::with_capacity(TELEMETRY_SAMPLE_CAP),
            sample_dropped_count: 0,
            events: VecDeque::with_capacity(TELEMETRY_EVENT_CAP),
            event_dropped_count: 0,
            next_event_seq: 0,
            world_end_logged: false,
            worst_jank: None,
        }
    }

    fn push_sample(&mut self, sample: TelemetrySample) {
        if self.samples.len() == TELEMETRY_SAMPLE_CAP {
            self.samples.pop_front();
            self.sample_dropped_count = self.sample_dropped_count.saturating_add(1);
        }
        self.samples.push_back(sample);
    }

    fn push_event(&mut self, tick: u32, at_ms: f64, kind: &str, payload: serde_json::Value) {
        if self.events.len() == TELEMETRY_EVENT_CAP {
            self.events.pop_front();
            self.event_dropped_count = self.event_dropped_count.saturating_add(1);
        }
        let event = TelemetryEvent {
            seq: self.next_event_seq,
            tick,
            at_ms,
            kind: kind.to_string(),
            payload,
        };
        self.next_event_seq = self.next_event_seq.saturating_add(1);
        self.events.push_back(event);
    }
}

/// Canonical ordered list of dev-panel slider names. Indices are stable across
/// the SAB transport: TS writes `value` into `CTRL_SLIDERS[idx]` + bumps
/// `CTRL_CONTROL_EPOCH`, the worker reads at top of tick and dispatches via
/// [`WorldHandle::set_slider_by_index`]. Re-generate TS bindings after
/// modifying this list: `cargo run --bin gen-bindings`. A drift unit test
/// catches mismatches at `cargo test --lib`.
pub const SLIDER_NAMES: &[&str] = &[
    "mutation_rate_multiplier",           // 0  f32
    "_reserved_legacy_nn_mutation_sigma", // 1  f32 — v1.12 reserved no-op; was nn_mutation_sigma
    "eat_bite_fraction",                  // 2  f32
    "grass_propagation_rate_k",           // 3  f32
    "grass_in_cell_growth_r",             // 4  f32
    "upkeep_multiplier",                  // 5  f32
    "move_cost_multiplier",               // 6  f32
    "energy_max",                         // 7  f32
    "grass_energy_per_bite",              // 8  f32
    "grass_bites_per_block",              // 9  u32 (carried as f32)
    "digestion_cooldown",                 // 10 u32 (carried as f32)
    "repulsion_max",                      // 11 f32
    "max_age",                            // 12 u32 (carried as f32)
    "split_threshold",                    // 13 f32
    "split_gift",                         // 14 f32
    "split_jitter",                       // 15 f32
    "founder_count",                      // 16 u32 (carried as f32)
    "max_population",                     // 17 u32 (carried as f32) — user-tunable cap
    // v2.0 Wave 1a construction settings (shape the next world only). DROPPED
    // here vs v1.x: the four `_reserved_curriculum_*` slots and `full_grass_on_init`.
    "world_size", // 18 f32 (construction) — world extent
    "world_seed", // 19 u32 (carried as f32) — biome seed (Wave 1b)
    "wrap_world", // 20 bool (0.0 / non-zero) — torus vs walled
    // v2.1 P2: slots 21/22/23 retired; kept as no-ops to preserve index stability.
    "_reserved_legacy_water_movement_penalty", // 21 no-op (was water_movement_penalty)
    "_reserved_legacy_desert_movement_penalty", // 22 no-op (was desert_movement_penalty)
    "_reserved_legacy_trait_mutation_sigma_multiplier", // 23 no-op (was trait_mutation_sigma_multiplier)
    // v2.0 Wave 3a: species + sexual-mating construction/live settings.
    "species_mode",           // 24 bool (0/non-zero) construction — species + mating
    "crossover_mode",         // 25 enum (0=average, non-zero=fifty_fifty) construction
    "starting_species_count", // 26 u32 (carried as f32) construction
    "starting_species_member_count", // 27 u32 (carried as f32) construction
    "starting_species_member_variance", // 28 f32 construction (INERT in W3; W4 applies it)
    "mating_cooldown_ticks",  // 29 u32 (carried as f32) live — initiator-only cooldown
    // v1.12: 8 mutation buckets × 3 floats. Index = SLIDER_BUCKET_BASE + bucket*3 + field.
    // field 0 = weight, 1 = rate, 2 = sigma. See MUTATION_BUCKET_COUNT.
    "bucket_0_weight",
    "bucket_0_rate",
    "bucket_0_sigma", // 30..33
    "bucket_1_weight",
    "bucket_1_rate",
    "bucket_1_sigma", // 33..36
    "bucket_2_weight",
    "bucket_2_rate",
    "bucket_2_sigma", // 36..39
    "bucket_3_weight",
    "bucket_3_rate",
    "bucket_3_sigma", // 39..42
    "bucket_4_weight",
    "bucket_4_rate",
    "bucket_4_sigma", // 42..45
    "bucket_5_weight",
    "bucket_5_rate",
    "bucket_5_sigma", // 45..48
    "bucket_6_weight",
    "bucket_6_rate",
    "bucket_6_sigma", // 48..51
    "bucket_7_weight",
    "bucket_7_rate",
    "bucket_7_sigma", // 51..54
    // v2.0.2 Stream 1d: scatter kernel live-tuning sliders. APPEND-ONLY —
    // existing indices must stay stable (1e reads slider idx 8 by index).
    "grass_decay_pct",        // 54 f32 — decay probability per tick
    "grass_decay_amount",     // 55 f32 — density subtracted on decay
    "grass_spread_pct",       // 56 f32 — spread probability per tick
    "grass_spread_amount",    // 57 f32 — density added to spread target
    "grass_spread_ring1_pct", // 58 f32 — ring-1 disc-band weight
    "grass_spread_ring2_pct", // 59 f32 — ring-2 disc-band weight
    // v2.0.4 S1: render-side LOD knob. APPEND-ONLY — existing indices stable.
    "lod_bias", // 60 f32 — subtracted from computed mip level (finer detail)
    //    nudge-within-budget: biasing finer than budget crops edges.
    // v2.0.4 S6: multi-band NN grass sight construction toggle.
    "grass_multisight", // 61 bool (0/non-zero) construction — multi-band grass NN sight
    // v2.0.4 S2: grass cell size construction knob.
    "grass_size", // 62 f32 (construction) — grass cell size in world-units (5–20u)
    // v2.0.6 S10: discrete effective-resolution stepper (LIVE — affects next write_snapshot).
    // 0 = Auto (use formula + lod_bias), 1 = L0 (full ~grass_dim), 2 = L1, 3 = L2, 4 = L3, 5 = L4.
    // When non-zero, the stepper forces an absolute mip level, overriding the auto formula and
    // lod_bias entirely. Level 1 (full) is the one step finer than auto in typical zoom-out use.
    "grass_lod_step", // 63 u32 (carried as f32) — discrete LOD level stepper (0=Auto)
    // Mating reach + founder action-output boosts. APPEND-ONLY — existing indices
    // stay stable. These fit the existing reserved SAB headroom (16 + 67 = 83 <
    // CTRL_INSPECT_REQ_EPOCH = 96), so no offset shifts.
    "mate_reach_multiplier", // 64 f32 live — × on mating contact radius
    "init_graze_boost",      // 65 f32 construction — founder Graze-output boost
    "init_split_boost",      // 66 f32 construction — founder Split/Mate-output boost
    // v2.1 P3: food-limited equilibrium knobs. APPEND-ONLY (idx 67+).
    // All six are live sliders (take effect next tick).
    "crowding_strength",     // 67 f32 live — extra upkeep per neighbour per tick
    "crowding_radius",       // 68 f32 live — radius (world-units) for neighbour count
    "starvation_threshold",  // 69 f32 live — energy floor below which extra drain fires
    "starvation_drain_rate", // 70 f32 live — extra per-tick drain while below threshold
    "grass_capacity_scale",  // 71 f32 live — multiplier on per-cell carrying-cap
    "grass_regrowth_rate",   // 72 f32 live — multiplier on spread_pct + in-cell growth
];

/// First mutation-bucket slider slot. v2.0 Wave 3a (shifted from 24 to 30 after
/// adding the 6 species/mating slots).
pub const SLIDER_BUCKET_BASE: usize = 30;

/// Number of slots in `CTRL_SLIDERS`. Equal to `SLIDER_NAMES.len()`.
pub const SLIDER_COUNT: usize = SLIDER_NAMES.len();

// ─── v1.11 (A+D): wasm-memory-resident snapshot buffer ──────────────────────
//
// The snapshot region lives inside `WorldHandle::snapshot_buf` — a Rust-owned
// `Vec<u8>` in wasm linear memory. Wasm memory is itself a SharedArrayBuffer
// (we have shared memory enabled), so the main thread reads via a view over
// `wasm.memory.buffer` at the byte offset returned by
// `WorldHandle::snapshot_buf_byte_offset`. This eliminates the per-call
// `js_sys::Uint8Array::copy_from` boundary crossings that dominated snapshot
// cost (~4 ms / tick at pop 80).
//
// v2.0 Wave 1a: the grass region is now RUNTIME-SIZED (from `world_size`) and
// quantized to u8 — one byte per grass cell (density 0..GRASS_MAX → 0..255 on
// snapshot write). The creature region is UNCHANGED (stride 32 B / 8 lanes).
// The header + creature region are compile-time constants; only the grass region
// (and thus slot/buf totals) are computed at boot from the world's `grass_dim`
// and stored on the `WorldHandle`. The live biome channel is per-slot and
// windowed alongside grass; no full-field biome buffer is exposed to JS.

/// Bytes per snapshot stats header (matches `SNAPSHOT_HEADER_BYTES` TS-side).
/// v2.0.3 Stream 2b: bumped 32 → 64 to add 32 bytes of window-metadata fields
/// (8 × u32 LE) while keeping the creature SoA 32-byte aligned.
///
/// Full layout (all LE):
///   off  0: `tick`         u32
///   off  4: `pop`          u32
///   off  8: `world_ended`  u32 (0/1)
///   off 12: `tps_bits`     u32 (= f32::to_bits(tps))
///   off 16: `jank_count`   u32
///   off 20..32: reserved / padding (unchanged)
///   off 32: `mip_level`    u32  — pyramid LOD level used (0 = full-res)
///   off 36: `win_origin_x` i32  — logical window origin X in level-k cells
///   off 40: `win_origin_y` i32  — logical window origin Y in level-k cells
///   off 44: `win_w`        u32  — window width  in level-k cells
///   off 48: `win_h`        u32  — window height in level-k cells
///   off 52: `tex_dim_w`    u32  — texture upload width  (= win_w for now)
///   off 56: `tex_dim_h`    u32  — texture upload height (= win_h for now)
///   off 60: `wrap_mode`    u32  — 0 = clamp (walled), 1 = wrap (torus)
pub const SNAPSHOT_HEADER_BYTES: usize = 64;
/// Bytes per creature record in the snapshot SoA (matches CREATURE_STRIDE × 4).
pub const SNAPSHOT_CREATURE_STRIDE: usize = 32;
/// Total creature SoA region size per slot. UNCHANGED in v2.0 (world-size
/// independent — sized by `MAX_POP_FOR_SIM`).
pub const SNAPSHOT_CREATURE_BYTES: usize = MAX_POP_FOR_SIM * SNAPSHOT_CREATURE_STRIDE;
/// Per-slot byte alignment. The slot byte size is rounded up to this so each
/// slot's f32 creature SoA starts at a 4-aligned offset (required by
/// `Float32Array`) even when grass_dim is odd. MUST match the TS rounding in
/// `src/web/src/sim-bridge.ts::makeSlotLayout`. 4 is the f32 requirement.
pub const SNAPSHOT_SLOT_ALIGN: usize = 4;

/// v2.0.4 S1: clipmap budget axis in cells. The worker publishes at most
/// `GRASS_LOD_BUDGET_AXIS × GRASS_LOD_BUDGET_AXIS` grass bytes per slot.
/// Raised 2048 → 4096: full-res holds to ~16.7M cells/axis; costs ≈ 16 MB
/// per slot grass region at max scale.
/// Single source of truth: this Rust constant is emitted to TS via
/// `src/bin/gen_bindings.rs` → `src/web/src/generated/lod-constants.ts`.
/// `sim-bridge.ts` imports from the generated file — do NOT hard-code 4096 there.
pub const GRASS_LOD_BUDGET_AXIS: usize = 4096;

/// v2.0.3 Stream 2b: pan margin factor. The published window covers the
/// visible viewport × GRASS_LOD_MARGIN_FACTOR (centered). A full-viewport pan
/// in any direction stays covered for ≥ 1 tick without a new window.
pub const GRASS_LOD_MARGIN_FACTOR: f32 = 1.5;

#[inline]
fn snapshot_window_copy_origin(raw: isize, level_dim: usize, win_dim: usize, wrap: bool) -> usize {
    debug_assert!(level_dim > 0);
    if wrap {
        raw.rem_euclid(level_dim as isize) as usize
    } else {
        (raw.max(0) as usize).min(level_dim.saturating_sub(win_dim))
    }
}

// ─── v2.0.6 S9: static biome mode-pyramid ─────────────────────────────────────
//
// The biome grid is static (never changes after world construction). Recomputing
// the mode-downsampled biome window on every tick (the old loop in write_snapshot)
// is pure redundant work. `BiomePyramid` precomputes the full mode-downsampled
// grid for every mip level at construction time, so `write_snapshot` can replace
// the nested-loop recompute with a cheap window-slice copy.
//
// Memory layout: level_dims mirrors GrassPyramid (biome is always square /
// same cell count as grass). `levels[0]` is the L0 full grid (1920×1920 = 3.7M
// bytes at default scale); `levels[k]` for k≥1 is the mode-downsampled L(k)
// grid. Total ~5MB at default scale — well within the snapshot budget.

/// Precomputed mode-downsampled biome pyramid. One `Vec<u8>` per level;
/// `levels[0]` is the full L0 grid (byte-identical to `biome_grid`);
/// `levels[k]` is the mode-downsampled grid at 2^k × 2^k block size.
/// Invalidated only on world reconstruction (biome_grid is static).
struct BiomePyramid {
    /// Per-level `(width, height)`. `level_dims[0]` is the base (L0) resolution.
    level_dims: Vec<(usize, usize)>,
    /// Owned u8 buffers for L0, L1, L2, … Row-major.
    /// `levels[k].len() == level_dims[k].0 * level_dims[k].1`.
    levels: Vec<Vec<u8>>,
}

impl BiomePyramid {
    /// Build the full biome mode-pyramid from the level-0 biome grid.
    /// `level_dims` must match those from `GrassPyramid` (same cell count).
    fn build(biome_grid: &[u8], level_dims: &[(usize, usize)]) -> Self {
        let mut levels: Vec<Vec<u8>> = Vec::with_capacity(level_dims.len());

        // L0: copy the raw biome grid (1×1 block → pass-through, identical to
        // the old loop's output at mip_level=0). This makes write_snapshot
        // uniform across all levels: always copy from levels[mip_level].
        levels.push(biome_grid.to_vec());

        // L1+: mode-downsample from the L0 grid (not from the parent level).
        // Using L0 as the source for all levels is correct: each level-k cell
        // covers exactly a 2^k × 2^k block of L0 cells (by definition).
        let (l0_w, l0_h) = level_dims[0];
        debug_assert_eq!(biome_grid.len(), l0_w * l0_h, "biome_grid size mismatch");

        for k in 1..level_dims.len() {
            let (lk_w, lk_h) = level_dims[k];
            let block = 1usize << k;
            let mut lk = vec![0u8; lk_w * lk_h];
            for oy in 0..lk_h {
                let l0_row_base = (oy * block).min(l0_h.saturating_sub(1));
                for ox in 0..lk_w {
                    let l0_col_base = (ox * block).min(l0_w.saturating_sub(1));
                    let mut counts = [0u32; 4];
                    for dy in 0..block {
                        let row = (l0_row_base + dy).min(l0_h - 1);
                        for dx in 0..block {
                            let col = (l0_col_base + dx).min(l0_w - 1);
                            let tag = biome_grid[row * l0_w + col] as usize;
                            counts[tag.min(counts.len() - 1)] += 1;
                        }
                    }
                    let mut best_tag = 0u8;
                    let mut best_count = 0u32;
                    for (tag, &cnt) in counts.iter().enumerate() {
                        if cnt > best_count {
                            best_count = cnt;
                            best_tag = tag as u8;
                        }
                    }
                    lk[oy * lk_w + ox] = best_tag;
                }
            }
            levels.push(lk);
        }

        Self {
            level_dims: level_dims.to_vec(),
            levels,
        }
    }

    /// Copy the window (origin_x, origin_y, win_w, win_h) from level `k` into
    /// `dst`. `dst.len()` must equal `win_w * win_h`. The origin and window
    /// dimensions are in level-k cells. Toroidal windows wrap; walled windows
    /// are already clamped by the caller.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn copy_window(
        &self,
        k: usize,
        wrap: bool,
        origin_x: usize,
        origin_y: usize,
        win_w: usize,
        win_h: usize,
        dst: &mut [u8],
    ) {
        debug_assert!(k < self.levels.len(), "level {k} out of range");
        debug_assert_eq!(dst.len(), win_w * win_h, "dst size mismatch");
        let (lk_w, lk_h) = self.level_dims[k];
        debug_assert!(origin_x < lk_w && origin_y < lk_h);
        let src = &self.levels[k];
        for row in 0..win_h {
            let sy = if wrap {
                (origin_y + row) % lk_h
            } else {
                origin_y + row
            };
            let src_row = &src[sy * lk_w..(sy + 1) * lk_w];
            let dst_row = &mut dst[row * win_w..(row + 1) * win_w];
            if wrap {
                let first_len = win_w.min(lk_w - origin_x);
                dst_row[..first_len].copy_from_slice(&src_row[origin_x..origin_x + first_len]);
                if first_len < win_w {
                    dst_row[first_len..].copy_from_slice(&src_row[..win_w - first_len]);
                }
            } else {
                dst_row.copy_from_slice(&src_row[origin_x..origin_x + win_w]);
            }
        }
    }
}

/// Computed snapshot region sizes for a given grass cell count. v2.0 Wave 1a:
/// the grass region is runtime-sized from `grass_cell_count`; v2.0.3 Stream 2b:
/// the region is now bounded by `GRASS_LOD_BUDGET_AXIS²` (clipmap budget).
/// v2.0.3 Stream 2d: the slot now also carries a mode-downsampled biome window
/// of the same size as the grass window (appended after grass).
#[derive(Clone, Copy, Debug)]
struct SnapshotLayout {
    /// u8 grass region size per slot = min(grass_dim, budget_axis)² bytes.
    /// This is the allocation size; only `win_w * win_h` bytes are meaningful
    /// per slot (read from the window metadata in the header).
    grass_bytes: usize,
    /// v2.0.3 Stream 2d: u8 biome window region per slot = same max-allocation
    /// as grass_bytes (min(grass_dim, budget_axis)²). The actual written bytes
    /// are `win_w * win_h` matching the grass window, mode-downsampled.
    biome_win_bytes: usize,
    /// header + creatures + grass + biome_win.
    slot_bytes: usize,
    /// two double-buffered slots.
    buf_bytes: usize,
    /// The full grass_dim (cells per axis), for window-compute reference.
    grass_dim: usize,
}

impl SnapshotLayout {
    fn from_grass_cell_count(grass_cell_count: usize) -> Self {
        // grass_dim = sqrt(grass_cell_count) (square grid).
        let grass_dim = (grass_cell_count as f64).sqrt() as usize;
        debug_assert_eq!(
            grass_dim * grass_dim,
            grass_cell_count,
            "grass must be square"
        );
        // Grass window budget: min(grass_dim, budget_axis)².
        // At default scale (1920 < 2048) this equals grass_cell_count.
        let win_axis = grass_dim.min(GRASS_LOD_BUDGET_AXIS);
        let grass_bytes = win_axis * win_axis;
        // v2.0.3 Stream 2d: biome window allocation = same max size as grass window.
        let biome_win_bytes = grass_bytes;
        // v2.0.x alignment fix: pad each slot to a 4-byte multiple. `grass_bytes +
        // biome_win_bytes = 2 * win_axis²` is ≡ 2 (mod 4) when `win_axis` is ODD
        // (e.g. an odd grass_dim from a world_size/grass_size combo like 19200/7 →
        // 2743). Without padding, slot 1's creature SoA would start at a 2-mod-4
        // byte offset and main's `new Float32Array(buffer, base + creatureOffset,
        // …)` throws "start offset of Float32Array should be a multiple of 4" on
        // every other frame. Even grass_dim (e.g. the 1920/3840 defaults) is
        // already aligned, so this is a no-op there. TS `makeSlotLayout` applies
        // the identical rounding — keep them in lockstep.
        let slot_bytes =
            (SNAPSHOT_HEADER_BYTES + SNAPSHOT_CREATURE_BYTES + grass_bytes + biome_win_bytes)
                .next_multiple_of(SNAPSHOT_SLOT_ALIGN);
        let buf_bytes = 2 * slot_bytes;
        Self {
            grass_bytes,
            biome_win_bytes,
            slot_bytes,
            buf_bytes,
            grass_dim,
        }
    }
}

#[wasm_bindgen]
pub struct WorldHandle {
    inner: World,
    /// v1.11 (A): wasm-memory-resident snapshot region. Fixed size
    /// `SNAPSHOT_BUF_BYTES`, allocated once at construction. Main thread
    /// reads via `new Uint8Array(wasm.memory.buffer, snapshot_buf_byte_offset,
    /// SNAPSHOT_BUF_BYTES)`. The two double-buffered slots, the stats
    /// header layout, the creature SoA stride, and the grass f32 format
    /// are all locked at boot and never change for the worker's lifetime.
    snapshot_buf: Vec<u8>,
    /// v2.0 Wave 1a: runtime snapshot region sizes, computed at boot from the
    /// world's `grass_dim` (grass region is u8, `grass_cell_count` bytes/slot).
    snapshot_layout: SnapshotLayout,
    /// v2.0.6 S9: precomputed biome mode-pyramid. One level per GrassPyramid level;
    /// `biome_pyramid.levels[k]` holds the mode-downsampled biome grid at mip level k.
    /// Replaces the per-tick nested-loop recompute in `write_snapshot` with a
    /// cheap window-slice copy. Built once at construction (biome_grid is static).
    biome_pyramid: BiomePyramid,
    /// Rolling window of wall-clock intervals (ms) between successive tick
    /// completions — i.e. tick-end-to-tick-end, *including* whatever pacing
    /// wait the worker imposed between iterations. This is what the
    /// user-visible TPS counter divides into. Per-tick *work* time (used for
    /// jank accounting) is computed inline and not stored.
    tick_intervals_ms: std::collections::VecDeque<f64>,
    /// Wall-clock timestamp (ms) at which the previous tick completed, or
    /// `None` before the first tick. Drives the interval calculation above.
    last_tick_end_ms: Option<f64>,
    /// Count of ticks whose wall-clock duration exceeded JANK_BUDGET_MS.
    jank_count: u32,
    /// Bounded run-history samples, lifecycle events, and worst-jank summary.
    telemetry: TelemetryState,
    /// v2.0.4 S1: LOD bias — subtracted from the computed mip level so the
    /// renderer picks a finer pyramid level than the budget-threshold formula
    /// alone would choose. Clamped to [0, max_level] after subtraction so it
    /// can never produce a negative level. Default 0.0 (no bias).
    ///
    /// WARNING: biasing finer than the budget can sustain causes the window
    /// to clamp at the GRASS_LOD_BUDGET_AXIS ceiling, cropping viewport edges.
    /// This knob is a "nudge within budget," not an independent resolution control.
    lod_bias: f32,
    /// v2.0.6 S10: discrete LOD stepper. 0 = Auto (use formula + lod_bias).
    /// Non-zero values force an absolute mip level: 1 = L0 (full), 2 = L1,
    /// 3 = L2, 4 = L3, 5 = L4. When non-zero, overrides lod_bias entirely.
    /// Live-settable — affects the very next write_snapshot call.
    lod_step: u32,
    master_seed: u32,
}

#[wasm_bindgen]
impl WorldHandle {
    /// Construct from a string seed; empty string → random per build.
    /// Uses the multi-founder default (`STARTING_POP_DEFAULT`).
    #[wasm_bindgen(constructor)]
    pub fn new(seed: &str) -> Self {
        let actual_seed = if seed.is_empty() {
            // Generate a printable random seed using JS Math.random via getrandom.
            let mut bytes = [0u8; 8];
            getrandom::getrandom(&mut bytes).ok();
            let n = u64::from_le_bytes(bytes);
            format!("seed-{n:016x}")
        } else {
            seed.to_string()
        };
        let inner = World::new_with_sliders(actual_seed, crate::world::DevSliders::default());
        Self::from_world_with_master_seed(inner, 0)
    }

    /// Allocate the runtime-sized snapshot buffer from a constructed world's
    /// grass dims and assemble the handle. v2.0: the snapshot carries grass and
    /// biome windows per slot; the old full-field biome buffer is gone.
    /// v2.0.6 S9: also builds the biome mode-pyramid (one-time, static).
    fn from_world(inner: World) -> Self {
        Self::from_world_with_master_seed(inner, 0)
    }

    fn from_world_with_master_seed(inner: World, master_seed: u32) -> Self {
        let grass_cell_count = inner.dims.grass_cell_count;
        let layout = SnapshotLayout::from_grass_cell_count(grass_cell_count);
        debug_assert_eq!(
            inner.biome_grid_bytes().len(),
            grass_cell_count,
            "biome grid length must equal grass cell count"
        );
        // v2.0.6 S9: build the static biome mode-pyramid once.
        // `GrassPyramid::level_dims` records the (w,h) pairs for every level;
        // we mirror them for the biome pyramid so copy_window always has the
        // correct row stride for each level.
        let level_dims = &inner.grass.pyramid.level_dims;
        let biome_pyramid = BiomePyramid::build(inner.biome_grid_bytes(), level_dims);
        let now_ms = wasm_now_ms();
        let mut telemetry = TelemetryState::new(now_ms);
        telemetry.push_event(
            inner.tick,
            now_ms,
            "world_start",
            serde_json::json!({
                "seed": &inner.seed,
                "world_seed": inner.world_seed,
                "world_size": inner.world_size(),
                "wrap_world": inner.dims.wrap_world,
                "species_mode": inner.sliders.species_mode,
                "reason": "boot_or_restart",
            }),
        );
        Self {
            inner,
            snapshot_buf: vec![0u8; layout.buf_bytes],
            snapshot_layout: layout,
            biome_pyramid,
            tick_intervals_ms: std::collections::VecDeque::new(),
            last_tick_end_ms: None,
            jank_count: 0,
            telemetry,
            lod_bias: 0.0,
            lod_step: 0,
            master_seed,
        }
    }

    /// Construct with explicit initial grass seed count, energy-max cap,
    /// founder count, the `full_grass_on_init` flag, and (v1.12) an
    /// optional `nn_topology_json` payload. Sole construction path used by
    /// the sim worker.
    ///
    /// `nn_topology_json` accepts `{"hidden_sizes":[…],"activations":[…]}`
    /// where activation strings are `"lrelu"|"relu"|"tanh"|"sigmoid"|"linear"`
    /// and `activations.len() == hidden_sizes.len() + 1`. Pass an empty string
    /// for the legacy 32→48→24→5 default. Returns `Err` (throws on JS side)
    /// if the JSON is malformed or fails `NnTopology::new` validation —
    /// surfaces a typo immediately instead of silently falling back.
    #[wasm_bindgen(js_name = newWithConfigJson)]
    pub fn new_with_config_json(config_json: &str) -> Result<WorldHandle, JsValue> {
        let config: WorldConfig = serde_json::from_str(config_json)
            .map_err(|e| JsValue::from_str(&format!("world_config parse error: {e}")))?;
        Self::new_from_world_config(config)
    }

    fn new_from_world_config(config: WorldConfig) -> Result<WorldHandle, JsValue> {
        if config.schema_version != WORLD_CONFIG_SCHEMA_VERSION {
            return Err(JsValue::from_str(&format!(
                "unsupported WorldConfig schema_version {} (expected {})",
                config.schema_version, WORLD_CONFIG_SCHEMA_VERSION
            )));
        }
        let master_seed = if config.master_seed == 0 {
            random_nonzero_u32()
        } else {
            config.master_seed
        };
        let sim_seed = derive_world_seed(master_seed, WorldSeedStream::SimRng);
        let world_seed = derive_world_seed(master_seed, WorldSeedStream::Biome);
        let actual_seed = format!("master-{master_seed:08x}-sim-{sim_seed:08x}");
        let sliders = crate::world::DevSliders {
            grass_initial_seed_count: config.grass.initial_seed_count,
            energy_max: config.population.energy_max.max(1.0),
            founder_count: config
                .population
                .founder_count
                .clamp(1, MAX_POP_FOR_SIM as u32),
            full_grass_on_init: config.grass.full_on_init,
            world_size: config.world.size,
            wrap_world: config.world.wrap,
            world_seed,
            species_mode: config.species.enabled,
            crossover_mode: config.species.crossover_mode.into(),
            starting_species_count: config.species.starting_species_count.max(1),
            starting_species_member_count: config.species.starting_species_member_count.max(1),
            starting_species_member_variance: config
                .species
                .starting_species_member_variance
                .max(0.0),
            grass_cell_size: config.grass.cell_size.max(1.0),
            grass_multisight: config.grass.multisight,
            grass_clump_count: config.grass.clump_count,
            grass_clump_size: config.grass.clump_size,
            init_graze_boost: config.founders.init_graze_boost.max(0.0),
            init_split_boost: config.founders.init_split_boost.max(0.0),
            ..Default::default()
        };
        let topology = parse_nn_topology_config(&config.nn_topology)
            .map_err(|e| JsValue::from_str(&format!("nn_topology error: {e}")))?;
        let inner = World::new_with_sliders_topology(actual_seed, sliders, topology);
        Ok(Self::from_world_with_master_seed(inner, master_seed))
    }

    #[wasm_bindgen(js_name = newWithFounderCount)]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_founder_count(
        seed: &str,
        initial_grass_seed_count: u32,
        energy_max: f32,
        founder_count: u32,
        full_grass_on_init: bool,
        nn_topology_json: &str,
        world_size: f32,
        wrap_world: bool,
        world_seed: u32,
        // v2.0 Wave 3a: species + sexual-mating construction settings. These
        // shape the world topology + seeding at construction, so they ride the
        // explicit construction path (not the live slider SAB). `crossover_mode`
        // is carried as an f32 (0 = average, non-zero = fifty_fifty).
        species_mode: bool,
        crossover_mode: f32,
        starting_species_count: u32,
        starting_species_member_count: u32,
        starting_species_member_variance: f32,
        // v2.0.4 S2 / v2.0.5 S4: grass cell size construction knob. Floored at 1.0.
        // Must be passed explicitly (not via initial_sliders) because
        // `initial_sliders` is applied AFTER World construction — too late to
        // affect dims. Default 5.0 (= GRASS_CELL_SIZE_DEFAULT) when not set.
        grass_cell_size: f32,
        // v2.0.4 S6 / Wave 2: multi-band grass NN sight. Construction-only:
        // it changes the NN input layout and must be applied before founder
        // brain/topology construction. Default true (= GRASS_MULTISIGHT_DEFAULT).
        grass_multisight: bool,
        // v2.0.6 S3: seeded grass clumps construction knobs. Must ride the
        // explicit boot param (not initial_sliders) — same constraint as
        // grass_cell_size; clump seeding runs at world construction, before
        // initial_sliders is applied. A clump_count of 0 falls back to the
        // old uniform-scatter behaviour.
        grass_clump_count: u32,
        grass_clump_size: u32,
        // Founder action-output boosts. Must ride the explicit boot param (not
        // initial_sliders) — they're read during founder-brain construction,
        // before initial_sliders is applied (same constraint as grass_cell_size).
        // Default 1.0 (= neutral) when not set.
        init_graze_boost: f32,
        init_split_boost: f32,
    ) -> Result<WorldHandle, JsValue> {
        let actual_seed = if seed.is_empty() {
            let mut bytes = [0u8; 8];
            getrandom::getrandom(&mut bytes).ok();
            let n = u64::from_le_bytes(bytes);
            format!("seed-{n:016x}")
        } else {
            seed.to_string()
        };
        // v2.0 Wave 1a: a `world_seed` of 0 from the boot payload means "pick a
        // random one" so every fresh world differs unless the caller pins it.
        // This is SEPARATE from the string RNG seed above — they don't couple.
        let actual_world_seed = if world_seed == 0 {
            let mut bytes = [0u8; 4];
            getrandom::getrandom(&mut bytes).ok();
            u32::from_le_bytes(bytes)
        } else {
            world_seed
        };
        let sliders = crate::world::DevSliders {
            grass_initial_seed_count: initial_grass_seed_count,
            energy_max: energy_max.max(1.0),
            // Raised to the structural ceiling so single-pool worlds can start
            // with a large population. Lineage color is continuous, so no palette
            // limit applies. The safety max-population cull still applies at the
            // first birth phase.
            founder_count: founder_count.clamp(1, MAX_POP_FOR_SIM as u32),
            full_grass_on_init,
            world_size,
            wrap_world,
            world_seed: actual_world_seed,
            // v2.0 Wave 3a species + sexual-mating construction settings.
            species_mode,
            crossover_mode: crate::constants::CrossoverMode::from_slider(crossover_mode),
            starting_species_count: starting_species_count.max(1),
            starting_species_member_count: starting_species_member_count.max(1),
            starting_species_member_variance: starting_species_member_variance.max(0.0),
            // v2.0.4 S2 / v2.0.5 S4: grass cell size — explicit construction arg.
            // Floored at 1.0; passed pre-construction so dims are correct.
            grass_cell_size: grass_cell_size.max(1.0),
            // v2.0.4 S6: explicit construction arg so restart applies the
            // persisted setting before NN input-layout/topology construction.
            grass_multisight,
            // v2.0.6 S3: seeded grass clumps — explicit construction args.
            // clump_count=0 falls back to uniform-scatter.
            grass_clump_count,
            grass_clump_size,
            // Founder action-output boosts — explicit construction args (floored
            // at 0; 1.0 = neutral). Read during founder-brain seeding.
            init_graze_boost: init_graze_boost.max(0.0),
            init_split_boost: init_split_boost.max(0.0),
            ..Default::default()
        };
        let topology = parse_nn_topology(nn_topology_json)
            .map_err(|e| JsValue::from_str(&format!("nn_topology error: {e}")))?;
        let inner = World::new_with_sliders_topology(actual_seed, sliders, topology);
        Ok(Self::from_world(inner))
    }

    /// One tick. Returns true while alive.
    #[wasm_bindgen]
    pub fn step(&mut self) -> bool {
        self.inner.tick_once()
    }

    fn unknown_phase_attribution() -> JankPhaseAttribution {
        JankPhaseAttribution {
            phase: "unknown".to_string(),
            confidence: "unavailable".to_string(),
            reason: "whole-tick jank is measured around WorldHandle::step_n; profiler spans are rolling aggregates, not per-jank-tick traces".to_string(),
        }
    }

    fn grass_aggregate(&self) -> (u32, f32) {
        let mut live = 0u32;
        let mut total = 0.0f32;
        for c in 0..self.inner.grass.density.len() {
            let byte = self.inner.grass.dget_u8(c);
            if byte > 0 {
                live = live.saturating_add(1);
            }
            total += self.inner.grass.dget(c);
        }
        (live, total)
    }

    fn maybe_sample_telemetry(&mut self, now_ms: f64) {
        let tick = self.inner.tick;
        if tick == 0 || !tick.is_multiple_of(TELEMETRY_SAMPLE_PERIOD_TICKS) {
            return;
        }
        let (live_grass_cell_count, total_grass_density) = self.grass_aggregate();
        let prev_jank = self.telemetry.last_sample_jank_count;
        let jank_delta = self.jank_count.saturating_sub(prev_jank);
        let sample = TelemetrySample {
            tick_start: self.telemetry.last_sample_tick,
            tick_end: tick,
            at_ms: now_ms,
            wall_elapsed_ms: (now_ms - self.telemetry.last_sample_ms).max(0.0),
            population: self.inner.population(),
            tps: self.tps(),
            jank_count_delta: jank_delta,
            jank_count_total: self.jank_count,
            live_grass_cell_count,
            total_grass_density,
        };
        self.telemetry.last_sample_tick = tick;
        self.telemetry.last_sample_ms = now_ms;
        self.telemetry.last_sample_jank_count = self.jank_count;
        self.telemetry.push_sample(sample);
    }

    fn observe_completed_tick(
        &mut self,
        tick_started_at_ms: f64,
        tick_ended_at_ms: f64,
        alive: bool,
    ) {
        let work_dur_ms = (tick_ended_at_ms - tick_started_at_ms).max(0.0);
        if let Some(prev) = self.last_tick_end_ms {
            let interval_ms = tick_ended_at_ms - prev;
            if self.tick_intervals_ms.len() >= TPS_WINDOW {
                self.tick_intervals_ms.pop_front();
            }
            self.tick_intervals_ms.push_back(interval_ms);
        }
        self.last_tick_end_ms = Some(tick_ended_at_ms);

        if work_dur_ms > JANK_BUDGET_MS {
            self.jank_count = self.jank_count.saturating_add(1);
            let worst_replaced = self
                .telemetry
                .worst_jank
                .as_ref()
                .is_none_or(|worst| work_dur_ms > worst.duration_ms);
            if worst_replaced {
                let worst = WorstJank {
                    tick: self.inner.tick,
                    duration_ms: work_dur_ms,
                    population: self.inner.population(),
                    phase: Self::unknown_phase_attribution(),
                };
                self.telemetry.worst_jank = Some(worst.clone());
                self.telemetry.push_event(
                    self.inner.tick,
                    tick_ended_at_ms,
                    "jank_worst",
                    serde_json::json!({
                        "duration_ms": work_dur_ms,
                        "population": self.inner.population(),
                        "phase": &worst.phase,
                    }),
                );
            } else if work_dur_ms >= LARGE_JANK_EVENT_MS {
                self.telemetry.push_event(
                    self.inner.tick,
                    tick_ended_at_ms,
                    "jank_large",
                    serde_json::json!({
                        "duration_ms": work_dur_ms,
                        "population": self.inner.population(),
                        "phase": Self::unknown_phase_attribution(),
                    }),
                );
            }
        }

        if !alive && !self.telemetry.world_end_logged {
            self.telemetry.world_end_logged = true;
            self.telemetry.push_event(
                self.inner.tick,
                tick_ended_at_ms,
                "world_ended",
                serde_json::json!({
                    "reason": if self.inner.population() == 0 { "extinction" } else { "ended" },
                    "population": self.inner.population(),
                }),
            );
        }

        self.maybe_sample_telemetry(tick_ended_at_ms);
    }

    /// Run multiple ticks; stops early on game-over.
    /// Tracks per-tick wall-clock duration for TPS and jank accounting.
    #[wasm_bindgen]
    pub fn step_n(&mut self, n: u32) -> bool {
        for _ in 0..n {
            let t0 = wasm_now_ms();
            let alive = self.inner.tick_once();
            let now = wasm_now_ms();
            self.observe_completed_tick(t0, now, alive);
            if !alive {
                return false;
            }
        }
        true
    }

    /// Rolling average ticks-per-second over the last `TPS_WINDOW` tick
    /// intervals (tick-end-to-tick-end, includes pacing waits). Returns 0.0
    /// until at least one interval has been observed.
    #[wasm_bindgen(getter)]
    pub fn tps(&self) -> f32 {
        let n = self.tick_intervals_ms.len();
        if n == 0 {
            return 0.0;
        }
        let total_ms: f64 = self.tick_intervals_ms.iter().sum();
        if total_ms <= 0.0 {
            return 0.0;
        }
        (n as f64 / (total_ms / 1000.0)) as f32
    }

    /// Cumulative count of ticks that exceeded JANK_BUDGET_MS.
    #[wasm_bindgen(getter)]
    pub fn jank_count(&self) -> u32 {
        self.jank_count
    }

    /// Reset the jank counter to zero.
    #[wasm_bindgen]
    pub fn reset_jank(&mut self) {
        self.jank_count = 0;
        self.telemetry.worst_jank = None;
        self.telemetry.last_sample_jank_count = 0;
        let now_ms = wasm_now_ms();
        self.telemetry.push_event(
            self.inner.tick,
            now_ms,
            "jank_reset",
            serde_json::json!({
                "history_preserved": true,
            }),
        );
    }

    /// v1.10: publish a worker-side JS-measured span into the always-on Rust
    /// profile tree so it reaches the perf panel through `profile_report_json`.
    /// Used by the sim worker for the three `sim_worker.*` spans bracketing
    /// one loop iteration (the worker's TS perf module is a separate instance
    /// from main's, so we route through the Rust profiler instead).
    #[wasm_bindgen]
    pub fn record_profile_sample(&self, root: &str, path: &str, dur_us: u32, call_count: u32) {
        self.inner
            .profile
            .record_under_root(root, path, dur_us, call_count.max(1));
    }

    #[wasm_bindgen(getter)]
    pub fn tick(&self) -> u32 {
        self.inner.tick
    }

    #[wasm_bindgen(getter)]
    pub fn seed(&self) -> String {
        self.inner.seed.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn population(&self) -> u32 {
        self.inner.population()
    }

    #[wasm_bindgen(getter)]
    pub fn world_ended(&self) -> bool {
        self.inner.world_ended
    }

    /// Runtime world extent in world-units. v2.0 Wave 1a: derived from the
    /// construction `world_size` setting (no longer a compile-time constant).
    #[wasm_bindgen(getter)]
    pub fn world_size(&self) -> f32 {
        self.inner.world_size()
    }

    /// Whether the world is toroidal (wrap) or walled. v2.0 Wave 1a. The
    /// renderer mirrors this for trail/cull seam handling (TS stream).
    #[wasm_bindgen(getter)]
    pub fn wrap_world(&self) -> bool {
        self.inner.dims.wrap_world
    }

    /// Numeric world seed (v2.0 Wave 1a). Carried for biome gen (Wave 1b);
    /// separate from the string RNG seed.
    #[wasm_bindgen(getter)]
    pub fn world_seed(&self) -> u32 {
        self.inner.world_seed
    }

    #[wasm_bindgen(getter)]
    pub fn master_seed(&self) -> u32 {
        self.master_seed
    }

    // ─── Grass render API (P1g) ──────────────────────────────────────────────

    /// Runtime grass grid dimension (cells per axis). v2.0 Wave 1a: computed at
    /// boot from `world_size` (1920 at the 9600u default). The boot handshake
    /// carries this so the main thread sizes its grass + biome views.
    #[wasm_bindgen(getter)]
    pub fn grass_dim(&self) -> u32 {
        self.inner.dims.grass_dim as u32
    }

    /// Grass cell size in world-units. v2.0.4 S2: returns the construction-time
    /// value (default 5.0; adjustable via the `grass_size` slider). Used by the
    /// render layer for world→screen UV computation. Exposed in the boot_ready
    /// payload so the renderer reads the correct cell size even at non-default
    /// slider values.
    #[wasm_bindgen(getter)]
    pub fn grass_cell_size(&self) -> f32 {
        self.inner.dims.grass_cell_size
    }

    // ─── v1.11 (A+D): wasm-memory snapshot writer ────────────────────────────

    /// Byte offset of the snapshot region's first byte inside wasm linear
    /// memory. Main thread constructs `new Uint8Array(wasm.memory.buffer,
    /// snapshot_buf_byte_offset(), SNAPSHOT_BUF_BYTES)` after `boot_ready`
    /// and reuses that view across frames.
    #[wasm_bindgen(getter)]
    pub fn snapshot_buf_byte_offset(&self) -> u32 {
        self.snapshot_buf.as_ptr() as u32
    }

    /// Total length of the snapshot region in bytes. Stable across the
    /// worker's lifetime — no growth, no reallocation. v2.0: runtime value
    /// (grass region is u8, sized from `grass_cell_count`).
    #[wasm_bindgen(getter)]
    pub fn snapshot_buf_byte_len(&self) -> u32 {
        self.snapshot_layout.buf_bytes as u32
    }

    /// Byte offset within the snapshot region for slot `slot` (0 or 1).
    #[wasm_bindgen(getter)]
    pub fn snapshot_slot_bytes(&self) -> u32 {
        self.snapshot_layout.slot_bytes as u32
    }

    /// Bytes per snapshot stats header (header + alignment padding).
    #[wasm_bindgen(getter)]
    pub fn snapshot_header_bytes(&self) -> u32 {
        SNAPSHOT_HEADER_BYTES as u32
    }

    /// Bytes per creature region per slot. UNCHANGED in v2.0 (32 B stride ×
    /// `MAX_POP_FOR_SIM`).
    #[wasm_bindgen(getter)]
    pub fn snapshot_creature_bytes(&self) -> u32 {
        SNAPSHOT_CREATURE_BYTES as u32
    }

    /// Bytes per grass region per slot. v2.0 Wave 1a: u8 grass, one byte per
    /// cell → `grass_cell_count` bytes (runtime).
    #[wasm_bindgen(getter)]
    pub fn snapshot_grass_bytes(&self) -> u32 {
        self.snapshot_layout.grass_bytes as u32
    }

    /// v2.0.3 Stream 2d: bytes per biome window region per slot.
    /// = min(grass_dim, budget_axis)² — same allocation budget as grass_bytes.
    /// The actual written bytes are win_w × win_h per tick (from window metadata).
    #[wasm_bindgen(getter)]
    pub fn snapshot_biome_win_bytes(&self) -> u32 {
        self.snapshot_layout.biome_win_bytes as u32
    }

    /// Write a full snapshot (stats header + creature SoA + clipmap grass window)
    /// into the requested slot of `self.snapshot_buf` (which lives in wasm linear
    /// memory). No JS-side `Uint8Array` parameters — main reads the same
    /// region via a view over `wasm.memory.buffer`. The atomic flip + seq
    /// bump on the control SAB remain the worker's responsibility.
    ///
    /// v2.0.3 Stream 2b: the camera parameters (`cam_cx`, `cam_cy`, `cam_zoom`,
    /// `viewport_w`, `viewport_h`) are supplied by the worker (read from the
    /// camera SAB lanes). The function computes the LOD level and clipmap window
    /// from these values, fills `grass_region` via `pyramid.viewport_window`, and
    /// writes the window metadata into header bytes [32..64).
    ///
    /// Layout per slot (little-endian throughout):
    ///   `[0..20)`    tick, pop, world_ended, tps_bits, jank_count (u32 each)
    ///   `[20..32)`   padding (unchanged; 32-byte align for creature SoA)
    ///   `[32..64)`   window metadata: mip_level (u32), win_origin_x/y (i32),
    ///                win_origin_y, win_w, win_h, tex_dim_w, tex_dim_h, wrap_mode
    ///   `[64..64+SNAPSHOT_CREATURE_BYTES)` creature SoA, 32 B stride
    ///   `[grass region: min(grass_dim,2048)² bytes]` clipmap window grass bytes
    ///   `[biome window: min(grass_dim,2048)² bytes]` mode-downsampled biome tags
    ///
    /// v2.0.3 Stream 2d: the biome window immediately follows the grass region.
    /// It holds the same win_w × win_h bytes mode-downsampled from the static
    /// level-0 biome grid. UV transform is identical to the grass channel.
    #[wasm_bindgen]
    pub fn write_snapshot(
        &mut self,
        slot: u32,
        cam_cx: f32,
        cam_cy: f32,
        cam_zoom: f32,
        viewport_w: u32,
        viewport_h: u32,
    ) {
        let profile_on = self.inner.profile.enabled();
        let snap_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };

        let slot_bytes = self.snapshot_layout.slot_bytes;
        let grass_bytes = self.snapshot_layout.grass_bytes;
        let grass_dim = self.snapshot_layout.grass_dim;
        let slot_idx = (slot as usize) & 1;
        let slot_base = slot_idx * slot_bytes;

        // ── v2.0.4 S1 / v2.0.3 Stream 2b: Compute clipmap window from camera params ─
        //
        // LOD formula: level = floor(log2(max(1.0, visible_cell_span / budget_axis)))
        //   clamped to [0, max_level].
        // visible_cell_span_x = (world_size / cam_zoom) / GRASS_CELL_SIZE
        //   (span in the horizontal direction; see also vis_cells_y for vertical).
        //
        // Default scale (world_size=9600, zoom=1.0, GRASS_CELL_SIZE=5.0):
        //   visible_cell_span_x = (9600 / 1.0) / 5.0 = 1920
        //   ratio = 1920 / 4096 ≈ 0.469  (< 1.0)
        //   .max(1.0) clamps ratio to 1.0 → log2(1.0) = 0 → floor(0) = 0
        //   → full field at default zoom.
        //
        // v2.0.4 S1 lod_bias: subtract self.lod_bias from the computed float
        //   level before floor+clamp so the renderer picks a finer level.
        //   Clamped to ≥ 0 after bias so we never pick a negative level.
        //   WARNING: if lod_bias drives level below what the budget can hold,
        //   the window will clamp at GRASS_LOD_BUDGET_AXIS and crop edges.
        let world_size = self.inner.world_size();
        // v2.0.4 S2: use the construction-time cell size (not the constant 5.0)
        // so the LOD threshold is correct when grass_size != 5.0.
        let cell_size = self.inner.dims.grass_cell_size;
        let safe_zoom = if cam_zoom > 0.0 { cam_zoom } else { 1.0 };
        let visible_cell_span_x = (world_size / safe_zoom) / cell_size;
        let max_level = self.inner.grass.pyramid.num_levels().saturating_sub(1);
        // v2.0.6 S10: discrete LOD stepper. When lod_step > 0, force the absolute
        // mip level = (lod_step - 1) clamped to [0, max_level]. The stepper option
        // 1 (lod_step=1) maps to L0 (full resolution) — one finer step than auto
        // in zoom-out scenarios. Options 2–5 map to L1–L4 (increasingly coarse).
        // When lod_step == 0 (Auto), use the original formula + lod_bias.
        let mip_level = if self.lod_step > 0 {
            ((self.lod_step - 1) as usize).min(max_level)
        } else {
            // Auto: compute from visible span + apply lod_bias nudge.
            let level_f = (visible_cell_span_x / GRASS_LOD_BUDGET_AXIS as f32)
                .max(1.0)
                .log2()
                .floor();
            // Apply lod_bias (finer detail): subtract, floor, clamp ≥ 0.
            let biased_level_f = (level_f - self.lod_bias.max(0.0)).max(0.0);
            (biased_level_f as usize).min(max_level)
        };

        // Level dims at chosen mip level.
        let (level_w, level_h) = self.inner.grass.pyramid.level_dim(mip_level);

        // Visible region in level-k cells (derived from viewport + zoom + mip scale).
        // At level k, each cell represents 2^k base cells.
        //
        // v2.0.4 S1 aspect-ratio fix: derive vis_cells_y from viewport_h
        // independently (previously vis_cells_y = vis_cells_x which assumed a
        // square viewport and mis-sized the vertical window on non-square viewports).
        // The horizontal visible span is world_size/zoom; the vertical span scales
        // by the viewport aspect (viewport_h / viewport_w), or 1.0 if viewport_w=0.
        let scale = (1usize << mip_level) as f32;
        let vis_cells_x = (visible_cell_span_x / scale).ceil() as usize;
        let aspect = if viewport_w > 0 {
            viewport_h as f32 / viewport_w as f32
        } else {
            1.0
        };
        let visible_cell_span_y = visible_cell_span_x * aspect;
        let vis_cells_y = (visible_cell_span_y / scale).ceil() as usize;

        // Window = viewport × GRASS_LOD_MARGIN_FACTOR, clamped to level bounds.
        // The margin ensures that a full-viewport pan stays covered for ≥ 1 tick.
        let raw_win_w = ((vis_cells_x as f32 * GRASS_LOD_MARGIN_FACTOR).ceil() as usize)
            .min(level_w)
            .min(GRASS_LOD_BUDGET_AXIS);
        let raw_win_h = ((vis_cells_y as f32 * GRASS_LOD_MARGIN_FACTOR).ceil() as usize)
            .min(level_h)
            .min(GRASS_LOD_BUDGET_AXIS);

        // Window origin: center on camera. Toroidal worlds publish the signed
        // logical origin so render can map ghost tiles into the rotated upload;
        // copy-out normalizes that origin modulo the level dimensions. Walled
        // worlds retain edge clamps for both metadata and copy-out.
        // Camera world coords → level-k cell coords.
        let cam_cx_cells = (cam_cx / (cell_size * scale)).floor() as isize;
        let cam_cy_cells = (cam_cy / (cell_size * scale)).floor() as isize;
        let half_w = (raw_win_w / 2) as isize;
        let half_h = (raw_win_h / 2) as isize;
        let raw_origin_x = cam_cx_cells - half_w;
        let raw_origin_y = cam_cy_cells - half_h;
        let wrap = self.inner.dims.wrap_world;
        let copy_origin_x = snapshot_window_copy_origin(raw_origin_x, level_w, raw_win_w, wrap);
        let copy_origin_y = snapshot_window_copy_origin(raw_origin_y, level_h, raw_win_h, wrap);
        let meta_origin_x = if wrap {
            raw_origin_x
        } else {
            copy_origin_x as isize
        };
        let meta_origin_y = if wrap {
            raw_origin_y
        } else {
            copy_origin_y as isize
        };
        let win_w = raw_win_w.min(level_w);
        let win_h = raw_win_h.min(level_h);

        // DEFAULT-SCALE INVARIANT: at default (zoom=1, world_size=9600, grass_dim=1920,
        //   square 1920×1080 viewport):
        //   mip_level=0 (budget=4096, span=1920, ratio=0.469<1.0 → level=0).
        //   visible_cell_span_x=1920, aspect=1080/1920≈0.5625, span_y≈1080.
        //   raw_win_w = ceil(1920 * 1.5) clamped to min(1920, 4096) = 1920.
        //   raw_win_h = ceil(1080 * 1.5) = 1620, clamped to min(1920, 4096) = 1620.
        //   win_w=1920, win_h=1620. Full horizontal field, correct vertical slice.
        _ = grass_dim; // verified via debug_assert below

        debug_assert!(
            win_w * win_h <= grass_bytes,
            "window ({win_w}×{win_h}={}) exceeds allocated slot grass_bytes {grass_bytes}",
            win_w * win_h
        );

        // Creatures: write the per-creature 32 B records directly into the
        // creature region. No JS boundary — `write_creatures_each` builds an
        // f32x8 scratch per creature and we copy it byte-for-byte. v1.11: the
        // dest is just a `&mut [u8]` slice inside our own wasm-memory buffer.
        let creatures_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };
        let creatures_off = slot_base + SNAPSHOT_HEADER_BYTES;
        let pop_written = {
            let buf = &mut self.snapshot_buf;
            let creatures_region = &mut buf[creatures_off..creatures_off + SNAPSHOT_CREATURE_BYTES];
            write_creatures_each_into(&self.inner, creatures_region)
        };
        let creatures_end = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };

        // Stats header — bytes [0..20) of the slot, LE.
        let tps_bits = self.tps().to_bits();
        let jank = self.jank_count;
        let tick = self.inner.tick;
        let ended = if self.inner.world_ended { 1u32 } else { 0u32 };
        {
            let header = &mut self.snapshot_buf[slot_base..slot_base + 20];
            header[0..4].copy_from_slice(&tick.to_le_bytes());
            header[4..8].copy_from_slice(&(pop_written as u32).to_le_bytes());
            header[8..12].copy_from_slice(&ended.to_le_bytes());
            header[12..16].copy_from_slice(&tps_bits.to_le_bytes());
            header[16..20].copy_from_slice(&jank.to_le_bytes());
        }

        // v2.0.3 Stream 2b: window metadata — bytes [32..64) of the slot, LE.
        // Window metadata: mip_level u32, logical win_origin_x/y i32, then
        // win_w, win_h, tex_dim_w, tex_dim_h, wrap_mode as u32.
        let wrap_mode: u32 = if self.inner.dims.wrap_world { 1 } else { 0 };
        {
            let wmeta = &mut self.snapshot_buf[slot_base + 32..slot_base + 64];
            wmeta[0..4].copy_from_slice(&(mip_level as u32).to_le_bytes());
            wmeta[4..8].copy_from_slice(&(meta_origin_x as i32).to_le_bytes());
            wmeta[8..12].copy_from_slice(&(meta_origin_y as i32).to_le_bytes());
            wmeta[12..16].copy_from_slice(&(win_w as u32).to_le_bytes());
            wmeta[16..20].copy_from_slice(&(win_h as u32).to_le_bytes());
            wmeta[20..24].copy_from_slice(&(win_w as u32).to_le_bytes()); // tex_dim_w = win_w
            wmeta[24..28].copy_from_slice(&(win_h as u32).to_le_bytes()); // tex_dim_h = win_h
            wmeta[28..32].copy_from_slice(&wrap_mode.to_le_bytes());
        }

        // Grass: v2.0.3 Stream 2b — extract clipmap window via pyramid.viewport_window.
        // At default scale: mip_level=0, win=(0,0,1920,1920), full-field copy.
        let grass_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };
        let grass_off = slot_base + SNAPSHOT_HEADER_BYTES + SNAPSHOT_CREATURE_BYTES;
        let grass_region = &mut self.snapshot_buf[grass_off..grass_off + grass_bytes];
        // Split-borrow: density and pyramid are separate fields of grass.
        let density = &self.inner.grass.density;
        let pyramid = &self.inner.grass.pyramid;
        pyramid.viewport_window(
            density,
            mip_level,
            wrap,
            copy_origin_x,
            copy_origin_y,
            win_w,
            win_h,
            grass_region,
        );
        let grass_end = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };

        // v2.0.3 Stream 2d / v2.0.6 S9: Biome window — mode-downsampled, same window as grass.
        //
        // v2.0.6 S9 OPTIMIZATION: the biome grid is static (never changes after
        // world construction). The old path recomputed the mode-downsampled window
        // on every tick via a nested O(win_w × win_h × block²) loop. We now use
        // a precomputed `BiomePyramid` (built once at construction) and replace
        // the loop with a cheap window-slice copy. Before: ~11ms/call at default
        // scale (1920×1920 window, mip_level=0). After: ~0.1ms/call.
        let biome_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };
        let biome_win_off = grass_off + grass_bytes;
        let biome_win_bytes = self.snapshot_layout.biome_win_bytes;
        {
            let biome_region =
                &mut self.snapshot_buf[biome_win_off..biome_win_off + biome_win_bytes];
            // Copy the precomputed level-k window into the snapshot region.
            // `copy_window` is a simple row-by-row memcpy from the pyramid level.
            self.biome_pyramid.copy_window(
                mip_level,
                wrap,
                copy_origin_x,
                copy_origin_y,
                win_w,
                win_h,
                &mut biome_region[..win_w * win_h],
            );
        }
        let biome_end = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };

        // v2.0.4 S1: viewport_w and viewport_h are now used in the aspect-ratio fix
        // (vis_cells_y derivation above) — the suppress is no longer needed.

        if let (Some(start), Some(c0), Some(c1), Some(g0), Some(g1), Some(b0), Some(b1)) = (
            snap_start,
            creatures_start,
            creatures_end,
            grass_start,
            grass_end,
            biome_start,
            biome_end,
        ) {
            let prof = &self.inner.profile;
            prof.record_under_root(
                "sim_worker",
                "write_output_sab.snapshot",
                b1.saturating_sub(start) as u32,
                1,
            );
            prof.record_under_root(
                "sim_worker",
                "write_output_sab.snapshot.creatures",
                c1.saturating_sub(c0) as u32,
                pop_written.max(1) as u32,
            );
            prof.record_under_root(
                "sim_worker",
                "write_output_sab.snapshot.grass_copy",
                g1.saturating_sub(g0) as u32,
                1,
            );
            prof.record_under_root(
                "sim_worker",
                "write_output_sab.snapshot.biome",
                b1.saturating_sub(b0) as u32,
                1,
            );
        }
    }

    // ─── Per-slider typed setters (S17) ─────────────────────────────────────
    // Private apply_* helpers: exactly one mutation site per DevSliders field.

    fn apply_mutation_rate_multiplier(&mut self, value: f32) {
        self.inner.sliders.mutation_rate_multiplier = value;
    }
    /// v1.12: bucket field is 0=weight, 1=rate, 2=sigma. All clamped non-negative
    /// so the cumulative-weight scan in `Brain::child_from` is safe.
    fn apply_mutation_bucket(&mut self, bucket: usize, field: usize, value: f32) {
        if bucket >= MUTATION_BUCKET_COUNT {
            return;
        }
        let b = &mut self.inner.sliders.mutation_policy.buckets[bucket];
        let v = value.max(0.0);
        match field {
            0 => b.weight = v,
            1 => b.rate = v.min(1.0),
            2 => b.sigma = v,
            _ => {}
        }
    }
    fn apply_eat_bite_fraction(&mut self, value: f32) {
        self.inner.sliders.eat_bite_fraction = value;
    }
    fn apply_grass_propagation_rate_k(&mut self, value: f32) {
        self.inner.sliders.grass_propagation_rate_k = value;
    }
    fn apply_grass_in_cell_growth_r(&mut self, value: f32) {
        self.inner.sliders.grass_in_cell_growth_r = value;
    }
    fn apply_upkeep_multiplier(&mut self, value: f32) {
        self.inner.sliders.upkeep_multiplier = value.max(0.0);
    }
    fn apply_move_cost_multiplier(&mut self, value: f32) {
        self.inner.sliders.move_cost_multiplier = value.max(0.0);
    }
    fn apply_energy_max(&mut self, value: f32) {
        // Guard against pathological values; the slider UI already clamps to [50, 400].
        self.inner.sliders.energy_max = value.max(1.0);
    }
    fn apply_grass_energy_per_bite(&mut self, value: f32) {
        self.inner.sliders.grass_energy_per_bite = value.max(0.0);
    }
    fn apply_grass_bites_per_block(&mut self, value: u32) {
        self.inner.sliders.grass_bites_per_block = value.max(1);
    }
    fn apply_digestion_cooldown(&mut self, value: u32) {
        self.inner.sliders.digestion_cooldown_ticks = value;
    }
    fn apply_repulsion_max(&mut self, value: f32) {
        self.inner.sliders.repulsion_max = value.max(0.0);
    }
    fn apply_max_age(&mut self, value: u32) {
        self.inner.sliders.max_age = value.max(1);
    }
    fn apply_split_threshold(&mut self, value: f32) {
        self.inner.sliders.split_threshold = value.max(0.0);
    }
    fn apply_split_gift(&mut self, value: f32) {
        self.inner.sliders.split_gift = value.max(0.0);
    }
    fn apply_split_jitter(&mut self, value: f32) {
        self.inner.sliders.split_jitter = value.max(0.0);
    }
    fn apply_founder_count(&mut self, value: u32) {
        // Stored for next world construction (active world keeps its current population).
        // v2.0.x: cap at the structural ceiling (was 32) so single-pool worlds can
        // seed a large founder population. See new_with_founder_count.
        self.inner.sliders.founder_count = value.clamp(1, MAX_POP_FOR_SIM as u32);
    }
    /// v2.0 Wave 1a construction-only: world extent for the *next* world. Stored
    /// on DevSliders so the boot payload round-trips it; live world keeps its dims.
    fn apply_world_size(&mut self, value: f32) {
        self.inner.sliders.world_size = value.max(crate::constants::GRASS_CELL_SIZE);
    }
    /// v2.0 Wave 1a construction-only: numeric world seed (biome gen, Wave 1b).
    fn apply_world_seed(&mut self, value: u32) {
        self.inner.sliders.world_seed = value;
    }
    /// v2.0 Wave 1a construction-only: torus vs walled for the next world.
    fn apply_wrap_world(&mut self, value: bool) {
        self.inner.sliders.wrap_world = value;
    }
    // v2.1 P2: sliders 21/22/23 are retired no-ops (genome + biome penalties removed).
    fn apply_reserved_legacy_noop(&mut self, _value: f32) {
        // No-op: index retired; field no longer exists on DevSliders.
    }
    /// v2.0 Wave 3a construction-only: species + sexual-mating mode for the
    /// *next* world. Live world keeps its current mode.
    fn apply_species_mode(&mut self, value: bool) {
        self.inner.sliders.species_mode = value;
    }
    /// v2.0 Wave 3a construction-only: crossover policy (0 = average, non-zero =
    /// fifty_fifty). Live world keeps its current policy.
    fn apply_crossover_mode(&mut self, value: f32) {
        self.inner.sliders.crossover_mode = crate::constants::CrossoverMode::from_slider(value);
    }
    /// v2.0 Wave 3a construction-only: number of seeded species. Clamped ≥ 1.
    fn apply_starting_species_count(&mut self, value: u32) {
        self.inner.sliders.starting_species_count = value.max(1);
    }
    /// v2.0 Wave 3a construction-only: founders per species. Clamped ≥ 1.
    fn apply_starting_species_member_count(&mut self, value: u32) {
        self.inner.sliders.starting_species_member_count = value.max(1);
    }
    /// v2.0 Wave 3a construction-only: per-founder spread multiplier. ACCEPTED
    /// but INERT in Wave 3 (the placeholder seeding ignores it); Wave 4 applies
    /// it to the per-founder bucket draw. Clamped non-negative.
    fn apply_starting_species_member_variance(&mut self, value: f32) {
        self.inner.sliders.starting_species_member_variance = value.max(0.0);
    }
    /// v2.0 Wave 3a live: initiator-only post-mating cooldown in ticks.
    fn apply_mating_cooldown_ticks(&mut self, value: u32) {
        self.inner.sliders.mating_cooldown_ticks = value;
    }
    fn apply_max_population(&mut self, value: u32) {
        // Clamp into [1, MAX_POP_FOR_SIM]. The SAB-bound cap is structural —
        // we never let the soft cap exceed it (extra births above MAX_POP_FOR_SIM
        // would have no slot in the snapshot creature region).
        self.inner.sliders.max_population = value.clamp(1, MAX_POP_FOR_SIM as u32);
    }
    // ── v2.0.4 S1: render-side LOD knob ───────────────────────────────────
    /// LOD bias — subtracted from the computed mip level in `write_snapshot`.
    /// Clamped non-negative so negative input is a no-op. Biasing finer than the
    /// GRASS_LOD_BUDGET_AXIS can hold causes the window to clamp and crop edges.
    fn apply_lod_bias(&mut self, value: f32) {
        self.lod_bias = value.max(0.0);
    }
    // ── v2.0.6 S10: discrete effective-resolution stepper ─────────────────
    /// Discrete LOD stepper: 0 = Auto (use formula + lod_bias), 1 = L0 (full),
    /// 2 = L1, 3 = L2, 4 = L3, 5 = L4. When non-zero, forces the absolute mip
    /// level and overrides lod_bias. Clamped to a reasonable maximum (16) to
    /// prevent runaway values; the actual level is further clamped to max_level
    /// in write_snapshot.
    fn apply_lod_step(&mut self, value: u32) {
        self.lod_step = value.min(16);
    }

    // ── v2.0.2 Stream 1d: scatter kernel live setters ─────────────────────
    /// Decay probability per tick, clamped [0, 1].
    fn apply_grass_decay_pct(&mut self, value: f32) {
        self.inner.sliders.grass_decay_pct = value.clamp(0.0, 1.0);
    }
    /// Decay amount (f32 grass-amount domain), clamped non-negative.
    fn apply_grass_decay_amount(&mut self, value: f32) {
        self.inner.sliders.grass_decay_amount = value.max(0.0);
    }
    /// Spread probability per tick, clamped [0, 1].
    fn apply_grass_spread_pct(&mut self, value: f32) {
        self.inner.sliders.grass_spread_pct = value.clamp(0.0, 1.0);
    }
    /// Spread amount (f32 grass-amount domain), clamped non-negative.
    fn apply_grass_spread_amount(&mut self, value: f32) {
        self.inner.sliders.grass_spread_amount = value.max(0.0);
    }
    /// Ring-1 disc-band weight, clamped [0, 1].
    fn apply_grass_spread_ring1_pct(&mut self, value: f32) {
        self.inner.sliders.grass_spread_ring1_pct = value.clamp(0.0, 1.0);
    }
    /// Ring-2 disc-band weight, clamped [0, 1].
    fn apply_grass_spread_ring2_pct(&mut self, value: f32) {
        self.inner.sliders.grass_spread_ring2_pct = value.clamp(0.0, 1.0);
    }
    /// v2.0.4 S6: construction-only multi-band grass NN sight toggle.
    /// Writing to the slider updates the DevSliders field so the NEXT world
    /// picks up the value; the running world's layout is unchanged.
    fn apply_grass_multisight(&mut self, value: bool) {
        self.inner.sliders.grass_multisight = value;
    }
    /// v2.0.4 S2: construction-only grass cell size. Floored at 1.0 to prevent
    /// a degenerate grid. The active world keeps its dims; only shapes next world.
    fn apply_grass_cell_size(&mut self, value: f32) {
        self.inner.sliders.grass_cell_size = value.max(1.0);
    }
    /// Live multiplier on the mating contact radius (species mode). Floored at
    /// 0 (0 disables mating reach entirely; 1.0 = touching-bodies default).
    fn apply_mate_reach_multiplier(&mut self, value: f32) {
        self.inner.sliders.mate_reach_multiplier = value.max(0.0);
    }
    /// Construction-only founder Graze-output boost. Floored at 0; only shapes
    /// the next world's founders (the running world keeps its brains).
    fn apply_init_graze_boost(&mut self, value: f32) {
        self.inner.sliders.init_graze_boost = value.max(0.0);
    }
    /// Construction-only founder Split/Mate-output boost. Floored at 0; only
    /// shapes the next world's founders.
    fn apply_init_split_boost(&mut self, value: f32) {
        self.inner.sliders.init_split_boost = value.max(0.0);
    }

    // ── v2.1 P3: food-limited equilibrium live setters ───────────────────────
    /// Extra upkeep per crowding neighbour per tick. Floored at 0.
    fn apply_crowding_strength(&mut self, value: f32) {
        self.inner.sliders.crowding_strength = value.max(0.0);
    }
    /// Radius for crowding neighbour count (world-units). Clamped to the
    /// proximity-query range so the spatial hash remains a tight candidate set.
    fn apply_crowding_radius(&mut self, value: f32) {
        self.inner.sliders.crowding_radius = value.clamp(0.0, PROXIMITY_RANGE);
    }
    /// Starvation threshold (energy floor). Floored at 0.
    fn apply_starvation_threshold(&mut self, value: f32) {
        self.inner.sliders.starvation_threshold = value.max(0.0);
    }
    /// Extra per-tick drain below starvation threshold. Floored at 0.
    fn apply_starvation_drain_rate(&mut self, value: f32) {
        self.inner.sliders.starvation_drain_rate = value.max(0.0);
    }
    /// Grass carrying-capacity scale multiplier, clamped [0, 10].
    fn apply_grass_capacity_scale(&mut self, value: f32) {
        self.inner.sliders.grass_capacity_scale = value.clamp(0.0, 10.0);
    }
    /// Grass regrowth rate multiplier, floored at 0.
    fn apply_grass_regrowth_rate(&mut self, value: f32) {
        self.inner.sliders.grass_regrowth_rate = value.max(0.0);
    }
    /// Apply a dev-panel slider live by name. JS console workflow
    /// (BUILD-REPORT Known Issue #4). Returns `Err` on unknown name so a
    /// console typo is visible instead of silently ignored.
    ///
    /// JS-side semantics: `Err(JsValue)` causes the generated wrapper to
    /// throw, so a typo at the console prompt surfaces immediately.
    #[wasm_bindgen]
    pub fn set_slider(&mut self, name: &str, value: f32) -> Result<(), JsValue> {
        if !self.try_set_slider(name, value) {
            return Err(JsValue::from_str(&format!("unknown slider: {name}")));
        }
        Ok(())
    }

    /// SAB hot-path entry: apply slider by canonical index (see
    /// [`SLIDER_NAMES`]). Indices are stable and shared with TS via
    /// `src/web/src/generated/slider-ids.ts` (regenerate via
    /// `cargo run --bin gen-bindings`). Out-of-range indices are ignored.
    #[wasm_bindgen]
    pub fn set_slider_by_index(&mut self, idx: u32, value: f32) {
        self.apply_slider_by_index(idx as usize, value);
    }

    /// Pure Rust helper: apply a named slider; returns `true` on success.
    /// Factored out so native tests can exercise the name-dispatch table
    /// without constructing a `JsValue` (which panics on non-wasm32 targets).
    fn try_set_slider(&mut self, name: &str, value: f32) -> bool {
        match SLIDER_NAMES.iter().position(|n| *n == name) {
            Some(idx) => {
                self.apply_slider_by_index(idx, value);
                true
            }
            None => false,
        }
    }

    /// Inner dispatch shared by `set_slider` (name lookup) and
    /// `set_slider_by_index` (direct index from SAB). Indices map to
    /// [`SLIDER_NAMES`] in declaration order.
    fn apply_slider_by_index(&mut self, idx: usize, value: f32) {
        match idx {
            0 => self.apply_mutation_rate_multiplier(value),
            1 => { /* v1.12 reserved no-op (was nn_mutation_sigma) */ }
            2 => self.apply_eat_bite_fraction(value),
            3 => self.apply_grass_propagation_rate_k(value),
            4 => self.apply_grass_in_cell_growth_r(value),
            5 => self.apply_upkeep_multiplier(value),
            6 => self.apply_move_cost_multiplier(value),
            7 => self.apply_energy_max(value),
            8 => self.apply_grass_energy_per_bite(value),
            // u32 sliders: round float input; apply_ clamps below floor.
            9 => self.apply_grass_bites_per_block(value.max(0.0) as u32),
            10 => self.apply_digestion_cooldown(value.max(0.0) as u32),
            11 => self.apply_repulsion_max(value),
            12 => self.apply_max_age(value.max(0.0) as u32),
            13 => self.apply_split_threshold(value),
            14 => self.apply_split_gift(value),
            15 => self.apply_split_jitter(value),
            16 => self.apply_founder_count(value.max(0.0) as u32),
            17 => self.apply_max_population(value.max(1.0) as u32),
            // v2.0 Wave 1a construction-only settings (shape the next world).
            18 => self.apply_world_size(value),
            19 => self.apply_world_seed(value.max(0.0) as u32),
            20 => self.apply_wrap_world(value != 0.0),
            // v2.1 P2: slots 21/22/23 retired no-ops.
            21..=23 => self.apply_reserved_legacy_noop(value),
            // v2.0 Wave 3a species + sexual-mating settings.
            24 => self.apply_species_mode(value != 0.0),
            25 => self.apply_crossover_mode(value),
            26 => self.apply_starting_species_count(value.max(0.0) as u32),
            27 => self.apply_starting_species_member_count(value.max(0.0) as u32),
            28 => self.apply_starting_species_member_variance(value),
            29 => self.apply_mating_cooldown_ticks(value.max(0.0) as u32),
            // v1.12: 8 mutation buckets × 3 fields.
            n if (SLIDER_BUCKET_BASE..SLIDER_BUCKET_BASE + MUTATION_BUCKET_COUNT * 3)
                .contains(&n) =>
            {
                let rel = n - SLIDER_BUCKET_BASE;
                self.apply_mutation_bucket(rel / 3, rel % 3, value);
            }
            // v2.0.2 Stream 1d: scatter kernel sliders (indices 54–59).
            54 => self.apply_grass_decay_pct(value),
            55 => self.apply_grass_decay_amount(value),
            56 => self.apply_grass_spread_pct(value),
            57 => self.apply_grass_spread_amount(value),
            58 => self.apply_grass_spread_ring1_pct(value),
            59 => self.apply_grass_spread_ring2_pct(value),
            // v2.0.4 S1: render-side LOD knob (index 60).
            60 => self.apply_lod_bias(value),
            // v2.0.4 S6: multi-band grass sight construction toggle (index 61).
            61 => self.apply_grass_multisight(value != 0.0),
            // v2.0.4 S2: grass cell size construction knob (index 62).
            62 => self.apply_grass_cell_size(value),
            // v2.0.6 S10: discrete LOD stepper (index 63).
            63 => self.apply_lod_step(value.max(0.0) as u32),
            // Mating reach + founder action-output boosts (indices 64–66).
            64 => self.apply_mate_reach_multiplier(value),
            65 => self.apply_init_graze_boost(value),
            66 => self.apply_init_split_boost(value),
            // v2.1 P3: food-limited equilibrium knobs (indices 67–72).
            67 => self.apply_crowding_strength(value),
            68 => self.apply_crowding_radius(value),
            69 => self.apply_starvation_threshold(value),
            70 => self.apply_starvation_drain_rate(value),
            71 => self.apply_grass_capacity_scale(value),
            72 => self.apply_grass_regrowth_rate(value),
            _ => {} // out-of-range: silently ignore (forward-compat with newer TS).
        }
    }

    /// Returns the SoA index of the topmost creature whose body circle (or
    /// tap-tolerance bubble) contains (world_x, world_y), or None.
    /// O(N); called only on click (≤ 1 Hz).
    /// Returns the stable id of the topmost creature whose body circle (or
    /// tap-tolerance bubble) contains (world_x, world_y), or None.
    ///
    /// `tolerance_world` is an additional world-space radius added on top of
    /// the creature's body radius so that small creatures remain clickable.
    /// Pass `6.0 / cam.zoom` from JS so the minimum tap target is 6 screen px.
    ///
    /// The id is returned as `Option<f64>` because wasm-bindgen does not
    /// auto-bridge `u64`. f64 mantissa is exact up to 2^53 (DECISIONS E.21),
    /// which is far above any v1-session id count.
    ///
    /// S18 + S20: returns stable creature id (not SoA index); uses SpatialGrid
    /// `for_each_in_radius` to bound the scan.
    #[wasm_bindgen]
    pub fn creature_at(&self, world_x: f32, world_y: f32, tolerance_world: f32) -> Option<f64> {
        // S20: grid-backed scan. Query radius = tolerance + max possible body radius
        // (SIZE_MAX * BODY_RADIUS_PER_SIZE = 10.0) to ensure all candidates are
        // visited; we then filter by the exact per-creature distance check.
        // D3: body radius is constant — CREATURE_SIZE * BODY_RADIUS_PER_SIZE.
        let query_r = tolerance_world + CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        let mut found: Option<f64> = None;
        self.inner
            .grid
            .for_each_in_radius(world_x, world_y, query_r, |i| {
                if found.is_some() {
                    return;
                }
                let body_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
                let r = body_r + tolerance_world;
                // Walled: raw Euclidean distance (no torus seam).
                let ddx = world_x - self.inner.creatures.x[i];
                let ddy = world_y - self.inner.creatures.y[i];
                let d2 = ddx * ddx + ddy * ddy;
                if d2 <= r * r {
                    // S18: return stable id (f64) instead of SoA index.
                    found = Some(self.inner.creatures.id[i] as f64);
                }
            });
        found
    }

    /// Resolves a stable creature id back to its current SoA index, or
    /// None if the creature is dead (was removed from the SoA).
    ///
    /// O(N) linear scan over `creatures.id`. Acceptable at v1 scale
    /// (population <2k); a hashmap-backed lookup is deferred to v1.2.
    /// Called by the inspector at most once per frame while open.
    ///
    /// `id` is `f64` for wasm-bindgen compatibility (DECISIONS E.21);
    /// it must round-trip a `u64` exactly, guaranteed for any id below 2^53.
    #[wasm_bindgen]
    pub fn creature_idx_by_id(&self, id: f64) -> Option<u32> {
        let needle = id as u64;
        self.inner
            .creatures
            .id
            .iter()
            .position(|&x| x == needle)
            .map(|i| i as u32)
    }

    /// JSON blob for the Inspector panel. Returns None if idx is out of range.
    /// O(1) — all fields are direct SoA reads.
    /// v2.1 P2 schema: genome fields removed; lineage hue added.
    #[wasm_bindgen]
    pub fn creature_inspect_json(&self, idx: u32) -> Option<String> {
        let i = idx as usize;
        if i >= self.inner.creatures.len() {
            return None;
        }
        let action_name = format!("{:?}", self.inner.creatures.action_this_tick[i]);
        let brain = &self.inner.creatures.brains[i];
        // v1.5 S3: wall_proximity (N/S/E/W) computed inline; S5b promotes to
        // a shared proximity.rs helper. Range 50u, linear normalize: 1.0 at
        // wall contact → 0.0 at >=50u away.
        let cx = self.inner.creatures.x[i];
        let cy = self.inner.creatures.y[i];
        let world_size = self.inner.dims.world_size;
        let wall_range = 50.0_f32;
        let wp_n = (1.0 - (cy / wall_range)).clamp(0.0, 1.0);
        let wp_s = (1.0 - ((world_size - cy) / wall_range)).clamp(0.0, 1.0);
        let wp_w = (1.0 - (cx / wall_range)).clamp(0.0, 1.0);
        let wp_e = (1.0 - ((world_size - cx) / wall_range)).clamp(0.0, 1.0);
        // v2.0 Wave 3a: species fields (species_mode only). The species-history
        // breadcrumb is Wave 5 — left empty here.
        let species_mode = self.inner.sliders.species_mode;
        let species_id = self.inner.creatures.species_id[i];
        let species_color = self.inner.species.color_of(species_id);
        let species_name = self.inner.species.name_of(species_id);
        let hue = self.inner.creatures.hue[i];
        let mut json = serde_json::json!({
            "index": idx,
            "id": self.inner.creatures.id[i],
            "x": cx,
            "y": cy,
            "age": self.inner.creatures.age[i],
            "max_age": self.inner.sliders.max_age,
            "energy": self.inner.creatures.energy[i],
            "energy_frac": (self.inner.creatures.energy[i] / 100.0).clamp(0.0, 1.0),
            "size": CREATURE_SIZE * BODY_RADIUS_PER_SIZE,
            "current_action": action_name,
            "move_speed": MOVE_SPEED_MAX,
            "cooldown_remaining": self.inner.creatures.digestion_cooldown[i],
            // v2.1 P2: lineage hue replaces genome.
            "hue": hue,
            "wall_proximity": [wp_n, wp_s, wp_e, wp_w],
            "nn_weight_count": brain.weights.len(),
        });
        // v2.0 Wave 3a: species fields only in species mode. The species color is
        // exposed as the RGBA8 packed u32 (same lane the renderer paints) plus an
        // empty species-history breadcrumb (Wave 5 fills it).
        if species_mode {
            if let Some(obj) = json.as_object_mut() {
                obj.insert("species_id".into(), serde_json::json!(species_id));
                obj.insert("species_color".into(), serde_json::json!(species_color));
                // v2.0 Wave 4: species name (Wave 5 wires the display).
                obj.insert("species_name".into(), serde_json::json!(species_name));
                obj.insert("species_history".into(), serde_json::json!([] as [u16; 0]));
            }
        }
        Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
    }

    /// v2.1 P1 (Stream A): on-demand full NN I/O inspect for one creature.
    ///
    /// Recomputes the creature's NN input vector from scratch (same path as the
    /// tick), runs one `Brain::forward`, and returns a JSON blob. Returns `None`
    /// if `idx` is out of range.
    ///
    /// JSON shape:
    /// ```json
    /// {
    ///   "inputs": [
    ///     { "group": "<GroupName>", "label": "<label>", "value": <f32> },
    ///     ...
    ///   ],
    ///   "outputs": {
    ///     "vx": <f32>,
    ///     "vy": <f32>,
    ///     "logits": [<graze_f32>, <attack_f32>, <split_or_mate_f32>],
    ///     "chosen": "<Graze|Attack|Split>"
    ///   }
    /// }
    /// ```
    ///
    /// `inputs` has one entry per active slot in the current `NnInputLayout`
    /// (pad slots are not included). Group names match `NnInputGroup` variants;
    /// directional groups are labeled by compass sector (N/NE/E/SE/S/SW/W/NW
    /// for 8-sector groups; N/S/E/W for 4-slot groups). `chosen` is the
    /// decoded action string (`"Graze"`, `"Attack"`, or `"Split"`; in
    /// species_mode `"Split"` decodes as `"Mate"` when that action is valid).
    ///
    /// See `NnInputLayout::for_settings` for the active group set, which
    /// depends on `wrap_world`, `species_mode`, and `grass_multisight`.
    #[wasm_bindgen]
    pub fn creature_nn_inspect_json(&self, idx: u32) -> Option<String> {
        let i = idx as usize;
        if i >= self.inner.creatures.len() {
            return None;
        }
        let w = &self.inner;
        let (inputs, output_buf) = crate::world::nn::build_labeled_nn_inspect(
            i,
            &w.nn_input_layout,
            w.sliders.species_mode,
            &w.creatures,
            &w.grass,
            &w.grid,
            &w.sector_lut,
            w.sliders.energy_max,
            w.sliders.max_age,
            w.dims.world_size,
        );

        // Decode action from the logit outputs.
        let energy = w.creatures.energy[i];
        let cooldown = w.creatures.digestion_cooldown[i];
        let gate = crate::world::nn::ActionGate {
            species_mode: w.sliders.species_mode,
            split_threshold: w.sliders.split_threshold,
            mating_cost: w.sliders.split_gift,
            mating_cooldown: w.creatures.mating_cooldown[i],
        };
        let logits: &[f32; 3] = output_buf[2..5].try_into().unwrap();
        let action = crate::world::nn::decode_action(logits, energy, cooldown, &gate);
        // In species_mode, action[2] means "Mate" semantically but the enum
        // variant is still Split. We expose the mode-aware name so the UI can
        // display it correctly without knowing the mode.
        let chosen = match action {
            crate::creature::Action::Graze => "Graze",
            crate::creature::Action::Attack => "Attack",
            crate::creature::Action::Split => {
                if w.sliders.species_mode {
                    "Mate"
                } else {
                    "Split"
                }
            }
        };

        let vx = output_buf[0] * crate::constants::MOVE_SPEED_MAX;
        let vy = output_buf[1] * crate::constants::MOVE_SPEED_MAX;

        let json = serde_json::json!({
            "inputs": inputs,
            "outputs": {
                "vx": vx,
                "vy": vy,
                "logits": [logits[0], logits[1], logits[2]],
                "chosen": chosen,
            }
        });
        Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
    }

    // ─── P3d observability counters ──────────────────────────────────────────

    /// Count of grass cells where density > 0. O(921_600) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn live_grass_cell_count(&self) -> u32 {
        // v2.0.2 Stream 1a: density is u8-backed — a non-zero stored byte means
        // the cell has grass (decode is monotone so `byte > 0` ⇔ `density > 0`).
        self.inner
            .grass
            .density
            .iter()
            .filter(|b| b.load(core::sync::atomic::Ordering::Relaxed) > 0)
            .count() as u32
    }

    /// Sum of all grass cell densities. O(921_600) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn total_grass_density(&self) -> f32 {
        // v2.0.2 Stream 1a: decode each u8 cell to its f32 grass amount and sum.
        (0..self.inner.grass.density.len())
            .map(|c| self.inner.grass.dget(c))
            .sum()
    }

    // ─────────────────────────────────────────────────────────────────────
    // Profiler API (see docs/plans/perf-timing.md)

    /// Enable or disable the in-app profiler. Default: false (D9).
    /// The toggle state is NOT persisted — every page load returns to OFF.
    /// v1.9.1: the worker sends `profile_enable(true)` once at boot and
    /// leaves it on; the Settings panel toggle is visibility-only.
    #[wasm_bindgen]
    pub fn profile_enable(&self, on: bool) {
        self.inner.profile.set_enabled(on);
    }

    /// Clear all accumulated profiler samples (per-node ring buffers and the
    /// current span stack) and reset the epoch. Preserves the node tree and
    /// the enabled flag. v1.9.1: invoked by the panel's "Reset profiler +
    /// jank" button alongside `reset_jank` and the TS-side frame-tree clear.
    #[wasm_bindgen]
    pub fn profile_clear(&self) {
        self.inner.profile.clear();
    }

    /// Set the profiler's rolling-window length in milliseconds. Clamped
    /// inside `Profiler::set_window_ms` to [500, 600_000]. Plumbed from the
    /// perf panel via the control SAB (`CTRL_PROFILE_WINDOW_MS`).
    #[wasm_bindgen]
    pub fn profile_set_window_ms(&self, ms: u32) {
        self.inner.profile.set_window_ms(ms);
    }

    /// JSON report of the current rolling 60-second profile tree.
    /// Walks ~30 nodes, prunes stale samples, writes ~5 KB string.
    /// Stable shape — see docs/plans/perf-timing.md §D5.
    ///
    /// Returns ONLY the Rust `tick` subtree root. The TS-side `frame`
    /// subtree is maintained separately in `src/web/src/perf.ts` (R4).
    #[wasm_bindgen]
    pub fn profile_report_json(&self) -> String {
        self.inner.profile.report_json()
    }

    /// JSON map of every live-tunable + construction-only slider name → its
    /// canonical default value (the value `DevSliders::default()` returns).
    /// Bools encode as 0/1 so the map is uniformly `{name: f32}` — TS side
    /// compares against its own defaults to catch Rust ↔ TS drift.
    #[wasm_bindgen]
    pub fn sliders_defaults_json(&self) -> String {
        let d = crate::world::DevSliders::default();
        let mut json = serde_json::json!({
            "mutation_rate_multiplier": d.mutation_rate_multiplier,
            "_reserved_legacy_nn_mutation_sigma": 0.0_f32,
            "eat_bite_fraction": d.eat_bite_fraction,
            "grass_propagation_rate_k": d.grass_propagation_rate_k,
            "grass_in_cell_growth_r": d.grass_in_cell_growth_r,
            "upkeep_multiplier": d.upkeep_multiplier,
            "move_cost_multiplier": d.move_cost_multiplier,
            "energy_max": d.energy_max,
            "grass_energy_per_bite": d.grass_energy_per_bite,
            "grass_bites_per_block": d.grass_bites_per_block as f32,
            "digestion_cooldown": d.digestion_cooldown_ticks as f32,
            "repulsion_max": d.repulsion_max,
            "max_age": d.max_age as f32,
            "split_threshold": d.split_threshold,
            "split_gift": d.split_gift,
            "split_jitter": d.split_jitter,
            "founder_count": d.founder_count as f32,
            "grass_initial_seed_count": d.grass_initial_seed_count as f32,
            "max_population": d.max_population as f32,
            // v2.0 Wave 1a construction settings.
            "world_size": d.world_size,
            "world_seed": d.world_seed as f32,
            "wrap_world": if d.wrap_world { 1.0_f32 } else { 0.0 },
            // v2.1 P2: slots 21/22/23 retired; emit 0.0 to satisfy SLIDER_NAMES coverage.
            "_reserved_legacy_water_movement_penalty": 0.0_f32,
            "_reserved_legacy_desert_movement_penalty": 0.0_f32,
            "_reserved_legacy_trait_mutation_sigma_multiplier": 0.0_f32,
            // v2.0 Wave 3a species + sexual-mating settings (bools/enums as 0/1).
            "species_mode": if d.species_mode { 1.0_f32 } else { 0.0 },
            "crossover_mode": d.crossover_mode.to_slider(),
            "starting_species_count": d.starting_species_count as f32,
            "starting_species_member_count": d.starting_species_member_count as f32,
            "starting_species_member_variance": d.starting_species_member_variance,
            "mating_cooldown_ticks": d.mating_cooldown_ticks as f32,
            // v2.0.2 Stream 1d: scatter kernel defaults.
            "grass_decay_pct": d.grass_decay_pct,
            "grass_decay_amount": d.grass_decay_amount,
            "grass_spread_pct": d.grass_spread_pct,
            "grass_spread_amount": d.grass_spread_amount,
            "grass_spread_ring1_pct": d.grass_spread_ring1_pct,
            "grass_spread_ring2_pct": d.grass_spread_ring2_pct,
            // v2.0.4 S2: grass cell size construction knob. Default 5.0.
            "grass_size": d.grass_cell_size,
            // v2.0.6 S3: seeded grass clumps construction knobs.
            "grass_clump_count": d.grass_clump_count as f32,
            "grass_clump_size": d.grass_clump_size as f32,
            // v2.0.6 S10: discrete LOD stepper. Default 0 (Auto).
            "grass_lod_step": 0.0_f32,
        });
        // v1.12: 8 mutation buckets × 3 fields. Names match SLIDER_NAMES.
        let obj = json.as_object_mut().expect("json! produced an object");
        for (i, b) in d.mutation_policy.buckets.iter().enumerate() {
            obj.insert(format!("bucket_{i}_weight"), serde_json::json!(b.weight));
            obj.insert(format!("bucket_{i}_rate"), serde_json::json!(b.rate));
            obj.insert(format!("bucket_{i}_sigma"), serde_json::json!(b.sigma));
        }
        // v2.0.x: inserted here (not in the json! literal, which is at its macro
        // recursion limit) so EVERY SLIDER_NAMES entry has a defaults value. The
        // sim worker falls back to this map when seeding the SAB lanes, so a
        // missing key would leave a lane at 0 and the all-lane drain would apply 0
        // on the next Apply. Guarded by sliders_defaults_json_covers_every_slider_name.
        // lod_bias is a WorldHandle field (default 0.0), not a DevSliders field.
        obj.insert("lod_bias".into(), serde_json::json!(0.0_f32));
        obj.insert(
            "grass_multisight".into(),
            serde_json::json!(if d.grass_multisight { 1.0_f32 } else { 0.0 }),
        );
        // Mating reach + founder action-output boosts (defaults neutral = 1.0).
        obj.insert(
            "mate_reach_multiplier".into(),
            serde_json::json!(d.mate_reach_multiplier),
        );
        obj.insert(
            "init_graze_boost".into(),
            serde_json::json!(d.init_graze_boost),
        );
        obj.insert(
            "init_split_boost".into(),
            serde_json::json!(d.init_split_boost),
        );
        // v2.1 P3: food-limited equilibrium knobs.
        obj.insert(
            "crowding_strength".into(),
            serde_json::json!(d.crowding_strength),
        );
        obj.insert(
            "crowding_radius".into(),
            serde_json::json!(d.crowding_radius),
        );
        obj.insert(
            "starvation_threshold".into(),
            serde_json::json!(d.starvation_threshold),
        );
        obj.insert(
            "starvation_drain_rate".into(),
            serde_json::json!(d.starvation_drain_rate),
        );
        obj.insert(
            "grass_capacity_scale".into(),
            serde_json::json!(d.grass_capacity_scale),
        );
        obj.insert(
            "grass_regrowth_rate".into(),
            serde_json::json!(d.grass_regrowth_rate),
        );
        serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
    }

    /// Per-worker + per-sub-phase health snapshot for the parallel NN pass.
    /// Shape:
    /// ```text
    /// {
    ///   "world_uptime_us": <u64>,
    ///   "tick": {
    ///     "build_input_other_us": <u64>,
    ///     "proximity_creatures_us": <u64>,
    ///     "proximity_grass_us": <u64>,
    ///     "forward_us": <u64>,
    ///     "chunk_wall_us": <u64>,    // sum across all chunks (parallel total)
    ///     "workers_used": <u64>      // distinct workers that ran chunks
    ///   },
    ///   "workers": [
    ///     {
    ///       "idx": <usize>,
    ///       "first_seen_us": <u64>, "last_seen_us": <u64>, "uptime_us": <u64>,
    ///       "chunks": <u64>, "creatures": <u64>, "busy_us": <u64>,
    ///       "creatures_per_chunk": <u64>
    ///     }, ...
    ///   ]
    /// }
    /// ```
    /// `tick.*` values reflect the most recent tick only. Per-worker counters
    /// are cumulative since world construction. Requires the profiler to be
    /// enabled (`profile_enable(true)`) for any timing to accumulate.
    #[wasm_bindgen]
    pub fn nn_worker_stats_json(&self) -> String {
        let now = crate::profiler::clock_now_us_threadsafe();
        self.inner.nn_stats.to_json(now)
    }

    /// v2.0 Wave 4: polled species-table report. JSON map of every LIVE species
    /// (population_count > 0) → `{ color_u32, name, population_count }`, plus the
    /// current world tick (so the Wave-5 pop-graph consumer can stamp samples).
    ///
    /// Per-species counts are aggregated sim-side here (O(N) over the SoA). The
    /// table is **dynamic** — it reads the runtime registry + live counts, so a
    /// species that has gone extinct simply drops out, and v2.1 split/merge needs
    /// no protocol change. Single-pool mode returns an empty `species` list (no
    /// species exist). Mirrors the `nn_worker_stats_json` producer shape; the
    /// worker writes this into the `SPECIES_TABLE` SAB window on a cadence and
    /// bumps `CTRL_SPECIES_TABLE_EPOCH`.
    ///
    /// Shape:
    /// ```json
    /// { "tick": 1234, "species": [
    ///     { "id": 0, "color_u32": 4281545446, "name": "Species-A", "count": 73 },
    ///     ...
    /// ] }
    /// ```
    #[wasm_bindgen]
    pub fn species_table_json(&self) -> String {
        let w = &self.inner;
        let registry = w.species.all();
        // Aggregate live per-species counts. Index by species id over a tiny
        // registry (≤ tens), so a flat scan + linear find is fine.
        let mut counts: Vec<u32> = vec![0; registry.len()];
        for &sid in w.creatures.species_id.iter() {
            if let Some(pos) = registry.iter().position(|s| s.id == sid) {
                counts[pos] += 1;
            }
        }
        let entries: Vec<serde_json::Value> = registry
            .iter()
            .enumerate()
            .filter(|(pos, _)| counts[*pos] > 0)
            .map(|(pos, sp)| {
                serde_json::json!({
                    "id": sp.id,
                    "color_u32": sp.color_u32,
                    "name": sp.name,
                    "count": counts[pos],
                })
            })
            .collect();
        let obj = serde_json::json!({
            "tick": w.tick,
            "species": entries,
        });
        serde_json::to_string(&obj).unwrap_or_else(|_| "{\"tick\":0,\"species\":[]}".into())
    }

    /// Bounded longitudinal telemetry export. Unlike the profiler report, this
    /// is request-driven by the UI because the payload can be hundreds of KB
    /// once the in-memory history ring fills.
    #[wasm_bindgen]
    pub fn telemetry_report_json(&self) -> String {
        let w = &self.inner;
        let report = serde_json::json!({
            "schema_version": TELEMETRY_SCHEMA_VERSION,
            "truncated": false,
            "generated_at_ms": wasm_now_ms(),
            "meta": {
                "app_version": serde_json::Value::Null,
                "seed": &w.seed,
                "world_seed": w.world_seed,
                "world_size": w.world_size(),
                "wrap_world": w.dims.wrap_world,
                "grass_dim": w.dims.grass_dim,
                "grass_cell_size": w.dims.grass_cell_size,
                "started_at_ms": self.telemetry.created_at_ms,
            },
            "config": {
                "species_mode": w.sliders.species_mode,
                "max_population": w.sliders.max_population,
                "founder_count": w.sliders.founder_count,
                "energy_max": w.sliders.energy_max,
                "target_sample_period_ticks": TELEMETRY_SAMPLE_PERIOD_TICKS,
            },
            "caps": {
                "sample_period_ticks": TELEMETRY_SAMPLE_PERIOD_TICKS,
                "sample_cap": TELEMETRY_SAMPLE_CAP,
                "event_cap": TELEMETRY_EVENT_CAP,
                "samples_dropped": self.telemetry.sample_dropped_count,
                "events_dropped": self.telemetry.event_dropped_count,
            },
            "current": {
                "tick": w.tick,
                "population": w.population(),
                "world_ended": w.world_ended,
                "tps": self.tps(),
                "jank_count": self.jank_count,
            },
            "samples": &self.telemetry.samples,
            "events": &self.telemetry.events,
            "jank": JankSummary {
                budget_ms: JANK_BUDGET_MS,
                large_event_threshold_ms: LARGE_JANK_EVENT_MS,
                count: self.jank_count,
                worst: self.telemetry.worst_jank.clone(),
            },
        });
        serde_json::to_string(&report)
            .unwrap_or_else(|_| "{\"schema_version\":1,\"truncated\":true}".into())
    }
}

// ─── v1.6 S A1: shared snapshot byte writer ─────────────────────────────────
//
// Target-agnostic helpers used by both the wasm `write_snapshot_to` (which
// streams each creature's 32 bytes into a `js_sys::Uint8Array`) and the
// native `write_snapshot_to_native` test (which streams into a `Vec<u8>`).

/// v1.11: write all creatures' 32-byte records into `dst` (must be ≥
/// `MAX_POP_FOR_SIM * 32` bytes). Free function so callers can hold a
/// `&mut [u8]` borrowed from a `WorldHandle` field while also reading
/// `&World` — Rust's borrow checker forbids that pattern on a `&mut self`
/// method. Returns the actual number of creatures written.
fn fill_creature_bytes(world: &World, dst: &mut [u8]) -> usize {
    let pop = world.creatures.len();
    debug_assert!(
        pop <= MAX_POP_FOR_SIM,
        "pop={pop} exceeds MAX_POP_FOR_SIM={MAX_POP_FOR_SIM} — sim cull broke",
    );
    debug_assert!(dst.len() >= pop * 32, "dst too small for creature SoA",);
    // v2.1 P2: body radius is constant (genome removed).
    let body_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
    // v2.0 Wave 3a: in species mode the body color is the SPECIES color;
    // v2.1 P2: single-pool uses lineage hue color instead of genome color.
    let species_mode = world.sliders.species_mode;
    for i in 0..pop {
        let x = world.creatures.x[i];
        let y = world.creatures.y[i];
        let species_id = world.creatures.species_id[i];
        // Display color: species color (species_mode) or lineage hue (single-pool).
        let color = if species_mode {
            world.species.color_of(species_id)
        } else {
            lineage_color_u32(world.creatures.hue[i])
        };
        // v2.0 Wave 2a/3a: ring-flash + species_id, bit-packed. species_id is 0
        // (reserved) in single-pool, the creature's species in species_mode.
        let packed = pack_render_u32(
            world.creatures.flash_tag[i] as u8,
            world.creatures.flash_ticks[i],
            if species_mode { species_id } else { 0 },
        );
        let id = world.creatures.id[i];
        let id_lo = id as u32;
        let id_hi = (id >> 32) as u32;
        let off = i * 32;
        // Lane order (8 f32-lanes / 32 B): x, y, radius, color_u32, id_lo,
        // id_hi, packed_u32, (pad). color_u32 + packed_u32 + id halves are raw
        // u32 bits (the reader builds a Uint32Array view over the same bytes).
        dst[off..off + 4].copy_from_slice(&x.to_le_bytes());
        dst[off + 4..off + 8].copy_from_slice(&y.to_le_bytes());
        dst[off + 8..off + 12].copy_from_slice(&body_r.to_le_bytes());
        dst[off + 12..off + 16].copy_from_slice(&color.to_le_bytes());
        dst[off + 16..off + 20].copy_from_slice(&id_lo.to_le_bytes());
        dst[off + 20..off + 24].copy_from_slice(&id_hi.to_le_bytes());
        dst[off + 24..off + 28].copy_from_slice(&packed.to_le_bytes());
        // off+28..off+32: pad lane (reserved, zeroed).
        dst[off + 28..off + 32].copy_from_slice(&0u32.to_le_bytes());
    }
    pop
}

/// v2.1 P2: lineage hue (f32 in [0,1)) → RGBA8 display color packed into one u32.
///
/// HSV mapping: hue = hue * 360° (full wheel), saturation = 0.8, value = 0.9.
/// Returned as RGBA8 little-endian: R = bits 0..8, G = 8..16, B = 16..24, A =
/// 24..32 (A always 255). The TS reader does `R = u & 0xFF`, etc.
#[inline]
fn lineage_color_u32(hue: f32) -> u32 {
    let hue_deg = hue.rem_euclid(1.0) * 360.0;
    let (r, gg, b) = hsv_to_rgb8(hue_deg, 0.8, 0.9);
    (r as u32) | ((gg as u32) << 8) | ((b as u32) << 16) | (0xFFu32 << 24)
}

/// HSV (hue degrees, sat/val in [0,1]) → 8-bit RGB.
#[inline]
fn hsv_to_rgb8(h_deg: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = (h_deg.rem_euclid(360.0)) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to8 = |f: f32| ((f + m).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    (to8(r), to8(g), to8(b))
}

/// v2.0 Wave 2a: bit-pack the render-only `packed_u32` lane.
///
/// Bit layout (LSB first; 2b decode contract):
///   bits  0..3  (3 bits) : flash_tag       (FlashTag 0..5)
///   bits  3..7  (4 bits) : flash_ticks     (0..FLASH_TICKS, ≤ 15)
///   bits  7..23 (16 bits): species_id      (reserved; 0 in single-pool, W3 fills)
///   bits 23..32 (9 bits) : reserved        (0)
#[inline]
fn pack_render_u32(flash_tag: u8, flash_ticks: u8, species_id: u16) -> u32 {
    ((flash_tag as u32) & 0x7)
        | (((flash_ticks as u32) & 0xF) << 3)
        | (((species_id as u32) & 0xFFFF) << 7)
}

/// Wrapper for `fill_creature_bytes` matching the v1.11 method-call site so
/// the inner block can borrow `&mut self.snapshot_buf` without conflicting
/// with `&self.inner`.
#[inline]
fn write_creatures_each_into(world: &World, dst: &mut [u8]) -> usize {
    fill_creature_bytes(world, dst)
}

/// v2.0 Wave 1a: quantize f32 grass density (0..`GRASS_MAX`) → u8 (0..255), one
/// byte per cell, into `dst` (which must be exactly `density.len()` bytes). The
/// renderer (TS stream) uploads this as an R8 texture and rescales by 1/255.
///
/// v2.0.3 Stream 2b: gated off wasm32 — `write_snapshot` now uses
/// `pyramid.viewport_window` directly. `write_snapshot_to_native` (tests only)
/// still calls this.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn quantize_grass_into(density: &[core::sync::atomic::AtomicU8], dst: &mut [u8]) {
    debug_assert_eq!(
        density.len(),
        dst.len(),
        "u8 grass region must be one byte per cell"
    );
    // v2.0.2 Stream 1a: density is already stored on the snapshot u8 scale
    // (GRASS_MAX ⇒ 255), so quantizing is a direct Relaxed-load byte copy. This
    // produces the exact same bytes the old f32→u8 round-trip did.
    for (d, out) in density.iter().zip(dst.iter_mut()) {
        *out = d.load(core::sync::atomic::Ordering::Relaxed);
    }
}

impl WorldHandle {
    /// Native (non-wasm32) twin of `write_snapshot_to` used by tests. Writes
    /// the same byte layout into three caller-provided `&mut Vec<u8>`s — the
    /// vecs are cleared and then filled to exactly the sizes the wasm path
    /// would have written into the SAB-backed `Uint8Array`s.
    ///
    /// Kept off the wasm path because `js_sys::Uint8Array` is the only thing
    /// the worker ever passes in; mirroring the bytes here is purely so the
    /// layout is testable without wasm-bindgen.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_snapshot_to_native(
        &mut self,
        creatures_dst: &mut Vec<u8>,
        grass_dst: &mut Vec<u8>,
        stats_dst: &mut Vec<u8>,
    ) -> usize {
        // v2.0 Wave 1a: native test path mirrors the wasm-side layout — u8 grass
        // (one byte per cell, quantized). Each Vec is sized to match exactly what
        // main would read out of the wasm-memory snapshot region.
        creatures_dst.clear();
        creatures_dst.resize(self.inner.creatures.len() * 32, 0);
        let pop_written = fill_creature_bytes(&self.inner, creatures_dst);
        creatures_dst.truncate(pop_written * 32);

        stats_dst.clear();
        stats_dst.extend_from_slice(&self.inner.tick.to_le_bytes());
        stats_dst.extend_from_slice(&(pop_written as u32).to_le_bytes());
        stats_dst
            .extend_from_slice(&(if self.inner.world_ended { 1u32 } else { 0u32 }).to_le_bytes());
        stats_dst.extend_from_slice(&self.tps().to_bits().to_le_bytes());
        stats_dst.extend_from_slice(&self.jank_count.to_le_bytes());

        grass_dst.clear();
        grass_dst.resize(self.inner.grass.density.len(), 0);
        quantize_grass_into(&self.inner.grass.density, grass_dst);

        pop_written
    }
}

// ─── Clock helper ─────────────────────────────────────────────────────────────

/// Current wall-clock time in milliseconds. On wasm32 uses `Performance::now()`
/// via the profiler's scope-agnostic binding so this works on main thread AND
/// inside the sim worker (where `web_sys::window()` is `None`); on native
/// (tests) uses a monotonic `Instant` against a per-process epoch.
#[cfg(target_arch = "wasm32")]
fn wasm_now_ms() -> f64 {
    crate::profiler::clock_now_us_threadsafe() as f64 / 1000.0
}

#[cfg(not(target_arch = "wasm32"))]
fn wasm_now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_secs_f64() * 1000.0
}

/// v1.6 sim population cap, mirrored from `constants::MAX_POP_FOR_SIM`.
/// Sourced from Rust so the worker can pass it to main in `boot_ready`; main
/// asserts it matches the TS `MAX_POP_FOR_SIM` constant (rebuild-wasm guard).
#[wasm_bindgen]
pub fn max_pop_for_sim() -> u32 {
    MAX_POP_FOR_SIM as u32
}

/// v1.6 Wave D: runtime rayon thread-count probe. The sim worker calls this
/// once at boot, immediately after the threaded pool init resolves, and
/// `console.warn`s if the value is `<= 1` — surfacing the v1.5 silent
/// single-thread footgun (a threaded build that nonetheless collapses to one
/// worker because COOP/COEP headers or `--features threads` weren't applied
/// end-to-end).
///
/// Returns `1` on non-threaded builds (the sequential pass owns the only worker).
#[cfg(feature = "threads")]
#[wasm_bindgen]
pub fn rayon_current_num_threads() -> u32 {
    rayon::current_num_threads() as u32
}

#[cfg(not(feature = "threads"))]
#[wasm_bindgen]
pub fn rayon_current_num_threads() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_handle(seed: &str) -> WorldHandle {
        let mut handle = WorldHandle::new_with_founder_count(
            seed, 0, 1000.0, 1, true, "", 120.0, true, 1, false, 1.0, 1, 1, 0.0, 20.0, false, 0, 0,
            1.0, 1.0,
        )
        .unwrap();
        handle.try_set_slider("upkeep_multiplier", 0.0);
        handle.try_set_slider("move_cost_multiplier", 0.0);
        handle
    }

    /// v1.6 S A1: `write_snapshot_to_native` produces bytes that match the
    /// documented stride-8 layout. Exercises the same writer the wasm path
    /// drives so the SAB layout is testable off-wasm.
    #[test]
    fn write_snapshot_to_layout_matches_stride() {
        let mut handle = WorldHandle::new_with_founder_count(
            "a1-snapshot",
            0,
            100.0,
            3,
            false,
            "",
            1200.0,
            false,
            1,
            false,
            1.0,
            10,
            10,
            3.0,
            // v2.0.5 S4: new grass_cell_size arg — default 5.0.
            5.0,
            // v2.0.4 S6: grass_multisight default ON.
            true,
            // v2.0.6 S3: clump_count=0 → old uniform-scatter (no seeds here).
            0,
            0,
            // Founder action-output boosts — neutral (1.0).
            1.0,
            1.0,
        )
        .unwrap();
        let mut creatures = Vec::new();
        let mut grass = Vec::new();
        let mut stats = Vec::new();
        let pop_written = handle.write_snapshot_to_native(&mut creatures, &mut grass, &mut stats);

        // Stats header: 5 × 4 bytes.
        assert_eq!(stats.len(), 20);
        let read_u32 =
            |off: usize| -> u32 { u32::from_le_bytes(stats[off..off + 4].try_into().unwrap()) };
        assert_eq!(read_u32(0), handle.inner.tick);
        assert_eq!(read_u32(4), pop_written as u32);
        assert_eq!(read_u32(8), if handle.inner.world_ended { 1 } else { 0 });
        let tps_bits = read_u32(12);
        assert_eq!(tps_bits, handle.tps().to_bits());
        assert_eq!(read_u32(16), handle.jank_count);

        // Creature SoA: 32-byte stride, one record per founder (3 here).
        // v2.0 Wave 2a lane order: x, y, radius, color_u32, id_lo, id_hi,
        // packed_u32, (pad).
        assert_eq!(pop_written, 3);
        assert_eq!(creatures.len(), 3 * 32);
        for i in 0..3 {
            let base = i * 32;
            let x = f32::from_le_bytes(creatures[base..base + 4].try_into().unwrap());
            let y = f32::from_le_bytes(creatures[base + 4..base + 8].try_into().unwrap());
            let body = f32::from_le_bytes(creatures[base + 8..base + 12].try_into().unwrap());
            assert_eq!(x, handle.inner.creatures.x[i]);
            assert_eq!(y, handle.inner.creatures.y[i]);
            // v2.1 P2: radius is constant (genome removed).
            let expected_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
            assert!((body - expected_r).abs() < 1e-5, "radius lane mismatch");
            // color_u32 lane (3): alpha byte must be 255.
            let color = u32::from_le_bytes(creatures[base + 12..base + 16].try_into().unwrap());
            assert_eq!((color >> 24) & 0xFF, 0xFF, "color_u32 alpha must be 255");
            // Id round-trips via u32-halves at lanes 4,5 (off 16..24).
            let id_lo = u32::from_le_bytes(creatures[base + 16..base + 20].try_into().unwrap());
            let id_hi = u32::from_le_bytes(creatures[base + 20..base + 24].try_into().unwrap());
            let id = ((id_hi as u64) << 32) | (id_lo as u64);
            assert_eq!(id, handle.inner.creatures.id[i]);
            // packed_u32 lane (6): low 3 bits = flash_tag, next 4 = flash_ticks.
            let packed = u32::from_le_bytes(creatures[base + 24..base + 28].try_into().unwrap());
            assert_eq!(packed & 0x7, handle.inner.creatures.flash_tag[i] as u32);
            assert_eq!(
                (packed >> 3) & 0xF,
                handle.inner.creatures.flash_ticks[i] as u32
            );
            // species_id bits reserved 0 in single-pool.
            assert_eq!((packed >> 7) & 0xFFFF, 0, "species_id reserved must be 0");
            // pad lane (7) is zero.
            let pad = u32::from_le_bytes(creatures[base + 28..base + 32].try_into().unwrap());
            assert_eq!(pad, 0, "pad lane must be zero");
        }

        // Grass: v2.0 Wave 1a ships u8 quantized density (one byte per cell).
        let cell_count = handle.inner.dims.grass_cell_count;
        assert_eq!(
            grass.len(),
            cell_count,
            "grass region size = grass_cell_count bytes (u8 per cell)"
        );
        for i in 0..cell_count {
            // v2.0.2 Stream 1a: density is now stored on the snapshot u8 scale, so
            // the snapshot byte must equal the stored byte exactly (a byte copy).
            let expected = handle.inner.grass.dget_u8(i);
            assert_eq!(grass[i], expected, "cell {i} u8 quantize mismatch");
        }
    }

    /// v1.6: `max_pop_for_sim()` mirrors the Rust constant.
    #[test]
    fn max_pop_for_sim_matches_constant() {
        assert_eq!(max_pop_for_sim(), MAX_POP_FOR_SIM as u32);
    }

    /// E.21: creature_inspect_json returns None for out-of-range idx.
    #[test]
    fn creature_inspect_json_returns_none_for_out_of_range() {
        let handle = WorldHandle::new("e21-inspect");
        // Index 999 is way out of range (only 1 creature at start).
        let result = handle.creature_inspect_json(999);
        assert!(result.is_none(), "out-of-range idx must return None");
    }

    /// S18 + S20: creature_at returns the founder's stable id (f64) at its
    /// halton position. Uses a 1-founder world so we control placement exactly.
    #[test]
    fn creature_at_returns_stable_id() {
        let handle = WorldHandle::new_with_founder_count(
            "e21-creature-at",
            0,
            100.0,
            1,
            false,
            "",
            1200.0,
            false,
            1,
            false,
            1.0,
            10,
            10,
            3.0,
            // v2.0.5 S4: new grass_cell_size arg — default 5.0.
            5.0,
            // v2.0.4 S6: grass_multisight default ON.
            true,
            // v2.0.6 S3: clump_count=0 → old uniform-scatter.
            0,
            0,
            // Founder action-output boosts — neutral (1.0).
            1.0,
            1.0,
        )
        .unwrap();
        let founder_id = handle.inner.creatures.id[0] as f64;
        let cx = handle.inner.creatures.x[0];
        let cy = handle.inner.creatures.y[0];
        // Zero tolerance: hit exactly at the founder's position.
        let result = handle.creature_at(cx, cy, 0.0);
        assert_eq!(
            result,
            Some(founder_id),
            "founder id must be found at its spawn position"
        );
        // With tolerance: hit just outside the body radius should still hit.
        let result_tol = handle.creature_at(cx + 2.0, cy, 3.0);
        assert_eq!(
            result_tol,
            Some(founder_id),
            "founder must be found within tolerance radius"
        );
        // Far from any creature — should return None even with tolerance.
        let miss = handle.creature_at(cx + 200.0, cy + 200.0, 1.5);
        assert!(miss.is_none(), "empty area must return None");
    }

    /// S18: creature_idx_by_id resolves a live id back to its SoA index.
    #[test]
    fn creature_idx_by_id_finds_existing() {
        let handle = WorldHandle::new("s18-idx-by-id");
        let founder_id = handle.inner.creatures.id[0];
        let idx = handle.creature_idx_by_id(founder_id as f64);
        assert_eq!(idx, Some(0), "founder must be at index 0");
    }

    /// S18: creature_idx_by_id returns None for an unknown (never-assigned) id.
    #[test]
    fn creature_idx_by_id_returns_none_for_unknown() {
        let handle = WorldHandle::new("s18-idx-unknown");
        // u64::MAX is guaranteed never to be allocated in any v1 session.
        let idx = handle.creature_idx_by_id(u64::MAX as f64);
        assert!(idx.is_none(), "unknown id must return None");
    }

    /// Wave 2 boot/config regression: `grass_multisight` is construction-only
    /// because it changes the NN input layout. It must ride the explicit boot
    /// constructor so restart applies the persisted value before topology/founder
    /// brain construction.
    #[test]
    fn construction_grass_multisight_controls_nn_layout_width() {
        use crate::world::nn::NnInputGroup;

        let make = |grass_multisight: bool| {
            WorldHandle::new_with_founder_count(
                if grass_multisight {
                    "grass-multisight-on"
                } else {
                    "grass-multisight-off"
                },
                0,
                100.0,
                1,
                false,
                "",
                1200.0,
                true,
                1,
                false,
                1.0,
                10,
                10,
                3.0,
                5.0,
                grass_multisight,
                0,
                0,
                1.0,
                1.0,
            )
            .unwrap()
        };

        let off = make(false);
        assert_eq!(
            off.inner
                .nn_input_layout
                .offset_of(NnInputGroup::GrassBandsFar),
            None,
            "grass_multisight=false must omit the far grass band"
        );
        assert_eq!(
            off.inner.nn_input_layout.width(),
            32,
            "grass_multisight=false should keep the reduced single-band input width"
        );
        assert_eq!(
            off.inner.nn_topology.input_width(),
            off.inner.nn_input_layout.width(),
            "topology must be reconciled to the construction-time layout"
        );

        let on = make(true);
        let default = WorldHandle::new("grass-multisight-default");
        assert!(
            on.inner
                .nn_input_layout
                .offset_of(NnInputGroup::GrassBandsFar)
                .is_some(),
            "grass_multisight=true must include the far grass band"
        );
        assert_eq!(
            on.inner.nn_input_layout.width(),
            default.inner.nn_input_layout.width(),
            "grass_multisight=true must retain the default input width"
        );
        assert_eq!(
            on.inner.nn_topology.input_width(),
            on.inner.nn_input_layout.width(),
            "topology must be reconciled to the construction-time layout"
        );
    }

    // ─── S17: typed slider setter tests ─────────────────────────────────────

    // ─── P3d: observability counter tests ────────────────────────────────────

    /// live_grass_cell_count never exceeds the runtime grass_cell_count and
    /// doesn't double-count empty cells (cells with density == 0 are excluded).
    #[test]
    fn live_grass_cell_count_does_not_exceed_cell_count() {
        let handle = WorldHandle::new("p3d-grass-cells");
        let cell_count = handle.inner.dims.grass_cell_count as u32;
        let live = handle.live_grass_cell_count();
        assert!(
            live <= cell_count,
            "live_grass_cell_count={live} must not exceed grass_cell_count={cell_count}"
        );
        // All density values counted must be > 0 (no double-counting empty cells).
        let manual: u32 = handle
            .inner
            .grass
            .density
            .iter()
            .filter(|b| b.load(core::sync::atomic::Ordering::Relaxed) > 0)
            .count() as u32;
        assert_eq!(
            live, manual,
            "live_grass_cell_count must match manual filter count"
        );
    }

    /// total_grass_density is non-negative (no cell can have negative density).
    #[test]
    fn total_grass_density_is_non_negative() {
        let handle = WorldHandle::new("p3d-grass-density");
        let total = handle.total_grass_density();
        assert!(
            total >= 0.0,
            "total_grass_density must be non-negative, got {total}"
        );
    }

    #[test]
    fn telemetry_samples_on_fixed_cadence_and_exports_shape() {
        let mut handle = tiny_handle("telemetry-cadence");
        handle.step_n(TELEMETRY_SAMPLE_PERIOD_TICKS - 1);
        assert_eq!(handle.telemetry.samples.len(), 0);

        handle.step_n(1);
        assert_eq!(handle.telemetry.samples.len(), 1);

        let report: serde_json::Value =
            serde_json::from_str(&handle.telemetry_report_json()).expect("telemetry report parses");
        assert_eq!(
            report["schema_version"].as_u64(),
            Some(TELEMETRY_SCHEMA_VERSION as u64)
        );
        assert_eq!(report["truncated"], false);
        assert_eq!(
            report["caps"]["sample_period_ticks"].as_u64(),
            Some(TELEMETRY_SAMPLE_PERIOD_TICKS as u64)
        );
        let samples = report["samples"].as_array().expect("samples array");
        assert_eq!(samples.len(), 1);
        let sample = &samples[0];
        assert_eq!(
            sample["tick_end"].as_u64(),
            Some(TELEMETRY_SAMPLE_PERIOD_TICKS as u64)
        );
        assert!(sample.get("population").is_some());
        assert!(sample.get("jank_count_delta").is_some());
        assert!(sample.get("live_grass_cell_count").is_some());
        assert!(report["events"]
            .as_array()
            .expect("events array")
            .iter()
            .any(|event| event["kind"] == "world_start"));
    }

    #[test]
    fn telemetry_worst_jank_replaces_only_when_duration_increases() {
        let mut handle = tiny_handle("telemetry-worst-jank");
        handle.inner.tick = 1;
        handle.observe_completed_tick(0.0, 20.0, true);
        handle.inner.tick = 2;
        handle.observe_completed_tick(20.0, 38.0, true);
        handle.inner.tick = 3;
        handle.observe_completed_tick(38.0, 63.0, true);

        let report: serde_json::Value =
            serde_json::from_str(&handle.telemetry_report_json()).expect("telemetry report parses");
        assert_eq!(report["jank"]["count"].as_u64(), Some(3));
        assert_eq!(report["jank"]["worst"]["tick"].as_u64(), Some(3));
        assert_eq!(report["jank"]["worst"]["duration_ms"].as_f64(), Some(25.0));
        assert_eq!(report["jank"]["worst"]["phase"]["phase"], "unknown");

        let worst_events = report["events"]
            .as_array()
            .expect("events array")
            .iter()
            .filter(|event| event["kind"] == "jank_worst")
            .count();
        assert_eq!(
            worst_events, 2,
            "20ms and 25ms replace worst; 18ms only increments count"
        );
    }

    #[test]
    fn reset_jank_preserves_history_samples_and_clears_summary() {
        let mut handle = tiny_handle("telemetry-reset-jank");
        handle.step_n(TELEMETRY_SAMPLE_PERIOD_TICKS);
        assert_eq!(handle.telemetry.samples.len(), 1);

        handle.inner.tick = TELEMETRY_SAMPLE_PERIOD_TICKS + 1;
        handle.observe_completed_tick(1000.0, 1030.0, true);
        assert!(handle.telemetry.worst_jank.is_some());
        handle.reset_jank();

        let report: serde_json::Value =
            serde_json::from_str(&handle.telemetry_report_json()).expect("telemetry report parses");
        assert_eq!(report["jank"]["count"].as_u64(), Some(0));
        assert!(report["jank"]["worst"].is_null());
        assert_eq!(
            report["samples"].as_array().expect("samples array").len(),
            1
        );
        assert!(report["events"]
            .as_array()
            .expect("events array")
            .iter()
            .any(|event| event["kind"] == "jank_reset"));
    }

    /// S17: try_set_slider returns true for all known names. Uses the inner
    /// helper rather than set_slider directly because JsValue::from_str
    /// panics on non-wasm32 targets in the Err branch.
    #[test]
    fn set_slider_known_names_dispatch_ok() {
        let mut handle = WorldHandle::new("s17-known");
        for name in &[
            "mutation_rate_multiplier",
            "_reserved_legacy_nn_mutation_sigma",
            "bucket_0_weight",
            "bucket_7_sigma",
        ] {
            assert!(handle.try_set_slider(name, 0.5), "expected true for {name}");
        }
    }

    /// S17: try_set_slider returns false for an unknown name (exercises the
    /// Err branch of set_slider without touching JsValue).
    #[test]
    fn set_slider_unknown_returns_false() {
        let mut handle = WorldHandle::new("s17-unknown");
        assert!(
            !handle.try_set_slider("bogus_slider", 1.0),
            "unknown slider name must return false"
        );
    }

    /// v2.0.x regression guard for the "any slider Apply wipes the world" bug:
    /// the sim worker seeds EVERY SAB slider lane from
    /// `boot.initial_sliders[name] ?? sliders_defaults_json()[name]`, then drains
    /// ALL lanes on every control-epoch advance. If any `SLIDER_NAMES` entry is
    /// missing from `sliders_defaults_json`, its lane can be left at 0 and the
    /// next Apply re-applies 0 — which for `max_population` clamps the cull cap to
    /// 1 and kills the whole population. This test makes the defaults map a
    /// complete cover of `SLIDER_NAMES` so the worker fallback can never leave a
    /// lane unseeded.
    #[test]
    fn sliders_defaults_json_covers_every_slider_name() {
        let handle = WorldHandle::new("defaults-cover");
        let json: serde_json::Value =
            serde_json::from_str(&handle.sliders_defaults_json()).expect("defaults json parses");
        let obj = json.as_object().expect("defaults json is an object");
        for name in SLIDER_NAMES.iter() {
            assert!(
                obj.contains_key(*name),
                "sliders_defaults_json is missing \"{name}\" — its SAB lane could be \
                 seeded to 0 and the worker's all-lane drain would apply 0 on the next \
                 Apply (this is the max_population cull bug)"
            );
        }
    }

    /// v2.0.x regression guard for the snapshot Float32Array alignment crash:
    /// `new Float32Array(buffer, base + creatureSoAOffset(slot), …)` requires a
    /// 4-aligned byte offset. Slot 1's creature SoA starts at `slot_bytes + 64`,
    /// so `slot_bytes` must be a multiple of 4 — but the grass + biome windows are
    /// `2 * win_axis²`, which is ≡ 2 (mod 4) for ODD grass_dim. The slot must be
    /// padded so this holds for every world_size/grass_size combo.
    #[test]
    fn snapshot_slot_bytes_4_aligned_for_all_grass_dims() {
        // Even (default 1920 / large 3840) and ODD (from 19200/7, 9600/7, etc.).
        for grass_dim in [1usize, 1920, 3840, 2743, 1371, 2133, 1067] {
            let layout = SnapshotLayout::from_grass_cell_count(grass_dim * grass_dim);
            assert_eq!(
                layout.slot_bytes % 4,
                0,
                "slot_bytes not 4-aligned for grass_dim={grass_dim}"
            );
            assert_eq!(
                layout.buf_bytes % 4,
                0,
                "buf_bytes not 4-aligned for grass_dim={grass_dim}"
            );
            // Slot 1's f32 creature SoA byte offset within the buffer.
            assert_eq!(
                (layout.slot_bytes + SNAPSHOT_HEADER_BYTES) % 4,
                0,
                "slot-1 creature offset not 4-aligned for grass_dim={grass_dim}"
            );
        }
    }

    mod biome_pyramid;
    mod dims;
    mod lod_config;
    mod lod_stepper;
    mod nn_inspect;
    mod snapshot;
}
