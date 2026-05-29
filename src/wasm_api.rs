//! Thin wasm-bindgen wrapper exposing the World to JS. Stable function
//! shapes so the web shell can iterate independently.

use crate::constants::*;
use crate::world::World;
use wasm_bindgen::prelude::*;

/// Budget for "jank" detection: ticks that take longer than this are counted.
pub const JANK_BUDGET_MS: f64 = 16.0;

/// Rolling window size (in ticks) for the TPS average.
const TPS_WINDOW: usize = 10;

#[wasm_bindgen]
pub struct WorldHandle {
    inner: World,
    /// Reusable u8-quantized grass buffer used by `write_snapshot_to` to stage
    /// the per-cell `(d * 255.0) as u8` density before the SAB copy. Only read
    /// on wasm32 (the native `write_snapshot_to_native` test path quantizes
    /// directly into the caller's `Vec<u8>`).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    grass_buf_u8: Vec<u8>,
    /// Rolling window of per-tick wall-clock durations in milliseconds (last TPS_WINDOW ticks).
    /// Used to compute TPS rolling average.
    tick_durations_ms: std::collections::VecDeque<f64>,
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
            grass_buf_u8: Vec::new(),
            tick_durations_ms: std::collections::VecDeque::new(),
            jank_count: 0,
        }
    }

    /// Construct with explicit initial grass seed count, energy-max cap,
    /// founder count, and the `full_grass_on_init` flag. Sole construction
    /// path used by the sim worker — sets the construction-only DevSliders
    /// fields and lets `World::new_with_sliders` shape the world from there.
    #[wasm_bindgen(js_name = newWithFounderCount)]
    pub fn new_with_founder_count(
        seed: &str,
        initial_grass_seed_count: u32,
        energy_max: f32,
        founder_count: u32,
        full_grass_on_init: bool,
    ) -> Self {
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
        let inner = World::new_with_sliders(actual_seed, sliders);
        Self {
            inner,
            grass_buf_u8: Vec::new(),
            tick_durations_ms: std::collections::VecDeque::new(),
            jank_count: 0,
        }
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
            let dur_ms = wasm_now_ms() - t0;
            // Record in rolling window.
            if self.tick_durations_ms.len() >= TPS_WINDOW {
                self.tick_durations_ms.pop_front();
            }
            self.tick_durations_ms.push_back(dur_ms);
            if dur_ms > JANK_BUDGET_MS {
                self.jank_count = self.jank_count.saturating_add(1);
            }
            if !alive {
                return false;
            }
        }
        true
    }

    /// Rolling average ticks-per-second over the last 60 ticks.
    /// Returns 0.0 if fewer than 2 samples are available.
    #[wasm_bindgen(getter)]
    pub fn tps(&self) -> f32 {
        let n = self.tick_durations_ms.len();
        if n < 2 {
            return 0.0;
        }
        let total_ms: f64 = self.tick_durations_ms.iter().sum();
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

    // ─── v1.6 S A1: SAB snapshot writer ─────────────────────────────────────

    /// Write a full snapshot (stats header + creature SoA + grass density)
    /// into three caller-provided `Uint8Array` views over a SharedArrayBuffer.
    /// Wave C wires this from the sim worker after each `step_n(batch)`.
    ///
    /// Layout — all little-endian, all packed:
    ///
    /// `stats_dst` (≥ 20 bytes):
    ///   - `[0..4)`   `tick: u32`
    ///   - `[4..8)`   `pop: u32` (post-truncation count actually written)
    ///   - `[8..12)`  `world_ended: u32` (0/1)
    ///   - `[12..16)` `tps_bits: u32` = `self.tps().to_bits()` — reader does
    ///                 `new Float32Array(buf, off + 12, 1)[0]`. f32 round-trip,
    ///                 no encoding decision (plan-review I4).
    ///   - `[16..20)` `jank_count: u32`
    ///
    /// `creatures_dst` (≥ `MAX_POP_FOR_SIM × 32` bytes):
    ///   Creature SoA at 32-byte stride, identical layout to `creatures_buffer`:
    ///   `[x, y, body_r, r, g, b, f32::from_bits(id_lo), f32::from_bits(id_hi)]`.
    ///   All `pop` creatures written — `World::handle_births` keeps
    ///   `pop <= MAX_POP_FOR_SIM` as a sim invariant.
    ///
    /// `grass_dst` (≥ `GRASS_CELL_COUNT` bytes):
    ///   Quantized density `(d * 255.0).clamp(0, 255) as u8` per cell.
    ///
    /// **No Rust-side profile span.** The corresponding TS-side span
    /// `worker.snapshot.write` lands in Wave C (called from JS, no Rust RAII
    /// parent — opening a Rust span here would orphan at root level).
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen]
    pub fn write_snapshot_to(
        &mut self,
        creatures_dst: js_sys::Uint8Array,
        grass_dst: js_sys::Uint8Array,
        stats_dst: js_sys::Uint8Array,
    ) {
        let pop_written = self.write_creatures_each(|i, buf32| {
            // copy_from on a fresh subarray view — cheap (single memcpy).
            creatures_dst
                .subarray((i * 32) as u32, ((i + 1) * 32) as u32)
                .copy_from(buf32);
        });

        // Stats header — 20 bytes LE at offset 0.
        let mut header = [0u8; 20];
        header[0..4].copy_from_slice(&self.inner.tick.to_le_bytes());
        header[4..8].copy_from_slice(&(pop_written as u32).to_le_bytes());
        header[8..12]
            .copy_from_slice(&(if self.inner.world_ended { 1u32 } else { 0u32 }).to_le_bytes());
        header[12..16].copy_from_slice(&self.tps().to_bits().to_le_bytes());
        header[16..20].copy_from_slice(&self.jank_count.to_le_bytes());
        stats_dst.subarray(0, 20).copy_from(&header);

        // Grass — quantized to u8. Buffer reused across snapshots to avoid the
        // 921_600-byte re-allocation per tick.
        let density = &self.inner.grass.density;
        self.grass_buf_u8.clear();
        self.grass_buf_u8.reserve(density.len());
        for d in density {
            let q = (d * 255.0).clamp(0.0, 255.0) as u8;
            self.grass_buf_u8.push(q);
        }
        grass_dst
            .subarray(0, self.grass_buf_u8.len() as u32)
            .copy_from(&self.grass_buf_u8);
    }

    // ─── Per-slider typed setters (S17) ─────────────────────────────────────
    // Private apply_* helpers: exactly one mutation site per DevSliders field.

    fn apply_mutation_rate_multiplier(&mut self, value: f32) {
        self.inner.sliders.mutation_rate_multiplier = value;
    }
    fn apply_nn_mutation_sigma(&mut self, value: f32) {
        self.inner.sliders.nn_mutation_sigma = value;
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
    fn apply_curriculum_min_pop(&mut self, value: u32) {
        self.inner.sliders.curriculum_min_pop = value;
    }
    fn apply_curriculum_max_pop(&mut self, value: u32) {
        self.inner.sliders.curriculum_max_pop = value;
    }
    fn apply_curriculum_min_factor(&mut self, value: f32) {
        self.inner.sliders.curriculum_min_factor = value.clamp(0.0, 1.0);
    }
    fn apply_auto_curriculum(&mut self, value: bool) {
        self.inner.sliders.auto_curriculum = value;
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

    /// Pure Rust helper: apply a named slider; returns `true` on success.
    /// Factored out so native tests can exercise the name-dispatch table
    /// without constructing a `JsValue` (which panics on non-wasm32 targets).
    fn try_set_slider(&mut self, name: &str, value: f32) -> bool {
        match name {
            "mutation_rate_multiplier" => self.apply_mutation_rate_multiplier(value),
            "nn_mutation_sigma" => self.apply_nn_mutation_sigma(value),
            "eat_bite_fraction" => self.apply_eat_bite_fraction(value),
            "grass_propagation_rate_k" => self.apply_grass_propagation_rate_k(value),
            "grass_in_cell_growth_r" => self.apply_grass_in_cell_growth_r(value),
            "upkeep_multiplier" => self.apply_upkeep_multiplier(value),
            "move_cost_multiplier" => self.apply_move_cost_multiplier(value),
            "energy_max" => self.apply_energy_max(value),
            "grass_energy_per_bite" => self.apply_grass_energy_per_bite(value),
            // grass_bites_per_block is u32; round the float input. Below-1
            // values are clamped by the apply_ helper.
            "grass_bites_per_block" => self.apply_grass_bites_per_block(value.max(0.0) as u32),
            "digestion_cooldown" => self.apply_digestion_cooldown(value.max(0.0) as u32),
            "repulsion_max" => self.apply_repulsion_max(value),
            "max_age" => self.apply_max_age(value.max(0.0) as u32),
            "split_threshold" => self.apply_split_threshold(value),
            "split_gift" => self.apply_split_gift(value),
            "split_jitter" => self.apply_split_jitter(value),
            "founder_count" => self.apply_founder_count(value.max(0.0) as u32),
            "curriculum_min_pop" => self.apply_curriculum_min_pop(value.max(0.0) as u32),
            "curriculum_max_pop" => self.apply_curriculum_max_pop(value.max(0.0) as u32),
            "curriculum_min_factor" => self.apply_curriculum_min_factor(value),
            // Bools ride `set_slider` as 0/1 so the protocol surface stays
            // minimal — no dedicated bool message kinds.
            "auto_curriculum" => self.apply_auto_curriculum(value != 0.0),
            "full_grass_on_init" => self.apply_full_grass_on_init(value != 0.0),
            _ => return false,
        }
        true
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
            "nn_mutation_rate": brain.nn_mutation_rate,
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
        let json = serde_json::json!({
            "mutation_rate_multiplier": d.mutation_rate_multiplier,
            "nn_mutation_sigma": d.nn_mutation_sigma,
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
            "curriculum_min_pop": d.curriculum_min_pop as f32,
            "curriculum_max_pop": d.curriculum_max_pop as f32,
            "curriculum_min_factor": d.curriculum_min_factor,
            "auto_curriculum": if d.auto_curriculum { 1.0_f32 } else { 0.0 },
            "full_grass_on_init": if d.full_grass_on_init { 1.0_f32 } else { 0.0 },
        });
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

impl WorldHandle {
    /// Per-creature 32-byte serializer. Invokes `sink(i, &buf)` for each
    /// creature in `[0, pop)`, then returns the number of creatures written.
    /// `World::handle_births` keeps `pop <= MAX_POP_FOR_SIM` as a sim
    /// invariant, so no truncation or warning lives here — a `debug_assert!`
    /// surfaces the bug immediately in dev if that invariant ever breaks.
    fn write_creatures_each<F>(&mut self, mut sink: F) -> usize
    where
        F: FnMut(usize, &[u8]),
    {
        let pop = self.inner.creatures.len();
        debug_assert!(
            pop <= MAX_POP_FOR_SIM,
            "pop={pop} exceeds MAX_POP_FOR_SIM={MAX_POP_FOR_SIM} — sim cull broke",
        );
        let n = pop;
        let body_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        let mut buf = [0u8; 32];
        for i in 0..n {
            let x = self.inner.creatures.x[i];
            let y = self.inner.creatures.y[i];
            // v1.5 S3: action-EMA color with 0.15 display floor.
            let r = self.inner.creatures.color_r[i].max(0.15);
            let g = self.inner.creatures.color_g[i].max(0.15);
            let b = self.inner.creatures.color_b[i].max(0.15);
            let id = self.inner.creatures.id[i];
            let id_lo = id as u32;
            let id_hi = (id >> 32) as u32;
            buf[0..4].copy_from_slice(&x.to_le_bytes());
            buf[4..8].copy_from_slice(&y.to_le_bytes());
            buf[8..12].copy_from_slice(&body_r.to_le_bytes());
            buf[12..16].copy_from_slice(&r.to_le_bytes());
            buf[16..20].copy_from_slice(&g.to_le_bytes());
            buf[20..24].copy_from_slice(&b.to_le_bytes());
            // id_lo/id_hi as raw u32 bits — reader builds a Uint32Array view
            // over the same 8 bytes; no float→int conversion either side.
            buf[24..28].copy_from_slice(&id_lo.to_le_bytes());
            buf[28..32].copy_from_slice(&id_hi.to_le_bytes());
            sink(i, &buf);
        }
        n
    }

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
        creatures_dst.clear();
        let pop_written = self.write_creatures_each(|_i, buf32| {
            creatures_dst.extend_from_slice(buf32);
        });

        stats_dst.clear();
        stats_dst.extend_from_slice(&self.inner.tick.to_le_bytes());
        stats_dst.extend_from_slice(&(pop_written as u32).to_le_bytes());
        stats_dst
            .extend_from_slice(&(if self.inner.world_ended { 1u32 } else { 0u32 }).to_le_bytes());
        stats_dst.extend_from_slice(&self.tps().to_bits().to_le_bytes());
        stats_dst.extend_from_slice(&self.jank_count.to_le_bytes());

        grass_dst.clear();
        grass_dst.reserve(self.inner.grass.density.len());
        for d in &self.inner.grass.density {
            let q = (d * 255.0).clamp(0.0, 255.0) as u8;
            grass_dst.push(q);
        }

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
        let mut handle = WorldHandle::new_with_founder_count("a1-snapshot", 0, 100.0, 3, false);
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

        // Grass: one byte per cell, quantized.
        assert_eq!(grass.len(), crate::constants::GRASS_CELL_COUNT);
        for (i, &q) in grass.iter().enumerate() {
            let d = handle.inner.grass.density[i];
            let expected = (d * 255.0).clamp(0.0, 255.0) as u8;
            assert_eq!(q, expected, "cell {i} quantization mismatch");
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
        let handle = WorldHandle::new_with_founder_count("e21-creature-at", 0, 100.0, 1, false);
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
        for name in &["mutation_rate_multiplier", "nn_mutation_sigma"] {
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
