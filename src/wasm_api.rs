//! Thin wasm-bindgen wrapper exposing the World to JS. Stable function
//! shapes so the web shell can iterate independently.

use crate::brain::{Activation, NnTopology};
use crate::constants::*;
use crate::world::World;
use wasm_bindgen::prelude::*;

// v2.0 Wave 1a: computed-dims (snapshot grass u8 + biomeSab) tests live in their
// own file/module to avoid the shared-`mod tests` merge hazard.
#[cfg(test)]
#[path = "wasm_dims_tests.rs"]
mod wasm_dims_tests;

// v2.0 Wave 2a: snapshot-repack lane-offset tests (color_u32 / packed_u32 /
// region size) live in their own file/module (same merge-hazard rule).
#[cfg(test)]
#[path = "wasm_snapshot_tests.rs"]
mod wasm_snapshot_tests;

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

/// Canonical ordered list of dev-panel slider names. Indices are stable across
/// the SAB transport: TS writes `value` into `CTRL_SLIDERS[idx]` + bumps
/// `CTRL_CONTROL_EPOCH`, the worker reads at top of tick and dispatches via
/// [`WorldHandle::set_slider_by_index`]. Re-generate TS bindings after
/// modifying this list: `cargo run --bin gen-bindings`. A drift unit test
/// catches mismatches at `cargo test --lib`.
pub const SLIDER_NAMES: &[&str] = &[
    "mutation_rate_multiplier",            // 0  f32
    "_reserved_legacy_nn_mutation_sigma",  // 1  f32 — v1.12 reserved no-op; was nn_mutation_sigma
    "eat_bite_fraction",                   // 2  f32
    "grass_propagation_rate_k",            // 3  f32
    "grass_in_cell_growth_r",              // 4  f32
    "upkeep_multiplier",                   // 5  f32
    "move_cost_multiplier",                // 6  f32
    "energy_max",                          // 7  f32
    "grass_energy_per_bite",               // 8  f32
    "grass_bites_per_block",               // 9  u32 (carried as f32)
    "digestion_cooldown",                  // 10 u32 (carried as f32)
    "repulsion_max",                       // 11 f32
    "max_age",                             // 12 u32 (carried as f32)
    "split_threshold",                     // 13 f32
    "split_gift",                          // 14 f32
    "split_jitter",                        // 15 f32
    "founder_count",                       // 16 u32 (carried as f32)
    "max_population",                      // 17 u32 (carried as f32) — user-tunable cap
    // v2.0 Wave 1a construction settings (shape the next world only). DROPPED
    // here vs v1.x: the four `_reserved_curriculum_*` slots and `full_grass_on_init`.
    "world_size",                          // 18 f32 (construction) — world extent
    "world_seed",                          // 19 u32 (carried as f32) — biome seed (Wave 1b)
    "wrap_world",                          // 20 bool (0.0 / non-zero) — torus vs walled
    // v2.0 Wave 1b: live-tunable biome movement-penalty base severities [0,1].
    "water_movement_penalty",              // 21 f32 — Water biome base severity
    "desert_movement_penalty",             // 22 f32 — Desert biome base severity
    // v2.0 Wave 2a: live multiplier on the per-birth body-genome mutation step.
    "trait_mutation_sigma_multiplier",     // 23 f32 — genome trait-sigma multiplier
    // v2.0 Wave 3a: species + sexual-mating construction/live settings.
    "species_mode",                        // 24 bool (0/non-zero) construction — species + mating
    "crossover_mode",                      // 25 enum (0=average, non-zero=fifty_fifty) construction
    "starting_species_count",              // 26 u32 (carried as f32) construction
    "starting_species_member_count",       // 27 u32 (carried as f32) construction
    "starting_species_member_variance",    // 28 f32 construction (INERT in W3; W4 applies it)
    "mating_cooldown_ticks",               // 29 u32 (carried as f32) live — initiator-only cooldown
    // v1.12: 8 mutation buckets × 3 floats. Index = SLIDER_BUCKET_BASE + bucket*3 + field.
    // field 0 = weight, 1 = rate, 2 = sigma. See MUTATION_BUCKET_COUNT.
    "bucket_0_weight", "bucket_0_rate", "bucket_0_sigma", // 30..33
    "bucket_1_weight", "bucket_1_rate", "bucket_1_sigma", // 33..36
    "bucket_2_weight", "bucket_2_rate", "bucket_2_sigma", // 36..39
    "bucket_3_weight", "bucket_3_rate", "bucket_3_sigma", // 39..42
    "bucket_4_weight", "bucket_4_rate", "bucket_4_sigma", // 42..45
    "bucket_5_weight", "bucket_5_rate", "bucket_5_sigma", // 45..48
    "bucket_6_weight", "bucket_6_rate", "bucket_6_sigma", // 48..51
    "bucket_7_weight", "bucket_7_rate", "bucket_7_sigma", // 51..54
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
// and stored on the `WorldHandle`. The "computed-dims-equality" safety model
// replaces the old constant-equality asserts: the boot-computed grass region
// size must equal `grass_cell_count` bytes, and the dedicated `biomeSab` is the
// same size.

/// Bytes per snapshot stats header (matches `SNAPSHOT_HEADER_BYTES` TS-side).
/// 20 bytes payload + 12 bytes padding so the creature SoA that follows is
/// 32-byte aligned (creature stride is 32 bytes).
pub const SNAPSHOT_HEADER_BYTES: usize = 32;
/// Bytes per creature record in the snapshot SoA (matches CREATURE_STRIDE × 4).
pub const SNAPSHOT_CREATURE_STRIDE: usize = 32;
/// Total creature SoA region size per slot. UNCHANGED in v2.0 (world-size
/// independent — sized by `MAX_POP_FOR_SIM`).
pub const SNAPSHOT_CREATURE_BYTES: usize = MAX_POP_FOR_SIM * SNAPSHOT_CREATURE_STRIDE;

/// Computed snapshot region sizes for a given grass cell count. v2.0 Wave 1a:
/// the grass region is `grass_cell_count` BYTES (u8 per cell, quantized on
/// write), so the per-slot/buf totals are runtime values, not constants.
#[derive(Clone, Copy, Debug)]
struct SnapshotLayout {
    /// u8 grass region size per slot = grass_cell_count bytes.
    grass_bytes: usize,
    /// header + creatures + grass.
    slot_bytes: usize,
    /// two double-buffered slots.
    buf_bytes: usize,
    /// biomeSab size = grass_cell_count bytes (u8 per cell).
    biome_bytes: usize,
}

impl SnapshotLayout {
    fn from_grass_cell_count(grass_cell_count: usize) -> Self {
        // u8 grass: one byte per cell. The biomeSab mirrors the same cell count.
        let grass_bytes = grass_cell_count;
        let slot_bytes = SNAPSHOT_HEADER_BYTES + SNAPSHOT_CREATURE_BYTES + grass_bytes;
        let buf_bytes = 2 * slot_bytes;
        // Computed-dims-equality safety model: the grass region and the biomeSab
        // are both derived from the same `grass_cell_count`, so they must agree.
        let biome_bytes = grass_cell_count;
        debug_assert_eq!(
            grass_bytes, biome_bytes,
            "snapshot grass region (u8) and biomeSab must both equal grass_cell_count bytes"
        );
        Self {
            grass_bytes,
            slot_bytes,
            buf_bytes,
            biome_bytes,
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
    /// v2.0 Wave 1a: dedicated biome SAB — one u8 per grass cell (`Biome` enum,
    /// `repr(u8)`). Wave 1a fills it with `Plains` (0) placeholder; Wave 1b
    /// populates it from `world_seed`. Lives in wasm linear memory like the
    /// snapshot buffer; the main thread reads via a view at `biome_buf_byte_offset`.
    biome_buf: Vec<u8>,
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
        Self::from_world(inner)
    }

    /// Allocate the runtime-sized snapshot + biome buffers from a constructed
    /// world's grass dims and assemble the handle. v2.0 Wave 1a: the snapshot
    /// grass region and the biomeSab are both `grass_cell_count` u8 bytes/slot.
    fn from_world(inner: World) -> Self {
        let grass_cell_count = inner.dims.grass_cell_count;
        let layout = SnapshotLayout::from_grass_cell_count(grass_cell_count);
        // v2.0 Wave 1b: fill the biomeSab from the world's generated biome grid
        // (deterministic from `world_seed`). The grid is exactly
        // `grass_cell_count` u8 tags, matching `biome_bytes`.
        debug_assert_eq!(
            inner.biome_grid_bytes().len(),
            layout.biome_bytes,
            "biome grid length must equal biomeSab byte size"
        );
        let biome_buf = inner.biome_grid_bytes().to_vec();
        Self {
            inner,
            snapshot_buf: vec![0u8; layout.buf_bytes],
            snapshot_layout: layout,
            biome_buf,
            tick_intervals_ms: std::collections::VecDeque::new(),
            last_tick_end_ms: None,
            jank_count: 0,
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
            founder_count: founder_count.clamp(1, 32),
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

    /// Run multiple ticks; stops early on game-over.
    /// Tracks per-tick wall-clock duration for TPS and jank accounting.
    #[wasm_bindgen]
    pub fn step_n(&mut self, n: u32) -> bool {
        for _ in 0..n {
            let t0 = wasm_now_ms();
            let alive = self.inner.tick_once();
            let now = wasm_now_ms();
            let work_dur_ms = now - t0;
            // TPS uses the wall-clock interval between consecutive tick
            // completions so it correctly reflects pacing waits, not just
            // computation time. Skip the very first tick (no prior anchor).
            if let Some(prev) = self.last_tick_end_ms {
                let interval_ms = now - prev;
                if self.tick_intervals_ms.len() >= TPS_WINDOW {
                    self.tick_intervals_ms.pop_front();
                }
                self.tick_intervals_ms.push_back(interval_ms);
            }
            self.last_tick_end_ms = Some(now);
            if work_dur_ms > JANK_BUDGET_MS {
                self.jank_count = self.jank_count.saturating_add(1);
            }
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

    // ─── Grass render API (P1g) ──────────────────────────────────────────────

    /// Runtime grass grid dimension (cells per axis). v2.0 Wave 1a: computed at
    /// boot from `world_size` (1920 at the 9600u default). The boot handshake
    /// carries this so the main thread sizes its grass + biome views.
    #[wasm_bindgen(getter)]
    pub fn grass_dim(&self) -> u32 {
        self.inner.dims.grass_dim as u32
    }

    /// Grass cell size in world-units (5.0). Constant accessor; used by the
    /// render layer for world→screen coordinate conversion.
    #[wasm_bindgen(getter)]
    pub fn grass_cell_size(&self) -> f32 {
        GRASS_CELL_SIZE
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

    // ─── v2.0 Wave 1a: dedicated biome SAB ───────────────────────────────────

    /// Byte offset of the biome buffer (one u8 `Biome` per grass cell) inside
    /// wasm linear memory. Main thread builds a view at this offset of length
    /// `biome_buf_byte_len()`. Wave 1b: filled from `world_seed` via
    /// `biome::generate_biome_grid` (Plains/Water/Desert blobs).
    #[wasm_bindgen(getter)]
    pub fn biome_buf_byte_offset(&self) -> u32 {
        self.biome_buf.as_ptr() as u32
    }

    /// Length of the biome buffer in bytes = `grass_cell_count` (u8 per cell).
    #[wasm_bindgen(getter)]
    pub fn biome_buf_byte_len(&self) -> u32 {
        self.snapshot_layout.biome_bytes as u32
    }

    /// Write a full snapshot (stats header + creature SoA + f32 grass) into
    /// the requested slot of `self.snapshot_buf` (which lives in wasm linear
    /// memory). No JS-side `Uint8Array` parameters — main reads the same
    /// region via a view over `wasm.memory.buffer`. The atomic flip + seq
    /// bump on the control SAB remain the worker's responsibility.
    ///
    /// Layout per slot (little-endian throughout):
    ///   `[0..20)`   tick, pop, world_ended, tps_bits, jank_count   (u32 each)
    ///   `[20..32)`  padding (32-byte align for creature SoA)
    ///   `[32..32 + MAX_POP_FOR_SIM × 32)` creature SoA, 32 B stride. v2.0 Wave
    ///     2a lane order (8 lanes): x, y, radius, color_u32, id_lo, id_hi,
    ///     packed_u32, (pad). See `fill_creature_bytes` / `genome_color_u32` /
    ///     `pack_render_u32` for the packed-field bit layouts.
    ///   `[trailing grass_cell_count bytes)` u8 quantized grass density
    #[wasm_bindgen]
    pub fn write_snapshot(&mut self, slot: u32) {
        let profile_on = self.inner.profile.enabled();
        let snap_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };

        let slot_bytes = self.snapshot_layout.slot_bytes;
        let grass_bytes = self.snapshot_layout.grass_bytes;
        let slot_idx = (slot as usize) & 1;
        let slot_base = slot_idx * slot_bytes;

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

        // Stats header — 20 bytes LE at slot offset 0.
        let tps_bits = self.tps().to_bits();
        let jank = self.jank_count;
        let tick = self.inner.tick;
        let ended = if self.inner.world_ended { 1u32 } else { 0u32 };
        let header = &mut self.snapshot_buf[slot_base..slot_base + 20];
        header[0..4].copy_from_slice(&tick.to_le_bytes());
        header[4..8].copy_from_slice(&(pop_written as u32).to_le_bytes());
        header[8..12].copy_from_slice(&ended.to_le_bytes());
        header[12..16].copy_from_slice(&tps_bits.to_le_bytes());
        header[16..20].copy_from_slice(&jank.to_le_bytes());

        // Grass: v2.0 Wave 1a quantizes f32 density (0..GRASS_MAX) → u8 (0..255),
        // one byte per cell, directly into the snapshot region. No JS boundary.
        let grass_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };
        let grass_off = slot_base + SNAPSHOT_HEADER_BYTES + SNAPSHOT_CREATURE_BYTES;
        let density: &[f32] = &self.inner.grass.density;
        let grass_region = &mut self.snapshot_buf[grass_off..grass_off + grass_bytes];
        quantize_grass_into(density, grass_region);
        let grass_end = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };

        if let (Some(start), Some(c0), Some(c1), Some(g0), Some(g1)) = (
            snap_start,
            creatures_start,
            creatures_end,
            grass_start,
            grass_end,
        ) {
            let prof = &self.inner.profile;
            prof.record_under_root(
                "sim_worker",
                "write_output_sab.snapshot",
                g1.saturating_sub(start) as u32,
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
        self.inner.sliders.founder_count = value.clamp(1, 32);
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
    /// v2.0 Wave 1b live: Water biome base movement-penalty severity, clamped [0, 1].
    fn apply_water_movement_penalty(&mut self, value: f32) {
        self.inner.sliders.water_movement_penalty = value.clamp(0.0, 1.0);
    }
    /// v2.0 Wave 1b live: Desert biome base movement-penalty severity, clamped [0, 1].
    fn apply_desert_movement_penalty(&mut self, value: f32) {
        self.inner.sliders.desert_movement_penalty = value.clamp(0.0, 1.0);
    }
    /// v2.0 Wave 2a live: per-birth body-genome mutation-step multiplier. Clamped
    /// non-negative (a negative sigma is meaningless).
    fn apply_trait_mutation_sigma_multiplier(&mut self, value: f32) {
        self.inner.sliders.trait_mutation_sigma_multiplier = value.max(0.0);
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
    /// `web/src/generated/slider-ids.ts` (regenerate via
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
            // v2.0 Wave 1b live biome movement-penalty severities [0, 1].
            21 => self.apply_water_movement_penalty(value),
            22 => self.apply_desert_movement_penalty(value),
            // v2.0 Wave 2a live genome trait-sigma multiplier.
            23 => self.apply_trait_mutation_sigma_multiplier(value),
            // v2.0 Wave 3a species + sexual-mating settings.
            24 => self.apply_species_mode(value != 0.0),
            25 => self.apply_crossover_mode(value),
            26 => self.apply_starting_species_count(value.max(0.0) as u32),
            27 => self.apply_starting_species_member_count(value.max(0.0) as u32),
            28 => self.apply_starting_species_member_variance(value),
            29 => self.apply_mating_cooldown_ticks(value.max(0.0) as u32),
            // v1.12: 8 mutation buckets × 3 fields.
            n if n >= SLIDER_BUCKET_BASE
                && n < SLIDER_BUCKET_BASE + MUTATION_BUCKET_COUNT * 3 =>
            {
                let rel = n - SLIDER_BUCKET_BASE;
                self.apply_mutation_bucket(rel / 3, rel % 3, value);
            }
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
    /// v2.0 Wave 2a schema: drops `color_ema` (color now derives from the
    /// genome); adds the 6 genome traits + the creature's current
    /// genome-modulated movement penalty.
    #[wasm_bindgen]
    pub fn creature_inspect_json(&self, idx: u32) -> Option<String> {
        let i = idx as usize;
        if i >= self.inner.creatures.len() {
            return None;
        }
        let action_name = format!("{:?}", self.inner.creatures.action_this_tick[i]);
        let brain = &self.inner.creatures.brains[i];
        let g = &self.inner.creatures.genome[i];
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
        // v2.0 Wave 2a: genome-modulated movement penalty at the current cell.
        let movement_penalty = self.inner.movement_penalty_for(i);
        // v2.0 Wave 3a: species fields (species_mode only). The species-history
        // breadcrumb is Wave 5 — left empty here.
        let species_mode = self.inner.sliders.species_mode;
        let species_id = self.inner.creatures.species_id[i];
        let species_color = self.inner.species.color_of(species_id);
        let mut json = serde_json::json!({
            "index": idx,
            "id": self.inner.creatures.id[i],
            "x": cx,
            "y": cy,
            "age": self.inner.creatures.age[i],
            "max_age": self.inner.sliders.max_age,
            "energy": self.inner.creatures.energy[i],
            "energy_frac": (self.inner.creatures.energy[i] / 100.0).clamp(0.0, 1.0),
            "size": CREATURE_SIZE * g.body_size_factor(),
            "current_action": action_name,
            "move_speed": MOVE_SPEED_MAX * g.max_speed_factor(),
            "cooldown_remaining": self.inner.creatures.digestion_cooldown[i],
            // v2.0 Wave 2a: the 6 evolving body-genome traits (raw [0,1]).
            "genome": {
                "body_size": g.body_size,
                "max_speed": g.max_speed,
                "metabolism": g.metabolism,
                "diet": g.diet,
                "water_affinity": g.water_affinity,
                "heat_tolerance": g.heat_tolerance,
            },
            // v2.0 Wave 2a: current genome-modulated biome movement penalty [0,1].
            "movement_penalty": movement_penalty,
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
                obj.insert("species_history".into(), serde_json::json!([] as [u16; 0]));
            }
        }
        Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
    }

    // ─── P3d observability counters ──────────────────────────────────────────

    /// Count of grass cells where density > 0. O(921_600) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn live_grass_cell_count(&self) -> u32 {
        self.inner
            .grass
            .density
            .iter()
            .filter(|&&d| d > 0.0)
            .count() as u32
    }

    /// Sum of all grass cell densities. O(921_600) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn total_grass_density(&self) -> f32 {
        self.inner.grass.density.iter().sum()
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
    /// subtree is maintained separately in `web/src/perf.ts` (R4).
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
            // v2.0 Wave 1b live biome movement-penalty severities.
            "water_movement_penalty": d.water_movement_penalty,
            "desert_movement_penalty": d.desert_movement_penalty,
            // v2.0 Wave 2a genome trait-sigma multiplier.
            "trait_mutation_sigma_multiplier": d.trait_mutation_sigma_multiplier,
            // v2.0 Wave 3a species + sexual-mating settings (bools/enums as 0/1).
            "species_mode": if d.species_mode { 1.0_f32 } else { 0.0 },
            "crossover_mode": d.crossover_mode.to_slider(),
            "starting_species_count": d.starting_species_count as f32,
            "starting_species_member_count": d.starting_species_member_count as f32,
            "starting_species_member_variance": d.starting_species_member_variance,
            "mating_cooldown_ticks": d.mating_cooldown_ticks as f32,
        });
        // v1.12: 8 mutation buckets × 3 fields. Names match SLIDER_NAMES.
        let obj = json.as_object_mut().expect("json! produced an object");
        for (i, b) in d.mutation_policy.buckets.iter().enumerate() {
            obj.insert(format!("bucket_{i}_weight"), serde_json::json!(b.weight));
            obj.insert(format!("bucket_{i}_rate"), serde_json::json!(b.rate));
            obj.insert(format!("bucket_{i}_sigma"), serde_json::json!(b.sigma));
        }
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
    let base_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
    // v2.0 Wave 3a: in species mode the body color is the SPECIES color (looked
    // up sim-side from the registry) so the renderer needs no change this wave;
    // single-pool keeps the genome-derived color. The per-creature `species_id`
    // also rides the snapshot `packed_u32` (bits 7..23).
    let species_mode = world.sliders.species_mode;
    for i in 0..pop {
        let x = world.creatures.x[i];
        let y = world.creatures.y[i];
        let g = &world.creatures.genome[i];
        // v2.0 Wave 2a: sprite radius is body_size-derived.
        let body_r = base_r * g.body_size_factor();
        let species_id = world.creatures.species_id[i];
        // Display color: species color (species_mode) or genome color (single-pool).
        let color = if species_mode {
            world.species.color_of(species_id)
        } else {
            genome_color_u32(g)
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

/// v2.0 Wave 2a: single-pool genome → RGBA8 display color packed into one u32.
///
/// HSV mapping (visual identity, FEEL KNOB). hue ← `diet` (`hue_deg = 120 * (1 -
/// diet)`, grazer green 120° → predator red 0°); saturation ← `body_size`
/// (mapped to [0.4, 1.0]; floor keeps small grazers visible); value ←
/// `max_speed` (mapped to [0.5, 1.0]; faster = brighter).
///
/// Returned as RGBA8 little-endian: R = bits 0..8, G = 8..16, B = 16..24, A =
/// 24..32 (A always 255). The TS reader (2b) does `R = u & 0xFF`, etc.
#[inline]
fn genome_color_u32(g: &crate::creature::Genome) -> u32 {
    let hue = 120.0 * (1.0 - g.diet); // degrees, 120(green)→0(red)
    let sat = 0.4 + 0.6 * g.body_size; // [0.4, 1.0]
    let val = 0.5 + 0.5 * g.max_speed; // [0.5, 1.0]
    let (r, gg, b) = hsv_to_rgb8(hue, sat, val);
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
#[inline]
fn quantize_grass_into(density: &[f32], dst: &mut [u8]) {
    debug_assert_eq!(
        density.len(),
        dst.len(),
        "u8 grass region must be one byte per cell"
    );
    let inv_max = 1.0 / GRASS_MAX;
    for (d, out) in density.iter().zip(dst.iter_mut()) {
        // clamp to [0, GRASS_MAX] then scale to [0, 255], rounding to nearest.
        let q = (d * inv_max).clamp(0.0, 1.0) * 255.0;
        *out = (q + 0.5) as u8;
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

    /// v1.6 S A1: `write_snapshot_to_native` produces bytes that match the
    /// documented stride-8 layout. Exercises the same writer the wasm path
    /// drives so the SAB layout is testable off-wasm.
    #[test]
    fn write_snapshot_to_layout_matches_stride() {
        let mut handle = WorldHandle::new_with_founder_count(
            "a1-snapshot", 0, 100.0, 3, false, "", 1200.0, false, 1, false, 1.0, 10, 10, 3.0,
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
            // Radius is body_size-derived (genome factor 0.5..1.5).
            let expected_r = CREATURE_SIZE
                * BODY_RADIUS_PER_SIZE
                * handle.inner.creatures.genome[i].body_size_factor();
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
            assert_eq!((packed >> 3) & 0xF, handle.inner.creatures.flash_ticks[i] as u32);
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
            let expected = {
                let q = (handle.inner.grass.density[i] / GRASS_MAX).clamp(0.0, 1.0) * 255.0;
                (q + 0.5) as u8
            };
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
            "e21-creature-at", 0, 100.0, 1, false, "", 1200.0, false, 1, false, 1.0, 10, 10, 3.0,
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
            .filter(|&&d| d > 0.0)
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
}
