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
    /// Reusable buffer for `grass_buffer` — avoids re-allocating 230_400 f32s each frame.
    grass_buf: Vec<f32>,
    /// Reusable u8-quantized buffer for `grass_buffer_u8` — backs the R8 GPU upload (v1.5 S6).
    grass_buf_u8: Vec<u8>,
    /// Reusable f64 buffer for `creature_ids_buffer` — index-aligned with creatures_buffer.
    id_buf: Vec<f64>,
    /// Reusable buffer for `pack_render_buffer` — already in GPU instance layout
    /// (8 floats per visible creature: cx, cy, outer_px, inner_px, r, g, b, a).
    render_buf: Vec<f32>,
    /// Rolling window of per-tick wall-clock durations in milliseconds (last TPS_WINDOW ticks).
    /// Used to compute TPS rolling average.
    tick_durations_ms: std::collections::VecDeque<f64>,
    /// Count of ticks whose wall-clock duration exceeded JANK_BUDGET_MS.
    jank_count: u32,
    /// One-shot guard for the `write_snapshot_to` truncation warning. Latches
    /// `true` the first time population exceeds `MAX_POP_FOR_SAB` so we don't
    /// flood the console at every snapshot write.
    snapshot_truncation_warned: bool,
}

#[wasm_bindgen]
impl WorldHandle {
    /// Construct from a string seed; empty string → random per build.
    /// Uses the multi-founder default (`FOUNDER_COUNT_DEFAULT`).
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
            creature_buf: Vec::new(),
            grass_buf: Vec::new(),
            grass_buf_u8: Vec::new(),
            id_buf: Vec::new(),
            render_buf: Vec::new(),
            tick_durations_ms: std::collections::VecDeque::new(),
            jank_count: 0,
            snapshot_truncation_warned: false,
        }
    }

    /// Construct from a string seed with explicit initial grass seed count
    /// and energy-max cap. Both override the corresponding DevSliders defaults
    /// for world creation; founders spawn at `min(FOUNDER_ENERGY, energy_max)`.
    /// Kept for backwards compat — uses the default founder count.
    #[wasm_bindgen(js_name = newWithGrassSeed)]
    pub fn new_with_grass_seed(seed: &str, initial_grass_seed_count: u32, energy_max: f32) -> Self {
        Self::new_with_founder_count(
            seed,
            initial_grass_seed_count,
            energy_max,
            FOUNDER_COUNT_DEFAULT,
        )
    }

    /// Construct with explicit initial grass seed count, energy-max cap, and
    /// founder count (clamped to [1, 32] inside `new_with_sliders`).
    #[wasm_bindgen(js_name = newWithFounderCount)]
    pub fn new_with_founder_count(
        seed: &str,
        initial_grass_seed_count: u32,
        energy_max: f32,
        founder_count: u32,
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
            ..Default::default()
        };
        let inner = World::new_with_sliders(actual_seed, sliders);
        Self {
            inner,
            creature_buf: Vec::new(),
            grass_buf: Vec::new(),
            grass_buf_u8: Vec::new(),
            id_buf: Vec::new(),
            render_buf: Vec::new(),
            tick_durations_ms: std::collections::VecDeque::new(),
            jank_count: 0,
            snapshot_truncation_warned: false,
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
    /// (8 floats, stride = [`creature_stride`]):
    /// `[x, y, radius_world, r, g, b, id_lo, id_hi]`.
    /// v1.5 S3: color is per-creature action-EMA with a 0.15 display floor so
    /// fresh-born creatures (EMA=(0,0,0)) stay visible against the dark BG.
    /// v1.6 S A1: id_lo/id_hi carry the stable creature id as two `u32`s
    /// reinterpreted via `f32::from_bits`. JS reads them as a `Uint32Array`
    /// view over the same memory — no float→u32 round-tripping.
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
            // v1.5 S3: action-EMA color with display floor.
            self.creature_buf[off + 3] = self.inner.creatures.color_r[i].max(0.15);
            self.creature_buf[off + 4] = self.inner.creatures.color_g[i].max(0.15);
            self.creature_buf[off + 5] = self.inner.creatures.color_b[i].max(0.15);
            // v1.6 S A1: id column — split u64 into two u32 halves, reinterpret
            // each as f32 via `from_bits`. JS undoes this with
            // `new Uint32Array(buf.buffer, byteOffset + 24, 2)` over the same
            // 8 bytes (no conversion cost; both sides see the raw bit pattern).
            let id = self.inner.creatures.id[i];
            let id_lo = id as u32;
            let id_hi = (id >> 32) as u32;
            self.creature_buf[off + 6] = f32::from_bits(id_lo);
            self.creature_buf[off + 7] = f32::from_bits(id_hi);
        }
        unsafe { js_sys::Float32Array::view(&self.creature_buf) }
    }

    /// Pack visible creatures directly into a GPU-ready instance buffer.
    /// Frustum culls against the viewport in one pass and writes the same
    /// 8-float instance layout that `render-gl.ts` consumes:
    /// `[cx, cy, outer_px, inner_px, r, g, b, a]` per creature.
    ///
    /// `inner_px` and `a` are always `0.0` / `1.0` for bodies (no rings).
    /// Highlights remain handled JS-side because they're rare (≤ 1–2 per frame).
    ///
    /// Why this lives in Rust: the JS pack loop was the main per-frame CPU
    /// cost at high creature counts (every field read crossed the typed-array
    /// boundary). Packing in Rust skips that boundary entirely and lets us
    /// frustum cull cheaply against the SoA.
    ///
    /// `px_per_size` mirrors `PX_PER_SIZE` from `render.ts` so the screen
    /// radius computation is identical on both sides.
    #[wasm_bindgen]
    pub fn pack_render_buffer(
        &mut self,
        cam_cx: f32,
        cam_cy: f32,
        zoom: f32,
        viewport_w: f32,
        viewport_h: f32,
        px_per_size: f32,
    ) -> js_sys::Float32Array {
        let n = self.inner.creatures.len();
        self.render_buf.clear();
        if n == 0 || zoom <= 0.0 {
            return unsafe { js_sys::Float32Array::view(&self.render_buf) };
        }

        // Frustum bounds in world units. `margin` covers the body radius so
        // creatures straddling the edge still get drawn.
        let body_r = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let half_w = viewport_w / zoom * 0.5;
        let half_h = viewport_h / zoom * 0.5;
        let margin = body_r + 2.0;
        let min_x = cam_cx - half_w - margin;
        let max_x = cam_cx + half_w + margin;
        let min_y = cam_cy - half_h - margin;
        let max_y = cam_cy + half_h + margin;

        let radius_px = (body_r * px_per_size * zoom).max(1.0);

        self.render_buf.reserve(n * 8);

        let xs = &self.inner.creatures.x;
        let ys = &self.inner.creatures.y;
        let rs = &self.inner.creatures.color_r;
        let gs = &self.inner.creatures.color_g;
        let bs = &self.inner.creatures.color_b;

        for i in 0..n {
            let x = xs[i];
            let y = ys[i];
            if x < min_x || x > max_x || y < min_y || y > max_y {
                continue;
            }
            // v1.5 S3: EMA color with 0.15 display floor.
            let r = rs[i].max(0.15);
            let g = gs[i].max(0.15);
            let b = bs[i].max(0.15);
            self.render_buf.push(x);
            self.render_buf.push(y);
            self.render_buf.push(radius_px);
            self.render_buf.push(0.0); // inner_px (filled disc)
            self.render_buf.push(r);
            self.render_buf.push(g);
            self.render_buf.push(b);
            self.render_buf.push(1.0); // alpha
        }

        unsafe { js_sys::Float32Array::view(&self.render_buf) }
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

    /// Copy the current grass density field (230_400 f32s) into a cached Vec
    /// and return a Float32Array view over it. Retained for callers that need
    /// raw f32 density (debug / tests); the WebGL2 R8 path uses
    /// `grass_buffer_u8` instead. The view is valid only until the next Rust
    /// call that moves `self.grass_buf` (safe for single-frame use).
    #[wasm_bindgen]
    pub fn grass_buffer(&mut self) -> js_sys::Float32Array {
        self.grass_buf.clear();
        self.grass_buf.extend_from_slice(&self.inner.grass.density);
        unsafe { js_sys::Float32Array::view(&self.grass_buf) }
    }

    /// v1.5 S6: u8-quantized density for the R8 GPU upload (230_400 bytes/frame
    /// at the current grid dim). Returns a fresh `Uint8Array` copy — safe across
    /// subsequent Rust calls. `(d * 255.0).clamp(0, 255) as u8`.
    #[wasm_bindgen]
    pub fn grass_buffer_u8(&mut self) -> js_sys::Uint8Array {
        let density = &self.inner.grass.density;
        self.grass_buf_u8.clear();
        self.grass_buf_u8.reserve(density.len());
        for d in density {
            let q = (d * 255.0).clamp(0.0, 255.0) as u8;
            self.grass_buf_u8.push(q);
        }
        js_sys::Uint8Array::from(&self.grass_buf_u8[..])
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
    /// `creatures_dst` (≥ `MAX_POP_FOR_SAB × 32` bytes):
    ///   Creature SoA at 32-byte stride, identical layout to `creatures_buffer`:
    ///   `[x, y, body_r, r, g, b, f32::from_bits(id_lo), f32::from_bits(id_hi)]`.
    ///   Up to `MAX_POP_FOR_SAB` creatures; the first overflow is `log::warn`ed.
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

        // Grass — quantized to u8.
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
    fn apply_eat_cost_multiplier(&mut self, value: f32) {
        self.inner.sliders.eat_cost_multiplier = value.max(0.0);
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
    fn apply_founder_count(&mut self, value: u32) {
        // Stored for next world construction (active world keeps its current population).
        self.inner.sliders.founder_count = value.clamp(1, 32);
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

    /// Typed setter — per-birth mutation rate multiplier.
    #[wasm_bindgen]
    pub fn set_mutation_rate_multiplier(&mut self, value: f32) {
        self.apply_mutation_rate_multiplier(value);
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

    /// Typed setter — per-tick idle upkeep multiplier (0 = no drain, 1 = default).
    #[wasm_bindgen]
    pub fn set_upkeep_multiplier(&mut self, value: f32) {
        self.apply_upkeep_multiplier(value);
    }

    /// Typed setter — multiplier on the per-distance movement cost.
    #[wasm_bindgen]
    pub fn set_move_cost_multiplier(&mut self, value: f32) {
        self.apply_move_cost_multiplier(value);
    }

    /// Typed setter — multiplier on the per-attempt eat cost.
    #[wasm_bindgen]
    pub fn set_eat_cost_multiplier(&mut self, value: f32) {
        self.apply_eat_cost_multiplier(value);
    }

    /// Typed setter — per-creature max energy cap.
    #[wasm_bindgen]
    pub fn set_energy_max(&mut self, value: f32) {
        self.apply_energy_max(value);
    }

    /// Typed setter — energy gained per successful graze bite.
    #[wasm_bindgen]
    pub fn set_grass_energy_per_bite(&mut self, value: f32) {
        self.apply_grass_energy_per_bite(value);
    }

    /// Typed setter — number of bites to drain a ripe grass cell.
    #[wasm_bindgen]
    pub fn set_grass_bites_per_block(&mut self, value: u32) {
        self.apply_grass_bites_per_block(value);
    }

    /// Typed setter — digestion-cooldown duration in ticks. Eat is gated while
    /// the per-creature cooldown is > 0.
    #[wasm_bindgen]
    pub fn set_digestion_cooldown(&mut self, value: u32) {
        self.apply_digestion_cooldown(value);
    }

    /// Typed setter — per-tick cap on the repulsion position nudge. 0 disables
    /// physical separation; ~5 = historical default.
    #[wasm_bindgen]
    pub fn set_repulsion_max(&mut self, value: f32) {
        self.apply_repulsion_max(value);
    }

    /// Typed setter — past-lifespan threshold (ticks).
    #[wasm_bindgen]
    pub fn set_max_age(&mut self, value: u32) {
        self.apply_max_age(value);
    }

    /// Typed setter — energy threshold required for a Split action.
    #[wasm_bindgen]
    pub fn set_split_threshold(&mut self, value: f32) {
        self.apply_split_threshold(value);
    }

    /// Typed setter — energy gifted to each newborn at split.
    #[wasm_bindgen]
    pub fn set_split_gift(&mut self, value: f32) {
        self.apply_split_gift(value);
    }

    /// Typed setter — founder count for the next world construction.
    #[wasm_bindgen]
    pub fn set_founder_count(&mut self, value: u32) {
        self.apply_founder_count(value);
    }

    /// Typed setter — curriculum lower-pop knee.
    #[wasm_bindgen]
    pub fn set_curriculum_min_pop(&mut self, value: u32) {
        self.apply_curriculum_min_pop(value);
    }

    /// Typed setter — curriculum upper-pop knee.
    #[wasm_bindgen]
    pub fn set_curriculum_max_pop(&mut self, value: u32) {
        self.apply_curriculum_max_pop(value);
    }

    /// Typed setter — curriculum floor (clamped to [0, 1]).
    #[wasm_bindgen]
    pub fn set_curriculum_min_factor(&mut self, value: f32) {
        self.apply_curriculum_min_factor(value);
    }

    /// Typed setter — curriculum master switch. Separate setter (not in
    /// `try_set_slider`) because the value is a bool, not an f32.
    #[wasm_bindgen]
    pub fn set_auto_curriculum(&mut self, value: bool) {
        self.apply_auto_curriculum(value);
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
            "eat_cost_multiplier" => self.apply_eat_cost_multiplier(value),
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
            "founder_count" => self.apply_founder_count(value.max(0.0) as u32),
            "curriculum_min_pop" => self.apply_curriculum_min_pop(value.max(0.0) as u32),
            "curriculum_max_pop" => self.apply_curriculum_max_pop(value.max(0.0) as u32),
            "curriculum_min_factor" => self.apply_curriculum_min_factor(value),
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
            "size": FOUNDER_SIZE,
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

    /// Count of grass cells where density > 0. O(230_400) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn live_grass_cell_count(&self) -> u32 {
        self.inner
            .grass
            .density
            .iter()
            .filter(|&&d| d > 0.0)
            .count() as u32
    }

    /// Sum of all grass cell densities. O(230_400) — negligible at v1 scale.
    #[wasm_bindgen]
    pub fn total_grass_density(&self) -> f32 {
        self.inner.grass.density.iter().sum()
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
    /// creature in `[0, min(pop, MAX_POP_FOR_SAB))`, then returns the number
    /// of creatures actually written. Logs once per `WorldHandle` lifetime
    /// the first time population exceeds the SAB cap.
    fn write_creatures_each<F>(&mut self, mut sink: F) -> usize
    where
        F: FnMut(usize, &[u8]),
    {
        let pop = self.inner.creatures.len();
        let n = pop.min(MAX_POP_FOR_SAB);
        if pop > MAX_POP_FOR_SAB && !self.snapshot_truncation_warned {
            self.snapshot_truncation_warned = true;
            let msg = format!(
                "write_snapshot_to: pop={pop} exceeds MAX_POP_FOR_SAB={MAX_POP_FOR_SAB}; truncating (first {n} creatures rendered, remainder dropped this frame and going forward)"
            );
            #[cfg(target_arch = "wasm32")]
            web_sys::console::warn_1(&JsValue::from_str(&msg));
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!("{msg}");
        }
        let body_r = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
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

/// Per-creature float count in [`WorldHandle::creatures_buffer`] and in the
/// SAB-backed snapshot SoA produced by [`WorldHandle::write_snapshot_to`].
///
/// v1.6 S A1 layout: 8 floats (stride bumped 6 → 8). The trailing two slots
/// carry the stable creature id split as `(id_lo, id_hi)` and reinterpreted
/// from `u32` via `f32::from_bits`; readers undo the encoding with
/// `new Uint32Array(buf.buffer, byteOffset + 24, 2)` over the same memory.
/// Layout: `[x, y, radius_world, r, g, b, id_lo, id_hi]`.
#[wasm_bindgen]
pub fn creature_stride() -> u32 {
    8
}

/// v1.6 SAB snapshot creature cap, mirrored from `constants::MAX_POP_FOR_SAB`.
/// Sourced from Rust so the worker can pass it to main in `boot_ready`; main
/// asserts it matches the TS `MAX_POP_FOR_SAB` constant (rebuild-wasm guard).
#[wasm_bindgen]
pub fn max_pop_for_sab() -> u32 {
    MAX_POP_FOR_SAB as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v1.6 S A1: creature_stride() == 8 (id_lo/id_hi columns appended).
    /// We can't call creatures_buffer() in native tests (js_sys::Float32Array
    /// requires wasm32), so we test the stride constant and fill-math directly.
    #[test]
    fn creature_stride_is_8() {
        assert_eq!(creature_stride(), 8);
        let n: usize = 3;
        let expected = n * creature_stride() as usize;
        assert_eq!(expected, 24);
    }

    /// v1.6 S A1: fill-math test. Manually resize the creature_buf as the wasm
    /// path would and assert the length matches population * stride (8).
    #[test]
    fn creature_buf_length_matches_population_times_stride() {
        let mut handle = WorldHandle::new("s21-stride");
        // Founder population at boot = FOUNDER_COUNT_DEFAULT (v1.5 multi-founder).
        let n = handle.inner.creatures.len();
        let stride = creature_stride() as usize;
        assert_eq!(stride, 8);
        handle.creature_buf.clear();
        handle.creature_buf.resize(n * stride, 0.0);
        assert_eq!(handle.creature_buf.len(), n * stride);
        assert_eq!(
            handle.creature_buf.len(),
            FOUNDER_COUNT_DEFAULT as usize * stride
        );
    }

    /// v1.6 S A1: `write_snapshot_to_native` produces bytes that match the
    /// documented stride-8 layout. Exercises the same writer the wasm path
    /// drives so the SAB layout is testable off-wasm.
    #[test]
    fn write_snapshot_to_layout_matches_stride() {
        let mut handle = WorldHandle::new_with_founder_count("a1-snapshot", 0, 100.0, 3);
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
            assert_eq!(body, FOUNDER_SIZE * BODY_RADIUS_PER_SIZE);
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

    /// v1.6 S A1: `max_pop_for_sab()` mirrors the Rust constant.
    #[test]
    fn max_pop_for_sab_matches_constant() {
        assert_eq!(max_pop_for_sab(), MAX_POP_FOR_SAB as u32);
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
        let handle = WorldHandle::new_with_founder_count("e21-creature-at", 0, 100.0, 1);
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

    /// S17: per-slider typed setters mutate the correct DevSliders field.
    #[test]
    fn set_slider_typed_mutates_field() {
        let mut handle = WorldHandle::new("s17-typed");
        handle.set_mutation_rate_multiplier(2.5);
        assert!((handle.inner.sliders.mutation_rate_multiplier - 2.5).abs() < 1e-6);
        handle.set_nn_mutation_sigma(0.05);
        assert!((handle.inner.sliders.nn_mutation_sigma - 0.05).abs() < 1e-6);
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
