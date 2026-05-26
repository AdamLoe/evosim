//! GrassGrid — 240×240 = 57_600 flat-Vec<f32> density field over the
//! 600u toroidal world. Each cell is 2.5u square. Independent of
//! SpatialGrid (5u, body queries). See v1.2 grass-mechanic-brief §Grass
//! storage / §Grass dynamics per tick / §Initial grass seed / §Bilinear
//! sampling.

use crate::constants::{GRASS_CELL_COUNT, GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX, WORLD_SIZE};
use crate::rng::SimRng;

// Compile-time invariant checks (v1.2 grass mechanic brief §Grass storage).
const _: () = assert!(GRASS_CELL_COUNT == 57_600);
const _: () = assert!(GRASS_GRID_DIM == 240);

/// 240×240 grass density field over the 600u toroidal world.
///
/// Row-major layout: cell (ix, iy) is at index `iy * GRASS_GRID_DIM + ix`.
/// Density values are clamped to `[0.0, GRASS_MAX]` after each `step` call.
#[derive(Clone, Debug)]
pub struct GrassGrid {
    /// Row-major density, length GRASS_CELL_COUNT (= 57_600). Added by P1b+P1f.
    pub density: Vec<f32>,
    /// Double-buffer scratch. Same length as `density`. Recomputed every
    /// `step` call; never read between ticks. NOT saved (rebuilt on load
    /// as a zero-fill).
    pub scratch: Vec<f32>,
}

impl GrassGrid {
    /// World-init constructor. Seeds `initial_seed_count` random cells
    /// with `GRASS_MAX`; all others zero. Draws from `rng` via the same
    /// world RNG used everywhere else (deterministic given fixed seed).
    /// Idempotent re-seeding allowed: if the same cell is drawn twice,
    /// the second write is harmless (density stays at GRASS_MAX).
    /// For large `initial_seed_count` the actual unique-cell count may be
    /// slightly fewer than N due to collisions — acceptable per brief §Initial grass seed.
    pub fn new(rng: &mut SimRng, initial_seed_count: u32) -> Self {
        let mut density = vec![0.0f32; GRASS_CELL_COUNT];
        let scratch = vec![0.0f32; GRASS_CELL_COUNT];
        for _ in 0..initial_seed_count {
            let idx = rng.index(GRASS_CELL_COUNT);
            density[idx] = GRASS_MAX;
        }
        Self { density, scratch }
    }

    /// One propagation tick. Reads `self.density`, writes `self.scratch`, swaps.
    ///
    /// Formula per cell c (toroidal cardinal neighbors n, s, e, w):
    /// ```text
    /// d' = clamp(
    ///   d[c]
    ///     + r_in_cell * d[c] * (1 - d[c] / GRASS_MAX)   // logistic in-cell growth
    ///     + k_propagate * (d[n] + d[s] + d[e] + d[w]),  // cross-kernel propagation
    ///   0.0,
    ///   GRASS_MAX,
    /// )
    /// ```
    /// Empty cells with all-zero neighbors remain zero (no spontaneous spawn).
    /// Scalar implementation — see v1.2 p1c plan §2 for SIMD deferral rationale.
    /// TODO(v1.3): SIMD via wide::f32x8 for cells 0..57_600.
    pub fn step(&mut self, r_in_cell: f32, k_propagate: f32) {
        debug_assert_eq!(self.density.len(), GRASS_CELL_COUNT);
        debug_assert_eq!(self.scratch.len(), GRASS_CELL_COUNT);

        let dim = GRASS_GRID_DIM;
        let inv_max = 1.0 / GRASS_MAX;
        let d = &self.density;
        let s = &mut self.scratch;

        for iy in 0..dim {
            // Precompute row-base offsets with toroidal wrap on Y.
            let row_self = iy * dim;
            let row_n = if iy == 0 {
                (dim - 1) * dim
            } else {
                (iy - 1) * dim
            };
            let row_s = if iy + 1 == dim { 0 } else { (iy + 1) * dim };

            for ix in 0..dim {
                let c = row_self + ix;
                let ce = row_self + (if ix + 1 == dim { 0 } else { ix + 1 });
                let cw = row_self + (if ix == 0 { dim - 1 } else { ix - 1 });
                let cn = row_n + ix;
                let cs = row_s + ix;

                let v = d[c];
                // Logistic in-cell growth term (only nonzero when v > 0):
                let logistic = r_in_cell * v * (1.0 - v * inv_max);
                // Cross-kernel propagation from 4 cardinal neighbors:
                let prop = k_propagate * (d[cn] + d[cs] + d[ce] + d[cw]);
                s[c] = (v + logistic + prop).clamp(0.0, GRASS_MAX);
            }
        }

        std::mem::swap(&mut self.density, &mut self.scratch);
    }

