//! Thin wasm-bindgen wrapper exposing the World to JS. Stable function
//! shapes so the web shell can iterate independently.

use crate::constants::*;
use crate::world::{World, BODY_RADIUS_PER_SIZE};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WorldHandle {
    inner: World,
    /// Reusable serialization buffer for `creatures_buffer` — one f32 vec
    /// shared across calls so JS can read a stable typed-array view.
    creature_buf: Vec<f32>,
    sun_buf: Vec<f32>,
    carrion_buf: Vec<f32>,
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
            sun_buf: Vec::new(),
            carrion_buf: Vec::new(),
        }
    }

    /// One tick. Returns true while alive.
    #[wasm_bindgen]
    pub fn step(&mut self) -> bool {
        self.inner.tick_once()
    }

    /// Run multiple ticks; stops early on game-over.
    #[wasm_bindgen]
    pub fn step_n(&mut self, n: u32) -> bool {
        for _ in 0..n {
            if !self.inner.tick_once() {
                return false;
            }
        }
        true
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
    pub fn species_count(&self) -> u32 {
        self.inner.live_species_count
    }

    #[wasm_bindgen(getter)]
    pub fn world_ended(&self) -> bool {
        self.inner.world_ended
    }

    #[wasm_bindgen(getter)]
    pub fn world_size(&self) -> f32 {
        WORLD_SIZE
    }

    #[wasm_bindgen(getter)]
    pub fn sun_dim(&self) -> u32 {
        SUN_DIM as u32
    }

    /// Repack creature SoA into a contiguous Float32Array. Layout per creature
    /// (13 floats, stride = [`creature_stride`]):
    /// `[x, y, radius_world, r, g, b, energy_frac, age_frac,
    ///   flag_eye, flag_move, flag_scav, flag_mouth, flag_armor]`.
    /// Ring flags: 1.0 if trait > 0, else 0.0. See v6 §B for ring order.
    #[wasm_bindgen]
    pub fn creatures_buffer(&mut self) -> js_sys::Float32Array {
        let n = self.inner.creatures.len();
        let stride = creature_stride() as usize;
        self.creature_buf.clear();
        self.creature_buf.resize(n * stride, 0.0);
        for i in 0..n {
            let g = &self.inner.creatures.genomes[i];
            let off = i * stride;
            self.creature_buf[off] = self.inner.creatures.x[i];
            self.creature_buf[off + 1] = self.inner.creatures.y[i];
            self.creature_buf[off + 2] = g.size * BODY_RADIUS_PER_SIZE;
            self.creature_buf[off + 3] = g.pigment_r;
            self.creature_buf[off + 4] = g.pigment_g;
            self.creature_buf[off + 5] = g.pigment_b;
            // energy_frac: rough — use 100 as nominal max for UI scaling.
            self.creature_buf[off + 6] = (self.inner.creatures.energy[i] / 100.0).clamp(0.0, 1.0);
            self.creature_buf[off + 7] =
                (self.inner.creatures.age[i] as f32 / g.max_age.max(1) as f32).clamp(0.0, 1.0);
            // Feature ring flags (v6 §B, ring order: eye→move→scav→mouth→armor).
            self.creature_buf[off + 8] = if g.eye_count > 0 { 1.0 } else { 0.0 };
            self.creature_buf[off + 9] = if g.move_speed > 0.0 { 1.0 } else { 0.0 };
            self.creature_buf[off + 10] = if g.scavenge_efficiency > 0.0 { 1.0 } else { 0.0 };
            self.creature_buf[off + 11] = if g.eat_efficiency > 0.0 { 1.0 } else { 0.0 };
            self.creature_buf[off + 12] = if g.armor > 0.0 { 1.0 } else { 0.0 };
        }
        unsafe { js_sys::Float32Array::view(&self.creature_buf) }
    }

    #[wasm_bindgen]
    pub fn sun_buffer(&mut self) -> js_sys::Float32Array {
        // current / capacity per cell, normalized [0, 1].
        let n = self.inner.sun.current.len();
        self.sun_buf.clear();
        self.sun_buf.resize(n, 0.0);
        for k in 0..n {
            let c = self.inner.sun.capacity[k].max(1e-6);
            self.sun_buf[k] = (self.inner.sun.current[k] / c).clamp(0.0, 1.0);
        }
        unsafe { js_sys::Float32Array::view(&self.sun_buf) }
    }

    #[wasm_bindgen]
    pub fn sun_capacity_buffer(&mut self) -> js_sys::Float32Array {
        // capacity only (used by the renderer to draw the underlying potential).
        unsafe { js_sys::Float32Array::view(&self.inner.sun.capacity) }
    }

    /// Per-carrion: [x, y, pool_frac]
    #[wasm_bindgen]
    pub fn carrion_buffer(&mut self) -> js_sys::Float32Array {
        let n = self.inner.carrion.len();
        self.carrion_buf.clear();
        self.carrion_buf.resize(n * 3, 0.0);
        for k in 0..n {
            let c = &self.inner.carrion[k];
            self.carrion_buf[k * 3] = c.x;
            self.carrion_buf[k * 3 + 1] = c.y;
            self.carrion_buf[k * 3 + 2] = (c.pool / CARRION_POOL_CAP).clamp(0.0, 1.0);
        }
        unsafe { js_sys::Float32Array::view(&self.carrion_buf) }
    }

    /// Apply a dev-panel slider live.
    #[wasm_bindgen]
    pub fn set_slider(&mut self, name: &str, value: f32) {
        match name {
            "base_sun_rate" => self.inner.sliders.base_sun_rate = value,
            "mutation_rate_multiplier" => self.inner.sliders.mutation_rate_multiplier = value,
            "sun_gradient_strength" => {
                self.inner.sliders.sun_gradient_strength = value;
                self.inner.sun.recompute_capacity(value);
            }
            "mouth_tax" => self.inner.sliders.mouth_tax = value,
            "nn_mutation_sigma" => self.inner.sliders.nn_mutation_sigma = value,
            _ => {}
        }
    }

    /// JSON of recent events (UI ring buffer).
    #[wasm_bindgen]
    pub fn recent_events_json(&self) -> String {
        serde_json::to_string(&self.inner.events.recent).unwrap_or_else(|_| "[]".into())
    }
}

/// Per-creature float count in [`WorldHandle::creatures_buffer`].
/// v1.0 layout (Milestone C.11): 13 floats.
/// Offset 0..8: x,y,radius,r,g,b,energy_frac,age_frac.
/// Offset 8..13: flag_eye, flag_move, flag_scav, flag_mouth, flag_armor.
#[wasm_bindgen]
pub fn creature_stride() -> u32 {
    13
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke-test that creature_stride() == 13 and the fill loop writes that
    /// many floats per creature. We can't call creatures_buffer() in native
    /// tests (js_sys::Float32Array requires wasm32), so we test the stride
    /// constant and the fill-loop math directly.
    #[test]
    fn creature_stride_is_13() {
        assert_eq!(creature_stride(), 13);
        // verify that n creatures × stride == expected buffer size
        let n: usize = 3;
        let expected = n * creature_stride() as usize;
        assert_eq!(expected, 39);
    }
}
