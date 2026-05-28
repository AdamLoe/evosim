//! GrassGrid — 480×480 = 230_400 flat-Vec<f32> density field over the
//! 600u walled world. Each cell is 1.25u square (v1.5 S6 — 16× density bump
//! from the 5u/120-dim v1.2 layout). Independent of SpatialGrid (5u, body
//! queries). v1.2 grass-mechanic-brief §Grass storage / §Grass dynamics per
//! tick / §Initial grass seed / §Bilinear sampling.

use crate::constants::{GRASS_CELL_COUNT, GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX, WORLD_SIZE};
use crate::rng::SimRng;
#[cfg(feature = "threads")]
use rayon::prelude::*;

// Compile-time invariant checks.
const _: () = assert!(GRASS_CELL_COUNT == 230_400);
const _: () = assert!(GRASS_GRID_DIM == 480);

/// 480×480 grass density field over the 600u walled world.
///
/// Row-major layout: cell (ix, iy) is at index `iy * GRASS_GRID_DIM + ix`.
/// Density values are clamped to `[0.0, GRASS_MAX]` after each `step` call.
#[derive(Clone, Debug)]
pub struct GrassGrid {
    /// Row-major density, length GRASS_CELL_COUNT (= 230_400).
    pub density: Vec<f32>,
    /// Double-buffer scratch. Same length as `density`. Recomputed every
    /// `step` call; never read between ticks.
    pub scratch: Vec<f32>,
    /// Per-row "any non-empty" bitset (v1.5 S5b). One bit per row: 1 if any
    /// cell in that row has `density > 0`, 0 otherwise. Lets the grass-sector
    /// NN scan skip whole rows in O(1) when sparse. Length =
    /// `GRASS_GRID_DIM.div_ceil(64)` u64s; regenerated after every `step()`.
    pub row_has_density: Vec<u64>,
}

impl GrassGrid {
    /// World-init constructor. Seeds `initial_seed_count` random cells
    /// with `GRASS_MAX`; all others zero. Draws from `rng` via the same
    /// world RNG used everywhere else (deterministic given fixed seed).
    pub fn new(rng: &mut SimRng, initial_seed_count: u32) -> Self {
        let mut density = vec![0.0f32; GRASS_CELL_COUNT];
        let scratch = vec![0.0f32; GRASS_CELL_COUNT];
        for _ in 0..initial_seed_count {
            let idx = rng.index(GRASS_CELL_COUNT);
            density[idx] = GRASS_MAX;
        }
        let mut g = Self {
            density,
            scratch,
            row_has_density: vec![0u64; GRASS_GRID_DIM.div_ceil(64)],
        };
        g.rebuild_row_bitset();
        g
    }

