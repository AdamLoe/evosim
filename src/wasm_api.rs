//! Thin wasm-bindgen wrapper exposing the World to JS. Stable function
//! shapes so the web shell can iterate independently.

use crate::brain::{Activation, NnTopology};
use crate::constants::*;
use crate::world::World;
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
    "_reserved_curriculum_min_pop",        // 17 reserved no-op (was curriculum_min_pop)
    "_reserved_curriculum_max_pop",        // 18 reserved no-op (was curriculum_max_pop)
    "_reserved_curriculum_min_factor",     // 19 reserved no-op (was curriculum_min_factor)
    "_reserved_auto_curriculum",           // 20 reserved no-op (was auto_curriculum)
    "full_grass_on_init",                  // 21 bool (0.0 / non-zero)
    "max_population",                      // 22 u32 (carried as f32) — user-tunable cap
    // v1.12: 8 mutation buckets × 3 floats. Index = 23 + bucket * 3 + field.
    // field 0 = weight, 1 = rate, 2 = sigma. See MUTATION_BUCKET_COUNT.
    "bucket_0_weight", "bucket_0_rate", "bucket_0_sigma", // 23..26
    "bucket_1_weight", "bucket_1_rate", "bucket_1_sigma", // 26..29
    "bucket_2_weight", "bucket_2_rate", "bucket_2_sigma", // 29..32
    "bucket_3_weight", "bucket_3_rate", "bucket_3_sigma", // 32..35
    "bucket_4_weight", "bucket_4_rate", "bucket_4_sigma", // 35..38
    "bucket_5_weight", "bucket_5_rate", "bucket_5_sigma", // 38..41
    "bucket_6_weight", "bucket_6_rate", "bucket_6_sigma", // 41..44
    "bucket_7_weight", "bucket_7_rate", "bucket_7_sigma", // 44..47
];

/// First mutation-bucket slider slot. v1.12.
pub const SLIDER_BUCKET_BASE: usize = 23;

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
// Grass density is stored as raw f32 (no `(d * 255) as u8` quantize). The
// renderer uploads it as an `R32F` texture (WebGL2 supports this natively).
// Trade: 4× more grass bytes per slot (3.7 MB vs 921 KB) for zero quantize
// work in the sim worker — net win, since the wasm-internal memcpy is much
// faster than the boundary copy plus quantize.

/// Bytes per snapshot stats header (matches `SNAPSHOT_HEADER_BYTES` TS-side).
/// 20 bytes payload + 12 bytes padding so the creature SoA that follows is
/// 32-byte aligned (creature stride is 32 bytes).
pub const SNAPSHOT_HEADER_BYTES: usize = 32;
/// Bytes per creature record in the snapshot SoA (matches CREATURE_STRIDE × 4).
pub const SNAPSHOT_CREATURE_STRIDE: usize = 32;
/// Total creature SoA region size per slot.
pub const SNAPSHOT_CREATURE_BYTES: usize = MAX_POP_FOR_SIM * SNAPSHOT_CREATURE_STRIDE;
/// Total grass region size per slot — v1.11: f32 per cell (no quantize).
pub const SNAPSHOT_GRASS_BYTES: usize = GRASS_CELL_COUNT * 4;
/// Bytes per snapshot slot.
pub const SNAPSHOT_SLOT_BYTES: usize =
    SNAPSHOT_HEADER_BYTES + SNAPSHOT_CREATURE_BYTES + SNAPSHOT_GRASS_BYTES;
