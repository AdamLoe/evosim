//! Species registry (v2.0 Wave 3a — scaffolding).
//!
//! A `Species` carries an id + a display color. The registry is a `Vec<Species>`
//! keyed by id (id == index for the seeded set), built to be **dynamic** — species
//! can be added/removed at runtime — even though Wave 3 seeds a fixed set and
//! v2.1 splits/merges arrive later. Keeping the API add/remove-capable now means
//! the later split work needs no protocol change.
//!
//! Wave 3 placeholder color: an evenly-spread saturated hue around the wheel,
//! derived from the species id, written into the existing `color_u32` snapshot
//! lane sim-side when `species_mode` is on (so the renderer shows species colors
//! with no change this wave). Wave 4 replaces this with the polled species→color
//! table (note that seam).

use serde::{Deserialize, Serialize};

/// One species: a stable id + a saturated display color (RGBA8 packed u32).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Species {
    /// Stable species id. For the Wave-3 seeded set, `id == index` in the
    /// registry, but the registry does not rely on that (so removals are safe).
    pub id: u16,
    /// Display color, RGBA8 packed little-endian (R bits 0..8 … A bits 24..32).
    /// Wave 3 placeholder: an evenly-spread saturated hue from the id.
    pub color_u32: u32,
}

/// Dynamic species registry. `Vec<Species>` keyed by id; lookup is O(N) over a
/// tiny N (≤ ~tens of species). Add/remove are supported for the v2.1 split work.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SpeciesRegistry {
    species: Vec<Species>,
    /// Next id to hand out. Monotonic so a removed-then-readded species never
    /// reuses an id (stable identity across the run).
    next_id: u16,
}

impl SpeciesRegistry {
    pub fn new() -> Self {
        Self {
            species: Vec::new(),
            next_id: 0,
        }
    }

    /// Seed `count` species with evenly-spread saturated placeholder hues.
    /// Wave 3 helper; Wave 4 replaces the color source with the polled table.
    pub fn seed(count: u32) -> Self {
        let mut reg = Self::new();
        for k in 0..count {
            let color = placeholder_species_color(k as u16, count.max(1) as u16);
            reg.add(color);
        }
        reg
    }

    /// Append a new species with the given color; returns the assigned id.
    pub fn add(&mut self, color_u32: u32) -> u16 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.species.push(Species { id, color_u32 });
        id
    }

    /// Remove the species with `id`, if present (dynamic-registry support for
    /// v2.1). Returns true if a species was removed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn remove(&mut self, id: u16) -> bool {
        if let Some(pos) = self.species.iter().position(|s| s.id == id) {
            self.species.remove(pos);
            true
        } else {
            false
        }
    }

    /// Look up a species by id.
    #[inline]
    pub fn get(&self, id: u16) -> Option<&Species> {
        self.species.iter().find(|s| s.id == id)
    }

    /// Display color for a species id; falls back to opaque white if the id is
    /// unknown (so a stale `species_id` never renders an undefined color).
    #[inline]
    pub fn color_of(&self, id: u16) -> u32 {
        self.get(id).map(|s| s.color_u32).unwrap_or(0xFFFF_FFFF)
    }

    /// Number of registered species.
    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.species.len()
    }

    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }

    /// Read-only view of the registered species (for the snapshot color writer /
    /// the future polled species→color table). Dynamic-registry surface for v2.1.
    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn all(&self) -> &[Species] {
        &self.species
    }
}

/// Wave 3 placeholder species color: an evenly-spread saturated hue around the
/// wheel from the species id, fully saturated and bright. RGBA8 packed
/// little-endian (alpha 255). `total` spreads the hues evenly; a `total` of 0/1
/// maps everything to red.
///
/// FEEL KNOB (placeholder). Wave 4 replaces this with the hand-spread palette +
/// the polled species→color table.
pub fn placeholder_species_color(id: u16, total: u16) -> u32 {
    let total = total.max(1) as f32;
    let hue = (id as f32 / total) * 360.0;
    let (r, g, b) = hsv_to_rgb8(hue, 0.85, 0.95);
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | (0xFFu32 << 24)
}

/// HSV (hue degrees, sat/val in [0,1]) → 8-bit RGB. Local copy of the
/// `wasm_api` helper so species coloring doesn't depend on that module.
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
