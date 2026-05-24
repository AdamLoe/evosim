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
    /// Reusable f64 buffer for `creature_ids_buffer` — index-aligned with creatures_buffer.
    id_buf: Vec<f64>,
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
            id_buf: Vec::new(),
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
            self.creature_buf[off + 10] = if g.scavenge_efficiency > 0.0 {
                1.0
            } else {
                0.0
            };
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

    /// Total count of all-time events appended (monotone u32). Used by the TS
    /// event-diff algorithm to detect new events since last poll (E.22 / PIN C).
    #[wasm_bindgen(getter)]
    pub fn events_total_count(&self) -> u32 {
        self.inner.events.all.len() as u32
    }

    /// JSON array of alive species (population > 0). Shape: SpeciesRow[].
    /// O(N+S). Called at 1Hz from the rail poll (E.21).
    #[wasm_bindgen]
    pub fn species_list_json(&self) -> String {
        self.species_list_json_inner()
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

    /// Returns the SoA index of the topmost creature whose body circle contains
    /// (world_x, world_y), or None. O(N); called only on click (≤ 1 Hz).
    /// Returns an index, not a stable id — see DECISIONS.
    #[wasm_bindgen]
    pub fn creature_at(&self, world_x: f32, world_y: f32) -> Option<u32> {
        let n = self.inner.creatures.len();
        for i in 0..n {
            let dx = self.inner.creatures.x[i] - world_x;
            let dy = self.inner.creatures.y[i] - world_y;
            let r = self.inner.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
            if dx * dx + dy * dy <= r * r {
                return Some(i as u32);
            }
        }
        None
    }

    /// JSON blob for the Inspector panel. Returns None if idx is out of range.
    /// O(1) — all fields are direct SoA reads.
    #[wasm_bindgen]
    pub fn creature_inspect_json(&self, idx: u32) -> Option<String> {
        let i = idx as usize;
        if i >= self.inner.creatures.len() {
            return None;
        }
        let g = &self.inner.creatures.genomes[i];
        let sp = self.inner.species.get(self.inner.creatures.species_id[i]);
        let parent_name: Option<String> =
            sp.parent_id.map(|pid| self.inner.species.get(pid).name.clone());
        let action_name = format!("{:?}", self.inner.creatures.action_this_tick[i]);
        let json = serde_json::json!({
            "index": idx,
            "id": self.inner.creatures.id[i],
            "species_id": sp.id,
            "species_name": sp.name,
            "parent_species_id": self.inner.creatures.parent_species_id[i],
            "parent_species_name": parent_name,
            "x": self.inner.creatures.x[i],
            "y": self.inner.creatures.y[i],
            "age": self.inner.creatures.age[i],
            "max_age": g.max_age,
            "energy": self.inner.creatures.energy[i],
            "energy_frac": (self.inner.creatures.energy[i] / 100.0).clamp(0.0, 1.0),
            "size": g.size,
            "current_action": action_name,
            "photosynth_efficiency": g.photosynth_efficiency,
            "eat_efficiency": g.eat_efficiency,
            "scavenge_efficiency": g.scavenge_efficiency,
            "move_speed": g.move_speed,
            "vision_range": g.vision_range,
            "eye_count": g.eye_count,
            "armor": g.armor,
            "bite_reach": g.bite_reach,
            "pigment": [g.pigment_r, g.pigment_g, g.pigment_b],
        });
        Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
    }

    /// Stats sample: [tick, population, live_species_count] as f32.
    /// O(1). Called every 10 sim-ticks from the Stats panel (E.23).
    #[wasm_bindgen]
    pub fn stats_sample(&self) -> Box<[f32]> {
        Box::new([
            self.inner.tick as f32,
            self.inner.population() as f32,
            self.inner.live_species_count as f32,
        ])
    }
}

