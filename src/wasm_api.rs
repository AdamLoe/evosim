//! Thin wasm-bindgen wrapper exposing the World to JS. Stable function
//! shapes so the web shell can iterate independently.

use crate::constants::*;
use crate::save::{LoadError, SaveV1};
use crate::world::World;
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

    // ─── Per-slider typed setters (S17) ─────────────────────────────────────
    // Private apply_* helpers: exactly one mutation site per DevSliders field.

    fn apply_base_sun_rate(&mut self, value: f32) {
        self.inner.sliders.base_sun_rate = value;
    }
    fn apply_mutation_rate_multiplier(&mut self, value: f32) {
        self.inner.sliders.mutation_rate_multiplier = value;
    }
    /// Side effect: also triggers sun.recompute_capacity (v6 §D).
    fn apply_sun_gradient_strength(&mut self, value: f32) {
        self.inner.sliders.sun_gradient_strength = value;
        self.inner.sun.recompute_capacity(value);
    }
    fn apply_mouth_tax(&mut self, value: f32) {
        self.inner.sliders.mouth_tax = value;
    }
    fn apply_nn_mutation_sigma(&mut self, value: f32) {
        self.inner.sliders.nn_mutation_sigma = value;
    }

    /// Typed setter — base sun refill rate.
    #[wasm_bindgen]
    pub fn set_base_sun_rate(&mut self, value: f32) {
        self.apply_base_sun_rate(value);
    }

    /// Typed setter — per-birth mutation rate multiplier.
    #[wasm_bindgen]
    pub fn set_mutation_rate_multiplier(&mut self, value: f32) {
        self.apply_mutation_rate_multiplier(value);
    }

    /// Typed setter — sun gradient strength (also triggers sun.recompute_capacity).
    #[wasm_bindgen]
    pub fn set_sun_gradient_strength(&mut self, value: f32) {
        self.apply_sun_gradient_strength(value);
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
            "base_sun_rate" => self.apply_base_sun_rate(value),
            "mutation_rate_multiplier" => self.apply_mutation_rate_multiplier(value),
            "sun_gradient_strength" => self.apply_sun_gradient_strength(value),
            "mouth_tax" => self.apply_mouth_tax(value),
            "nn_mutation_sigma" => self.apply_nn_mutation_sigma(value),
            _ => return false,
        }
        true
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

    /// Returns the SoA index of the topmost creature whose body circle (or
    /// tap-tolerance bubble) contains (world_x, world_y), or None.
    /// O(N); called only on click (≤ 1 Hz).
    ///
    /// `tolerance_world` is an additional world-space radius added on top of
    /// the creature's body radius so that small creatures remain clickable.
    /// Pass `6.0 / cam.zoom` from JS so the minimum tap target is 6 screen px.
    ///
    /// Returns an index, not a stable id — see DECISIONS.
    #[wasm_bindgen]
    pub fn creature_at(&self, world_x: f32, world_y: f32, tolerance_world: f32) -> Option<u32> {
        let n = self.inner.creatures.len();
        for i in 0..n {
            let dx = self.inner.creatures.x[i] - world_x;
            let dy = self.inner.creatures.y[i] - world_y;
            let body_r = self.inner.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
            let r = body_r + tolerance_world;
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
        let parent_name: Option<String> = sp
            .parent_id
            .map(|pid| self.inner.species.get(pid).name.clone());
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

    // ─────────────────────────────────────────────────────────────────────
    // F.26 — persistence (snapshot_json + fromJson)

    /// Serialize the world to JSON for autosave / Download Save (F.26).
    /// ~1–5 ms at v1 sizes (brains dominate). Returns "{}" on the (unreachable)
    /// serialization error path rather than panicking.
    #[wasm_bindgen]
    pub fn snapshot_json(&self) -> String {
        let save = self.inner.to_save_v1();
        serde_json::to_string(&save).unwrap_or_else(|_| "{}".into())
    }

    /// Construct a WorldHandle from a JSON save string (F.26).
    ///
    /// Returns `Err(JsValue)` with a prefix string for JS to switch on:
    /// - `"schema-mismatch:<found>:<expected>"` — schema version mismatch
    /// - `"invalid-json:<msg>"` — JSON parse error
    /// - `"structural:<msg>"` — structural integrity failure
    ///
    /// JS treats any failure as "start fresh" (show schema-mismatch modal).
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(json: &str) -> Result<WorldHandle, JsValue> {
        let save: SaveV1 = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("invalid-json:{e}")))?;
        let inner = World::from_save_v1(save).map_err(|e| match e {
            LoadError::SchemaVersionMismatch { found, expected } => {
                JsValue::from_str(&format!("schema-mismatch:{found}:{expected}"))
            }
            LoadError::InvalidJson(e) => JsValue::from_str(&format!("invalid-json:{e}")),
            LoadError::StructuralError(s) => JsValue::from_str(&format!("structural:{s}")),
        })?;
        Ok(Self {
            inner,
            creature_buf: Vec::new(),
            sun_buf: Vec::new(),
            carrion_buf: Vec::new(),
            id_buf: Vec::new(),
        })
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

    // ─────────────────────────────────────────────────────────────────────
    // F.28 — eulogy card (hof_json)

    /// JSON of the hall-of-fame eulogy snapshot (F.28).
    ///
    /// Shape (TS interface HoFJson):
    /// ```json
    /// {
    ///   "first_mover": HoFEntry | null,
    ///   "biggest": HoFEntry | null,
    ///   "weirdest": HoFEntry | null,  // falls back to longest_lived per v5 §11.1
    ///   "last_survivor": HoFEntry | null,
    ///   "day_count": number,
    ///   "peak_population": number,
    ///   "peak_species": number,
    ///   "seed": string
    /// }
    /// ```
    #[wasm_bindgen]
    pub fn hof_json(&self) -> String {
        // v5 §11.1: weirdest falls back to longest_lived when no creature survived ≥ 500 ticks.
        let weirdest = self
            .inner
            .weirdest
            .as_ref()
            .or(self.inner.longest_lived.as_ref());

        let payload = serde_json::json!({
            "first_mover": self.inner.first_mover_snapshot,
            "biggest": self.inner.biggest_ever,
            "weirdest": weirdest,
            "last_survivor": self.inner.last_survivor,
            "day_count": self.inner.tick / DAY_TICKS,
            "peak_population": self.inner.peak_population,
            "peak_species": self.inner.peak_species_count,
            "seed": self.inner.seed,
        });
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
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
        use crate::brain::Brain;
        use crate::genome::Genome;
        use crate::rng::SimRng;
        let mut rng = SimRng::from_u64(42);
        let g = Genome::founder();
        let b = Brain::founder(&mut rng);
        let _phantom_id = handle.inner.species.speciate(0, g, b.weights, 0);
        let json2 = handle.species_list_json_inner();
        let rows2: Vec<serde_json::Value> = serde_json::from_str(&json2).unwrap();
        assert_eq!(
            rows2.len(),
            1,
            "phantom species (no creatures) must be filtered out"
        );
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
        // Zero tolerance: hit exactly at the center.
        let result = handle.creature_at(cx, cy, 0.0);
        assert_eq!(result, Some(0), "founder must be found at world center");
        // With tolerance: hit just outside the body radius should still hit.
        let result_tol = handle.creature_at(cx + 2.0, cy, 3.0);
        assert_eq!(
            result_tol,
            Some(0),
            "founder must be found within tolerance radius"
        );
        // Far outside any creature — should return None even with tolerance.
        let miss = handle.creature_at(0.0, 0.0, 1.5);
        assert!(miss.is_none(), "empty corner must return None");
    }

    // ─── S17: typed slider setter tests ─────────────────────────────────────

    /// S17: per-slider typed setters mutate the correct DevSliders field.
    #[test]
    fn set_slider_typed_mutates_field() {
        let mut handle = WorldHandle::new("s17-typed");
        handle.set_base_sun_rate(0.99);
        assert!((handle.inner.sliders.base_sun_rate - 0.99).abs() < 1e-6);
        handle.set_mutation_rate_multiplier(2.5);
        assert!((handle.inner.sliders.mutation_rate_multiplier - 2.5).abs() < 1e-6);
        handle.set_sun_gradient_strength(0.5);
        assert!((handle.inner.sliders.sun_gradient_strength - 0.5).abs() < 1e-6);
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
        for name in &[
            "base_sun_rate",
            "mutation_rate_multiplier",
            "sun_gradient_strength",
            "mouth_tax",
            "nn_mutation_sigma",
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
        assert!(
            json.contains("\"type\":\"Speciation\""),
            "flat tag must be present: {json}"
        );
        assert!(
            !json.contains("\"Speciation\":{"),
            "nested form must NOT be present: {json}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // F.26 wasm API tests

    /// snapshot_json round-trips back through fromJson.
    #[test]
    fn f26_snapshot_json_round_trips() {
        let mut handle = WorldHandle::new("f26-snap-rt");
        for _ in 0..200 {
            handle.step();
        }
        let json = handle.snapshot_json();
        let handle2 = WorldHandle::from_json(&json).expect("fromJson must succeed");
        assert_eq!(
            handle2.tick(),
            handle.tick(),
            "tick must match after round-trip"
        );
        assert_eq!(
            handle2.population(),
            handle.population(),
            "population must match"
        );
    }

    /// fromJson returns a schema-mismatch JsValue for version 999.
    #[test]
    fn f26_from_json_schema_mismatch_prefix() {
        // Craft a JSON with schema_version=999 by round-tripping and patching.
        let mut handle = WorldHandle::new("f26-schema-err");
        handle.step();
        let json = handle.snapshot_json();
        let patched = json.replace("\"schema_version\":1", "\"schema_version\":999");
        // We can test the underlying save/load path without going through JsValue
        // (JsValue::as_string() panics in native test context outside wasm32).
        use crate::save::SaveV1;
        let save: SaveV1 = serde_json::from_str(&patched).expect("parse");
        let result = crate::world::World::from_save_v1(save);
        assert!(result.is_err(), "must return Err for wrong schema version");
        match result.err().unwrap() {
            crate::save::LoadError::SchemaVersionMismatch { found: 999, .. } => {}
            e => panic!("expected SchemaVersionMismatch, got: {e}"),
        }
    }

    /// fromJson returns an error for garbage JSON (tests via raw parse path).
    #[test]
    fn f26_from_json_invalid_json_prefix() {
        use crate::save::SaveV1;
        let result: Result<SaveV1, _> = serde_json::from_str("not valid json at all {{{");
        assert!(result.is_err(), "must fail on garbage JSON");
    }

    // ─────────────────────────────────────────────────────────────────────
    // F.28 wasm API tests

    /// hof_json includes all four slots.
    #[test]
    fn f28_hof_json_includes_all_slots() {
        let handle = WorldHandle::new("f28-hof");
        let json = handle.hof_json();
        let v: serde_json::Value =
            serde_json::from_str(&json).expect("hof_json must be valid JSON");
        assert!(
            v.get("first_mover").is_some(),
            "first_mover key must be present"
        );
        assert!(v.get("biggest").is_some(), "biggest key must be present");
        assert!(v.get("weirdest").is_some(), "weirdest key must be present");
        assert!(
            v.get("last_survivor").is_some(),
            "last_survivor key must be present"
        );
        assert!(
            v.get("day_count").is_some(),
            "day_count key must be present"
        );
        assert!(
            v.get("peak_population").is_some(),
            "peak_population key must be present"
        );
        assert!(
            v.get("peak_species").is_some(),
            "peak_species key must be present"
        );
        assert!(v.get("seed").is_some(), "seed key must be present");
    }

    /// hof_json day_count uses DAY_TICKS constant.
    #[test]
    fn f28_hof_json_day_count_uses_day_ticks() {
        // Tick exactly DAY_TICKS * 5 + some ticks.
        let mut handle = WorldHandle::new("f28-day");
        // Advance tick to 5500 by running ticks (we accept that the world may end
        // before then, in which case we test at wherever it stopped).
        for _ in 0..5500 {
            if !handle.step() {
                break;
            }
        }
        let json = handle.hof_json();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let expected_day = handle.tick() / DAY_TICKS;
        assert_eq!(
            v["day_count"].as_u64().unwrap(),
            expected_day as u64,
            "day_count must equal tick / DAY_TICKS"
        );
    }

    /// hof_json weirdest falls back to longest_lived when weirdest is None.
    #[test]
    fn f28_hof_json_weirdest_falls_back_to_longest_lived() {
        use crate::genome::Genome;
        use crate::hof::HallOfFame;
        let mut handle = WorldHandle::new("f28-fallback");
        // Inject a longest_lived but leave weirdest as None.
        handle.inner.longest_lived = Some(HallOfFame {
            creature_id: 42,
            genome: Genome::founder(),
            species_name: "FallbackSpecies".to_string(),
            captured_tick: 100,
            captured_size: 1.0,
            captured_age: 100,
        });
        handle.inner.weirdest = None;
        let json = handle.hof_json();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let weirdest = &v["weirdest"];
        assert!(
            !weirdest.is_null(),
            "weirdest must not be null when longest_lived is set (fallback)"
        );
        assert_eq!(
            weirdest["creature_id"].as_u64().unwrap(),
            42,
            "weirdest must equal longest_lived.creature_id via fallback"
        );
    }
}
