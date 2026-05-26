//! Thin wasm-bindgen wrapper exposing the World to JS. Stable function
//! shapes so the web shell can iterate independently.

use crate::constants::*;
use crate::world::World;
use wasm_bindgen::prelude::*;

/// Budget for "jank" detection: ticks that take longer than this are counted.
pub const JANK_BUDGET_MS: f64 = 16.0;

/// Rolling window size (in ticks) for the TPS average.
const TPS_WINDOW: usize = 60;

#[wasm_bindgen]
pub struct WorldHandle {
    inner: World,
    /// Reusable serialization buffer for `creatures_buffer` — one f32 vec
    /// shared across calls so JS can read a stable typed-array view.
    creature_buf: Vec<f32>,
    /// Reusable buffer for `grass_buffer` — avoids re-allocating 57_600 f32s each frame.
    grass_buf: Vec<f32>,
    /// Reusable f64 buffer for `creature_ids_buffer` — index-aligned with creatures_buffer.
    id_buf: Vec<f64>,
    /// Rolling window of per-tick wall-clock durations in milliseconds (last TPS_WINDOW ticks).
    /// Used to compute TPS rolling average.
    tick_durations_ms: std::collections::VecDeque<f64>,
    /// Count of ticks whose wall-clock duration exceeded JANK_BUDGET_MS.
    jank_count: u32,
}

