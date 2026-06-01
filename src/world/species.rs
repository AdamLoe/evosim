//! Species registry (v2.0 Wave 3a scaffolding → Wave 4 real seeding).
//!
//! A `Species` carries an id + a display color + a label. The registry is a
//! `Vec<Species>` keyed by id (id == index for the seeded set), built to be
//! **dynamic** — species can be added/removed/recolored at runtime — even though
//! v2.0 seeds a fixed set and v2.1 splits/merges arrive later. Keeping the API
//! add/remove-capable now means the later split work needs no protocol change.
//!
//! Wave 4 replaces the Wave-3 placeholder `hue = id/total × 360°` color with a
//! **hand-spread saturated palette** (`SPECIES_PALETTE` below). The palette is
//! single-sourced here so both the per-creature `color_u32` the sim writes in
//! species mode AND the polled species→color table (in `wasm_api.rs`) draw from
//! the same place — canvas and Monitor can never drift.

use serde::{Deserialize, Serialize};

/// Hand-spread saturated 10-hue palette (RGBA8 packed little-endian, alpha 255).
///
/// FEEL KNOB. 10 well-spaced hues around the wheel, with value/saturation nudged
/// slightly per slot so adjacent species stay readable as distinct (a flat
/// even-hue ring puts cyan next to teal next to green — too close). Ordered so
/// neighbours in the seed sequence (`Species-A`, `Species-B`, …) land on
/// far-apart hues, not a slow gradient. Typical runs end with 1–3 survivors, so
/// reuse past 10 species (v2.1 splits) is acceptable and handled by wrapping the
/// index modulo the palette length.
///
/// Colors (R, G, B), alpha implied 255:
///   A red · B azure · C amber · D violet · E green · F magenta · G cyan
///   H orange · I blue · J chartreuse
pub const SPECIES_PALETTE: [(u8, u8, u8); 10] = [
    (230, 50, 50),   // A — red
    (40, 130, 230),  // B — azure
    (240, 180, 30),  // C — amber/gold
    (150, 70, 210),  // D — violet
    (40, 190, 90),   // E — green
    (230, 60, 180),  // F — magenta
    (40, 200, 200),  // G — cyan/teal
    (240, 120, 30),  // H — orange
    (60, 70, 220),   // I — deep blue
    (170, 210, 40),  // J — chartreuse
];

/// Pack an `(r, g, b)` palette entry into an RGBA8 little-endian `u32`
/// (R bits 0..8 … A bits 24..32, alpha 255). Matches the snapshot `color_u32`
/// lane the renderer decodes (`R = u & 0xFF`, etc.).
#[inline]
pub const fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | (0xFFu32 << 24)
}

/// The hand-spread palette color for the `k`-th seeded species (wraps modulo the
/// palette length so >10 species — v2.1 splits — still get a color). Single
/// source of truth for both the per-creature color and the polled table.
#[inline]
pub fn palette_color(k: usize) -> u32 {
    let (r, g, b) = SPECIES_PALETTE[k % SPECIES_PALETTE.len()];
    pack_rgb(r, g, b)
}

/// Human label for the `k`-th seeded species: `Species-A`, `Species-B`, …,
/// `Species-Z`, then `Species-AA` style wrap for >26 (only v2.1 territory). v2.0
/// seeds ≤ 10 so this is always `Species-A..J`.
pub fn species_label(k: usize) -> String {
    // 0 → "A", 25 → "Z", 26 → "AA", … (spreadsheet-column style, 1-indexed).
    let mut n = k + 1;
    let mut letters = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        letters.push((b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    let label: String = letters.iter().rev().collect();
    format!("Species-{label}")
}

/// One species: a stable id + a saturated display color (RGBA8 packed u32) + a
/// human label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Species {
    /// Stable species id. For the seeded set, `id == index` in the registry, but
    /// the registry does not rely on that (so removals are safe).
    pub id: u16,
    /// Display color, RGBA8 packed little-endian (R bits 0..8 … A bits 24..32).
    /// Drawn from the hand-spread `SPECIES_PALETTE`.
    pub color_u32: u32,
    /// Human label (`Species-A`..). Low-priority identity per the decisions log;
    /// rides the inspector JSON and the polled species table.
    pub name: String,
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

    /// Seed `count` species with hand-spread palette colors + `Species-A..`
    /// labels. The k-th species gets `palette_color(k)` and `species_label(k)`,
    /// single-sourced so the polled table matches the per-creature color exactly.
    pub fn seed(count: u32) -> Self {
        let mut reg = Self::new();
        for k in 0..count {
            reg.add(palette_color(k as usize), species_label(k as usize));
        }
        reg
    }

    /// Append a new species with the given color + name; returns the assigned id.
    pub fn add(&mut self, color_u32: u32, name: String) -> u16 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.species.push(Species {
            id,
            color_u32,
            name,
        });
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

    /// Human label for a species id; falls back to a generic stringified id if
    /// the id is unknown (so a stale `species_id` never renders an empty name).
    #[inline]
    pub fn name_of(&self, id: u16) -> String {
        self.get(id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("Species-#{id}"))
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
    /// the polled species→color table). Dynamic-registry surface for v2.1.
    #[inline]
    pub fn all(&self) -> &[Species] {
        &self.species
    }
}