    /// Toroidal-aware bilinear sample at continuous world position `(x, y)`.
    ///
    /// Used by PR-2 P2d to build the 25 grass-patch NN inputs per creature.
    /// World wraps at `WORLD_SIZE = 600.0`. Returns a value in `[0.0, GRASS_MAX]`.
    pub fn bilinear_sample(&self, x: f32, y: f32) -> f32 {
        // Step 1: wrap continuous coords into [0, WORLD_SIZE).
        let w = WORLD_SIZE;
        let xw = ((x % w) + w) % w;
        let yw = ((y % w) + w) % w;

        // Step 2: convert to fractional cell coordinates. Cell i's center is at
        // (i + 0.5) * GRASS_CELL_SIZE. Subtract 0.5 so cell centers fall at integers.
        let fx = xw / GRASS_CELL_SIZE - 0.5;
        let fy = yw / GRASS_CELL_SIZE - 0.5;

        // Step 3: get integer floor and fractional offset, wrapping.
        let dim = GRASS_GRID_DIM as i32;
        let ix0 = (fx.floor() as i32).rem_euclid(dim) as usize;
        let iy0 = (fy.floor() as i32).rem_euclid(dim) as usize;
        let ix1 = if ix0 + 1 == GRASS_GRID_DIM {
            0
        } else {
            ix0 + 1
        };
        let iy1 = if iy0 + 1 == GRASS_GRID_DIM {
            0
        } else {
            iy0 + 1
        };

        let tx = fx - fx.floor(); // in [0, 1)
        let ty = fy - fy.floor();

        // Step 4: read four neighbors and bilinear interpolate.
        let d = &self.density;
        let d00 = d[iy0 * GRASS_GRID_DIM + ix0];
        let d10 = d[iy0 * GRASS_GRID_DIM + ix1];
        let d01 = d[iy1 * GRASS_GRID_DIM + ix0];
        let d11 = d[iy1 * GRASS_GRID_DIM + ix1];

        let a = d00 * (1.0 - tx) + d10 * tx; // first row
        let b = d01 * (1.0 - tx) + d11 * tx; // second row
        a * (1.0 - ty) + b * ty
    }

    /// Clamp-take with NaN guard. Returns the actual delta removed
    /// (≤ `max_take`, ≤ `density[cell_idx]`).
    ///
    /// Called by P1e in the sequential graze loop — single-threaded, no atomics.
    #[inline]
    pub fn consume(&mut self, cell_idx: usize, max_take: f32) -> f32 {
        if !max_take.is_finite() {
            return 0.0;
        }
        let cur = self.density[cell_idx];
        let take = cur.min(max_take).max(0.0); // defend against negative max_take
        self.density[cell_idx] = cur - take; // never negative: take ≤ cur
        take
    }