/// Total snapshot region size — two double-buffered slots.
pub const SNAPSHOT_BUF_BYTES: usize = 2 * SNAPSHOT_SLOT_BYTES;

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
        Self {
            inner,
            snapshot_buf: vec![0u8; SNAPSHOT_BUF_BYTES],
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
    pub fn new_with_founder_count(
        seed: &str,
        initial_grass_seed_count: u32,
        energy_max: f32,
        founder_count: u32,
        full_grass_on_init: bool,
        nn_topology_json: &str,
    ) -> Result<WorldHandle, JsValue> {
        let actual_seed = if seed.is_empty() {
            let mut bytes = [0u8; 8];
            getrandom::getrandom(&mut bytes).ok();
            let n = u64::from_le_bytes(bytes);
            format!("seed-{n:016x}")
        } else {
            seed.to_string()
        };
        let sliders = crate::world::DevSliders {
            grass_initial_seed_count: initial_grass_seed_count,
            energy_max: energy_max.max(1.0),
            founder_count: founder_count.clamp(1, 32),
            full_grass_on_init,
            ..Default::default()
        };
        let topology = parse_nn_topology(nn_topology_json)
            .map_err(|e| JsValue::from_str(&format!("nn_topology error: {e}")))?;
        let inner = World::new_with_sliders_topology(actual_seed, sliders, topology);
        Ok(Self {
            inner,
            snapshot_buf: vec![0u8; SNAPSHOT_BUF_BYTES],
            tick_intervals_ms: std::collections::VecDeque::new(),
            last_tick_end_ms: None,
            jank_count: 0,
        })
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

    #[wasm_bindgen(getter)]
    pub fn world_size(&self) -> f32 {
        WORLD_SIZE
    }

    // ─── Grass render API (P1g) ──────────────────────────────────────────────

    /// Grass grid dimension (480). Constant accessor; called per frame by the
    /// renderer to avoid hard-coding the value in TS.
    #[wasm_bindgen(getter)]
    pub fn grass_dim(&self) -> u32 {
        GRASS_GRID_DIM as u32
    }

    /// Grass cell size in world-units (1.25). Constant accessor; used by the
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
    /// worker's lifetime — no growth, no reallocation.
    #[wasm_bindgen(getter)]
    pub fn snapshot_buf_byte_len(&self) -> u32 {
        SNAPSHOT_BUF_BYTES as u32
    }

    /// Byte offset within the snapshot region for slot `slot` (0 or 1).
    #[wasm_bindgen(getter)]
    pub fn snapshot_slot_bytes(&self) -> u32 {
        SNAPSHOT_SLOT_BYTES as u32
    }

    /// Bytes per snapshot stats header (header + alignment padding).
    #[wasm_bindgen(getter)]
    pub fn snapshot_header_bytes(&self) -> u32 {
        SNAPSHOT_HEADER_BYTES as u32
    }

    /// Bytes per creature region per slot.
    #[wasm_bindgen(getter)]
    pub fn snapshot_creature_bytes(&self) -> u32 {
        SNAPSHOT_CREATURE_BYTES as u32
    }

    /// Bytes per grass region per slot (v1.11: f32 grass, 4× per cell).
    #[wasm_bindgen(getter)]
    pub fn snapshot_grass_bytes(&self) -> u32 {
        SNAPSHOT_GRASS_BYTES as u32
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
    ///   `[32..32 + MAX_POP_FOR_SIM × 32)` creature SoA, 32 B stride
    ///   `[trailing GRASS_CELL_COUNT × 4 bytes)` f32 grass density
    #[wasm_bindgen]
    pub fn write_snapshot(&mut self, slot: u32) {
        let profile_on = self.inner.profile.enabled();
        let snap_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };

        let slot_idx = (slot as usize) & 1;
        let slot_base = slot_idx * SNAPSHOT_SLOT_BYTES;

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

        // Grass: raw f32 density bytes. No quantize, no boundary crossing.
        // Single contiguous memcpy from the sim's internal density `Vec<f32>`
        // into the snapshot region. ~3.7 MB at typical RAM bandwidth ≈ 0.4 ms
        // versus the prior 2.8 ms parallel quantize + 0.65 ms boundary copy.
        let grass_start = if profile_on {
            Some(crate::profiler::clock_now_us_threadsafe())
        } else {
            None
        };
        let grass_off = slot_base + SNAPSHOT_HEADER_BYTES + SNAPSHOT_CREATURE_BYTES;
        let density: &[f32] = &self.inner.grass.density;
        let density_bytes: &[u8] = bytemuck_slice_cast(density);
        self.snapshot_buf[grass_off..grass_off + SNAPSHOT_GRASS_BYTES]
            .copy_from_slice(density_bytes);
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
    fn apply_full_grass_on_init(&mut self, value: bool) {
        // Construction-only: only affects the next world. Stored on DevSliders
        // so the boot payload can round-trip it via set_slider.
        self.inner.sliders.full_grass_on_init = value;
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
            17..=20 => { /* reserved no-op: legacy curriculum slots */ }
            // Bools encoded as 0|1 so protocol surface stays minimal.
            21 => self.apply_full_grass_on_init(value != 0.0),
            22 => self.apply_max_population(value.max(1.0) as u32),
            // v1.12: 8 mutation buckets × 3 fields = indices 23..47.
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
    /// v1.5 S3 schema: drops color_rgb/weight_hash/photo_eff/eat_eff/armor/
    /// bite_reach/vision_range/eye_count. Adds color_ema (raw, unfloored),
    /// cooldown_remaining, and wall_proximity (N/S/E/W, range 50u).
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
        let wall_range = 50.0_f32;
        let wp_n = (1.0 - (cy / wall_range)).clamp(0.0, 1.0);
        let wp_s = (1.0 - ((WORLD_SIZE - cy) / wall_range)).clamp(0.0, 1.0);
        let wp_w = (1.0 - (cx / wall_range)).clamp(0.0, 1.0);
        let wp_e = (1.0 - ((WORLD_SIZE - cx) / wall_range)).clamp(0.0, 1.0);
        let json = serde_json::json!({
            "index": idx,
            "id": self.inner.creatures.id[i],
            "x": cx,
            "y": cy,
            "age": self.inner.creatures.age[i],
            "max_age": self.inner.sliders.max_age,
            "energy": self.inner.creatures.energy[i],
            "energy_frac": (self.inner.creatures.energy[i] / 100.0).clamp(0.0, 1.0),
            "size": CREATURE_SIZE,
            "current_action": action_name,
            "move_speed": MOVE_SPEED_MAX,
            "cooldown_remaining": self.inner.creatures.digestion_cooldown[i],
            // v1.5 S3: action-EMA color, raw (unfloored) for the inspector readout.
            "color_ema": [
                self.inner.creatures.color_r[i],
                self.inner.creatures.color_g[i],
                self.inner.creatures.color_b[i],
            ],
            "wall_proximity": [wp_n, wp_s, wp_e, wp_w],
            "nn_weight_count": brain.weights.len(),
        });
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
            "full_grass_on_init": if d.full_grass_on_init { 1.0_f32 } else { 0.0 },
            "max_population": d.max_population as f32,
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
    let body_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
    for i in 0..pop {
        let x = world.creatures.x[i];
        let y = world.creatures.y[i];
        // v1.5 S3: action-EMA color with 0.15 display floor.
        let r = world.creatures.color_r[i].max(0.15);
        let g = world.creatures.color_g[i].max(0.15);
        let b = world.creatures.color_b[i].max(0.15);
        let id = world.creatures.id[i];
        let id_lo = id as u32;
        let id_hi = (id >> 32) as u32;
        let off = i * 32;
        dst[off..off + 4].copy_from_slice(&x.to_le_bytes());
        dst[off + 4..off + 8].copy_from_slice(&y.to_le_bytes());
        dst[off + 8..off + 12].copy_from_slice(&body_r.to_le_bytes());
        dst[off + 12..off + 16].copy_from_slice(&r.to_le_bytes());
        dst[off + 16..off + 20].copy_from_slice(&g.to_le_bytes());
        dst[off + 20..off + 24].copy_from_slice(&b.to_le_bytes());
        // id_lo/id_hi as raw u32 bits — reader builds a Uint32Array view
        // over the same 8 bytes; no float→int conversion either side.
        dst[off + 24..off + 28].copy_from_slice(&id_lo.to_le_bytes());
        dst[off + 28..off + 32].copy_from_slice(&id_hi.to_le_bytes());
    }
    pop
}

/// Wrapper for `fill_creature_bytes` matching the v1.11 method-call site so
/// the inner block can borrow `&mut self.snapshot_buf` without conflicting
/// with `&self.inner`.
#[inline]
fn write_creatures_each_into(world: &World, dst: &mut [u8]) -> usize {
    fill_creature_bytes(world, dst)
}

/// Reinterpret `&[f32]` as `&[u8]` for byte-level copy. Sound on wasm32
/// (little-endian, no alignment trap on byte reads of f32-aligned data).
#[inline]
fn bytemuck_slice_cast(s: &[f32]) -> &[u8] {
    // SAFETY: f32 is 4 bytes; wasm32 is little-endian. The bytes are read
    // through a `*const u8`, which is always validly aligned. No mutation,
    // no aliasing risk.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
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
        // v1.11: native test path mirrors the wasm-side layout (f32 grass,
        // no quantize). Each Vec is sized to match exactly what main would
        // read out of the wasm-memory snapshot region.
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
        grass_dst.extend_from_slice(bytemuck_slice_cast(&self.inner.grass.density));

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
        let mut handle = WorldHandle::new_with_founder_count("a1-snapshot", 0, 100.0, 3, false, "").unwrap();
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
        assert_eq!(pop_written, 3);
        assert_eq!(creatures.len(), 3 * 32);
        for i in 0..3 {
            let base = i * 32;
            let x = f32::from_le_bytes(creatures[base..base + 4].try_into().unwrap());
            let y = f32::from_le_bytes(creatures[base + 4..base + 8].try_into().unwrap());
            let body = f32::from_le_bytes(creatures[base + 8..base + 12].try_into().unwrap());
            assert_eq!(x, handle.inner.creatures.x[i]);
            assert_eq!(y, handle.inner.creatures.y[i]);
            assert_eq!(body, CREATURE_SIZE * BODY_RADIUS_PER_SIZE);
            // Id round-trips via u32-halves.
            let id_lo = u32::from_le_bytes(creatures[base + 24..base + 28].try_into().unwrap());
            let id_hi = u32::from_le_bytes(creatures[base + 28..base + 32].try_into().unwrap());
            let id = ((id_hi as u64) << 32) | (id_lo as u64);
            assert_eq!(id, handle.inner.creatures.id[i]);
        }

        // Grass: v1.11 ships f32 density (no quantize). One little-endian
        // f32 per cell, contiguous.
        assert_eq!(
            grass.len(),
            crate::constants::GRASS_CELL_COUNT * 4,
            "grass region size = cells × 4 bytes per f32"
        );
        for i in 0..crate::constants::GRASS_CELL_COUNT {
            let bytes: [u8; 4] = grass[i * 4..i * 4 + 4].try_into().unwrap();
            let d_read = f32::from_le_bytes(bytes);
            assert_eq!(
                d_read.to_bits(),
                handle.inner.grass.density[i].to_bits(),
                "cell {i} f32 mismatch"
            );
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
        let handle = WorldHandle::new_with_founder_count("e21-creature-at", 0, 100.0, 1, false, "").unwrap();
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

    /// live_grass_cell_count never exceeds GRASS_CELL_COUNT and doesn't
    /// double-count empty cells (cells with density == 0 are excluded).
    #[test]
    fn live_grass_cell_count_does_not_exceed_cell_count() {
        use crate::constants::GRASS_CELL_COUNT;
        let handle = WorldHandle::new("p3d-grass-cells");
        let live = handle.live_grass_cell_count();
        assert!(
            live <= GRASS_CELL_COUNT as u32,
            "live_grass_cell_count={live} must not exceed GRASS_CELL_COUNT={GRASS_CELL_COUNT}"
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
