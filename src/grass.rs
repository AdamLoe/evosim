//! GrassGrid — runtime-sized flat-`Vec<f32>` density field over the world.
//!
//! v2.0 Wave 1a: the grid is `grass_dim × grass_dim` cells (computed at
//! construction from `world_size`; 1920² at the 9600u default), each
//! `GRASS_CELL_SIZE` (5u) square. Independent of SpatialGrid (10u, body
//! queries). The world may be toroidal (`dims.wrap_world`): propagation,
//! overlap, and bilinear sampling wrap across the seam when so. v1.2
//! grass-mechanic-brief §Grass storage / §Grass dynamics per tick /
//! §Initial grass seed / §Bilinear sampling.

use crate::constants::{
    WorldDims, GRASS_DECAY_AMOUNT_DEFAULT, GRASS_DECAY_PCT_DEFAULT, GRASS_MAX,
    GRASS_PYRAMID_MAX_LEVELS, GRASS_SPREAD_AMOUNT_DEFAULT, GRASS_SPREAD_PCT_DEFAULT,
    GRASS_SPREAD_RADIUS, GRASS_SPREAD_RING1_PCT_DEFAULT, GRASS_SPREAD_RING2_PCT_DEFAULT,
    GRASS_SPREAD_RING3_PCT_DEFAULT,
};
use crate::profiler::clock_now_us_threadsafe;
use crate::rng::{grass_hash_fused_4, SimRng};
use core::sync::atomic::AtomicU8;
#[cfg(feature = "threads")]
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

// ─── v2.0.2 Stream 1a: u8 density quantization ──────────────────────────────
//
// The density field is stored as `Vec<AtomicU8>` (1 byte/cell). The scale is
// the SAME one the snapshot path has always used (`quantize_grass_into` /
// `quantize_dirty_tiles_into`): a cell's grass amount `d ∈ [0, GRASS_MAX]` maps
// to `round(clamp(d / GRASS_MAX, 0, 1) * 255)`, so `GRASS_MAX` ⇒ 255 and 0 ⇒ 0.
// Decode is the exact inverse `u8 / 255 * GRASS_MAX`. Because the in-memory
// field now holds those same bytes, the snapshot copy-out is a trivial byte copy
// and the rendered picture is unchanged. All reads `load(Relaxed)→decode→f32`;
// all writes `encode→store(Relaxed)`.

/// Encode an f32 grass amount in `[0, GRASS_MAX]` to the u8 storage byte, using
/// the snapshot quantization scale. Clamps then rounds to nearest.
#[inline]
fn encode_density(d: f32) -> u8 {
    let q = (d / GRASS_MAX).clamp(0.0, 1.0) * 255.0;
    (q + 0.5) as u8
}

/// Decode a u8 storage byte back to the f32 grass-amount domain `[0, GRASS_MAX]`.
#[inline]
fn decode_density(b: u8) -> f32 {
    (b as f32 / 255.0) * GRASS_MAX
}