    /// Iterate every cell whose center is within toroidal distance `r` of `(cx, cy)`.
    ///
    /// Invokes `f(cell_idx)` once per hit. Used by P1e to find graze-overlap cells.
    /// Deterministic: row-major, left-to-right iteration order.
    /// Contract: `r` must be finite, non-negative, and ≤ `WORLD_SIZE * 0.5`.
    pub fn for_each_cell_overlapping_circle(
        &self,
        cx: f32,
        cy: f32,
        r: f32,
        mut f: impl FnMut(usize),
    ) {
        debug_assert!(r.is_finite() && r >= 0.0);
        debug_assert!(
            r <= WORLD_SIZE * 0.5,
            "radius must be ≤ half-world (300u); got {r}"
        );

        // Bounding box in cell coords (signed; we wrap per-cell below).
        let r_cells = (r / GRASS_CELL_SIZE).ceil() as i32 + 1; // +1 for floor/center slop
        let cx_cell = (cx / GRASS_CELL_SIZE).floor() as i32;
        let cy_cell = (cy / GRASS_CELL_SIZE).floor() as i32;
        let dim = GRASS_GRID_DIM as i32;
        let half_w = WORLD_SIZE * 0.5;
        let r2 = r * r;

        for jy in (cy_cell - r_cells)..=(cy_cell + r_cells) {
            let iy = jy.rem_euclid(dim) as usize;
            for jx in (cx_cell - r_cells)..=(cx_cell + r_cells) {
                let ix = jx.rem_euclid(dim) as usize;
                // Use pre-wrap jx/jy for distance computation so the geometry
                // stays correct relative to (cx, cy) without spurious half-world jumps.
                let world_x = (jx as f32 + 0.5) * GRASS_CELL_SIZE;
                let world_y = (jy as f32 + 0.5) * GRASS_CELL_SIZE;

                // Shortest-vector torus distance:
                let mut dx = world_x - cx;
                let mut dy = world_y - cy;
                if dx > half_w {
                    dx -= WORLD_SIZE;
                }
                if dx < -half_w {
                    dx += WORLD_SIZE;
                }
                if dy > half_w {
                    dy -= WORLD_SIZE;
                }
                if dy < -half_w {
                    dy += WORLD_SIZE;
                }

                if dx * dx + dy * dy <= r2 {
                    f(iy * GRASS_GRID_DIM + ix);
                }
            }
        }
    }

    /// Cheap getter for the snapshot/render path.
    #[inline]
    pub fn density_at_cell(&self, cell_idx: usize) -> f32 {
        self.density[cell_idx]
    }
}