    /// Recompute the per-row "any non-empty" bitset. Called from `step()`
    /// after the density swap; exposed for tests that mutate `density` directly.
    pub fn rebuild_row_bitset(&mut self) {
        let words = GRASS_GRID_DIM.div_ceil(64);
        if self.row_has_density.len() != words {
            self.row_has_density.resize(words, 0);
        }
        for w in self.row_has_density.iter_mut() {
            *w = 0;
        }
        for iy in 0..GRASS_GRID_DIM {
            let row_off = iy * GRASS_GRID_DIM;
            let any = self.density[row_off..row_off + GRASS_GRID_DIM]
                .iter()
                .any(|&d| d > 0.0);
            if any {
                self.row_has_density[iy / 64] |= 1u64 << (iy % 64);
            }
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

    /// One propagation tick. Reads `self.density`, writes `self.scratch`, swaps.
    ///
    /// Formula per cell c (walled cardinal neighbors n, s, e, w — ghost zero at boundary):
    /// ```text
    /// d' = clamp(
    ///   d[c]
    ///     + r_in_cell * d[c] * (1 - d[c] / GRASS_MAX)   // logistic in-cell growth
    ///     + k_propagate * (d[n] + d[s] + d[e] + d[w]),  // cross-kernel propagation
    ///   0.0,
    ///   GRASS_MAX,
    /// )
    /// ```
    /// Out-of-bounds neighbors are treated as zero density (ghost-zero boundary).
    /// Empty cells with all-zero neighbors remain zero (no spontaneous spawn).
    ///
    /// Production callers in `World::tick_once` now invoke the split
    /// `compute_propagation` + `rebuild_row_bitset` directly so the profiler
    /// can break the two phases apart; this convenience wrapper survives for
    /// the grass-only unit tests in this module.
    #[allow(dead_code)]
    pub fn step(&mut self, r_in_cell: f32, k_propagate: f32) {
        self.compute_propagation(r_in_cell, k_propagate);
        self.rebuild_row_bitset();
    }

    /// Phase 1 of `step()`: write the next density frame into `scratch` and
    /// swap. Exposed separately so the World-side profiler can break the grass
    /// step into compute-vs-bitset sub-spans.
    pub fn compute_propagation(&mut self, r_in_cell: f32, k_propagate: f32) {
        debug_assert_eq!(self.density.len(), GRASS_CELL_COUNT);
        debug_assert_eq!(self.scratch.len(), GRASS_CELL_COUNT);

        let dim = GRASS_GRID_DIM;
        let inv_max = 1.0 / GRASS_MAX;
        let d = &self.density;

        // Single-row body: writes into `s_row` (one dim-long slice of scratch)
        // by reading the (iy-1, iy, iy+1) rows from `d`. Ghost-zero N/S/E/W.
        let row_body = |iy: usize, s_row: &mut [f32]| {
            let row_self = iy * dim;
            let row_n = if iy == 0 { None } else { Some((iy - 1) * dim) };
            let row_s = if iy + 1 == dim {
                None
            } else {
                Some((iy + 1) * dim)
            };
            for ix in 0..dim {
                let c = row_self + ix;
                let ce = if ix + 1 == dim {
                    None
                } else {
                    Some(row_self + ix + 1)
                };
                let cw = if ix == 0 {
                    None
                } else {
                    Some(row_self + ix - 1)
                };
                let cn = row_n.map(|r| r + ix);
                let cs = row_s.map(|r| r + ix);
                let v = d[c];
                let logistic = r_in_cell * v * (1.0 - v * inv_max);
                // Max-of-neighbors spill: 1 ripe neighbor already grants the max
                // propagation boost; additional neighbors don't stack. Avoids the
                // "every cell next to a saturated patch quickly fills" cascade.
                let max_neighbor = cn
                    .map_or(0.0, |i| d[i])
                    .max(cs.map_or(0.0, |i| d[i]))
                    .max(ce.map_or(0.0, |i| d[i]))
                    .max(cw.map_or(0.0, |i| d[i]));
                let prop = k_propagate * max_neighbor;
                s_row[ix] = (v + logistic + prop).clamp(0.0, GRASS_MAX);
            }
        };

        #[cfg(feature = "threads")]
        {
            // Split scratch into contiguous row-chunks. ~30 rows/chunk at 480
            // dims keeps the per-chunk cost meaty enough to outweigh dispatch.
            let chunk_rows = (dim / 16).max(1);
            self.scratch
                .par_chunks_mut(chunk_rows * dim)
                .enumerate()
                .for_each(|(ci, s_chunk)| {
                    let iy0 = ci * chunk_rows;
                    for (k, s_row) in s_chunk.chunks_mut(dim).enumerate() {
                        row_body(iy0 + k, s_row);
                    }
                });
        }
        #[cfg(not(feature = "threads"))]
        {
            for (iy, s_row) in self.scratch.chunks_mut(dim).enumerate() {
                row_body(iy, s_row);
            }
        }

        std::mem::swap(&mut self.density, &mut self.scratch);
    }

    /// Walled bilinear sample at continuous world position `(x, y)`.
    ///
    /// Clamps input to [0, WORLD_SIZE). Out-of-bounds indices return ghost zero
    /// (Q-D7-3 default: ghost-zero).
    pub fn bilinear_sample(&self, x: f32, y: f32) -> f32 {
        let w = WORLD_SIZE;
        // Clamp to world bounds instead of wrapping.
        let xw = x.clamp(0.0, w - 1e-4);
        let yw = y.clamp(0.0, w - 1e-4);

        // Convert to fractional cell coordinates. Cell i's center is at
        // (i + 0.5) * GRASS_CELL_SIZE. Subtract 0.5 so cell centers fall at integers.
        let fx = xw / GRASS_CELL_SIZE - 0.5;
        let fy = yw / GRASS_CELL_SIZE - 0.5;

        let dim = GRASS_GRID_DIM as i32;
        let ix0 = fx.floor() as i32;
        let iy0 = fy.floor() as i32;
        let ix1 = ix0 + 1;
        let iy1 = iy0 + 1;

        let tx = fx - fx.floor(); // in [0, 1)
        let ty = fy - fy.floor();

        // Ghost-zero: out-of-bounds indices read as 0.
        let sample = |iy: i32, ix: i32| -> f32 {
            if iy < 0 || iy >= dim || ix < 0 || ix >= dim {
                return 0.0;
            }
            self.density[iy as usize * GRASS_GRID_DIM + ix as usize]
        };

        let d00 = sample(iy0, ix0);
        let d10 = sample(iy0, ix1);
        let d01 = sample(iy1, ix0);
        let d11 = sample(iy1, ix1);

        let a = d00 * (1.0 - tx) + d10 * tx;
        let b = d01 * (1.0 - tx) + d11 * tx;
        a * (1.0 - ty) + b * ty
    }

    /// Partial-bite consume. Drains `min(density, density_chunk)` and awards
    /// `energy_per_bite * taken / density_chunk` energy (proportional to what was
    /// actually consumed). A ripe cell (density ≥ chunk) still yields the full
    /// `energy_per_bite`; an under-threshold cell yields a fraction.
    /// Returns `0.0` only when the cell is fully empty.
    #[inline]
    pub fn consume(&mut self, cell_idx: usize, density_chunk: f32, energy_per_bite: f32) -> f32 {
        let cur = self.density[cell_idx];
        if cur <= 0.0 || density_chunk <= 0.0 {
            return 0.0;
        }
        let taken = cur.min(density_chunk);
        self.density[cell_idx] = cur - taken;
        energy_per_bite * (taken / density_chunk)
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
        let search_r = r + GRASS_CELL_SIZE * 0.5;
        let r_cells = (search_r / GRASS_CELL_SIZE).ceil() as i32 + 1;
        let cx_cell = (cx / GRASS_CELL_SIZE).floor() as i32;
        let cy_cell = (cy / GRASS_CELL_SIZE).floor() as i32;
        let dim = GRASS_GRID_DIM as i32;
        let r2 = r * r;

        for jy in (cy_cell - r_cells)..=(cy_cell + r_cells) {
            if jy < 0 || jy >= dim {
                continue;
            }
            let iy = jy as usize;
            for jx in (cx_cell - r_cells)..=(cx_cell + r_cells) {
                if jx < 0 || jx >= dim {
                    continue;
                }
                let ix = jx as usize;
                // Circle-vs-AABB: find nearest point on the cell box to (cx, cy),
                // then check if that distance is ≤ r.
                let box_x_min = jx as f32 * GRASS_CELL_SIZE;
                let box_x_max = box_x_min + GRASS_CELL_SIZE;
                let box_y_min = jy as f32 * GRASS_CELL_SIZE;
                let box_y_max = box_y_min + GRASS_CELL_SIZE;
                let nearest_x = cx.clamp(box_x_min, box_x_max);
                let nearest_y = cy.clamp(box_y_min, box_y_max);
                let dx = nearest_x - cx;
                let dy = nearest_y - cy;

                if dx * dx + dy * dy <= r2 {
                    f(iy * GRASS_GRID_DIM + ix);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SimRng;

    fn fresh_grid() -> GrassGrid {
        GrassGrid {
            density: vec![0.0f32; GRASS_CELL_COUNT],
            scratch: vec![0.0f32; GRASS_CELL_COUNT],
            row_has_density: vec![0u64; GRASS_GRID_DIM.div_ceil(64)],
        }
    }

    /// Walled: step propagates only within bounds — corner cell (0,0) should
    /// NOT propagate to the "far edge" cells (the torus seam neighbors no longer
    /// exist). Only the east (1,0) and south (0,1) cells should receive signal.
    #[test]
    fn step_zero_density_at_edge() {
        let mut g = fresh_grid();
        // Seed cell (0, 0) — top-left corner.
        g.density[0] = GRASS_MAX;
        g.step(0.0, 0.05); // pure propagation, no logistic

        // East neighbor: cell (1, 0) = index 1 — should receive propagation.
        let east = g.density[1];
        // West neighbor would be at ix=-1 — ghost zero; cell (dim-1, 0) should NOT receive.
        let west = g.density[GRASS_GRID_DIM - 1];
        assert!(
            east > 0.0,
            "east neighbor must receive propagation from (0,0); east={east}"
        );
        assert!(
            west < 1e-9,
            "west (wrap) must NOT receive propagation in walled world; west={west}"
        );

        // North neighbor at iy=-1 is ghost zero; cell (0, dim-1) must NOT receive.
        let north_wrap = g.density[(GRASS_GRID_DIM - 1) * GRASS_GRID_DIM];
        assert!(
            north_wrap < 1e-9,
            "north wrap cell must NOT receive propagation in walled world; north_wrap={north_wrap}"
        );

        // South neighbor: cell (0, 1) = index GRASS_GRID_DIM — should receive.
        let south = g.density[GRASS_GRID_DIM];
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
            row_has_density: vec![0u64; GRASS_GRID_DIM.div_ceil(64)],
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
        let mut g = fresh_grid();
        // Only the corner cell (0,0) has density.
        g.density[0] = 0.5;
        // Far corner (dim-1, dim-1) has different density.
        g.density[GRASS_GRID_DIM * GRASS_GRID_DIM - 1] = 0.9;

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
    /// energy is proportional (`energy_per_bite * taken / density_chunk`).
    #[test]
    fn consume_below_chunk_yields_partial_bite() {
        let mut g = fresh_grid();
        g.density[10] = 0.4;
        let taken = g.consume(10, 0.5, 10.0);
        let expected = 10.0 * (0.4 / 0.5);
        assert!(
            (taken - expected).abs() < 1e-5,
            "partial bite must scale energy; expected {expected}, got {taken}"
        );
        assert!(
            g.density[10].abs() < 1e-6,
            "density must drain to 0; got {}",
            g.density[10]
        );
    }

    /// Consume on an empty cell returns zero.
    #[test]
    fn consume_returns_zero_on_empty() {
        let mut g = fresh_grid();
        let taken = g.consume(0, 0.5, 10.0);
        assert_eq!(taken, 0.0, "consume on empty cell must return 0");
        assert_eq!(g.density[0], 0.0, "density must stay 0");
    }

    /// Ripe cell yields the configured energy and drains by the configured chunk.
    #[test]
    fn consume_ripe_cell_yields_energy_and_drains() {
        use crate::constants::GRASS_MAX;
        let mut g = fresh_grid();
        g.density[0] = GRASS_MAX;
        let taken = g.consume(0, 0.5, 7.5);
        assert!(
            (taken - 7.5).abs() < 1e-6,
            "consume should return the energy_per_bite arg; got {taken}"
        );
        assert!(
            (g.density[0] - (GRASS_MAX - 0.5)).abs() < 1e-6,
            "density must drop by exactly density_chunk; got {}",
            g.density[0]
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
        let g1 = GrassGrid::new(&mut rng1, 16);
        let mut rng2 = SimRng::from_u64(42);
        let g2 = GrassGrid::new(&mut rng2, 16);
        assert_eq!(
            g1.density, g2.density,
            "same seed must produce identical density vectors"
        );
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
        g.density[7 * GRASS_GRID_DIM + 5] = 0.4;
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