/// Non-wasm helper extracted for native tests. Returns species_list JSON string.
/// Filters out species with zero population (extinct or never populated).
impl WorldHandle {
    pub fn species_list_json_inner(&self) -> String {
        let mut counts: std::collections::HashMap<u32, u32> = Default::default();
        for &sid in &self.inner.creatures.species_id {
            *counts.entry(sid).or_default() += 1;
        }
        let rows: Vec<_> = self
            .inner
            .species
            .list
            .iter()
            .filter_map(|sp| {
                let pop = counts.get(&sp.id).copied().unwrap_or(0);
                if pop == 0 {
                    return None;
                }
                Some(serde_json::json!({
                    "id": sp.id,
                    "name": sp.name,
                    "population": pop,
                    "parent_id": sp.parent_id,
                }))
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
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

    /// E.21: species_list_json_inner filters out extinct (zero-population) species.
    #[test]
    fn species_list_json_filters_extinct() {
        let mut handle = WorldHandle::new("e21-species");
        // Initially 1 creature, 1 live species. species_list should return 1 row.
        let json = handle.species_list_json_inner();
        let rows: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(rows.len(), 1, "should have 1 live species at start");
        assert_eq!(rows[0]["name"], "Lineage A");
        assert_eq!(rows[0]["population"], 1);

        // Create a second species via speciate but don't put any creature in it.
        // It should NOT appear in species_list_json.
        use crate::genome::Genome;
        use crate::rng::SimRng;
        use crate::brain::Brain;
        let mut rng = SimRng::from_u64(42);
        let g = Genome::founder();
        let b = Brain::founder(&mut rng);
        let _phantom_id = handle.inner.species.speciate(0, g, b.weights, 0);
        let json2 = handle.species_list_json_inner();
        let rows2: Vec<serde_json::Value> = serde_json::from_str(&json2).unwrap();
        assert_eq!(rows2.len(), 1, "phantom species (no creatures) must be filtered out");
    }

    /// E.21: creature_inspect_json returns None for out-of-range idx.
    #[test]
    fn creature_inspect_json_returns_none_for_out_of_range() {
        let handle = WorldHandle::new("e21-inspect");
        // Index 999 is way out of range (only 1 creature at start).
        let result = handle.creature_inspect_json(999);
        assert!(result.is_none(), "out-of-range idx must return None");
    }

    /// E.21: creature_at finds the founder at center.
    #[test]
    fn creature_at_finds_founder() {
        use crate::constants::WORLD_SIZE;
        let handle = WorldHandle::new("e21-creature-at");
        let cx = WORLD_SIZE * 0.5;
        let cy = WORLD_SIZE * 0.5;
        let result = handle.creature_at(cx, cy);
        assert_eq!(result, Some(0), "founder must be found at world center");
        // Far outside any creature — should return None.
        let miss = handle.creature_at(0.0, 0.0);
        assert!(miss.is_none(), "empty corner must return None");
    }

    /// E.21: events_total_count is monotone.
    #[test]
    fn events_total_count_is_monotone() {
        let mut handle = WorldHandle::new("e21-monotone");
        let initial = handle.events_total_count();
        // Run some ticks; WorldEnded eventually fires if pop=0.
        for _ in 0..500 {
            handle.step();
        }
        let after = handle.events_total_count();
        assert!(
            after >= initial,
            "events_total_count must be monotone: {} >= {}",
            after,
            initial
        );
    }

    /// E.21: EventKind serde uses flat #[serde(tag = "type")] shape.
    #[test]
    fn event_kind_serde_flat_tag() {
        use crate::events::{Event, EventKind};
        let ev = Event {
            tick: 42,
            kind: EventKind::Speciation {
                new_species_id: 1,
                parent_species_id: 0,
                new_species_name: "Lineage A1".to_string(),
                creature_id: 99,
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        // Flat tag means {"tick":42,"kind":{"type":"Speciation","new_species_id":1,...}}
        assert!(json.contains("\"type\":\"Speciation\""), "flat tag must be present: {json}");
        assert!(!json.contains("\"Speciation\":{"), "nested form must NOT be present: {json}");
    }
}