#[wasm_bindgen]
impl WorldHandle {
    /// Construct from a string seed; empty string → random per build.
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
        let inner = World::new(actual_seed);
        Self {
            inner,
            creature_buf: Vec::new(),
            grass_buf: Vec::new(),
            id_buf: Vec::new(),
            tick_durations_ms: std::collections::VecDeque::new(),
            jank_count: 0,
        }
    }

    /// Construct from a string seed with an explicit initial grass seed count.
    /// Overrides the `GRASS_INITIAL_SEED_COUNT_DEFAULT` constant for world creation.
    /// Used by the dev panel's `grass_initial_seed_count` slider (P3b).
    #[wasm_bindgen(js_name = newWithGrassSeed)]
    pub fn new_with_grass_seed(seed: &str, initial_grass_seed_count: u32) -> Self {
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
            ..Default::default()
        };
        let inner = World::new_with_sliders(actual_seed, sliders);
        Self {
            inner,
            creature_buf: Vec::new(),
            grass_buf: Vec::new(),
            id_buf: Vec::new(),
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

    /// Repack creature SoA into a contiguous Float32Array. Layout per creature
    /// (11 floats, stride = [`creature_stride`]):
    /// `[x, y, radius_world, r, g, b,
    ///   flag_eye, flag_move, flag_scav, flag_mouth, flag_armor]`.
    /// Ring flags: 1.0 if trait > 0, else 0.0. See v6 §B for ring order.
    /// D3: genome deleted — body traits are compile-time constants; pigment is 0.5 gray.
    #[wasm_bindgen]
    pub fn creatures_buffer(&mut self) -> js_sys::Float32Array {
        let n = self.inner.creatures.len();
        let stride = creature_stride() as usize;
        self.creature_buf.clear();
        self.creature_buf.resize(n * stride, 0.0);
        // D3: all body traits are constants.
        let body_r = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        for i in 0..n {
            let off = i * stride;
            self.creature_buf[off] = self.inner.creatures.x[i];
            self.creature_buf[off + 1] = self.inner.creatures.y[i];
            self.creature_buf[off + 2] = body_r;
            // D3: pigment is placeholder gray (M5 owns color hot-mirror).
            self.creature_buf[off + 3] = crate::vision::CREATURE_PIGMENT;
            self.creature_buf[off + 4] = crate::vision::CREATURE_PIGMENT;
            self.creature_buf[off + 5] = crate::vision::CREATURE_PIGMENT;
            // Feature ring flags (v6 §B, ring order: eye→move→scav→mouth→armor).
            // D3: eye_count=24 (>0), move_speed=MOVE_SPEED_MAX (>0), scav/eat_eff=1.0 (>0), armor=0.
            self.creature_buf[off + 6] = 1.0; // eye_count=24 always > 0
            self.creature_buf[off + 7] = 1.0; // move_speed=MOVE_SPEED_MAX always > 0
            self.creature_buf[off + 8] = 1.0; // scav_eff=1.0 constant
            self.creature_buf[off + 9] = 1.0; // eat_eff=1.0 constant
            self.creature_buf[off + 10] = 0.0; // armor=0 constant
        }
        unsafe { js_sys::Float32Array::view(&self.creature_buf) }
    }

    // ─── Grass render API (P1g) ──────────────────────────────────────────────

    /// Grass grid dimension (240). Constant accessor; called per frame by the
    /// Canvas2D render layer to avoid hard-coding the value in TS.
    #[wasm_bindgen(getter)]
    pub fn grass_dim(&self) -> u32 {
        GRASS_GRID_DIM as u32
    }

    /// Grass cell size in world-units (2.5). Constant accessor; used by the
    /// Canvas2D render layer for world→screen coordinate conversion.
    #[wasm_bindgen(getter)]
    pub fn grass_cell_size(&self) -> f32 {
        GRASS_CELL_SIZE
    }

    /// Copy the current grass density field (57_600 f32s) into a cached Vec
    /// and return a Float32Array view over it. Called once per frame by the
    /// Canvas2D render layer. The view is valid only until the next Rust call
    /// that moves `self.grass_buf` (safe for single-frame use).
    #[wasm_bindgen]
    pub fn grass_buffer(&mut self) -> js_sys::Float32Array {
        self.grass_buf.clear();
        self.grass_buf.extend_from_slice(&self.inner.grass.density);
        unsafe { js_sys::Float32Array::view(&self.grass_buf) }
    }

    // ─── Per-slider typed setters (S17) ─────────────────────────────────────
    // Private apply_* helpers: exactly one mutation site per DevSliders field.

    fn apply_mutation_rate_multiplier(&mut self, value: f32) {
        self.inner.sliders.mutation_rate_multiplier = value;
    }
    fn apply_mouth_tax(&mut self, value: f32) {
        self.inner.sliders.mouth_tax = value;
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

    /// Typed setter — per-birth mutation rate multiplier.
    #[wasm_bindgen]
    pub fn set_mutation_rate_multiplier(&mut self, value: f32) {
        self.apply_mutation_rate_multiplier(value);
    }

    /// Typed setter — mouth tax (upkeep per eat-efficiency point).
    #[wasm_bindgen]
    pub fn set_mouth_tax(&mut self, value: f32) {
        self.apply_mouth_tax(value);
    }

    /// Typed setter — NN mutation sigma.
    #[wasm_bindgen]
    pub fn set_nn_mutation_sigma(&mut self, value: f32) {
        self.apply_nn_mutation_sigma(value);
    }

    /// Typed setter — per-bite energy transfer fraction.
    #[wasm_bindgen]
    pub fn set_eat_bite_fraction(&mut self, value: f32) {
        self.apply_eat_bite_fraction(value);
    }

    /// Typed setter — grass cross-cell propagation rate (k).
    #[wasm_bindgen]
    pub fn set_grass_propagation_rate_k(&mut self, value: f32) {
        self.apply_grass_propagation_rate_k(value);
    }

    /// Typed setter — grass in-cell logistic growth rate (r).
    #[wasm_bindgen]
    pub fn set_grass_in_cell_growth_r(&mut self, value: f32) {
        self.apply_grass_in_cell_growth_r(value);
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
            "mouth_tax" => self.apply_mouth_tax(value),
            "nn_mutation_sigma" => self.apply_nn_mutation_sigma(value),
            "eat_bite_fraction" => self.apply_eat_bite_fraction(value),
            "grass_propagation_rate_k" => self.apply_grass_propagation_rate_k(value),
            "grass_in_cell_growth_r" => self.apply_grass_in_cell_growth_r(value),
            _ => return false,
        }
        true
    }

    /// Index-aligned Float64Array of creature stable IDs (u64 as f64).
    /// Safe: u64 IDs stay below 2^53 in any v1 session.
    #[wasm_bindgen]
    pub fn creature_ids_buffer(&mut self) -> js_sys::Float64Array {
        self.id_buf.clear();
        for &id in &self.inner.creatures.id {
            self.id_buf.push(id as f64);
        }
        unsafe { js_sys::Float64Array::view(&self.id_buf) }
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
        // D3: body radius is constant — FOUNDER_SIZE * BODY_RADIUS_PER_SIZE.
        let query_r = tolerance_world + FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let mut found: Option<f64> = None;
        self.inner
            .grid
            .for_each_in_radius(world_x, world_y, query_r, |i| {
                if found.is_some() {
                    return;
                }
                let body_r = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
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
    /// D3: genome deleted; body traits are compile-time constants.
    #[wasm_bindgen]
    pub fn creature_inspect_json(&self, idx: u32) -> Option<String> {
        let i = idx as usize;
        if i >= self.inner.creatures.len() {
            return None;
        }
        let action_name = format!("{:?}", self.inner.creatures.action_this_tick[i]);
        let json = serde_json::json!({
            "index": idx,
            "id": self.inner.creatures.id[i],
            "x": self.inner.creatures.x[i],
            "y": self.inner.creatures.y[i],
            "age": self.inner.creatures.age[i],
            "max_age": FOUNDER_MAX_AGE,
            "energy": self.inner.creatures.energy[i],
            "energy_frac": (self.inner.creatures.energy[i] / 100.0).clamp(0.0, 1.0),
            "size": FOUNDER_SIZE,
            "current_action": action_name,
            "graze_efficiency": FOUNDER_GRAZE_EFF,
            "eat_efficiency": 1.0_f32,
            "scavenge_efficiency": 1.0_f32,
            "move_speed": MOVE_SPEED_MAX,
            "vision_range": VISION_RANGE_MAX,
            "eye_count": 24_u8,
            "nose_count": 0_u8,
            "armor": 0.0_f32,
            "bite_reach": BITE_REACH_MIN,
            "pigment": [0.5_f32, 0.5_f32, 0.5_f32],
        });
        Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
    }

    // ─── P3d observability counters ──────────────────────────────────────────

    /// Count of grass cells where density > 0. O(57_600) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn live_grass_cell_count(&self) -> u32 {
        self.inner
            .grass
            .density
            .iter()
            .filter(|&&d| d > 0.0)
            .count() as u32
    }

    /// Sum of all grass cell densities. O(57_600) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn total_grass_density(&self) -> f32 {
        self.inner.grass.density.iter().sum()
    }

    /// Mean nose_count across all live creatures. Returns 0.0 when population is 0.
    /// D3: nose_count is removed as a genome trait; returns 0.0 always (all creatures are
    /// nose-free; grass inputs are always filled).
    #[wasm_bindgen]
    pub fn mean_nose_count(&self) -> f32 {
        0.0
    }

    /// Mean eye_count across all live creatures. Returns 0.0 when population is 0.
    /// D3: eye_count is removed as a genome trait; all creatures use 24 sectors.
    /// Returns 24.0 if any creatures are alive, 0.0 otherwise.
    #[wasm_bindgen]
    pub fn mean_eye_count(&self) -> f32 {
        let n = self.inner.creatures.len();
        if n == 0 {
            return 0.0;
        }
        24.0
    }

    /// Stats sample: [tick, population] as f32.
    /// O(1). Called every 10 sim-ticks from the Stats panel (E.23).
    #[wasm_bindgen]
    pub fn stats_sample(&self) -> Box<[f32]> {
        Box::new([self.inner.tick as f32, self.inner.population() as f32])
    }

    // ─────────────────────────────────────────────────────────────────────
    // Profiler API (see docs/plans/perf-timing.md)

    /// Enable or disable the in-app profiler. Default: false (D9).
    /// The toggle state is NOT persisted — every page load returns to OFF.
    #[wasm_bindgen]
    pub fn profile_enable(&self, on: bool) {
        self.inner.profile.set_enabled(on);
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

}

// ─── Clock helper ─────────────────────────────────────────────────────────────

/// Current wall-clock time in milliseconds. On wasm32 uses `Performance::now()`;
/// on native (tests) uses a monotonic `Instant` against a per-process epoch.
#[cfg(target_arch = "wasm32")]
fn wasm_now_ms() -> f64 {
    web_sys::window()
        .expect("no window")
        .performance()
        .expect("no performance")
        .now()
}

#[cfg(not(target_arch = "wasm32"))]
fn wasm_now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_secs_f64() * 1000.0
}

/// Per-creature float count in [`WorldHandle::creatures_buffer`].
/// v1.1 layout (audit S21): 11 floats.
/// Offset 0..6: x, y, radius_world, pigment_r, pigment_g, pigment_b.
/// Offset 6..11: flag_eye, flag_move, flag_scav, flag_mouth, flag_armor.
#[wasm_bindgen]
pub fn creature_stride() -> u32 {
    11
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S21: creature_stride() == 11 (dropped energy_frac + age_frac).
    /// We can't call creatures_buffer() in native tests (js_sys::Float32Array
    /// requires wasm32), so we test the stride constant and fill-math directly.
    #[test]
    fn creature_stride_is_11() {
        assert_eq!(creature_stride(), 11);
        // verify that n creatures × stride == expected buffer size
        let n: usize = 3;
        let expected = n * creature_stride() as usize;
        assert_eq!(expected, 33);
    }

    /// S21: fill-math test. Manually resize the creature_buf as the wasm
    /// path would and assert the length matches population * stride.
    #[test]
    fn creature_buf_length_matches_population_times_stride() {
        let mut handle = WorldHandle::new("s21-stride");
        // Founder population = 1 at boot.
        let n = handle.inner.creatures.len();
        let stride = creature_stride() as usize;
        handle.creature_buf.clear();
        handle.creature_buf.resize(n * stride, 0.0);
        assert_eq!(handle.creature_buf.len(), n * stride);
        assert_eq!(handle.creature_buf.len(), 11); // 1 creature × 11 floats
    }

    /// E.21: creature_inspect_json returns None for out-of-range idx.
    #[test]
    fn creature_inspect_json_returns_none_for_out_of_range() {
        let handle = WorldHandle::new("e21-inspect");
        // Index 999 is way out of range (only 1 creature at start).
        let result = handle.creature_inspect_json(999);
        assert!(result.is_none(), "out-of-range idx must return None");
    }

    /// S18 + S20: creature_at returns the founder's stable id (f64) at center.
    #[test]
    fn creature_at_returns_stable_id() {
        use crate::constants::WORLD_SIZE;
        let handle = WorldHandle::new("e21-creature-at");
        let cx = WORLD_SIZE * 0.5;
        let cy = WORLD_SIZE * 0.5;
        // Read founder's stable id directly from SoA.
        let founder_id = handle.inner.creatures.id[0] as f64;
        // Zero tolerance: hit exactly at the center.
        let result = handle.creature_at(cx, cy, 0.0);
        assert_eq!(
            result,
            Some(founder_id),
            "founder id must be found at world center"
        );
        // With tolerance: hit just outside the body radius should still hit.
        let result_tol = handle.creature_at(cx + 2.0, cy, 3.0);
        assert_eq!(
            result_tol,
            Some(founder_id),
            "founder must be found within tolerance radius"
        );
        // Far outside any creature — should return None even with tolerance.
        let miss = handle.creature_at(0.0, 0.0, 1.5);
        assert!(miss.is_none(), "empty corner must return None");
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

    /// mean_nose_count always returns 0.0 (D3: nose_count removed as genome trait).
    /// mean_eye_count returns 24.0 when creatures are alive (D3: all creatures use 24 sectors).
    #[test]
    fn mean_nose_eye_count_fresh_world() {
        let handle = WorldHandle::new("p3d-mean-traits");
        // D3: nose_count is removed; always 0.0.
        assert_eq!(
            handle.mean_nose_count(),
            0.0,
            "D3: mean_nose_count must always be 0.0"
        );
        // D3: eye_count is constant 24; with 1 founder alive, mean = 24.0.
        assert_eq!(
            handle.mean_eye_count(),
            24.0,
            "D3: mean_eye_count must be 24.0 when creatures are alive"
        );
    }

    /// S17: per-slider typed setters mutate the correct DevSliders field.
    #[test]
    fn set_slider_typed_mutates_field() {
        let mut handle = WorldHandle::new("s17-typed");
        handle.set_mutation_rate_multiplier(2.5);
        assert!((handle.inner.sliders.mutation_rate_multiplier - 2.5).abs() < 1e-6);
        handle.set_mouth_tax(0.1);
        assert!((handle.inner.sliders.mouth_tax - 0.1).abs() < 1e-6);
        handle.set_nn_mutation_sigma(0.05);
        assert!((handle.inner.sliders.nn_mutation_sigma - 0.05).abs() < 1e-6);
    }

    /// S17: try_set_slider returns true for all known names. Uses the inner
    /// helper rather than set_slider directly because JsValue::from_str
    /// panics on non-wasm32 targets in the Err branch.
    #[test]
    fn set_slider_known_names_dispatch_ok() {
        let mut handle = WorldHandle::new("s17-known");
        for name in &["mutation_rate_multiplier", "mouth_tax", "nn_mutation_sigma"] {
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