/// Convert continuous world coords to a flat cell index (toroidal wrap).
///
/// Uses `rem_euclid` on integer cell coordinates for bit-stable results
/// across sequential and threaded paths. Handles negative or > WORLD_SIZE inputs.
#[allow(dead_code)] // Used by P1g (grass density render) — not yet landed.
#[inline]
pub fn cell_index_for(x: f32, y: f32) -> usize {
    let ix = ((x / GRASS_CELL_SIZE).floor() as i32).rem_euclid(GRASS_GRID_DIM as i32) as usize;
    let iy = ((y / GRASS_CELL_SIZE).floor() as i32).rem_euclid(GRASS_GRID_DIM as i32) as usize;
    iy * GRASS_GRID_DIM + ix
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SimRng;

    fn fresh_grid() -> GrassGrid {
        GrassGrid {
            density: vec![0.0f32; GRASS_CELL_COUNT],
            scratch: vec![0.0f32; GRASS_CELL_COUNT],
        }
    }

    /// Test 1: step preserves toroidal seam symmetry — seeding (0,0) should
    /// propagate symmetrically to east (1,0) and west (dim-1, 0).
    #[test]
    fn step_preserves_torus_seam_symmetry() {
        let mut g = fresh_grid();
        // Seed cell (0, 0) — top-left corner.
        g.density[0] = GRASS_MAX;
        g.step(0.0, 0.05); // pure propagation, no logistic
                           // East neighbor: cell (1, 0) = index 1
        let east = g.density[1];
        // West neighbor (wraps): cell (dim-1, 0) = index GRASS_GRID_DIM - 1
        let west = g.density[GRASS_GRID_DIM - 1];
        assert!(
            (east - west).abs() < 1e-6,
            "east={east} west={west} should be equal after seam propagation"
        );
        // North neighbor (wraps): cell (0, dim-1) = index (dim-1)*dim
        let north = g.density[(GRASS_GRID_DIM - 1) * GRASS_GRID_DIM];
        // South neighbor: cell (0, 1) = index GRASS_GRID_DIM
        let south = g.density[GRASS_GRID_DIM];
        assert!(
            (north - south).abs() < 1e-6,
            "north={north} south={south} should be equal after seam propagation"
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
            g.density.iter().all(|&d| d == 0.0),
            "empty world must remain empty after 100 steps"
        );
    }

    /// Test 3: step clamps density to GRASS_MAX after 100 steps.
    #[test]
    fn step_clamps_to_grass_max() {
        let mut g = GrassGrid {
            density: vec![GRASS_MAX; GRASS_CELL_COUNT],
            scratch: vec![0.0f32; GRASS_CELL_COUNT],
        };
        for _ in 0..100 {
            g.step(0.005, 0.05);
        }
        let max_val = g.density.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max_val <= GRASS_MAX + 1e-6,
            "max density {max_val} exceeds GRASS_MAX after 100 steps"
        );
    }

    /// Test 4: single seeded cell with no propagation grows logistically toward GRASS_MAX.
    #[test]
    fn step_logistic_grows_seeded_cell() {
        let mut g = fresh_grid();
        g.density[0] = 0.1;
        let mut prev = 0.1f32;
        for _ in 0..50 {
            g.step(0.5, 0.0); // high r, no propagation
            let cur = g.density[0];
            assert!(
                cur >= prev,
                "logistic growth must be monotone; prev={prev} cur={cur}"
            );
            prev = cur;
        }
        assert!(
            g.density[0] > 0.9,
            "single seeded cell with r=0.5 should approach GRASS_MAX after 50 steps; got {}",
            g.density[0]
        );
    }

    /// Test 5: bilinear sample at cell center equals the cell's density exactly.
    #[test]
    fn bilinear_sample_at_cell_center_is_exact() {
        let mut g = fresh_grid();
        // Cell (5, 0): index = 5. Center at (5.5 * GRASS_CELL_SIZE, 0.5 * GRASS_CELL_SIZE).
        g.density[5] = 0.42;
        let x = 5.5 * GRASS_CELL_SIZE;
        let y = 0.5 * GRASS_CELL_SIZE;
        let val = g.bilinear_sample(x, y);
        assert!(
            (val - 0.42).abs() < 1e-5,
            "bilinear_sample at cell center must equal density; got {val}"
        );
    }

    /// Test 6: bilinear sample at midpoint between two cells equals their average.
    #[test]
    fn bilinear_sample_at_midpoint_is_average() {
        let mut g = fresh_grid();
        // Cell (0,0) at index 0, cell (1,0) at index 1.
        g.density[0] = 0.0;
        g.density[1] = 1.0;
        // Midpoint between cell-0 center (1.25) and cell-1 center (3.75) is x=2.5.
        // At x=2.5, y=1.25 (cell-0's center y), expected value = 0.5.
        let val = g.bilinear_sample(2.5, 1.25);
        assert!(
            (val - 0.5).abs() < 1e-5,
            "bilinear midpoint must be average of neighbors; got {val}"
        );
    }

    /// Test 7: bilinear sample wraps at world seam — (0,0) and (WORLD_SIZE, WORLD_SIZE) are identical.
    #[test]
    fn bilinear_sample_wraps_at_seam() {
        let mut g = fresh_grid();
        // Set a non-trivial density pattern near the corner cells.
        g.density[0] = 0.3;
        g.density[GRASS_GRID_DIM - 1] = 0.7;
        g.density[(GRASS_GRID_DIM - 1) * GRASS_GRID_DIM] = 0.5;
        g.density[GRASS_GRID_DIM * GRASS_GRID_DIM - 1] = 0.9;
        let v1 = g.bilinear_sample(0.0, 0.0);
        let v2 = g.bilinear_sample(WORLD_SIZE, WORLD_SIZE);
        assert!(
            (v1 - v2).abs() < 1e-6,
            "bilinear_sample(0,0)={v1} must equal bilinear_sample(WORLD_SIZE,WORLD_SIZE)={v2}"
        );
    }

    /// Test 8: consume clamps to existing density (can't take more than present).
    #[test]
    fn consume_clamps_to_density() {
        let mut g = fresh_grid();
        g.density[10] = 0.3;
        let taken = g.consume(10, 0.5);
        assert!(
            (taken - 0.3).abs() < 1e-6,
            "consume should return 0.3 (capped by density); got {taken}"
        );
        assert!(
            g.density[10].abs() < 1e-6,
            "cell should be empty after full drain; got {}",
            g.density[10]
        );
    }

    /// Test 9: consume returns zero when cell is empty.
    #[test]
    fn consume_returns_zero_on_empty() {
        let mut g = fresh_grid();
        let taken = g.consume(0, 1.0);
        assert_eq!(taken, 0.0, "consume on empty cell must return 0");
        assert_eq!(g.density[0], 0.0, "density must stay 0");
    }

    /// Test 10: consume with negative max_take returns 0 (NaN/negative guard).
    #[test]
    fn consume_clamps_negative_max_take() {
        let mut g = fresh_grid();
        g.density[0] = 0.5;
        let taken = g.consume(0, -1.0);
        assert_eq!(taken, 0.0, "negative max_take must return 0");
        assert!(
            (g.density[0] - 0.5).abs() < 1e-6,
            "density must be unchanged after negative max_take"
        );
    }

    /// Test 11: for_each_cell_overlapping_circle with tiny radius finds exactly one cell.
    #[test]
    fn cells_overlapping_circle_finds_central_cell() {
        let g = fresh_grid();
        // Place the query exactly at cell (119, 119)'s center: (119.5 * 2.5, 119.5 * 2.5).
        // A radius of 0.5 is smaller than GRASS_CELL_SIZE/2, so only the containing cell hits.
        let ix = 119usize;
        let iy = 119usize;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        let mut hits = vec![];
        g.for_each_cell_overlapping_circle(cx, cy, 0.5, |idx| hits.push(idx));
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

    /// Test 12: for_each_cell_overlapping_circle wraps across the west seam.
    #[test]
    fn cells_overlapping_circle_wraps() {
        let g = fresh_grid();
        // Circle at (0.5, 0.5) with radius 2.0 — should find ix=0 AND ix=dim-1.
        let mut ix_hits = std::collections::BTreeSet::new();
        g.for_each_cell_overlapping_circle(0.5, 0.5, 2.0, |idx| {
            ix_hits.insert(idx % GRASS_GRID_DIM);
        });
        assert!(
            ix_hits.contains(&0),
            "ix=0 must be in the hit set; got {:?}",
            ix_hits
        );
        assert!(
            ix_hits.contains(&(GRASS_GRID_DIM - 1)),
            "ix=dim-1 must be in the hit set (toroidal wrap); got {:?}",
            ix_hits
        );
    }

    /// Test 13: for_each_cell_overlapping_circle matches brute-force torus distance scan.
    #[test]
    fn cells_overlapping_circle_count_matches_brute_force() {
        let g = fresh_grid();
        // Use a fixed deterministic position and radius to avoid RNG dependency.
        let cx = 37.5_f32; // 15 cell widths from origin
        let cy = 22.5_f32;
        let r = 4.0_f32;
        let half_w = WORLD_SIZE * 0.5;

        // Collect via the method under test.
        let mut method_hits = std::collections::BTreeSet::new();
        g.for_each_cell_overlapping_circle(cx, cy, r, |idx| {
            method_hits.insert(idx);
        });

        // Brute-force: iterate all cells, compute toroidal center distance.
        let mut brute_hits = std::collections::BTreeSet::new();
        for iy in 0..GRASS_GRID_DIM {
            for ix in 0..GRASS_GRID_DIM {
                let world_x = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
                let world_y = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
                let mut dx = world_x - cx;
                let mut dy = world_y - cy;
                if dx > half_w {
                    dx -= WORLD_SIZE;
                }
                if dx < -half_w {
                    dx += WORLD_SIZE;
                }
                if dy > half_w {
                    dy -= WORLD_SIZE;
                }
                if dy < -half_w {
                    dy += WORLD_SIZE;
                }
                if dx * dx + dy * dy <= r * r {
                    brute_hits.insert(iy * GRASS_GRID_DIM + ix);
                }
            }
        }

        assert_eq!(
            method_hits, brute_hits,
            "for_each_cell_overlapping_circle must match brute-force scan"
        );
    }

    /// Test 14: initial seed is deterministic and produces the expected density pattern.
    #[test]
    fn initial_seed_is_deterministic() {
        let mut rng1 = SimRng::from_u64(42);
        let g1 = GrassGrid::new(&mut rng1, 16);
        let mut rng2 = SimRng::from_u64(42);
        let g2 = GrassGrid::new(&mut rng2, 16);
        assert_eq!(
            g1.density, g2.density,
            "same seed must produce identical density vectors"
        );
        // Count cells at GRASS_MAX (at most 16 seeded; may be fewer due to collisions).
        let max_count = g1
            .density
            .iter()
            .filter(|&&d| (d - GRASS_MAX).abs() < 1e-6)
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

    /// Test 15: density and scratch vectors have correct length after new().
    #[test]
    fn density_length_invariant() {
        let mut rng = SimRng::from_u64(0);
        let g = GrassGrid::new(&mut rng, 8);
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
