//! Static biome map generation (v2.0 Wave 1b).
//!
//! The biome grid is one [`Biome`] tag per grass cell (`grass_cell_count` of
//! them), generated deterministically from the numeric `world_seed` by a
//! **dedicated** SplitMix64 PRNG that is *independent* of the string/XxHash64
//! sim RNG. Keeping the two streams separate means the live run still varies
//! (founder brains, births, grass seeding all ride the sim RNG) while the
//! biome map is pinnable to `world_seed` alone.
//!
//! Algorithm: a few large *blobs*, not noise speckle. We scatter a handful of
//! water-blob centers and a handful of desert-blob centers at random positions,
//! each with a large radius (~`world_size/5`..`world_size/4`). A cell inside a
//! water blob is Water; inside a desert blob (and no water) is Desert; else
//! Plains (water wins overlaps). When `wrap_world`, distance to a blob center is
//! toroidal (minimum-image) so blobs don't hard-clip at the seam.
//!
//! Tuned for roughly plains ~60% / water ~20% / desert ~20%.
//!
//! The generated grid is stored sim-side on [`super::World`] for O(1) per-tick
//! `biome_at` lookups AND copied byte-for-byte into the boot `biome_buf` SAB.

use crate::constants::{
    Biome, WorldDims, GRASS_CAPACITY_DESERT, GRASS_CAPACITY_PLAINS, GRASS_CAPACITY_WATER,
    GRASS_CELL_SIZE,
};

/// Independent SplitMix64 PRNG, seeded from `world_seed`. Deliberately NOT the
/// sim `SimRng` (xoshiro/XxHash) — biome generation must be reproducible from
/// `world_seed` alone and must not perturb the sim RNG stream.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        // Reference SplitMix64 (Steele, Lea & Flood 2014).
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f32 in [0, 1).
    #[inline]
    fn unit(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32; // top 24 bits
        (bits as f32) / ((1u32 << 24) as f32)
    }

    /// Uniform f32 in [lo, hi).
    #[inline]
    fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// Uniform integer in [lo, hi].
    #[inline]
    fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        debug_assert!(hi >= lo);
        let span = (hi - lo + 1) as u64;
        lo + ((self.next_u64() % span) as u32)
    }
}

/// A blob: a center in world-units plus a radius. Membership is by distance.
#[derive(Clone, Copy)]
struct Blob {
    cx: f32,
    cy: f32,
    r2: f32,
}

/// Number of blobs per biome kind (water, desert): 2..=4 each.
const BLOB_COUNT_MIN: u32 = 2;
const BLOB_COUNT_MAX: u32 = 4;
/// Blob radius range as a fraction of `world_size`. Tuned (with the 2..=4 blob
/// counts and water-wins-overlap) so proportions land near the target plains
/// ~60% / water ~20% / desert ~20% with plains always the plurality. Larger
/// fractions (~world_size/4) over-cover; this ~world_size/6..world_size/5.5
/// band hits the target. Balance knob.
const BLOB_RADIUS_FRAC_LO: f32 = 0.13;
const BLOB_RADIUS_FRAC_HI: f32 = 0.18;

/// Scatter `count` blobs with centers anywhere in `[0, world_size)` and radius
/// in `[lo_frac, hi_frac] * world_size`.
fn scatter_blobs(rng: &mut SplitMix64, count: u32, world_size: f32) -> Vec<Blob> {
    let mut blobs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let cx = rng.uniform(0.0, world_size);
        let cy = rng.uniform(0.0, world_size);
        let r = rng.uniform(
            BLOB_RADIUS_FRAC_LO * world_size,
            BLOB_RADIUS_FRAC_HI * world_size,
        );
        blobs.push(Blob {
            cx,
            cy,
            r2: r * r,
        });
    }
    blobs
}

/// Squared distance from `(x, y)` to `(cx, cy)`, toroidal (minimum-image) when
/// `wrap`, raw Euclidean otherwise.
#[inline]
fn dist2(x: f32, y: f32, cx: f32, cy: f32, world_size: f32, wrap: bool) -> f32 {
    let mut dx = (x - cx).abs();
    let mut dy = (y - cy).abs();
    if wrap {
        if dx > world_size * 0.5 {
            dx = world_size - dx;
        }
        if dy > world_size * 0.5 {
            dy = world_size - dy;
        }
    }
    dx * dx + dy * dy
}

/// Generate the biome grid (one `Biome as u8` per grass cell) deterministically
/// from `world_seed`. Row-major, `grass_dim × grass_dim`. Cell `(ix, iy)` is
/// sampled at its center in world-units.
pub(crate) fn generate_biome_grid(world_seed: u32, dims: &WorldDims) -> Vec<u8> {
    let world_size = dims.world_size;
    let grass_dim = dims.grass_dim;
    let wrap = dims.wrap_world;

    let mut rng = SplitMix64::new(world_seed as u64);
    // Draw counts first (in a fixed order) so the stream is well-defined.
    let n_water = rng.range_u32(BLOB_COUNT_MIN, BLOB_COUNT_MAX);
    let n_desert = rng.range_u32(BLOB_COUNT_MIN, BLOB_COUNT_MAX);
    let water = scatter_blobs(&mut rng, n_water, world_size);
    let desert = scatter_blobs(&mut rng, n_desert, world_size);

    let mut grid = vec![Biome::Plains as u8; dims.grass_cell_count];
    let half = GRASS_CELL_SIZE * 0.5;
    for iy in 0..grass_dim {
        let cy = iy as f32 * GRASS_CELL_SIZE + half;
        let row = iy * grass_dim;
        for ix in 0..grass_dim {
            let cx = ix as f32 * GRASS_CELL_SIZE + half;
            // Water wins overlaps; check water first.
            let in_water = water
                .iter()
                .any(|b| dist2(cx, cy, b.cx, b.cy, world_size, wrap) <= b.r2);
            let tag = if in_water {
                Biome::Water
            } else if desert
                .iter()
                .any(|b| dist2(cx, cy, b.cx, b.cy, world_size, wrap) <= b.r2)
            {
                Biome::Desert
            } else {
                Biome::Plains
            };
            grid[row + ix] = tag as u8;
        }
    }
    grid
}

/// Map a stored biome byte to its [`Biome`]. Unknown bytes fall back to Plains.
#[inline]
pub(crate) fn biome_from_u8(b: u8) -> Biome {
    match b {
        1 => Biome::Water,
        2 => Biome::Desert,
        _ => Biome::Plains,
    }
}

/// Grass carrying-capacity factor (× `GRASS_MAX`) for a stored biome byte.
/// v2.0 Wave 1: drives per-cell grass richness so biome is a selection pressure
/// (Plains ×1.0, Desert ×0.30, Water ×0.04). Unknown bytes fall back to Plains.
#[inline]
pub(crate) fn capacity_factor_from_u8(b: u8) -> f32 {
    match b {
        1 => GRASS_CAPACITY_WATER,
        2 => GRASS_CAPACITY_DESERT,
        _ => GRASS_CAPACITY_PLAINS,
    }
}