/// v2.0.2 Stream 1b: lossy relaxed-atomic ADD on a density cell, clamped to
/// `cap_byte` (the target cell's biome-cap byte). Relaxed **load → add → clamp →
/// store** — NOT `fetch_add` (which can't express the cap and wraps on u8
/// overflow). Under true concurrent collision one update is lost wholesale
/// (a store clobbers another store); accepted noise per the hub decision. Returns
/// `true` if the stored byte actually changed (used to decide tile activation).
#[inline]
fn scatter_add(cell: &AtomicU8, add_byte: u8, cap_byte: u8) -> bool {
    let cur = cell.load(Ordering::Relaxed);
    let next = cur.saturating_add(add_byte).min(cap_byte);
    if next != cur {
        cell.store(next, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// v2.0.2 Stream 1b: lossy relaxed-atomic SUB on a density cell, floored at 0.
/// Relaxed **load → sub → floor → store** (saturating, so no u8 underflow wrap).
/// Same lossy-collision semantics as [`scatter_add`]. Returns `true` if the byte
/// changed.
#[inline]
fn scatter_sub(cell: &AtomicU8, sub_byte: u8) -> bool {
    let cur = cell.load(Ordering::Relaxed);
    let next = cur.saturating_sub(sub_byte);
    if next != cur {
        cell.store(next, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// v2.0.2 Stream 1b: which propagation kernel `compute_propagation` runs.
///
/// `Scatter` is the LIVE path (default at World/wasm construction) — the
/// stochastic u8 dice-roll spread/decay model. `Blur` is the retained
/// deterministic separable-Gaussian path; it stays fully intact behind this
/// selector (deletion is deferred to Stage 3, after the bench). Low-level
/// `GrassGrid` test constructors default to `Blur` so the existing blur
/// propagation tests keep passing without rewriting their assertions; the World
/// boot path flips the live grid to `Scatter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrassPropagation {
    /// Stochastic u8 scatter (v2.0.2) — the live propagation path.
    Scatter,
    /// Deterministic separable-Gaussian blur (v2.0.1) — retained for the bench
    /// and as the default for low-level grid tests.
    #[default]
    Blur,
}

/// v2.0.2 Stream 1b/1c: precomputed disc offset table for scatter spread targets.
///
/// Holds every `(dx, dy)` offset within `GRASS_SPREAD_RADIUS` (excluding the
/// centre), partitioned into three radial **bands** by Euclidean distance:
/// band 0 (ring 1) = `r ∈ (0, 1.5]`, band 1 (ring 2) = `(1.5, 2.5]`, band 2
/// (ring 3) = `(2.5, radius]`. A spread roll first picks a band via the
/// normalized cumulative ring weights, then a **uniform** offset *within* that
/// band. Because the within-band pick is uniform over all offsets at that radius
/// (all angles equally likely) and the bands are Euclidean rings (not Chebyshev
/// squares), the cumulative footprint is a round disc — the ring weights tune
/// only the radial falloff, never the angular shape. This is the v2.0.2 resolved
/// "precomputed disc offset table, NOT Chebyshev rings" decision.
#[derive(Debug, Clone)]
struct DiscTable {
    /// Flat list of `(dx, dy)` offsets, band 0 then band 1 then band 2.
    offsets: Vec<(i32, i32)>,
    /// `band_end[b]` is the exclusive end index into `offsets` for band `b`
    /// (so band 0 = `offsets[0..band_end[0]]`, band 1 = `band_end[0]..band_end[1]`,
    /// band 2 = `band_end[1]..band_end[2]`). `band_end[2] == offsets.len()`.
    band_end: [usize; 3],
    /// Normalized cumulative ring weights: `cum[b]` = P(band ≤ b). A band with no
    /// offsets at this radius gets zero mass (folded into the next non-empty band).
    cum: [f32; 3],
}

impl DiscTable {
    /// Build the disc table for a given radius (cells) and ring weights. The
    /// weights need not sum to 1 (normalized here); a band with no cells at this
    /// radius is dropped from the cumulative distribution so it never gets picked.
    fn build(radius: i32, ring1: f32, ring2: f32, ring3: f32) -> Self {
        let radius = radius.max(1);
        let rf = radius as f32 + 1e-3;
        let mut bands: [Vec<(i32, i32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                if dist > rf {
                    continue; // outside the disc → keeps the footprint round
                }
                let band = if dist <= 1.5 {
                    0
                } else if dist <= 2.5 {
                    1
                } else {
                    2
                };
                bands[band].push((dx, dy));
            }
        }
        // Drop the weight of any empty band so it is never sampled.
        let raw = [
            if bands[0].is_empty() {
                0.0
            } else {
                ring1.max(0.0)
            },
            if bands[1].is_empty() {
                0.0
            } else {
                ring2.max(0.0)
            },
            if bands[2].is_empty() {
                0.0
            } else {
                ring3.max(0.0)
            },
        ];
        let total: f32 = raw.iter().sum();
        // Fall back to uniform-over-present-bands if all weights are zero.
        let norm = if total > 0.0 {
            [raw[0] / total, raw[1] / total, raw[2] / total]
        } else {
            let present = [
                !bands[0].is_empty() as u8 as f32,
                !bands[1].is_empty() as u8 as f32,
                !bands[2].is_empty() as u8 as f32,
            ];
            let pc: f32 = present.iter().sum::<f32>().max(1.0);
            [present[0] / pc, present[1] / pc, present[2] / pc]
        };
        let cum = [norm[0], norm[0] + norm[1], norm[0] + norm[1] + norm[2]];
        let band_end = [
            bands[0].len(),
            bands[0].len() + bands[1].len(),
            bands[0].len() + bands[1].len() + bands[2].len(),
        ];
        let mut offsets = Vec::with_capacity(band_end[2]);
        offsets.extend_from_slice(&bands[0]);
        offsets.extend_from_slice(&bands[1]);
        offsets.extend_from_slice(&bands[2]);
        Self {
            offsets,
            band_end,
            cum,
        }
    }

    /// Pick one `(dx, dy)` spread offset from two `[0, 1)` draws: `band_roll`
    /// chooses the radial band (via the cumulative ring weights), `pick_roll`
    /// chooses a uniform offset within that band. Returns `None` only if the
    /// table is degenerate (empty), which cannot happen for radius ≥ 1.
    #[inline]
    fn sample(&self, band_roll: f32, pick_roll: f32) -> Option<(i32, i32)> {
        if self.offsets.is_empty() {
            return None;
        }
        // Choose band by cumulative weight; clamp to the last present band.
        let band = if band_roll < self.cum[0] {
            0
        } else if band_roll < self.cum[1] {
            1
        } else {
            2
        };
        let (lo, hi) = match band {
            0 => (0, self.band_end[0]),
            1 => (self.band_end[0], self.band_end[1]),
            _ => (self.band_end[1], self.band_end[2]),
        };
        // Guard against an empty chosen band (rounding) by falling through to the
        // whole table — keeps sampling total even at pathological weights.
        let (lo, hi) = if lo >= hi {
            (0, self.offsets.len())
        } else {
            (lo, hi)
        };
        let span = hi - lo;
        let k = ((pick_roll * span as f32) as usize).min(span - 1);
        Some(self.offsets[lo + k])
    }
}

/// v2.0.1 §2: side length (in cells) of an active-frontier tile. 32×32 = 1024
/// cells/tile; at `grass_dim = 1920` that is 60×60 = 3600 tiles. Chosen to
/// amortize per-tile bookkeeping while keeping the frontier-only active set small.
pub const GRASS_TILE_SIZE: usize = 32;

/// v2.0.1 §2: equilibrium epsilon. A cell is "empty" at ≤ EPS and "saturated"
/// at ≥ cap−EPS; a tile with no cell strictly inside `(EPS, cap−EPS)` and no
/// active neighbour is at equilibrium and can be skipped.
///
/// v2.0.3 Stage-3 fix (u8 regression): was 1e-4, which is 39× smaller than one
/// u8 quantum (1/255 ≈ 3.92e-3). Water-cap cells (cap=0.04, byte=10/255≈0.03922)
/// never classified SATURATED because 0.03922 < 0.04 − 1e-4 = 0.0399. All Water
/// tiles remained permanently MIXED, preventing the frontier skip. Fix: raise to
/// exactly one u8 step (1/255) so that any cell whose f32 value round-trips to
/// within one quantum of cap − EPS is still correctly classified.
const GRASS_EQ_EPS: f32 = 1.0 / 255.0;

/// v2.0.1 §2 tile equilibrium classes.
const TILE_EMPTY: u8 = 0;
const TILE_SATURATED: u8 = 1;
const TILE_MIXED: u8 = 2;

/// `Send` wrapper around the raw scratch pointer so the rayon closure can write
/// into disjoint tile cell-ranges in parallel. SAFETY at the call site: active
/// tiles are pairwise-disjoint cell ranges, each closure writes only its tile.
#[cfg(feature = "threads")]
#[derive(Clone, Copy)]
struct ScratchPtr(*mut f32);
#[cfg(feature = "threads")]
unsafe impl Send for ScratchPtr {}
#[cfg(feature = "threads")]
unsafe impl Sync for ScratchPtr {}

#[cfg(not(feature = "threads"))]
#[derive(Clone, Copy)]
struct ScratchPtr(*mut f32);

/// Read a cell from the frozen prior frame `d`, returning 0.0 (ghost) for an
/// out-of-bounds index when walled, or the opposite-edge cell when toroidal.
///
/// v2.0.2 Stream 1a: `d` is now the `AtomicU8` density store; this loads the
/// byte (Relaxed) and decodes it into the f32 grass-amount domain so the blur
/// math below is byte-for-byte the same as the old f32 path.
#[inline]
fn sample_d(d: &[AtomicU8], dim: usize, wrap: bool, ix: i64, iy: i64) -> f32 {
    let dimi = dim as i64;
    if wrap {
        let wx = ix.rem_euclid(dimi) as usize;
        let wy = iy.rem_euclid(dimi) as usize;
        decode_density(d[wy * dim + wx].load(Ordering::Relaxed))
    } else if ix < 0 || ix >= dimi || iy < 0 || iy >= dimi {
        0.0
    } else {
        decode_density(d[iy as usize * dim + ix as usize].load(Ordering::Relaxed))
    }
}

/// v2.0.1 §1 PASS H — horizontal `[1,2,1]/4` tap of the frozen frame `d` at
/// (ix, iy): `0.5·d[c] + 0.25·(d[w] + d[e])`. This is the first of the two
/// separable passes; the vertical pass ([`blur_at`]) consumes its output. Kept a
/// pure function of `d` (boundary via `sample_d`) so it composes identically
/// whether materialized into an intermediate buffer or evaluated on demand.
#[inline]
fn h_at(d: &[AtomicU8], dim: usize, wrap: bool, ix: i64, iy: i64) -> f32 {
    let c = sample_d(d, dim, wrap, ix, iy);
    let w = sample_d(d, dim, wrap, ix - 1, iy);
    let e = sample_d(d, dim, wrap, ix + 1, iy);
    0.5 * c + 0.25 * (w + e)
}

/// v2.0.1 §1 PASS V — vertical `[1,2,1]/4` tap applied to the horizontal pass
/// `h` at (ix, iy): `0.5·h[c] + 0.25·(h[n] + h[s])`. The composition V∘H is the
/// TRUE two-pass separable blur (its outer product is the 3×3
/// `[[1,2,1],[2,4,2],[1,2,1]]/16` Gaussian), evaluated here directly off `d` so
/// the result is bit-identical to running an explicit H-into-scratch pass then a
/// V pass — and identical regardless of how the grid is tiled, because every tap
/// reads only the frozen prior frame `d`. Boundary via `sample_d`.
#[inline]
fn blur_at(d: &[AtomicU8], dim: usize, wrap: bool, ix: i64, iy: i64) -> f32 {
    let c = h_at(d, dim, wrap, ix, iy);
    let n = h_at(d, dim, wrap, ix, iy - 1);
    let s = h_at(d, dim, wrap, ix, iy + 1);
    0.5 * c + 0.25 * (n + s)
}

/// v2.0.1 §1: next-frame value for cell (ix, iy). Separable-Gaussian-blurred
/// neighbour field + logistic in-cell growth, clamped to the cell's biome cap.
/// Pure function of the prior frame `d` → order-independent (single-thread ==
/// rayon bit-identical for a fixed seed).
#[inline]
#[allow(clippy::too_many_arguments)]
fn grass_cell_next(
    d: &[AtomicU8],
    cap: &[f32],
    dim: usize,
    wrap: bool,
    r_in_cell: f32,
    k_propagate: f32,
    ix: usize,
    iy: usize,
) -> f32 {
    let c = iy * dim + ix;
    let v = decode_density(d[c].load(Ordering::Relaxed));
    let k_cap = cap[c];
    let inv_cap = if k_cap > 0.0 { 1.0 / k_cap } else { 0.0 };
    let logistic = r_in_cell * v * (1.0 - v * inv_cap);
    // Max-of-neighbours spill over the TWO-PASS-BLURRED field's 8 neighbours.
    // Two reasons this is isotropic where the old 4-cardinal max was a diamond:
    //   (1) the spill samples ALL 8 neighbours (incl. diagonals), not just N/S/E/W
    //       — a cardinal-only max structurally favours the axes (an L1/Von-Neumann
    //       reach), which is the diamond; the 8-neighbour max gives an L∞ reach
    //       that the isotropic blur then rounds into a disc.
    //   (2) each sample is the separable two-pass Gaussian `blur_at`, so the
    //       diagonal corners are already smeared in before the max reads them.
    // Still a MAX (not a sum), so the anti-cascade "one ripe neighbour grants the
    // boost, neighbours don't stack" semantics are preserved.
    let ixi = ix as i64;
    let iyi = iy as i64;
    let mut max_neighbor = f32::NEG_INFINITY;
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            max_neighbor = max_neighbor.max(blur_at(d, dim, wrap, ixi + dx, iyi + dy));
        }
    }
    let prop = k_propagate * max_neighbor;
    (v + logistic + prop).clamp(0.0, k_cap)
}

/// v2.0.2 Stream 1d: live-tunable parameters for the stochastic scatter kernel.
/// Wired from DevSliders via WorldHandle apply_* setters → World → GrassGrid.
/// All six fields default to the working constants added by Stream 1b so
/// default behavior is byte-identical before sliders land.
#[derive(Debug, Clone, Copy)]
pub struct ScatterParams {
    /// Per-tick probability a grass cell rolls a decay event (0..=1).
    pub decay_pct: f32,
    /// Density subtracted on a decay event (f32 grass-amount domain, floored at 0).
    pub decay_amount: f32,
    /// Per-tick probability a grass cell rolls a spread event (0..=1).
    pub spread_pct: f32,
    /// Density added to the spread target (f32 grass-amount domain).
    pub spread_amount: f32,
    /// Ring-1 (distance 0–1.5 cells) weight for disc-table band sampling.
    pub ring1_pct: f32,
    /// Ring-2 (distance 1.5–2.5 cells) weight. Ring-3 is the remainder
    /// (the disc table normalizes all three, so ring3 need not be stored).
    pub ring2_pct: f32,
}

impl Default for ScatterParams {
    fn default() -> Self {
        Self {
            decay_pct: GRASS_DECAY_PCT_DEFAULT,
            decay_amount: GRASS_DECAY_AMOUNT_DEFAULT,
            spread_pct: GRASS_SPREAD_PCT_DEFAULT,
            spread_amount: GRASS_SPREAD_AMOUNT_DEFAULT,
            ring1_pct: GRASS_SPREAD_RING1_PCT_DEFAULT,
            ring2_pct: GRASS_SPREAD_RING2_PCT_DEFAULT,
        }
    }
}

// ─── v2.0.3 Stream 2a: GrassPyramid — u8 box-filter mip pyramid ─────────────
//
// L0 ALIASES the live `GrassGrid::density` field (an `AtomicU8` slice passed at
// refresh time — NOT copied or stored here). L1..N are OWNED, box-filtered u8
// buffers. Each Lk cell is the arithmetic mean of its 2×2 (edge-clamped) Lk-1
// parent cells, truncated to u8 (round-before-truncate: +2 before >>2).
//
// Borrow structure: `GrassPyramid` is a field of `GrassGrid`, but `refresh`
// takes `density: &[AtomicU8]` as a separate parameter. Callers split-borrow
// `grass.density` and `grass.pyramid` via field access — the compiler allows
// this because they are distinct named fields. See `World::step` for the call
// site (`let density = &self.grass.density; self.grass.pyramid.refresh(density, ...)`).
//
// Refresh design: FULL recompute every tick (correct at all scales; O(N×1/3)).
// Active-set-keyed partial refresh (dirty-subtree walk) is the planned Stage-3
// optimization — deferred from this stream for correctness-first shipping.
// TODO(stage3-pyramid-perf): replace the full recompute with a dirty-subtree walk
//   keyed off `tile_active` / `tile_dirty` bitsets to restore the sparse-frontier
//   win at large scale. The interface is already active-set-aware (refresh takes
//   `tile_active` + `tiles_per_axis`); only the loop body needs to change.

/// v2.0.3 Stream 2a: u8 box-filter mip pyramid over the grass density field.
///
/// `level_dims[k]` gives `(width, height)` of level Lk (k=0 is the full base
/// resolution, k=1 is the first downsample, etc.). `levels[k]` holds the OWNED
/// u8 buffer for L(k+1) (index offset: `levels[0]` is L1, `levels[1]` is L2,
/// etc.). L0 is **not** stored; callers pass `&[AtomicU8]` (the live density
/// field) directly into `refresh` and `sample`.
///
/// All level dims are computed by repeated `div_ceil(2)` halving of the base
/// `(grass_dim_x, grass_dim_y)`, stopping when both axes are 1 or the level
/// count reaches `GRASS_PYRAMID_MAX_LEVELS`.
#[derive(Debug, Clone)]
pub struct GrassPyramid {
    /// Per-level `(width, height)` including L0. `level_dims[0]` is the base
    /// (L0 alias) resolution; `level_dims[k]` for k≥1 is the owned L(k) buffer.
    pub level_dims: Vec<(usize, usize)>,
    /// Owned u8 buffers for L1, L2, … Row-major; `levels[k].len() ==
    /// level_dims[k+1].0 * level_dims[k+1].1`.
    pub levels: Vec<Vec<u8>>,
}

impl GrassPyramid {
    /// Compute the level-dims chain by repeated `div_ceil(2)` halving, starting
    /// from `(w0, h0)` (the L0 / base-layer dimensions). Returns a `Vec` of
    /// `(w, h)` pairs from L0 down to the coarsest level (where both axes are 1
    /// or the count hits `GRASS_PYRAMID_MAX_LEVELS`).
    fn compute_level_dims(w0: usize, h0: usize) -> Vec<(usize, usize)> {
        let mut dims = Vec::with_capacity(GRASS_PYRAMID_MAX_LEVELS);
        dims.push((w0, h0));
        loop {
            if dims.len() >= GRASS_PYRAMID_MAX_LEVELS {
                break;
            }
            let (pw, ph) = *dims.last().unwrap();
            if pw == 1 && ph == 1 {
                break;
            }
            let nw = pw.div_ceil(2);
            let nh = ph.div_ceil(2);
            dims.push((nw, nh));
        }
        dims
    }

    /// Construct a new pyramid for a square grass grid of side `grass_dim`.
    /// Allocates zeroed owned buffers for L1+; L0 is the caller-supplied density
    /// slice (not stored here).
    pub fn new(grass_dim: usize) -> Self {
        Self::new_rect(grass_dim, grass_dim)
    }

    /// Construct for a potentially-non-square `(grass_dim_x, grass_dim_y)` base.
    /// (Square grids use `new`; this generalizes for future non-square worlds.)
    pub fn new_rect(grass_dim_x: usize, grass_dim_y: usize) -> Self {
        let level_dims = Self::compute_level_dims(grass_dim_x, grass_dim_y);
        // Allocate owned zeroed buffers for L1+ (level_dims[1..]).
        let levels: Vec<Vec<u8>> = level_dims[1..]
            .iter()
            .map(|&(w, h)| vec![0u8; w * h])
            .collect();
        Self { level_dims, levels }
    }

    /// Number of levels (including L0).
    #[inline]
    pub fn num_levels(&self) -> usize {
        self.level_dims.len()
    }

    /// `(width, height)` of level `k` (0-based; L0 = base resolution).
    #[inline]
    pub fn level_dim(&self, k: usize) -> (usize, usize) {
        self.level_dims[k]
    }

    /// Rebuild the owned mip levels (L1+) from the L0 source density field.
    ///
    /// Each output cell at level Lk is the arithmetic mean of the (up to) 4
    /// in-bounds parent cells at Lk-1, computed as u8 via `(sum + 2) >> 2`
    /// (round-before-truncate, matching "nearest-round" behavior). Edge-clamped:
    /// a ragged (odd-dim) border averages 1–3 in-bounds parents instead of
    /// padding with zeros. This avoids a dark-border artifact at non-pow-2 dims.
    ///
    /// `density` MUST be the `GrassGrid::density` slice (`AtomicU8`, Relaxed
    /// loads). `tile_active` + `tiles_per_axis` are accepted for API completeness
    /// (forwarded for the future active-set-keyed optimization); the current
    /// implementation ignores them and does a full recompute.
    ///
    /// Borrow note: callers split-borrow `grass.density` and `grass.pyramid`
    /// separately (distinct fields → compiler allows it). Do NOT pass
    /// `self.grass.density` through a single `&GrassGrid` borrow.
    pub fn refresh(
        &mut self,
        density: &[AtomicU8],
        _tile_active: &[AtomicU64],
        _tiles_per_axis: usize,
    ) {
        // L1 comes from L0 (the density field); Lk (k≥2) comes from Lk-1 (owned).
        for k in 0..self.levels.len() {
            let (pw, ph) = self.level_dims[k]; // parent dims
            let (cw, ch) = self.level_dims[k + 1]; // child (output) dims
                                                   // We need to read from the level-k source and write to level-(k+1).
                                                   // For k=0 source is `density`; for k≥1 source is `self.levels[k-1]`.
                                                   // To avoid borrow conflicts when k≥1 we use split_at_mut on `self.levels`.
            if k == 0 {
                // Source: density (&[AtomicU8])
                let dst = &mut self.levels[0];
                downsample_from_atomic(density, pw, ph, dst, cw, ch);
            } else {
                // Source: self.levels[k-1]; Dest: self.levels[k].
                // split_at_mut lets us hold immutable ref to src and mutable ref to dst.
                let (src_part, dst_part) = self.levels.split_at_mut(k);
                let src: &[u8] = &src_part[k - 1];
                let dst: &mut [u8] = &mut dst_part[0];
                downsample_from_u8(src, pw, ph, dst, cw, ch);
            }
        }
    }

    /// Sample level `level` at integer cell coordinates `(x, y)`, clamped to
    /// the level's bounds. Level 0 reads from `density` (AtomicU8, Relaxed);
    /// level k≥1 reads from the owned buffer.
    ///
    /// For bilinear/world-space sampling see `GrassGrid::bilinear_sample`; this
    /// is the integer-cell accessor for direct pyramid reads (e.g. NN sensing,
    /// render LOD). Clamped, never wrapping — callers that need wrap must
    /// `rem_euclid` themselves.
    #[inline]
    pub fn sample_clamped(&self, density: &[AtomicU8], level: usize, x: usize, y: usize) -> u8 {
        let (w, h) = self.level_dims[level];
        let cx = x.min(w.saturating_sub(1));
        let cy = y.min(h.saturating_sub(1));
        if level == 0 {
            let (lw, _) = self.level_dims[0];
            density[cy * lw + cx].load(Ordering::Relaxed)
        } else {
            let buf = &self.levels[level - 1];
            buf[cy * w + cx]
        }
    }

    /// Fill a row-major destination buffer `dst` with a `(win_w × win_h)`
    /// window of `level`, with origin `(origin_x, origin_y)` in level cells.
    /// Out-of-bounds cells are CLAMPED (edge-repeat). `dst` must have length
    /// `>= win_w * win_h`.
    ///
    /// This is the extraction API for 2b (snapshot publish) and 2c (render
    /// LOD upload): the worker calls this to populate the snapshot window bytes.
    ///
    /// TODO (Stage-3 wrap-seam): For toroidal worlds this function should WRAP
    /// out-of-bounds coordinates modulo `(lw, lh)` instead of clamping. The
    /// origin computation in `wasm_api::write_snapshot` (the
    /// `.max(0).min(level_w - raw_win_w)` clamp) must also be removed for the
    /// toroidal path so that a camera near the seam can have an origin that
    /// overflows the high edge. Required changes:
    ///   1. Add a `wrap: bool` parameter to this function.
    ///   2. Change `origin_x`/`origin_y` to `isize` (or perform modular
    ///      offset computation inside) so negative origins are representable.
    ///   3. In the inner loop: `let sx = (origin_x + col as isize).rem_euclid(lw as isize) as usize`.
    ///   4. Update `write_snapshot` origin calculation: for toroidal worlds skip
    ///      the `saturating_sub` clamp; keep the existing clamp for walled worlds.
    ///   5. Apply the same wrapping in the biome mode-downsample loop.
    ///
    /// The default (non-toroidal, zoom=1, full-field) path must stay byte-identical.
    /// This was deferred because the signature change is breaking for tests and the
    /// biome loop requires parallel changes; risk outweighed the single-pass benefit.
    #[allow(clippy::too_many_arguments)]
    pub fn viewport_window(
        &self,
        density: &[AtomicU8],
        level: usize,
        origin_x: usize,
        origin_y: usize,
        win_w: usize,
        win_h: usize,
        dst: &mut [u8],
    ) {
        debug_assert!(
            dst.len() >= win_w * win_h,
            "viewport_window: dst too small ({} < {})",
            dst.len(),
            win_w * win_h
        );
        let (lw, lh) = self.level_dims[level];
        for row in 0..win_h {
            let sy = (origin_y + row).min(lh.saturating_sub(1));
            for col in 0..win_w {
                let sx = (origin_x + col).min(lw.saturating_sub(1));
                let byte = if level == 0 {
                    density[sy * lw + sx].load(Ordering::Relaxed)
                } else {
                    self.levels[level - 1][sy * lw + sx]
                };
                dst[row * win_w + col] = byte;
            }
        }
    }
}

/// Box-filter downsample from an AtomicU8 (L0 density) source into a u8 buffer.
/// Each output cell at `(ox, oy)` averages the 1–4 in-bounds parents at
/// `(2*ox, 2*oy)`, `(2*ox+1, 2*oy)`, `(2*ox, 2*oy+1)`, `(2*ox+1, 2*oy+1)`.
/// Edge-clamped: parents outside `[0, pw) × [0, ph)` are excluded from the
/// average (count denominator shrinks). Uses `(sum + 2) >> 2` rounding for 4-parent
/// case; exact for 1-parent; rounded for 2/3. This is faster than floating-point
/// and GPU-native u8-correct.
fn downsample_from_atomic(
    src: &[AtomicU8],
    pw: usize,
    ph: usize,
    dst: &mut [u8],
    cw: usize,
    ch: usize,
) {
    for oy in 0..ch {
        let sy0 = oy * 2;
        let sy1 = sy0 + 1;
        let has_y1 = sy1 < ph;
        for ox in 0..cw {
            let sx0 = ox * 2;
            let sx1 = sx0 + 1;
            let has_x1 = sx1 < pw;
            // Gather in-bounds parent values.
            let v00 = src[sy0 * pw + sx0].load(Ordering::Relaxed) as u32;
            let (sum, count) = if !has_x1 && !has_y1 {
                (v00, 1u32)
            } else if !has_x1 {
                let v01 = src[sy1 * pw + sx0].load(Ordering::Relaxed) as u32;
                (v00 + v01, 2)
            } else if !has_y1 {
                let v10 = src[sy0 * pw + sx1].load(Ordering::Relaxed) as u32;
                (v00 + v10, 2)
            } else {
                let v10 = src[sy0 * pw + sx1].load(Ordering::Relaxed) as u32;
                let v01 = src[sy1 * pw + sx0].load(Ordering::Relaxed) as u32;
                let v11 = src[sy1 * pw + sx1].load(Ordering::Relaxed) as u32;
                (v00 + v10 + v01 + v11, 4)
            };
            // Round-before-truncate: (sum + count/2) / count.
            dst[oy * cw + ox] = ((sum + count / 2) / count) as u8;
        }
    }
}

/// Box-filter downsample from a u8 (owned Lk) source into another u8 buffer.
/// Same edge-clamped averaging as `downsample_from_atomic`.
fn downsample_from_u8(src: &[u8], pw: usize, ph: usize, dst: &mut [u8], cw: usize, ch: usize) {
    for oy in 0..ch {
        let sy0 = oy * 2;
        let sy1 = sy0 + 1;
        let has_y1 = sy1 < ph;
        for ox in 0..cw {
            let sx0 = ox * 2;
            let sx1 = sx0 + 1;
            let has_x1 = sx1 < pw;
            let v00 = src[sy0 * pw + sx0] as u32;
            let (sum, count) = if !has_x1 && !has_y1 {
                (v00, 1u32)
            } else if !has_x1 {
                let v01 = src[sy1 * pw + sx0] as u32;
                (v00 + v01, 2)
            } else if !has_y1 {
                let v10 = src[sy0 * pw + sx1] as u32;
                (v00 + v10, 2)
            } else {
                let v10 = src[sy0 * pw + sx1] as u32;
                let v01 = src[sy1 * pw + sx0] as u32;
                let v11 = src[sy1 * pw + sx1] as u32;
                (v00 + v10 + v01 + v11, 4)
            };
            dst[oy * cw + ox] = ((sum + count / 2) / count) as u8;
        }
    }
}

/// Runtime-sized grass density field over the world.
///
/// Row-major layout: cell (ix, iy) is at index `iy * dims.grass_dim + ix`.
/// Density values are clamped to `[0.0, GRASS_MAX]` after each `step` call.
#[derive(Debug)]
pub struct GrassGrid {
    /// Runtime dimensions (grass_dim / cell_count / wrap). Set at construction.
    pub dims: WorldDims,
    /// Row-major density, length `dims.grass_cell_count`.
    ///
    /// v2.0.2 Stream 1a: stored as `AtomicU8` (1 byte/cell) on the snapshot
    /// quantization scale (`GRASS_MAX` ⇒ 255). Read via [`GrassGrid::dget`]
    /// (load Relaxed → decode f32), written via [`GrassGrid::dset`] (encode →
    /// store Relaxed). The separable-blur propagation still runs in f32; only
    /// the backing store changed, so the rendered field is unchanged.
    pub density: Vec<AtomicU8>,
    /// Per-cell grass *carrying capacity* (`biome_factor * GRASS_MAX`), row-major,
    /// length `dims.grass_cell_count`. v2.0 Wave 1: biome drives grass richness —
    /// a cell's density saturates at `capacity[c]` (Plains→GRASS_MAX, Desert→0.30,
    /// Water→0.04) for BOTH the logistic carrying capacity and the post-step
    /// clamp. Built once at construction; never mutated per tick (static map).
    pub capacity: Vec<f32>,
    /// Double-buffer scratch. Same length as `density`. Recomputed every
    /// `step` call; never read between ticks.
    pub scratch: Vec<f32>,
    /// Per-row "any non-empty" bitset (v1.5 S5b). One bit per row: 1 if any
    /// cell in that row has `density > 0`, 0 otherwise. Lets the grass-sector
    /// NN scan skip whole rows in O(1) when sparse. Length =
    /// `GRASS_GRID_DIM.div_ceil(64)` u64s; regenerated after every `step()`.
    pub row_has_density: Vec<u64>,
    /// v2.0.1 §2: tiles per axis = `grass_dim.div_ceil(GRASS_TILE_SIZE)`.
    pub tiles_per_axis: usize,
    /// v2.0.1 §2: per-tile "active this tick" bitset, 1 bit per tile, row-major
    /// over the tile grid (`tiles_per_axis²` bits). A tile is active iff it (or a
    /// neighbour) may change this tick: it is FRONTIER, borders a FRONTIER tile,
    /// or was grazed. The frontier propagation pass processes ONLY active tiles;
    /// equilibrium tiles (uniformly ≈0 or uniformly saturated) are skipped exactly.
    ///
    /// v2.0.2 Stream 1c: `AtomicU64` words so the parallel SCATTER pass can set a
    /// *neighbour* tile's active bit (cross-tile spread) via `fetch_or(Relaxed)`
    /// without a data race. The blur path never writes cross-tile, so this is a
    /// no-op cost for it (relaxed atomics are plain loads/stores on wasm).
    pub tile_active: Vec<AtomicU64>,
    /// v2.0.1 §2: scratch "active NEXT tick" bitset, built during a pass from the
    /// FRONTIER tiles + their 8-neighbour fringe, then swapped into `tile_active`.
    /// v2.0.2 Stream 1c: `AtomicU64` so scatter workers can `fetch_or` the
    /// next-tick active bit of any tile they spread into.
    tile_active_next: Vec<AtomicU64>,
    /// v2.0.1 §3: per-tile "dirty since each snapshot slot was written" — 2 bits
    /// per tile (one per ping-pong slot). A tile's bit for slot S is set when its
    /// density changed (propagation or graze) and cleared when re-quantized into
    /// slot S. Lets `quantize_dirty_tiles_into` re-quantize only changed tiles
    /// into a slot while keeping BOTH ping-pong slots valid. Indexed
    /// `tile_dirty[slot]`, each a `tiles_per_axis²`-bit bitset.
    ///
    /// v2.0.2 Stream 1c: `AtomicU64` words so a scatter worker that writes into a
    /// neighbour tile can mark that tile dirty (so the snapshot re-quantizes it)
    /// via `fetch_or(Relaxed)` without racing other workers.
    pub tile_dirty: [Vec<AtomicU64>; 2],
    /// v2.0.1 §2: persistent per-tile equilibrium class — `TILE_EMPTY` (all cells
    /// ≤ EPS), `TILE_SATURATED` (all cells ≥ cap−EPS), or `TILE_MIXED` (a moving
    /// frontier). One byte per tile (`tiles_per_axis²`). Only ACTIVE tiles are
    /// re-classified each pass; skipped tiles are unchanged so their class
    /// persists. The next-tick active set = MIXED tiles + any tile bordering a
    /// tile of a DIFFERENT class (so a saturated/empty boundary keeps spreading).
    tile_class: Vec<u8>,
    /// v2.0.2 Stream 1b: which propagation kernel `compute_propagation` runs.
    /// Defaults to `Blur` (so low-level grid test constructors keep the blur
    /// path); the World/wasm boot path sets it to `Scatter` for the live sim.
    pub propagation: GrassPropagation,
    /// v2.0.2 Stream 1b: world seed for the per-cell scatter hash RNG. The World
    /// caller writes its `world_seed` here once at construction (it is constant).
    /// Unused on the blur path.
    pub world_seed: u32,
    /// v2.0.2 Stream 1b: current tick for the per-cell scatter hash RNG. The World
    /// caller writes `self.tick` here before each scatter `compute_propagation`
    /// so each tick's rolls are independent. Unused on the blur path.
    pub scatter_tick: u32,
    /// v2.0.2 Stream 1d: live-tunable scatter kernel parameters wired from
    /// DevSliders via WorldHandle apply_* setters. Defaults mirror the working
    /// constants so default behavior is byte-identical to before sliders landed.
    pub scatter_params: ScatterParams,
    /// v2.0.2 Stream 1b: precomputed DISC offset table for spread targets — all
    /// `(dx, dy)` within `GRASS_SPREAD_RADIUS`, in three radial bands (ring 1/2/3).
    /// A spread roll picks a band by the ring weights then a uniform offset within
    /// it, so the angular distribution is uniform → the cumulative footprint is a
    /// round disc, not a Chebyshev square/diamond. Rebuilt whenever the ring
    /// sliders change. See `DiscTable`.
    disc: DiscTable,
    /// Per-tick wall time of the threaded `par_chunks_mut` dispatch block in
    /// `compute_propagation` (us). Zero in non-threaded builds. Reset at the
    /// top of each `compute_propagation` call; drained by the World caller.
    pub par_chunks_us: AtomicU64,
    /// Per-tick wall time of the sequential `chunks_mut` block in
    /// `compute_propagation` (us). Zero in threaded builds.
    pub chunks_mut_us: AtomicU64,
    /// Per-tick sum-busy time inside the row_body closure across all
    /// invocations (us). In threaded builds, this is summed across rayon
    /// workers via `fetch_add`, so it can exceed `par_chunks_us` wall.
    pub row_body_us: AtomicU64,
    /// Per-tick sum-busy time bracketing just the row_body closure body
    /// itself (us), measured from inside the closure. Differs from
    /// `row_body_us` by excluding the per-chunk `chunks_mut` dispatch overhead;
    /// also summed across rayon workers via `fetch_add`.
    pub row_body_self_us: AtomicU64,
    /// Per-tick count of row-body closure invocations. Always equals
    /// `GRASS_GRID_DIM` (one per row) regardless of how rows are partitioned
    /// across rayon workers. Paired with `row_body_us` / `row_body_self_us`
    /// so the profiler can report honest per-row `ms/call`. (v1.7.2)
    pub row_body_calls: AtomicU64,
    /// Per-tick count of dispatch invocations — exactly one per
    /// `compute_propagation` call. Paired with `par_chunks_us` + `chunks_mut_us`
    /// so the `grass_step.dispatch` row has a meaningful `ms/call`. (v1.7.2)
    pub dispatch_calls: AtomicU64,
    /// v2.0.3 Stream 2a: u8 box-filter mip pyramid over the density field.
    /// L0 aliases `density` (not stored here); L1+ owned by the pyramid.
    /// Refreshed each tick in `World::step` after `rebuild_row_bitset()`.
    pub pyramid: GrassPyramid,
    /// v2.0.4 C2: profiling gate for the scatter kernel. When `false` (the
    /// common case), all `clock_now_us_threadsafe()` calls and their associated
    /// atomic `fetch_add`s inside `compute_propagation_scatter` are skipped,
    /// eliminating false-sharing on the profiling counters across rayon workers.
    /// Set by `World` before each `compute_propagation` call to match the
    /// `Profiler::enabled()` state. Defaults to `false`.
    pub profile_scatter: bool,
}

/// v2.0.2 Stream 1c: deep-copy a `Vec<AtomicU64>` bitset (Relaxed loads) — the
/// atomic words are not `Clone`, so rebuild a fresh vector with the same values.
#[inline]
fn clone_atomic_bits(src: &[AtomicU64]) -> Vec<AtomicU64> {
    src.iter()
        .map(|w| AtomicU64::new(w.load(Ordering::Relaxed)))
        .collect()
}

/// v2.0.2 Stream 1b: build the default scatter disc table from the kernel
/// constants. Used by every constructor so the table is always present (the
/// scatter path indexes it; the blur path ignores it).
#[inline]
fn default_disc_table() -> DiscTable {
    DiscTable::build(
        GRASS_SPREAD_RADIUS,
        GRASS_SPREAD_RING1_PCT_DEFAULT,
        GRASS_SPREAD_RING2_PCT_DEFAULT,
        GRASS_SPREAD_RING3_PCT_DEFAULT,
    )
}

impl Clone for GrassGrid {
    fn clone(&self) -> Self {
        Self {
            dims: self.dims,
            // v2.0.2 Stream 1a: `AtomicU8` is not `Clone`; copy each byte out
            // (Relaxed) and rebuild a fresh atomic vector with the same bytes.
            density: self
                .density
                .iter()
                .map(|b| AtomicU8::new(b.load(Ordering::Relaxed)))
                .collect(),
            capacity: self.capacity.clone(),
            scratch: self.scratch.clone(),
            row_has_density: self.row_has_density.clone(),
            tiles_per_axis: self.tiles_per_axis,
            tile_active: clone_atomic_bits(&self.tile_active),
            tile_active_next: clone_atomic_bits(&self.tile_active_next),
            tile_dirty: [
                clone_atomic_bits(&self.tile_dirty[0]),
                clone_atomic_bits(&self.tile_dirty[1]),
            ],
            tile_class: self.tile_class.clone(),
            propagation: self.propagation,
            world_seed: self.world_seed,
            scatter_tick: self.scatter_tick,
            scatter_params: self.scatter_params,
            disc: self.disc.clone(),
            par_chunks_us: AtomicU64::new(0),
            chunks_mut_us: AtomicU64::new(0),
            row_body_us: AtomicU64::new(0),
            row_body_self_us: AtomicU64::new(0),
            row_body_calls: AtomicU64::new(0),
            dispatch_calls: AtomicU64::new(0),
            // v2.0.3 Stream 2a: pyramid is Clone (derives it); deep-copy the
            // owned level buffers. The freshly-cloned grid's levels are a
            // point-in-time copy; the next refresh will bring them current.
            pyramid: self.pyramid.clone(),
            // v2.0.4 C2: profiling gate — reset to off in clones; World will
            // re-set it before the next compute_propagation call.
            profile_scatter: false,
        }
    }
}

impl GrassGrid {
    /// World-init constructor. Seeds `initial_seed_count` random cells
    /// with `GRASS_MAX`; all others zero. Draws from `rng` via the same
    /// world RNG used everywhere else (deterministic given fixed seed).
    ///
    /// Capacity is uniform `GRASS_MAX` (all-Plains). Production callers use
    /// [`GrassGrid::new_with_capacity`] to apply per-biome carrying capacity;
    /// this plain ctor preserves today's all-plains behavior for the grass-only
    /// unit tests.
    ///
    /// Test/back-compat ctor: production routes through `new_with_capacity`.
    #[allow(dead_code)]
    pub fn new(rng: &mut SimRng, initial_seed_count: u32, dims: WorldDims) -> Self {
        Self::new_with_capacity(rng, initial_seed_count, dims, None)
    }

    /// World-init constructor with optional per-cell biome carrying capacity.
    ///
    /// v2.0 Wave 1: when `biome_grid` is `Some`, each cell's carrying capacity is
    /// `capacity_factor_from_u8(biome) * GRASS_MAX` (Plains ×1.0, Desert ×0.30,
    /// Water ×0.04). A seeded cell is filled to **its own** capacity (so a water
    /// cell seeds near 0.04, a desert cell near 0.30), and the same per-cell cap
    /// bounds logistic regrowth in [`compute_propagation`]. When `None`, capacity
    /// is uniform `GRASS_MAX` (== legacy all-plains behavior).
    ///
    /// Deterministic: the RNG draw order is identical to the legacy ctor (one
    /// `rng.index` per seed), so grass seeding stays pinned to the seed; biome
    /// only changes the *value* written, never the cell selection.
    pub fn new_with_capacity(
        rng: &mut SimRng,
        initial_seed_count: u32,
        dims: WorldDims,
        biome_grid: Option<&[u8]>,
    ) -> Self {
        let cell_count = dims.grass_cell_count;
        let capacity = match biome_grid {
            Some(bg) => {
                debug_assert_eq!(bg.len(), cell_count);
                bg.iter()
                    .map(|&b| crate::world::biome::capacity_factor_from_u8(b) * GRASS_MAX)
                    .collect::<Vec<f32>>()
            }
            None => vec![GRASS_MAX; cell_count],
        };
        // v2.0.2 Stream 1a: density is `AtomicU8` (one byte/cell), zero-filled.
        let mut density: Vec<AtomicU8> = (0..cell_count).map(|_| AtomicU8::new(0)).collect();
        let scratch = vec![0.0f32; cell_count];
        for _ in 0..initial_seed_count {
            let idx = rng.index(cell_count);
            // Seed to the cell's own carrying capacity (Plains→GRASS_MAX,
            // Desert→0.30, Water→0.04) so a freshly seeded cell already sits at
            // its biome's ceiling rather than over-filling.
            *density[idx].get_mut() = encode_density(capacity[idx]);
        }
        let tiles_per_axis = dims.grass_dim.div_ceil(GRASS_TILE_SIZE);
        let tile_words = (tiles_per_axis * tiles_per_axis).div_ceil(64);
        let mut g = Self {
            dims,
            density,
            capacity,
            scratch,
            row_has_density: vec![0u64; dims.grass_dim.div_ceil(64)],
            tiles_per_axis,
            tile_active: (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
            tile_active_next: (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
            tile_dirty: [
                (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
                (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
            ],
            tile_class: vec![TILE_EMPTY; tiles_per_axis * tiles_per_axis],
            // v2.0.2 Stream 1b: default to BLUR at this low-level ctor so existing
            // grid-direct propagation tests stay on the blur path; World/wasm flips
            // the live grid to Scatter via `set_propagation` after construction.
            propagation: GrassPropagation::Blur,
            world_seed: 0,
            scatter_tick: 0,
            // v2.0.2 Stream 1d: scatter params default to the working constants so
            // behavior is byte-identical before sliders are wired.
            scatter_params: ScatterParams::default(),
            disc: default_disc_table(),
            par_chunks_us: AtomicU64::new(0),
            chunks_mut_us: AtomicU64::new(0),
            row_body_us: AtomicU64::new(0),
            row_body_self_us: AtomicU64::new(0),
            row_body_calls: AtomicU64::new(0),
            dispatch_calls: AtomicU64::new(0),
            // v2.0.3 Stream 2a: pyramid — L1+ buffers zeroed; first tick's
            // refresh will populate them from the seeded density field.
            pyramid: GrassPyramid::new(dims.grass_dim),
            // v2.0.4 C2: profiling gate — off by default; World sets it before
            // each compute_propagation call to match the Profiler enabled state.
            profile_scatter: false,
        };
        g.rebuild_row_bitset();
        // Seed the active set from the initial density so the first tick
        // processes every non-equilibrium tile, and mark ALL tiles dirty so the
        // first snapshot writes the full grass region into both slots (the SAB
        // slots start zeroed; seeded-but-saturated tiles aren't "active").
        g.resync_active_from_density();
        g.mark_all_tiles_dirty();
        g
    }

    /// Recompute the per-row "any non-empty" bitset. Called from `step()`
    /// after the density swap; exposed for tests that mutate `density` directly.
    pub fn rebuild_row_bitset(&mut self) {
        let dim = self.dims.grass_dim;
        let words = dim.div_ceil(64);
        if self.row_has_density.len() != words {
            self.row_has_density.resize(words, 0);
        }
        for w in self.row_has_density.iter_mut() {
            *w = 0;
        }
        for iy in 0..dim {
            let row_off = iy * dim;
            // v2.0.2 Stream 1a: any non-zero stored byte ⇒ row has density.
            let any = self.density[row_off..row_off + dim]
                .iter()
                .any(|d| d.load(Ordering::Relaxed) > 0);
            if any {
                self.row_has_density[iy / 64] |= 1u64 << (iy % 64);
            }
        }
    }

    /// v2.0.3 Stream 2a: refresh the mip pyramid from the current density field.
    ///
    /// This is the `&mut self` wrapper that drives the pyramid refresh.
    /// `density`, `tile_active`, and `pyramid` are destructured as disjoint
    /// named-field borrows so the borrow checker accepts the split without
    /// any `unsafe`. The three fields are provably non-overlapping at the
    /// struct level; the explicit destructure is the idiomatic Rust proof.
    ///
    /// Callers (e.g. `World::step`) invoke this directly on `self.grass` — no
    /// raw-pointer gymnastics needed at the call site.
    pub fn refresh_pyramid(&mut self) {
        let tpa = self.tiles_per_axis;
        // Safe disjoint-field split: name the three fields we need from `self`
        // so the borrow checker sees they don't overlap.
        let GrassGrid {
            density,
            tile_active,
            pyramid,
            ..
        } = self;
        pyramid.refresh(density, tile_active, tpa);
    }

    // ─── v2.0.2 Stream 1a: u8 density accessors ────────────────────────────────
    //
    // The density store is `Vec<AtomicU8>`. These helpers are the ONLY way the
    // rest of the codebase touches a density cell: every reader does
    // `dget` (load Relaxed → decode to f32), every writer does `dset`
    // (encode → store Relaxed). Read-modify-write sites (e.g. `consume`) load,
    // compute in f32, then store — a plain RMW, no CAS (concurrent collisions
    // are accepted noise per the hub decisions).

    /// Load cell `idx` and decode it into the f32 grass-amount domain
    /// `[0, GRASS_MAX]`. Relaxed atomic load.
    #[inline]
    pub fn dget(&self, idx: usize) -> f32 {
        decode_density(self.density[idx].load(Ordering::Relaxed))
    }

    /// Encode an f32 grass amount `[0, GRASS_MAX]` and store it into cell `idx`.
    /// Relaxed atomic store. Values outside the range are clamped by the encoder.
    #[inline]
    pub fn dset(&self, idx: usize, v: f32) {
        self.density[idx].store(encode_density(v), Ordering::Relaxed);
    }

    /// Raw u8 load of cell `idx` (Relaxed). For the snapshot copy-out, which
    /// wants the stored byte directly (no f32 round-trip).
    #[inline]
    #[allow(dead_code)] // public u8 accessor; used by tests + downstream streams.
    pub fn dget_u8(&self, idx: usize) -> u8 {
        self.density[idx].load(Ordering::Relaxed)
    }

    /// Copy the whole density field out as raw u8 bytes (Relaxed loads). Used by
    /// the snapshot path and by tests that compare two fields for equality
    /// (`Vec<AtomicU8>` is not `PartialEq`).
    #[allow(dead_code)] // public snapshot helper; used by tests + downstream streams.
    pub fn density_u8_snapshot(&self) -> Vec<u8> {
        self.density
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect()
    }

    /// Set every cell to the encoded `v`. The `Vec<AtomicU8>` analogue of the old
    /// `density.fill(x)`; used by callers/tests that reset the whole field.
    #[allow(dead_code)] // wholesale-reset helper; used by tests + downstream streams.
    pub fn fill_density(&self, v: f32) {
        let byte = encode_density(v);
        for b in self.density.iter() {
            b.store(byte, Ordering::Relaxed);
        }
    }

    /// Test whether row `iy` may contain any non-zero density cells.
    #[inline]
    pub fn row_nonempty(&self, iy: usize) -> bool {
        let w = iy / 64;
        if w >= self.row_has_density.len() {
            return false;
        }
        (self.row_has_density[w] >> (iy % 64)) & 1 == 1
    }

    // ─── v2.0.1 §2/§3 active-tile + dirty-tile helpers ─────────────────────────

    /// Tile index for a cell index (row-major over the tile grid).
    #[inline]
    fn tile_of_cell(&self, cell_idx: usize) -> usize {
        let dim = self.dims.grass_dim;
        let ix = cell_idx % dim;
        let iy = cell_idx / dim;
        let tx = ix / GRASS_TILE_SIZE;
        let ty = iy / GRASS_TILE_SIZE;
        ty * self.tiles_per_axis + tx
    }

    // ─── v2.0.2 Stream 1c: atomic tile-bitset helpers ──────────────────────────
    //
    // `tile_active`/`tile_active_next`/`tile_dirty` are `Vec<AtomicU64>`. Single-
    // threaded callers (resync, the sequential setup) use Relaxed load/store via
    // these helpers; the parallel scatter pass uses `bitset_set_atomic` (fetch_or)
    // so a worker can set a NEIGHBOUR tile's bit (cross-tile spread) without a race.

    /// Relaxed-load test of bit `i` in an atomic bitset.
    #[inline]
    fn bitset_get(set: &[AtomicU64], i: usize) -> bool {
        (set[i / 64].load(Ordering::Relaxed) >> (i % 64)) & 1 == 1
    }

    /// Set bit `i` via a non-atomic Relaxed read-modify-store. Only sound when the
    /// caller holds `&mut self` (no other thread touches the bitset) — used by the
    /// sequential setup/teardown paths, NOT inside the parallel scatter closure.
    #[inline]
    fn bitset_set(set: &[AtomicU64], i: usize) {
        let w = &set[i / 64];
        w.store(
            w.load(Ordering::Relaxed) | (1u64 << (i % 64)),
            Ordering::Relaxed,
        );
    }

    /// Atomically set bit `i` (`fetch_or`, Relaxed). Safe to call concurrently
    /// from multiple scatter workers — this is how a worker marks a cross-tile
    /// neighbour active/dirty for next tick without losing a concurrent set.
    #[inline]
    fn bitset_set_atomic(set: &[AtomicU64], i: usize) {
        set[i / 64].fetch_or(1u64 << (i % 64), Ordering::Relaxed);
    }

    /// Clear every word of an atomic bitset (Relaxed). `&mut self`-only callers.
    #[inline]
    fn bitset_clear_all(set: &[AtomicU64]) {
        for w in set.iter() {
            w.store(0, Ordering::Relaxed);
        }
    }

    /// Mark a tile (and its 8 neighbours, honoring wrap) active for the NEXT
    /// tick AND dirty for both snapshot slots. `set_active` chooses whether the
    /// activation lands in the current `tile_active` set (used at resync / graze,
    /// which must affect the upcoming pass) — propagation builds the next-set
    /// separately. Marking the 8 neighbours guarantees the frontier can advance
    /// one tile/tick and that a re-quantize covers any cell a change can reach.
    fn activate_tile_and_fringe(&mut self, tile: usize) {
        let tpa = self.tiles_per_axis;
        let wrap = self.dims.wrap_world;
        let tx = tile % tpa;
        let ty = tile / tpa;
        for dy in -1i64..=1 {
            let ny = ty as i64 + dy;
            let ny = if wrap {
                ny.rem_euclid(tpa as i64)
            } else if ny < 0 || ny >= tpa as i64 {
                continue;
            } else {
                ny
            };
            for dx in -1i64..=1 {
                let nx = tx as i64 + dx;
                let nx = if wrap {
                    nx.rem_euclid(tpa as i64)
                } else if nx < 0 || nx >= tpa as i64 {
                    continue;
                } else {
                    nx
                };
                let nt = ny as usize * tpa + nx as usize;
                Self::bitset_set(&self.tile_active, nt);
                Self::bitset_set(&self.tile_dirty[0], nt);
                Self::bitset_set(&self.tile_dirty[1], nt);
            }
        }
    }

    /// Mark the tile containing `cell_idx` active+dirty (with its fringe).
    /// Called from the graze/`consume` path so a grazed (drained) cell re-enters
    /// frontier processing and regrows, and so the snapshot re-quantizes it.
    #[inline]
    pub fn mark_tile_active(&mut self, cell_idx: usize) {
        let t = self.tile_of_cell(cell_idx);
        // A drained cell makes its tile no longer uniformly saturated/empty in the
        // general case; conservatively flag it MIXED so the next pass processes it
        // (the pass re-classifies it exactly afterward). This guarantees the grazed
        // patch regrows and is re-snapshotted regardless of its prior class.
        self.tile_class[t] = TILE_MIXED;
        self.activate_tile_and_fringe(t);
    }

    /// Rebuild the per-tile class + active set from the current `density`. Used at
    /// construction and whenever `density` is mutated wholesale outside propagation
    /// (tests, full-grass-on-init). Classifies every tile, then sets the active set
    /// to the union of MIXED tiles and any tile bordering a different-class tile —
    /// the same rule the per-tick pass maintains incrementally.
    pub fn resync_active_from_density(&mut self) {
        let tpa = self.tiles_per_axis;
        let ntiles = tpa * tpa;
        for t in 0..ntiles {
            self.tile_class[t] = self.classify_tile(t);
        }
        self.rebuild_active_from_class();
    }

    /// Classify tile `t` from the current `density`: `TILE_EMPTY` if every cell is
    /// ≤ EPS, `TILE_SATURATED` if every cell is ≥ cap−EPS, else `TILE_MIXED`.
    fn classify_tile(&self, t: usize) -> u8 {
        let dim = self.dims.grass_dim;
        let tpa = self.tiles_per_axis;
        let tx = t % tpa;
        let ty = t / tpa;
        let ix0 = tx * GRASS_TILE_SIZE;
        let iy0 = ty * GRASS_TILE_SIZE;
        let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
        let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
        let mut all_empty = true;
        let mut all_sat = true;
        for iy in iy0..iy1 {
            let row = iy * dim;
            for ix in ix0..ix1 {
                let c = row + ix;
                let v = self.dget(c);
                let cap = self.capacity[c];
                if v > GRASS_EQ_EPS {
                    all_empty = false;
                }
                if v < cap - GRASS_EQ_EPS {
                    all_sat = false;
                }
                if !all_empty && !all_sat {
                    return TILE_MIXED;
                }
            }
        }
        if all_empty {
            TILE_EMPTY
        } else if all_sat {
            TILE_SATURATED
        } else {
            TILE_MIXED
        }
    }

    /// Rebuild `tile_active` (and dirty both slots for every active tile) from the
    /// current `tile_class`: active = MIXED tiles ∪ any tile bordering a tile of a
    /// different class. A uniform tile bordering a uniform tile of the SAME class
    /// is true equilibrium and stays inactive.
    fn rebuild_active_from_class(&mut self) {
        Self::bitset_clear_all(&self.tile_active);
        Self::bitset_clear_all(&self.tile_dirty[0]);
        Self::bitset_clear_all(&self.tile_dirty[1]);
        let tpa = self.tiles_per_axis;
        let wrap = self.dims.wrap_world;
        let ntiles = tpa * tpa;
        for t in 0..ntiles {
            let active = self.tile_class[t] == TILE_MIXED || self.borders_different_class(t);
            if active {
                Self::bitset_set(&self.tile_active, t);
                Self::bitset_set(&self.tile_dirty[0], t);
                Self::bitset_set(&self.tile_dirty[1], t);
            }
        }
        let _ = wrap;
    }

    /// True if any 8-neighbour of tile `t` has a different `tile_class` (wrap-aware).
    fn borders_different_class(&self, t: usize) -> bool {
        let tpa = self.tiles_per_axis;
        let wrap = self.dims.wrap_world;
        let tx = (t % tpa) as i64;
        let ty = (t / tpa) as i64;
        let my = self.tile_class[t];
        for dy in -1i64..=1 {
            let ny = ty + dy;
            let ny = if wrap {
                ny.rem_euclid(tpa as i64)
            } else if ny < 0 || ny >= tpa as i64 {
                continue;
            } else {
                ny
            };
            for dx in -1i64..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = tx + dx;
                let nx = if wrap {
                    nx.rem_euclid(tpa as i64)
                } else if nx < 0 || nx >= tpa as i64 {
                    continue;
                } else {
                    nx
                };
                if self.tile_class[ny as usize * tpa + nx as usize] != my {
                    return true;
                }
            }
        }
        false
    }

    /// One propagation tick. Reads `self.density`, writes `self.scratch`, swaps.
    ///
    /// v2.0.1 §1: organic spread via a TRUE two-pass SEPARABLE `[1,2,1]/4`-per-axis
    /// blur of the prior frame — a horizontal pass ([`h_at`]) feeding a vertical
    /// pass ([`blur_at`]); V∘H is the 3×3 `[[1,2,1],[2,4,2],[1,2,1]]/16` Gaussian.
    /// The max-of-neighbours spill is then taken over the BLURRED field's 8
    /// neighbours (not just the 4 cardinals — the cardinal-only max is an L1 reach
    /// = diamond), so a seed expands as a disc while keeping the "one ripe neighbour
    /// grants the boost, neighbours don't stack" anti-cascade behaviour (it stays a
    /// max, not a sum).
    ///
    /// Per cell c with `K = cap[c]` (ghost-zero / wrap boundary on each tap):
    /// ```text
    /// h[x]      = 0.5*d[x] + 0.25*(d[x-1] + d[x+1])     (PASS H)
    /// blur[x]   = 0.5*h[x] + 0.25*(h[x-dim] + h[x+dim]) (PASS V)
    /// v         = d[c]
    /// logistic  = r * v * (1 - v / K)
    /// prop      = k * max(blur over the 8 neighbours of c)
    /// d'[c]     = clamp(v + logistic + prop, 0, K)
    /// ```
    /// Out-of-bounds neighbours are zero (walled) / opposite edge (wrap). Empty
    /// cells with an empty neighbourhood stay zero (blur of zero is zero).
    ///
    /// Production callers in `World::tick_once` now invoke the split
    /// `compute_propagation` + `rebuild_row_bitset` directly so the profiler
    /// can break the two phases apart; this convenience wrapper survives for
    /// the grass-only unit tests in this module.
    #[cfg(test)]
    pub fn step(&mut self, r_in_cell: f32, k_propagate: f32) {
        // Test convenience: existing unit tests poke `density` directly between
        // steps, so resync the per-tile active/class state from the current field
        // before each step. Production callers drive the incremental active-set
        // path via `compute_propagation` directly (and graze's `mark_tile_active`).
        self.resync_active_from_density();
        self.compute_propagation(r_in_cell, k_propagate);
        self.rebuild_row_bitset();
    }

    /// Reference full-grid propagation: process EVERY cell (no tile skipping).
    /// Used by the active-tile equivalence test as the ground truth. Same per-cell
    /// math as `compute_propagation`; reads `self.density`, writes `self.scratch`,
    /// swaps. Not used in production (the tile path is the fast equivalent).
    #[cfg(test)]
    pub fn compute_propagation_full(&mut self, r_in_cell: f32, k_propagate: f32) {
        let dim = self.dims.grass_dim;
        let wrap = self.dims.wrap_world;
        let d = &self.density;
        let cap = &self.capacity;
        for iy in 0..dim {
            for ix in 0..dim {
                let c = iy * dim + ix;
                self.scratch[c] =
                    grass_cell_next(d, cap, dim, wrap, r_in_cell, k_propagate, ix, iy);
            }
        }
        // v2.0.2 Stream 1a: density is `AtomicU8` and scratch is `Vec<f32>`, so
        // we can no longer swap buffers — encode the fresh f32 frame back into
        // the atomic store (same quantization as the production copyback).
        for (c, dst) in self.density.iter().enumerate() {
            dst.store(encode_density(self.scratch[c]), Ordering::Relaxed);
        }
    }

    /// Phase 1 of `step()`: write the next density frame into `scratch` and
    /// swap. v2.0.1 §2: processes ONLY active (frontier + fringe + grazed) tiles;
    /// equilibrium tiles are an identity copy (`d`→`scratch`), which is exact
    /// because an EMPTY tile next to non-empty grass is kept active by the fringe
    /// rule and a SATURATED tile next to a frontier tile likewise — so any tile
    /// that is skipped has an unchanging value this tick. Builds the next-tick
    /// active set from the post-pass frontier + 8-neighbour fringe, and marks
    /// every changed tile dirty for both snapshot slots.
    pub fn compute_propagation(&mut self, r_in_cell: f32, k_propagate: f32) {
        // v2.0.2 Stream 1b: select the live SCATTER kernel or the retained BLUR
        // kernel. Default at the low-level grid ctor is `Blur` (so existing blur
        // tests keep passing); World/wasm sets `Scatter` for the live sim. The two
        // share the same active-tile frontier bookkeeping (built/torn-down here in
        // each branch); only the per-cell math differs.
        match self.propagation {
            GrassPropagation::Blur => self.compute_propagation_blur(r_in_cell, k_propagate),
            GrassPropagation::Scatter => self.compute_propagation_scatter(),
        }
    }

    /// v2.0.1 BLUR propagation (RETAINED behind the selector — Stage 3 deletes it
    /// after the scatter bench). Bit-identical to the pre-1b body. See the struct
    /// doc above for the per-cell math.
    fn compute_propagation_blur(&mut self, r_in_cell: f32, k_propagate: f32) {
        debug_assert_eq!(self.density.len(), self.dims.grass_cell_count);
        debug_assert_eq!(self.scratch.len(), self.dims.grass_cell_count);

        self.par_chunks_us.store(0, Ordering::Relaxed);
        self.chunks_mut_us.store(0, Ordering::Relaxed);
        self.row_body_us.store(0, Ordering::Relaxed);
        self.row_body_self_us.store(0, Ordering::Relaxed);
        self.row_body_calls.store(0, Ordering::Relaxed);
        self.dispatch_calls.store(1, Ordering::Relaxed);

        let dim = self.dims.grass_dim;
        let wrap = self.dims.wrap_world;
        let tpa = self.tiles_per_axis;
        let ntiles = tpa * tpa;

        // Build the dense active-tile list (sorted ascending → deterministic
        // dispatch order, so single-thread == rayon for a fixed seed).
        let mut active_list: Vec<u32> = Vec::with_capacity(ntiles);
        for t in 0..ntiles {
            if Self::bitset_get(&self.tile_active, t) {
                active_list.push(t as u32);
            }
        }

        // Per-active-tile post-pass class (EMPTY/SATURATED/MIXED). Index parallel
        // to `active_list`; written back into the persistent `tile_class` after.
        let mut new_class: Vec<u8> = vec![TILE_EMPTY; active_list.len()];

        let d = &self.density;
        let cap = &self.capacity;
        let row_body_self_us = &self.row_body_self_us;
        let row_body_calls = &self.row_body_calls;

        // Process one active tile: compute its cells into `scratch` and return the
        // tile's post-pass class. Writes are confined to the tile's own cells;
        // reads (the two-pass blur sampled at c's 8 neighbours) reach a 2-cell halo
        // into the FROZEN prior frame `d`, so tiles are independent → single-thread
        // == rayon bit-identical, and identical to a full-grid two-pass.
        let tile_body = |t: usize, scratch: &mut [f32]| -> u8 {
            let body_self_start = clock_now_us_threadsafe();
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
            let mut all_empty = true;
            let mut all_sat = true;
            for iy in iy0..iy1 {
                let row = iy * dim;
                for ix in ix0..ix1 {
                    let c = row + ix;
                    let nv = grass_cell_next(d, cap, dim, wrap, r_in_cell, k_propagate, ix, iy);
                    scratch[c] = nv;
                    let k_cap = cap[c];
                    if nv > GRASS_EQ_EPS {
                        all_empty = false;
                    }
                    if nv < k_cap - GRASS_EQ_EPS {
                        all_sat = false;
                    }
                }
            }
            row_body_self_us.fetch_add(
                clock_now_us_threadsafe().saturating_sub(body_self_start),
                Ordering::Relaxed,
            );
            row_body_calls.fetch_add(1, Ordering::Relaxed);
            if all_empty {
                TILE_EMPTY
            } else if all_sat {
                TILE_SATURATED
            } else {
                TILE_MIXED
            }
        };

        // Compute active tiles into scratch. The whole `scratch` slice is passed
        // (each tile writes only its own cells; no two active tiles overlap).
        #[cfg(feature = "threads")]
        {
            let par_start = clock_now_us_threadsafe();
            let row_body_us = &self.row_body_us;
            let scratch_ptr = ScratchPtr(self.scratch.as_mut_ptr());
            // SAFETY: active tiles are disjoint cell ranges; each closure only
            // writes its own tile's cells. Reads of `d` are immutable & shared.
            active_list
                .par_iter()
                .zip(new_class.par_iter_mut())
                .for_each(|(&t, cls)| {
                    // Capture the whole wrapper (Send+Sync) by copy, not the inner
                    // `*mut f32` field (which a precise closure capture would grab
                    // as `&*mut f32`, defeating the Send/Sync impl).
                    let sp = scratch_ptr;
                    let body_start = clock_now_us_threadsafe();
                    let scratch: &mut [f32] =
                        unsafe { std::slice::from_raw_parts_mut(sp.0, dim * dim) };
                    *cls = tile_body(t as usize, scratch);
                    let body_end = clock_now_us_threadsafe();
                    row_body_us.fetch_add(body_end.saturating_sub(body_start), Ordering::Relaxed);
                });
            let par_end = clock_now_us_threadsafe();
            self.par_chunks_us
                .store(par_end.saturating_sub(par_start), Ordering::Relaxed);
        }
        #[cfg(not(feature = "threads"))]
        {
            let seq_start = clock_now_us_threadsafe();
            let body_start = clock_now_us_threadsafe();
            let scratch_ptr = ScratchPtr(self.scratch.as_mut_ptr());
            for (i, &t) in active_list.iter().enumerate() {
                let scratch: &mut [f32] =
                    unsafe { std::slice::from_raw_parts_mut(scratch_ptr.0, dim * dim) };
                new_class[i] = tile_body(t as usize, scratch);
            }
            let body_end = clock_now_us_threadsafe();
            self.row_body_us
                .store(body_end.saturating_sub(body_start), Ordering::Relaxed);
            let seq_end = clock_now_us_threadsafe();
            self.chunks_mut_us
                .store(seq_end.saturating_sub(seq_start), Ordering::Relaxed);
        }

        // Copy ONLY active tiles' fresh values from `scratch` back into `density`
        // (in place — NO full-grid swap). Skipped equilibrium tiles keep their
        // existing `density` value untouched (identity), so total memory traffic
        // is O(active cells), not O(all cells) — this is what makes the snapshot
        // AND propagation cheap when the frontier is small.
        for &t in active_list.iter() {
            let t = t as usize;
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
            for iy in iy0..iy1 {
                let row = iy * dim;
                // v2.0.2 Stream 1a: scratch is f32, density is AtomicU8 — encode
                // each fresh cell back into the atomic store (Relaxed). Same
                // quantization scale as the snapshot, so the copyback is lossless
                // relative to what the snapshot would have produced anyway.
                for c in (row + ix0)..(row + ix1) {
                    self.density[c].store(encode_density(self.scratch[c]), Ordering::Relaxed);
                }
            }
        }

        // Persist the post-pass class of every processed tile, and mark it dirty
        // for both snapshot slots (its cells were recomputed). Skipped tiles kept
        // their density (identity copy) so their class + bytes are unchanged.
        for (i, &t) in active_list.iter().enumerate() {
            let t = t as usize;
            self.tile_class[t] = new_class[i];
            Self::bitset_set(&self.tile_dirty[0], t);
            Self::bitset_set(&self.tile_dirty[1], t);
        }

        // Build the NEXT-tick active set: a tile must be processed next tick iff it
        // is MIXED, or it borders a tile of a DIFFERENT class (a class boundary is
        // a moving front — e.g. saturated→empty spreads, empty→mixed advances).
        // O(tiles) over the persistent class map; far cheaper than O(cells).
        Self::bitset_clear_all(&self.tile_active_next);
        for t in 0..ntiles {
            if self.tile_class[t] == TILE_MIXED || self.borders_different_class(t) {
                Self::bitset_set(&self.tile_active_next, t);
            }
        }
        std::mem::swap(&mut self.tile_active, &mut self.tile_active_next);
    }

    /// v2.0.2 Stream 1b/1c: the LIVE stochastic u8 SCATTER propagation kernel.
    ///
    /// Per tick, over the ACTIVE tile set (rayon-parallel), each worker:
    ///   1. **Freezes its tile's source** — snapshots its 1024 source bytes into a
    ///      transient local buffer. All decisions (am I a source? roll spread/decay)
    ///      read this frozen copy → no intra-tile cascade, order-independent within
    ///      the tile (cross-tile seam skew accepted, per the plan).
    ///   2. For each cell that had grass in the snapshot, using the per-cell hash
    ///      RNG `(world_seed, cell, tick, salt)` (never `SimRng`):
    ///      - **decay roll** (prob `decay_pct`): lossy relaxed RMW subtract on the
    ///        global cell, floor 0;
    ///      - **spread roll** (prob `spread_pct`): pick ONE disc-table target
    ///        (radial band by ring weights, uniform angle → round footprint), lossy
    ///        relaxed RMW add on the global target, clamp to its biome-cap byte.
    ///   3. **Frontier (1c):** any tile a write touched — this tile or a cross-tile
    ///      neighbour — is marked active+dirty for next tick via atomic `fetch_or`,
    ///      so the frontier tracks where there is work. A tile with grass also
    ///      marks its 8-neighbour fringe active (so empty-but-adjacent tiles can
    ///      receive next tick). Empty-and-isolated tiles are the only skippable class.
    ///
    /// No blur, no f32 double-buffer; writes are lossy relaxed `AtomicU8` RMW
    /// (cross-tile legal, collisions clobber wholesale — accepted noise). NOT
    /// bit-reproducible across thread counts; the per-cell *draw* is reproducible
    /// (grass never perturbs `SimRng`). Coherence holds: grass advances at tick end.
    fn compute_propagation_scatter(&mut self) {
        debug_assert_eq!(self.density.len(), self.dims.grass_cell_count);

        self.par_chunks_us.store(0, Ordering::Relaxed);
        self.chunks_mut_us.store(0, Ordering::Relaxed);
        self.row_body_us.store(0, Ordering::Relaxed);
        self.row_body_self_us.store(0, Ordering::Relaxed);
        self.row_body_calls.store(0, Ordering::Relaxed);
        self.dispatch_calls.store(1, Ordering::Relaxed);

        let dim = self.dims.grass_dim;
        let wrap = self.dims.wrap_world;
        let tpa = self.tiles_per_axis;
        let ntiles = tpa * tpa;
        let world_seed = self.world_seed;
        let tick = self.scatter_tick;

        // Scatter kernel parameters — read from the live ScatterParams fields
        // (wired from DevSliders via WorldHandle apply_* setters). Stream 1d
        // landed these; defaults mirror the old working constants so behavior
        // is byte-identical when no slider has been changed.
        let decay_pct = self.scatter_params.decay_pct;
        let spread_pct = self.scatter_params.spread_pct;
        // Stage-3 fix (zero-amount gate): do NOT apply .max(1) here. A slider at
        // 0.0 encodes to byte 0; the tile_body gates the roll when decay_byte==0
        // or spread_byte==0 so a zero-amount setting fires NO effect. The .max(1)
        // floor is applied inside tile_body only when the effect is actually active
        // (byte > 0 path), preserving the live default behavior byte-for-byte.
        let decay_byte = encode_density(self.scatter_params.decay_amount);
        let spread_byte = encode_density(self.scatter_params.spread_amount);

        // Build the dense active-tile list (ascending → deterministic dispatch).
        let mut active_list: Vec<u32> = Vec::with_capacity(ntiles);
        for t in 0..ntiles {
            if Self::bitset_get(&self.tile_active, t) {
                active_list.push(t as u32);
            }
        }

        // The NEXT-tick active set is rebuilt fresh: workers `fetch_or` into it
        // (including cross-tile). Both dirty slots are LEFT AS-IS (the snapshot
        // consumes/clears them); workers `fetch_or` the tiles they changed.
        Self::bitset_clear_all(&self.tile_active_next);

        let d = &self.density;
        let cap = &self.capacity;
        let disc = &self.disc;
        let tile_active_next = &self.tile_active_next;
        let tile_dirty0 = &self.tile_dirty[0];
        let tile_dirty1 = &self.tile_dirty[1];
        let row_body_self_us = &self.row_body_self_us;
        let row_body_calls = &self.row_body_calls;

        // C2: snapshot the profile-on flag once outside the hot loop so the
        // clock_now_us_threadsafe() calls (and the atomic fetch_adds they feed)
        // are completely skipped on the common profiling-off path. The counters
        // are always reset to zero above, so they remain valid (all-zero) when
        // profiling is off — the World drain reads them and records 0µs, which
        // the Profiler no-ops via its own enabled() guard at record_under_root.
        // GrassGrid doesn't hold the Profiler directly; World sets this flag
        // before each compute_propagation call via `set_profile_scatter`.
        let profile_on = self.profile_scatter;

        // Process one active tile. Returns nothing; all effects are atomic RMWs on
        // the global density field + atomic set-bits on the frontier/dirty bitsets.
        let tile_body = |t: usize| {
            // C2: only clock at tile entry when profiling is on.
            let body_self_start = if profile_on {
                clock_now_us_threadsafe()
            } else {
                0
            };
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);

            // (1) C1: FREEZE the tile's source bytes into a STACK buffer (no heap
            // alloc) so spread/decay decisions read a stable snapshot (no
            // intra-tile cascade). GRASS_TILE_SIZE×GRASS_TILE_SIZE = 1024 bytes;
            // edge tiles only fill the tw×th prefix but the full array fits on the
            // stack cheaply. Zero-init ensures unused slots (s==0) are skipped.
            let tw = ix1 - ix0;
            let th = iy1 - iy0;
            let mut src = [0u8; GRASS_TILE_SIZE * GRASS_TILE_SIZE];
            for ly in 0..th {
                let iy = iy0 + ly;
                let row = iy * dim;
                for lx in 0..tw {
                    src[ly * GRASS_TILE_SIZE + lx] = d[row + ix0 + lx].load(Ordering::Relaxed);
                }
            }

            let mut tile_touched = false; // any write into THIS tile this tick
            let mut tile_has_grass = false; // any source cell had grass (frozen)

            // (2) Roll spread/decay for each frozen source cell that has grass.
            for ly in 0..th {
                let iy = iy0 + ly;
                for lx in 0..tw {
                    let ix = ix0 + lx;
                    let s = src[ly * GRASS_TILE_SIZE + lx];
                    if s == 0 {
                        continue; // no spontaneous spawn — dead cell stays a source-skip
                    }
                    tile_has_grass = true;
                    let c = iy * dim + ix;
                    let cid = c as u64;

                    // C3: fuse all four RNG draws into two hash calls (salts 1+2)
                    // then bit-slice each 64-bit word into two 24-bit uniforms.
                    // Behavior contract:
                    //   decay_u  ← same precision/distribution as grass_unit(*,1)
                    //   spread_u ← same precision/distribution as grass_unit(*,2)
                    //   band_u   ← same precision/distribution as grass_unit(*,3)
                    //   pick_u   ← same precision/distribution as grass_unit(*,4)
                    // The VALUES differ from the 4-hash path (different bit windows
                    // within each word) — this is accepted per the C3 spec. The
                    // STATISTICS (rate, fraction, radial distribution) are unchanged.
                    let (decay_u, spread_u, band_u, pick_u) =
                        grass_hash_fused_4(world_seed, cid, tick);

                    // DECAY roll.
                    // Stage-3 fix: gate on decay_byte > 0 so a zero-amount slider
                    // fires NO effect; floor to >=1 only when the effect is active.
                    if decay_byte > 0
                        && decay_u < decay_pct
                        && scatter_sub(&d[c], decay_byte.max(1))
                    {
                        tile_touched = true;
                    }

                    // SPREAD roll (gate + band + pick all from fused draws).
                    // Stage-3 fix: gate on spread_byte > 0 so a zero-amount slider
                    // fires NO effect; floor to >=1 only when the effect is active.
                    //
                    // S4 variant A — amount ∝ source density:
                    //   add_byte = spread_byte * (s / 255)
                    // Denser source cells push harder → self-reinforcing patch
                    // centres → tighter clumps. The spread PROBABILITY `spread_pct`
                    // is unchanged (only the add amount scales). Computed in integer
                    // arithmetic: `(spread_byte as u32 * s as u32 + 127) / 255`,
                    // then floored to ≥1 (same as the old flat path) so a cell with
                    // even one byte of grass still contributes a minimal add when
                    // the effect fires.
                    if spread_byte > 0 && spread_u < spread_pct {
                        if let Some((dx, dy)) = disc.sample(band_u, pick_u) {
                            // Resolve the target cell (wrap or walled bounds).
                            let txi = ix as i64 + dx as i64;
                            let tyi = iy as i64 + dy as i64;
                            let dimi = dim as i64;
                            let (gx, gy) = if wrap {
                                (txi.rem_euclid(dimi), tyi.rem_euclid(dimi))
                            } else if txi < 0 || txi >= dimi || tyi < 0 || tyi >= dimi {
                                (-1, -1)
                            } else {
                                (txi, tyi)
                            };
                            if gx >= 0 {
                                let tcell = gy as usize * dim + gx as usize;
                                let cap_byte = encode_density(cap[tcell]);
                                // S4: density-scaled add (variant A). Scales the add
                                // amount by the source cell's byte fraction (s/255).
                                // Floor to 1 once the effect is active (same as
                                // the old path) so very sparse cells still spread.
                                let density_add_byte =
                                    ((spread_byte as u32 * s as u32 + 127) / 255).max(1) as u8;
                                if cap_byte > 0
                                    && scatter_add(&d[tcell], density_add_byte, cap_byte)
                                {
                                    // Target may be in a NEIGHBOUR tile: activate +
                                    // dirty it atomically (cross-tile safe).
                                    let tt = (gy as usize / GRASS_TILE_SIZE) * tpa
                                        + (gx as usize / GRASS_TILE_SIZE);
                                    if tt == t {
                                        tile_touched = true;
                                    } else {
                                        Self::bitset_set_atomic(tile_active_next, tt);
                                        Self::bitset_set_atomic(tile_dirty0, tt);
                                        Self::bitset_set_atomic(tile_dirty1, tt);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // (3) FRONTIER: this tile stays active next tick if it had grass (decay
            // means even a saturated tile keeps developing gaps → never skippable)
            // OR it was written into; and a tile with grass marks its 8-neighbour
            // fringe so empty-but-adjacent tiles can receive next tick. Empty-and-
            // isolated tiles are the only skippable class.
            if tile_touched || tile_has_grass {
                Self::bitset_set_atomic(tile_active_next, t);
                Self::bitset_set_atomic(tile_dirty0, t);
                Self::bitset_set_atomic(tile_dirty1, t);
            }
            if tile_has_grass {
                for ddy in -1i64..=1 {
                    let ny = ty as i64 + ddy;
                    let ny = if wrap {
                        ny.rem_euclid(tpa as i64)
                    } else if ny < 0 || ny >= tpa as i64 {
                        continue;
                    } else {
                        ny
                    };
                    for ddx in -1i64..=1 {
                        let nx = tx as i64 + ddx;
                        let nx = if wrap {
                            nx.rem_euclid(tpa as i64)
                        } else if nx < 0 || nx >= tpa as i64 {
                            continue;
                        } else {
                            nx
                        };
                        Self::bitset_set_atomic(tile_active_next, ny as usize * tpa + nx as usize);
                    }
                }
            }

            // C2: only accumulate timers when profiling is on.
            if profile_on {
                row_body_self_us.fetch_add(
                    clock_now_us_threadsafe().saturating_sub(body_self_start),
                    Ordering::Relaxed,
                );
                row_body_calls.fetch_add(1, Ordering::Relaxed);
            }
        };

        #[cfg(feature = "threads")]
        {
            // C2: only clock the outer par dispatch when profiling is on.
            let par_start = if profile_on {
                clock_now_us_threadsafe()
            } else {
                0
            };
            let row_body_us = &self.row_body_us;
            active_list.par_iter().for_each(|&t| {
                // C2: only clock each tile body when profiling is on.
                let body_start = if profile_on {
                    clock_now_us_threadsafe()
                } else {
                    0
                };
                tile_body(t as usize);
                if profile_on {
                    row_body_us.fetch_add(
                        clock_now_us_threadsafe().saturating_sub(body_start),
                        Ordering::Relaxed,
                    );
                }
            });
            if profile_on {
                self.par_chunks_us.store(
                    clock_now_us_threadsafe().saturating_sub(par_start),
                    Ordering::Relaxed,
                );
            }
        }
        #[cfg(not(feature = "threads"))]
        {
            // C2: only clock the sequential dispatch when profiling is on.
            let seq_start = if profile_on {
                clock_now_us_threadsafe()
            } else {
                0
            };
            let body_start = if profile_on {
                clock_now_us_threadsafe()
            } else {
                0
            };
            for &t in active_list.iter() {
                tile_body(t as usize);
            }
            if profile_on {
                self.row_body_us.store(
                    clock_now_us_threadsafe().saturating_sub(body_start),
                    Ordering::Relaxed,
                );
                self.chunks_mut_us.store(
                    clock_now_us_threadsafe().saturating_sub(seq_start),
                    Ordering::Relaxed,
                );
            }
        }

        // Swap the freshly-built next-tick active set into place. `tile_class` is
        // not used by the scatter frontier (it tracks grass-presence, not the
        // blur's equilibrium classes), but is kept coherent enough for graze's
        // `mark_tile_active` (which sets TILE_MIXED) by leaving it untouched here.
        std::mem::swap(&mut self.tile_active, &mut self.tile_active_next);
    }

    /// v2.0.1 §3: quantize ONLY the tiles dirty for ping-pong snapshot `slot`
    /// into `dst` (the slot's `grass_cell_count`-byte u8 grass region), then clear
    /// those tiles' dirty bits for that slot. Each cell becomes
    /// `round(clamp(density/GRASS_MAX, 0, 1) * 255)` — the same value the old
    /// full-grid `quantize_grass_into` produced, so the SAB wire bytes are
    /// unchanged. Unchanged tiles are left as-is in `dst` (the slot persists
    /// across writes, so it already holds valid prior bytes for them).
    ///
    /// COORDINATION (v2.0.1 writer wave): this is the single self-contained
    /// callable the upcoming snapshot `pack()` invokes — pass it the destination
    /// slot's grass region and the slot index and it writes only dirty tiles. Do
    /// not inline it; `write_snapshot` calls it today and `pack()` will tomorrow.
    ///
    /// `dst.len()` must equal `grass_cell_count`. `slot` is masked to 0/1.
    pub fn quantize_dirty_tiles_into(&mut self, dst: &mut [u8], slot: usize) {
        debug_assert_eq!(dst.len(), self.dims.grass_cell_count);
        let slot = slot & 1;
        let dim = self.dims.grass_dim;
        let tpa = self.tiles_per_axis;
        let ntiles = tpa * tpa;
        for t in 0..ntiles {
            if !Self::bitset_get(&self.tile_dirty[slot], t) {
                continue;
            }
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
            for iy in iy0..iy1 {
                let row = iy * dim;
                for ix in ix0..ix1 {
                    let c = row + ix;
                    // v2.0.2 Stream 1a: density is now stored on the SAME u8 scale
                    // the snapshot used, so quantizing is a direct byte copy
                    // (Relaxed load) — identical bytes to the old f32→u8 path.
                    dst[c] = self.density[c].load(Ordering::Relaxed);
                }
            }
            // Clear this slot's dirty bit — the slot now holds current bytes for t.
            // v2.0.2 Stream 1c: tile_dirty is AtomicU64; clear via Relaxed RMW
            // (single-threaded snapshot path, no concurrent writers here).
            let w = &self.tile_dirty[slot][t / 64];
            w.store(
                w.load(Ordering::Relaxed) & !(1u64 << (t % 64)),
                Ordering::Relaxed,
            );
        }
    }

    /// v2.0.1 §3: mark EVERY tile dirty for both snapshot slots. Used after a
    /// wholesale `density` mutation outside propagation (full-grass-on-init,
    /// tests) so the next quantize refreshes the entire grass region.
    pub fn mark_all_tiles_dirty(&mut self) {
        for slot in 0..2 {
            for w in self.tile_dirty[slot].iter() {
                w.store(!0, Ordering::Relaxed);
            }
        }
    }

    /// v2.0.2 Stream 1b: select the propagation kernel (live SCATTER vs retained
    /// BLUR). World/wasm calls this once after construction to put the live grid on
    /// the scatter path; grid-direct tests leave it at the `Blur` default.
    #[inline]
    pub fn set_propagation(&mut self, mode: GrassPropagation) {
        self.propagation = mode;
    }

    /// v2.0.2 Stream 1d: apply updated scatter params from DevSliders. Updates the
    /// six live fields and, if the ring weights changed, rebuilds the disc table
    /// (the ring sliders feed the cumulative weights used to pick a radial band).
    /// Other params (pct/amount) take effect on the next tick's kernel read with no
    /// extra work. Called by WorldHandle apply_* setters after writing sliders.
    pub fn apply_scatter_params(&mut self, params: ScatterParams) {
        let ring_changed = (params.ring1_pct - self.scatter_params.ring1_pct).abs() > 1e-7
            || (params.ring2_pct - self.scatter_params.ring2_pct).abs() > 1e-7;
        self.scatter_params = params;
        if ring_changed {
            // Ring3 is implicitly (1 - ring1 - ring2) after disc-table normalization.
            let ring3 = 1.0_f32 - params.ring1_pct - params.ring2_pct;
            self.disc = DiscTable::build(
                GRASS_SPREAD_RADIUS,
                params.ring1_pct,
                params.ring2_pct,
                ring3.max(0.0),
            );
        }
    }

    /// v2.0.4 C2: enable or disable profiling clock calls inside the scatter
    /// kernel. Call with `Profiler::enabled()` before each `compute_propagation`
    /// to gate the per-tile `clock_now_us_threadsafe()` + atomic `fetch_add`s.
    /// When `false` (default), all timing atomics stay at the zero written by the
    /// reset at the top of `compute_propagation_scatter`; the World drain then
    /// records 0µs, which `Profiler::record_under_root` silently drops when the
    /// profiler is disabled anyway.
    #[inline]
    pub fn set_profile_scatter(&mut self, on: bool) {
        self.profile_scatter = on;
    }

    /// Bilinear sample at continuous world position `(x, y)`.
    ///
    /// Walled: clamps input to `[0, world_size)`; out-of-bounds cell indices read
    /// as ghost zero. Toroidal (`dims.wrap_world`): the world position wraps into
    /// `[0, world_size)` and the four sample cells wrap across the seam.
    pub fn bilinear_sample(&self, x: f32, y: f32) -> f32 {
        let w = self.dims.world_size;
        let dim = self.dims.grass_dim as i32;
        let wrap = self.dims.wrap_world;

        let (xw, yw) = if wrap {
            (x.rem_euclid(w), y.rem_euclid(w))
        } else {
            (x.clamp(0.0, w - 1e-4), y.clamp(0.0, w - 1e-4))
        };

        // Convert to fractional cell coordinates. Cell i's center is at
        // (i + 0.5) * cell_size. Subtract 0.5 so cell centers fall at integers.
        // v2.0.4 S2: use the construction-time cell size from dims.
        let cs = self.dims.grass_cell_size;
        let fx = xw / cs - 0.5;
        let fy = yw / cs - 0.5;

        let ix0 = fx.floor() as i32;
        let iy0 = fy.floor() as i32;
        let ix1 = ix0 + 1;
        let iy1 = iy0 + 1;

        let tx = fx - fx.floor(); // in [0, 1)
        let ty = fy - fy.floor();

        let dimu = self.dims.grass_dim;
        // Walled → ghost-zero out-of-bounds; wrap → fold indices into [0, dim).
        let sample = |iy: i32, ix: i32| -> f32 {
            if wrap {
                let wy = iy.rem_euclid(dim) as usize;
                let wx = ix.rem_euclid(dim) as usize;
                self.dget(wy * dimu + wx)
            } else {
                if iy < 0 || iy >= dim || ix < 0 || ix >= dim {
                    return 0.0;
                }
                self.dget(iy as usize * dimu + ix as usize)
            }
        };

        let d00 = sample(iy0, ix0);
        let d10 = sample(iy0, ix1);
        let d01 = sample(iy1, ix0);
        let d11 = sample(iy1, ix1);

        let a = d00 * (1.0 - tx) + d10 * tx;
        let b = d01 * (1.0 - tx) + d11 * tx;
        a * (1.0 - ty) + b * ty
    }

    /// Partial-bite consume. Drains `min(cur_byte, chunk_bytes)` u8 bytes and
    /// awards `energy_per_bite * taken_bytes / chunk_bytes` energy (proportional
    /// to what was actually consumed). A ripe cell (density ≥ chunk) yields the
    /// full `energy_per_bite`; an under-threshold cell yields a fraction.
    /// Returns `0.0` only when the cell is fully empty.
    ///
    /// v2.0.2 Stream 1e: energy reward is computed from actual u8 bytes removed,
    /// mirroring the scatter kernel's pattern (`scatter_sub`). `chunk_bytes` is
    /// clamped to `.max(1)` so a bite ALWAYS removes at least 1 byte and yields
    /// non-zero energy whenever grass is present, regardless of how small
    /// `density_chunk` is (guards the u8 floor). This keeps energy correctly
    /// quantized to 256 levels over `[0, GRASS_MAX]` and is equivalent to the
    /// prior f32 formula at the u8 precision the store provides.
    #[inline]
    pub fn consume(&mut self, cell_idx: usize, density_chunk: f32, energy_per_bite: f32) -> f32 {
        let cur_byte = self.dget_u8(cell_idx);
        if cur_byte == 0 || density_chunk <= 0.0 {
            return 0.0;
        }
        // Encode density_chunk to u8; clamp to at least 1 byte so a bite always
        // removes something (mirrors scatter_sub's `.max(1)` guard).
        let chunk_bytes = encode_density(density_chunk).max(1);
        let taken_bytes = cur_byte.min(chunk_bytes);
        // Write back reduced density. `saturating_sub` is safe here: taken_bytes
        // ≤ cur_byte, so the result is always ≥ 0.
        self.density[cell_idx].store(cur_byte - taken_bytes, Ordering::Relaxed);
        // v2.0.1 §2/§3: a drained cell may turn a SATURATED tile into a frontier
        // tile and changes a snapshot byte — reactivate it + its fringe and mark
        // it dirty so it re-enters propagation, regrows, and is re-quantized.
        self.mark_tile_active(cell_idx);
        energy_per_bite * (taken_bytes as f32 / chunk_bytes as f32)
    }

    /// Iterate every cell whose center is within Euclidean distance `r` of `(cx, cy)`.
    ///
    /// Walled: out-of-bounds cells are skipped (no toroidal wrap).
    /// Invokes `f(cell_idx)` once per hit. Used by P1e to find graze-overlap cells.
    /// Deterministic: row-major, left-to-right iteration order.
    /// Contract: `r` must be finite and non-negative.
    ///
    /// Uses circle-vs-AABB overlap (nearest point on the cell bounding box to the
    /// circle centre must be ≤ r). This is the correct test for "does a creature's
    /// body circle touch this grass cell" and avoids the old centre-to-centre test
    /// that could miss cells when `GRASS_CELL_SIZE >> r` (M1 regression fix).
    pub fn for_each_cell_overlapping_circle(
        &self,
        cx: f32,
        cy: f32,
        r: f32,
        mut f: impl FnMut(usize),
    ) {
        debug_assert!(r.is_finite() && r >= 0.0);

        // Search radius expanded by half cell size so the candidate window includes
        // every cell whose bounding box could intersect the circle.
        // v2.0.4 S2: use the construction-time cell size from dims.
        let cs = self.dims.grass_cell_size;
        let search_r = r + cs * 0.5;
        let r_cells = (search_r / cs).ceil() as i32 + 1;
        let cx_cell = (cx / cs).floor() as i32;
        let cy_cell = (cy / cs).floor() as i32;
        let dim = self.dims.grass_dim as i32;
        let dimu = self.dims.grass_dim;
        let wrap = self.dims.wrap_world;
        let r2 = r * r;

        for jy in (cy_cell - r_cells)..=(cy_cell + r_cells) {
            // Walled: skip out-of-bounds rows; toroidal: fold to the cell index
            // but keep the *un-folded* jy/jx for the world-space box so the
            // nearest-point distance is measured across the seam, not clamped.
            let iy = if wrap {
                jy.rem_euclid(dim) as usize
            } else {
                if jy < 0 || jy >= dim {
                    continue;
                }
                jy as usize
            };
            for jx in (cx_cell - r_cells)..=(cx_cell + r_cells) {
                let ix = if wrap {
                    jx.rem_euclid(dim) as usize
                } else {
                    if jx < 0 || jx >= dim {
                        continue;
                    }
                    jx as usize
                };
                // Circle-vs-AABB: find nearest point on the cell box to (cx, cy),
                // then check if that distance is ≤ r. The box uses the unfolded
                // jx/jy so a seam-straddling query measures the short way around.
                let box_x_min = jx as f32 * cs;
                let box_x_max = box_x_min + cs;
                let box_y_min = jy as f32 * cs;
                let box_y_max = box_y_min + cs;
                let nearest_x = cx.clamp(box_x_min, box_x_max);
                let nearest_y = cy.clamp(box_y_min, box_y_max);
                let dx = nearest_x - cx;
                let dy = nearest_y - cy;

                if dx * dx + dy * dy <= r2 {
                    f(iy * dimu + ix);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::GRASS_CELL_SIZE;
    use crate::rng::SimRng;

    // v2.0 Wave 1a: grass dims are runtime. These module-local test constants
    // pin a fixed *walled* grid so the existing walled-behavior assertions keep
    // their meaning. 1200u world / 5u cell → 240² cells.
    const TEST_DIMS: WorldDims = WorldDims {
        world_size: 1200.0,
        wrap_world: false,
        grass_dim: 240,
        grass_cell_count: 240 * 240,
        hash_dim: 120,
        grass_cell_size: 5.0,
    };
    const GRASS_GRID_DIM: usize = TEST_DIMS.grass_dim;
    const GRASS_CELL_COUNT: usize = TEST_DIMS.grass_cell_count;

    fn fresh_grid() -> GrassGrid {
        grid_with_density(
            vec![0.0f32; GRASS_CELL_COUNT],
            vec![GRASS_MAX; GRASS_CELL_COUNT],
        )
    }

    /// Build a test grid with the given density + capacity and a fully-synced
    /// tile state (class + active + dirty) so the active-tile path is correct.
    fn grid_with_density(density: Vec<f32>, capacity: Vec<f32>) -> GrassGrid {
        let tpa = TEST_DIMS.grass_dim.div_ceil(GRASS_TILE_SIZE);
        let tile_words = (tpa * tpa).div_ceil(64);
        // v2.0.2 Stream 1a: encode the f32 seed densities into the u8 store.
        let density: Vec<AtomicU8> = density
            .into_iter()
            .map(|d| AtomicU8::new(encode_density(d)))
            .collect();
        let mut g = GrassGrid {
            dims: TEST_DIMS,
            density,
            capacity,
            scratch: vec![0.0f32; GRASS_CELL_COUNT],
            row_has_density: vec![0u64; GRASS_GRID_DIM.div_ceil(64)],
            tiles_per_axis: tpa,
            tile_active: (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
            tile_active_next: (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
            tile_dirty: [
                (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
                (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
            ],
            tile_class: vec![TILE_EMPTY; tpa * tpa],
            // Low-level test grid: default to BLUR so existing assertions hold.
            propagation: GrassPropagation::Blur,
            world_seed: 0,
            scatter_tick: 0,
            scatter_params: ScatterParams::default(),
            disc: default_disc_table(),
            par_chunks_us: AtomicU64::new(0),
            chunks_mut_us: AtomicU64::new(0),
            row_body_us: AtomicU64::new(0),
            row_body_self_us: AtomicU64::new(0),
            row_body_calls: AtomicU64::new(0),
            dispatch_calls: AtomicU64::new(0),
            pyramid: GrassPyramid::new(TEST_DIMS.grass_dim),
            // v2.0.4 C2: profiling gate — off by default.
            profile_scatter: false,
        };
        g.rebuild_row_bitset();
        g.resync_active_from_density();
        g.mark_all_tiles_dirty();
        g
    }

    /// Walled: step propagates only within bounds — corner cell (0,0) should
    /// NOT propagate to the "far edge" cells (the torus seam neighbors no longer
    /// exist). Only the east (1,0) and south (0,1) cells should receive signal.
    #[test]
    fn step_zero_density_at_edge() {
        let mut g = fresh_grid();
        // Seed cell (0, 0) — top-left corner.
        g.dset(0, GRASS_MAX);
        g.step(0.0, 0.05); // pure propagation, no logistic

        // East neighbor: cell (1, 0) = index 1 — should receive propagation.
        let east = g.dget(1);
        // West neighbor would be at ix=-1 — ghost zero; cell (dim-1, 0) should NOT receive.
        let west = g.dget(GRASS_GRID_DIM - 1);
        assert!(
            east > 0.0,
            "east neighbor must receive propagation from (0,0); east={east}"
        );
        assert!(
            west < 1e-9,
            "west (wrap) must NOT receive propagation in walled world; west={west}"
        );

        // North neighbor at iy=-1 is ghost zero; cell (0, dim-1) must NOT receive.
        let north_wrap = g.dget((GRASS_GRID_DIM - 1) * GRASS_GRID_DIM);
        assert!(
            north_wrap < 1e-9,
            "north wrap cell must NOT receive propagation in walled world; north_wrap={north_wrap}"
        );

        // South neighbor: cell (0, 1) = index GRASS_GRID_DIM — should receive.
        let south = g.dget(GRASS_GRID_DIM);
        assert!(
            south > 0.0,
            "south neighbor must receive propagation; south={south}"
        );
    }

    /// Test 2: empty world stays empty after 100 steps (no spontaneous spawn).
    #[test]
    fn step_empty_world_stays_empty() {
        let mut g = fresh_grid();
        for _ in 0..100 {
            g.step(0.005, 0.05);
        }
        assert!(
            g.density_u8_snapshot().iter().all(|&b| b == 0),
            "empty world must remain empty after 100 steps"
        );
    }

    /// Test 3: step clamps density to GRASS_MAX after 100 steps.
    #[test]
    fn step_clamps_to_grass_max() {
        let mut g = grid_with_density(
            vec![GRASS_MAX; GRASS_CELL_COUNT],
            vec![GRASS_MAX; GRASS_CELL_COUNT],
        );
        for _ in 0..100 {
            g.step(0.005, 0.05);
        }
        let max_val = (0..GRASS_CELL_COUNT)
            .map(|c| g.dget(c))
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max_val <= GRASS_MAX + 1e-6,
            "max density {max_val} exceeds GRASS_MAX after 100 steps"
        );
    }

    /// Test 4: single seeded cell with no propagation grows logistically toward GRASS_MAX.
    #[test]
    fn step_logistic_grows_seeded_cell() {
        let mut g = fresh_grid();
        g.dset(0, 0.1);
        let mut prev = g.dget(0);
        for _ in 0..50 {
            g.step(0.5, 0.0); // high r, no propagation
            let cur = g.dget(0);
            assert!(
                cur >= prev,
                "logistic growth must be monotone; prev={prev} cur={cur}"
            );
            prev = cur;
        }
        assert!(
            g.dget(0) > 0.9,
            "single seeded cell with r=0.5 should approach GRASS_MAX after 50 steps; got {}",
            g.dget(0)
        );
    }

    /// Test 5: bilinear sample at cell center equals the cell's density exactly.
    #[test]
    fn bilinear_sample_at_cell_center_is_exact() {
        let g = fresh_grid();
        // Cell (5, 0): index = 5. Center at (5.5 * GRASS_CELL_SIZE, 0.5 * GRASS_CELL_SIZE).
        g.dset(5, 0.42);
        let x = 5.5 * GRASS_CELL_SIZE;
        let y = 0.5 * GRASS_CELL_SIZE;
        let val = g.bilinear_sample(x, y);
        // Stage 3: byte-exact assertions.
        // encode(0.42) = round(0.42 * 255) = round(107.1) = 107.
        // bilinear_sample at cell center equals dget(5) == decode(107) exactly
        // (only cell-5 has nonzero density; all neighbour weights land on zero cells).
        assert_eq!(g.dget_u8(5), 107u8, "dset(5, 0.42) must store byte 107");
        assert!(
            (val - g.dget(5)).abs() < 1e-6,
            "bilinear_sample at cell center must equal dget(5) exactly; got {val}"
        );
    }

    /// Test 6: bilinear sample at midpoint between two cells equals their average.
    #[test]
    fn bilinear_sample_at_midpoint_is_average() {
        let g = fresh_grid();
        // Cell (0,0) at index 0, cell (1,0) at index 1.
        g.dset(0, 0.0);
        g.dset(1, 1.0);
        // Cell-0 center at (0.5 * size, 0.5 * size); cell-1 center at (1.5 * size, 0.5 * size).
        // Midpoint x = 1.0 * size, y = 0.5 * size.
        let val = g.bilinear_sample(GRASS_CELL_SIZE, 0.5 * GRASS_CELL_SIZE);
        assert!(
            (val - 0.5).abs() < 1e-5,
            "bilinear midpoint must be average of neighbors; got {val}"
        );
    }

    /// Test 7 (walled): bilinear sample at x=0 returns the density of cell (0,0)
    /// and does NOT wrap to the far edge.
    #[test]
    fn bilinear_sample_walled_no_wrap() {
        let g = fresh_grid();
        // Only the corner cell (0,0) has density.
        g.dset(0, 0.5);
        // Far corner (dim-1, dim-1) has different density.
        g.dset(GRASS_GRID_DIM * GRASS_GRID_DIM - 1, 0.9);

        // Sample very close to (0,0) — ghost-zero boundaries mean it's mostly g.density[0].
        let val_corner = g.bilinear_sample(0.01, 0.01);
        // Should be influenced by g.density[0] = 0.5, NOT the far corner = 0.9.
        assert!(
            val_corner > 0.0,
            "near-origin sample must reflect cell(0,0) density"
        );
        // The far-corner cell must not affect the near-origin sample.
        assert!(
            val_corner < 0.8,
            "far corner (0.9) must not leak into near-origin sample via wrap; got {val_corner}"
        );
    }

    /// Below-threshold cell yields a partial bite. Density is drained to zero and
    /// energy is proportional to bytes consumed (`energy_per_bite * taken_bytes /
    /// chunk_bytes`).
    ///
    /// Stage 3: byte-exact assertion.
    /// encode(0.4) = round(0.4 * 255) = round(102.0) = 102 bytes.
    /// chunk_bytes = encode(0.5) = 128 bytes.
    /// taken_bytes = min(102, 128) = 102 (cell is below chunk).
    /// energy = 10.0 * (102 / 128) = 10.0 * 0.796875 = 7.96875 exactly.
    #[test]
    fn consume_below_chunk_yields_partial_bite() {
        let mut g = fresh_grid();
        g.dset(10, 0.4);
        let taken = g.consume(10, 0.5, 10.0);
        // Stage 3: assert the EXACT u8-domain energy (no tolerance band).
        // 0.4 → byte 102; chunk 0.5 → byte 128; energy = 10.0 * 102/128 = 7.96875.
        assert_eq!(
            g.dget_u8(10),
            0u8,
            "density must drain to byte 0 after partial bite"
        );
        assert!(
            (taken - 7.96875_f32).abs() < 1e-5,
            "partial bite energy must be exactly 10.0 * 102/128 = 7.96875; got {taken}"
        );
    }

    /// Consume on an empty cell returns zero.
    #[test]
    fn consume_returns_zero_on_empty() {
        let mut g = fresh_grid();
        let taken = g.consume(0, 0.5, 10.0);
        assert_eq!(taken, 0.0, "consume on empty cell must return 0");
        assert_eq!(g.dget(0), 0.0, "density must stay 0");
    }

    /// Ripe cell yields the configured energy and drains by the configured chunk.
    #[test]
    fn consume_ripe_cell_yields_energy_and_drains() {
        use crate::constants::GRASS_MAX;
        let mut g = fresh_grid();
        g.dset(0, GRASS_MAX);
        let taken = g.consume(0, 0.5, 7.5);
        assert!(
            (taken - 7.5).abs() < 1e-6,
            "consume should return the energy_per_bite arg; got {taken}"
        );
        // Stage 3: byte-exact assertion.
        // encode(GRASS_MAX=1.0) = 255; chunk 0.5 → encode(0.5) = 128.
        // post_byte = 255 - 128 = 127. decode(127) = 127/255 * GRASS_MAX.
        assert_eq!(
            g.dget_u8(0),
            127u8,
            "density must drop to exactly byte 127 after ripe consume; got byte {}",
            g.dget_u8(0)
        );
    }

    /// Test 11: for_each_cell_overlapping_circle with tiny radius finds exactly one cell.
    #[test]
    fn cells_overlapping_circle_finds_central_cell() {
        let g = fresh_grid();
        // Place the query at the last cell's center; a sub-cell radius hits only it.
        let ix = GRASS_GRID_DIM - 1;
        let iy = GRASS_GRID_DIM - 1;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        let mut hits = vec![];
        // Radius strictly less than half a cell guarantees a single hit.
        g.for_each_cell_overlapping_circle(cx, cy, GRASS_CELL_SIZE * 0.1, |idx| hits.push(idx));
        assert_eq!(
            hits.len(),
            1,
            "tiny circle at cell center should hit exactly 1 cell; got {:?}",
            hits
        );
        let expected = iy * GRASS_GRID_DIM + ix;
        assert_eq!(
            hits[0], expected,
            "hit cell {}, expected {}",
            hits[0], expected
        );
    }

    /// Test 12 (walled): for_each_cell_overlapping_circle at world edge does NOT
    /// wrap to cells on the far side.
    #[test]
    fn for_each_cell_skips_out_of_world() {
        let g = fresh_grid();
        // Circle near origin with sub-cell radius — cell (0,0) is hit via circle-vs-AABB
        // (the query point lies inside the cell's box), the far edge is not.
        let mut ix_hits = std::collections::BTreeSet::new();
        g.for_each_cell_overlapping_circle(1.0, 1.0, 3.0, |idx| {
            ix_hits.insert(idx % GRASS_GRID_DIM);
        });
        assert!(
            ix_hits.contains(&0),
            "ix=0 must be in the hit set; got {:?}",
            ix_hits
        );
        assert!(
            !ix_hits.contains(&(GRASS_GRID_DIM - 1)),
            "ix=dim-1 must NOT be in the hit set (walled world, no wrap); got {:?}",
            ix_hits
        );
    }

    /// Test 13: for_each_cell_overlapping_circle matches brute-force circle-vs-AABB scan.
    /// The overlap test uses the nearest-point-on-cell-box distance (not center-to-center)
    /// so that a creature circle touching a cell's edge or corner still counts as overlapping.
    #[test]
    fn cells_overlapping_circle_count_matches_brute_force() {
        let g = fresh_grid();
        // Use a fixed deterministic position and radius away from edges.
        let cx = 37.5_f32;
        let cy = 22.5_f32;
        let r = 4.0_f32;

        // Collect via the method under test.
        let mut method_hits = std::collections::BTreeSet::new();
        g.for_each_cell_overlapping_circle(cx, cy, r, |idx| {
            method_hits.insert(idx);
        });

        // Brute-force: iterate all cells, compute circle-vs-AABB (nearest point on cell box).
        let mut brute_hits = std::collections::BTreeSet::new();
        for iy in 0..GRASS_GRID_DIM {
            for ix in 0..GRASS_GRID_DIM {
                let box_x_min = ix as f32 * GRASS_CELL_SIZE;
                let box_x_max = box_x_min + GRASS_CELL_SIZE;
                let box_y_min = iy as f32 * GRASS_CELL_SIZE;
                let box_y_max = box_y_min + GRASS_CELL_SIZE;
                let nearest_x = cx.clamp(box_x_min, box_x_max);
                let nearest_y = cy.clamp(box_y_min, box_y_max);
                let dx = nearest_x - cx;
                let dy = nearest_y - cy;
                if dx * dx + dy * dy <= r * r {
                    brute_hits.insert(iy * GRASS_GRID_DIM + ix);
                }
            }
        }

        assert_eq!(
            method_hits, brute_hits,
            "for_each_cell_overlapping_circle must match brute-force circle-vs-AABB scan"
        );
    }

    /// Test 14: initial seed is deterministic and produces the expected density pattern.
    #[test]
    fn initial_seed_is_deterministic() {
        let mut rng1 = SimRng::from_u64(42);
        let g1 = GrassGrid::new(&mut rng1, 16, TEST_DIMS);
        let mut rng2 = SimRng::from_u64(42);
        let g2 = GrassGrid::new(&mut rng2, 16, TEST_DIMS);
        assert_eq!(
            g1.density_u8_snapshot(),
            g2.density_u8_snapshot(),
            "same seed must produce identical density vectors"
        );
        let max_count = (0..GRASS_CELL_COUNT)
            .filter(|&c| (g1.dget(c) - GRASS_MAX).abs() < 1e-6)
            .count();
        assert!(
            max_count <= 16,
            "seeded cell count must be ≤ initial_seed_count; got {max_count}"
        );
        assert!(
            max_count >= 1,
            "at least one cell must be seeded; got {max_count}"
        );
    }

    /// v1.5 S5b: per-row bitset reflects current density state.
    #[test]
    fn row_bitset_tracks_density() {
        let mut g = fresh_grid();
        // No density anywhere → all bits zero.
        g.rebuild_row_bitset();
        for iy in 0..GRASS_GRID_DIM {
            assert!(!g.row_nonempty(iy), "row {iy} must be empty initially");
        }
        // Seed cell (5, 7) — row 7 must now flag non-empty.
        g.dset(7 * GRASS_GRID_DIM + 5, 0.4);
        g.rebuild_row_bitset();
        assert!(
            g.row_nonempty(7),
            "row 7 must be flagged after seeding (5,7)"
        );
        for iy in 0..GRASS_GRID_DIM {
            if iy != 7 {
                assert!(!g.row_nonempty(iy), "row {iy} must remain empty");
            }
        }
    }

    /// Test 15: density and scratch vectors have correct length after new().
    #[test]
    fn density_length_invariant() {
        let mut rng = SimRng::from_u64(0);
        let g = GrassGrid::new(&mut rng, 8, TEST_DIMS);
        assert_eq!(
            g.density.len(),
            GRASS_CELL_COUNT,
            "density.len() must equal GRASS_CELL_COUNT"
        );
        assert_eq!(
            g.scratch.len(),
            GRASS_CELL_COUNT,
            "scratch.len() must equal GRASS_CELL_COUNT"
        );
    }
}

// v2.0.1 GRASS wave (#5 organic shape + #6 frontier/snapshot perf) tests live in
// their OWN file/module (merge-hazard rule: never appended to the shared `tests`
// module above). It is a child of `grass` so it can reach internal fields/fns.
#[cfg(test)]
#[path = "grass_v201_tests.rs"]
mod grass_v201_tests;

// v2.0.2 Stream 1b/1c: scatter kernel + frontier smoke test in its OWN per-feature
// file (merge-hazard rule). Child of `grass` so it can reach internal fields/fns.
#[cfg(test)]
#[path = "grass_scatter_smoke_tests.rs"]
mod grass_scatter_smoke_tests;

// v2.0.3 Stream 2a: GrassPyramid smoke test in its OWN per-feature file
// (merge-hazard rule). Child of `grass` so it can reach internal fields/fns.
#[cfg(test)]
#[path = "grass_pyramid_tests.rs"]
mod grass_pyramid_tests;

// Stage-3 scatter battery: per-feature deterministic tests for the scatter
// kernel (disc-roundness, decay-floor, persistence, biome-cap clamp). Wired
// as a child of `grass` so it can reach internal fields/fns.
#[cfg(test)]
#[path = "grass_scatter_tests.rs"]
mod grass_scatter_tests;

// v2.0.4 C3: fused-RNG distribution-match tests — verifies that the two-hash
// bit-slice approach preserves decay/spread/band/pick statistics vs the
// legacy 4-separate-hash path, and that grass never touches SimRng.
#[cfg(test)]
#[path = "grass_fused_rng_tests.rs"]
mod grass_fused_rng_tests;

// v2.0.4 S4: fertility (density-weighted spread + clumping) tests.
// Verifies amount∝density behaviour, persistence invariant (plains super-
// critical, water sub-critical), and net-growth sanity at the raised decay_pct.
#[cfg(test)]
#[path = "grass_fertility_tests.rs"]
mod grass_fertility_tests;
