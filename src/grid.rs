//! Spatial hash grid for neighbor queries. `HASH_CELL`-wide cells over the
//! runtime-sized world. v2.0 Wave 1a: `hash_dim = ceil(world_size / HASH_CELL)`
//! is computed at construction and carried in `WorldDims` (960² at the 9600u
//! default). Single shared structure for eat/repulsion neighbor queries
//! (v5 §3.3). Walled worlds clamp cell indices; toroidal worlds (`dims.wrap_world`)
//! wrap cell indices across the seam.

use crate::constants::*;

pub struct SpatialGrid {
    /// Runtime dimensions (hash_dim / wrap). Set at construction; the cell
    /// arrays are sized to `hash_dim²`.
    pub(crate) dims: WorldDims,
    /// Flattened cell index → contiguous slice of creature indices.
    /// `starts[k]` is the index of the first creature in cell `k`,
    /// `starts[k+1]` is past-the-end. `indices` holds them in cell order.
    pub(crate) starts: Vec<u32>,
    pub(crate) indices: Vec<u32>,
    /// Scratch cursors for the scatter pass of `rebuild`.
    /// Length matches `starts` (`hash_dim² + 1`).
    /// Reused across rebuilds via `copy_from_slice(&starts)` to avoid the
    /// per-tick allocation that `starts.clone()` would otherwise cost.
    /// Must always be the same length as `starts`; resize both in lockstep.
    cursors: Vec<u32>,
    /// Per-creature cached cell index, computed once in `rebuild`.
    /// S32: avoids re-calling `cell_of(x, y)` twice per creature (count pass
    /// and write pass). Length == creature count after each rebuild.
    /// Downstream consumers may read `grid.cells[i]` instead of re-calling `cell_of`.
    pub(crate) cells: Vec<u32>,
}

impl SpatialGrid {
    /// Construct a grid sized for the given world dims.
    pub fn new(dims: WorldDims) -> Self {
        let starts = vec![0; dims.hash_dim * dims.hash_dim + 1];
        let cursors = vec![0; starts.len()];
        Self {
            dims,
            starts,
            indices: Vec::with_capacity(2048),
            cursors,
            cells: Vec::with_capacity(2048),
        }
    }

    /// Cell index for a world position. Walled: clamp the per-axis index to
    /// `[0, hash_dim-1]`. Toroidal: wrap it into `[0, hash_dim)`.
    #[inline]
    pub fn cell_of(&self, x: f32, y: f32) -> usize {
        let dim = self.dims.hash_dim as i32;
        let ix = (x / HASH_CELL).floor() as i32;
        let iy = (y / HASH_CELL).floor() as i32;
        let (ix, iy) = if self.dims.wrap_world {
            (ix.rem_euclid(dim), iy.rem_euclid(dim))
        } else {
            (ix.clamp(0, dim - 1), iy.clamp(0, dim - 1))
        };
        iy as usize * self.dims.hash_dim + ix as usize
    }

    /// Rebuild the index from positions. O(N + cells).
    /// S32: computes `cell_of(x, y)` once per creature and caches it in
    /// `self.cells[k]`, avoiding a redundant second call in the write pass.
    pub fn rebuild(&mut self, xs: &[f32], ys: &[f32]) {
        let n = xs.len();
        // S32: cache cell_of per creature (count pass + write pass share the cache).
        self.cells.resize(n, 0);
        self.starts.iter_mut().for_each(|s| *s = 0);
        for k in 0..n {
            let c = self.cell_of(xs[k], ys[k]);
            self.cells[k] = c as u32;
            self.starts[c + 1] += 1;
        }
        // prefix sum → offsets
        for k in 1..self.starts.len() {
            self.starts[k] += self.starts[k - 1];
        }
        self.indices.clear();
        self.indices.resize(n, 0);
        // Reset cursors to the prefix-sum boundaries without allocating.
        self.cursors.copy_from_slice(&self.starts);
        for k in 0..n {
            let c = self.cells[k] as usize; // S32: read from cache, no recompute
            let pos = self.cursors[c] as usize;
            self.indices[pos] = k as u32;
            self.cursors[c] += 1;
        }
    }

    /// Find the first creature index in cells inside the bounding box around
    /// `(x, y)` with the given radius for which `pred(idx) == true`. Within
    /// each cell the iteration starts at `rotate_seed % K_cell` and wraps,
    /// so callers can de-correlate "first" from creature-insertion order
    /// (which biases toward older creatures since lower SoA indices skew
    /// older). Cells themselves are visited in the standard `iy, ix` walk.
    ///
    /// Used by `World::eat` so multi-creature stacks distribute their bite
    /// targets across the cell-mates rather than always biting the same
    /// (oldest) slot. Determinism is preserved — the rotation is a pure
    /// function of `rotate_seed` and the cell layout.
    pub fn find_first_in_radius(
        &self,
        x: f32,
        y: f32,
        radius: f32,
        rotate_seed: usize,
        mut pred: impl FnMut(usize) -> bool,
    ) -> Option<usize> {
        debug_assert!(
            radius < self.dims.world_size * 0.5,
            "find_first_in_radius requires radius < half-world; got {radius}"
        );
        let dim = self.dims.hash_dim as i32;
        let wrap = self.dims.wrap_world;
        let lo_x = ((x - radius) / HASH_CELL).floor() as i32;
        let hi_x = ((x + radius) / HASH_CELL).floor() as i32;
        let lo_y = ((y - radius) / HASH_CELL).floor() as i32;
        let hi_y = ((y + radius) / HASH_CELL).floor() as i32;
        for jy in lo_y..=hi_y {
            // Walled: skip out-of-bounds; toroidal: fold across the seam.
            let iy = if wrap {
                jy.rem_euclid(dim)
            } else if jy < 0 || jy >= dim {
                continue;
            } else {
                jy
            };
            let row = iy as usize * self.dims.hash_dim;
            for jx in lo_x..=hi_x {
                let ix = if wrap {
                    jx.rem_euclid(dim)
                } else if jx < 0 || jx >= dim {
                    continue;
                } else {
                    jx
                };
                let c = row + ix as usize;
                let s = self.starts[c] as usize;
                let e = self.starts[c + 1] as usize;
                let k_cell = e - s;
                if k_cell == 0 {
                    continue;
                }
                let off0 = rotate_seed % k_cell;
                for k in 0..k_cell {
                    let pos = s + ((off0 + k) % k_cell);
                    let idx = self.indices[pos] as usize;
                    if pred(idx) {
                        return Some(idx);
                    }
                }
            }
        }
        None
    }

    /// Iterate creature indices in cells inside a bounding box around
    /// `(x, y)` with the given radius. Caller must filter by exact distance.
    ///
    /// Walled world: cell indices are clamped to `[0, hash_dim-1]`; out-of-bounds
    /// cells are skipped (no seam wrap). Toroidal world (`dims.wrap_world`):
    /// cell indices wrap across the seam so a near-edge query also visits the
    /// opposite edge. The caller's exact-distance filter must be wrap-aware too.
    pub fn for_each_in_radius(&self, x: f32, y: f32, radius: f32, mut f: impl FnMut(usize)) {
        debug_assert!(
            radius < self.dims.world_size * 0.5,
            "for_each_in_radius requires radius < half-world; got {radius}"
        );
        let dim = self.dims.hash_dim as i32;
        let wrap = self.dims.wrap_world;
        let lo_x = ((x - radius) / HASH_CELL).floor() as i32;
        let hi_x = ((x + radius) / HASH_CELL).floor() as i32;
        let lo_y = ((y - radius) / HASH_CELL).floor() as i32;
        let hi_y = ((y + radius) / HASH_CELL).floor() as i32;
        for jy in lo_y..=hi_y {
            let iy = if wrap {
                jy.rem_euclid(dim)
            } else if jy < 0 || jy >= dim {
                continue;
            } else {
                jy
            };
            let row = iy as usize * self.dims.hash_dim;
            for jx in lo_x..=hi_x {
                let ix = if wrap {
                    jx.rem_euclid(dim)
                } else if jx < 0 || jx >= dim {
                    continue;
                } else {
                    jx
                };
                let c = row + ix as usize;
                let s = self.starts[c] as usize;
                let e = self.starts[c + 1] as usize;
                for &idx in &self.indices[s..e] {
                    f(idx as usize);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // v2.0 Wave 1a: grid dims are runtime. Fixed *walled* test dims pin a 1200u
    // world at the 10u hash cell → 120² cells, so the walled-behavior assertions
    // below keep their meaning. The wrap-aware behavior has its own test file.
    const TEST_DIMS: WorldDims = WorldDims {
        world_size: 1200.0,
        wrap_world: false,
        grass_dim: 240,
        grass_cell_count: 240 * 240,
        hash_dim: 120,
    };
    const HASH_DIM: usize = TEST_DIMS.hash_dim;
    const WORLD_SIZE: f32 = TEST_DIMS.world_size;

    fn grid() -> SpatialGrid {
        SpatialGrid::new(TEST_DIMS)
    }

    /// Walled world: cell_of clamps negative x/y to 0 (not wrap to last column).
    #[test]
    fn cell_of_clamps_negative() {
        let g = grid();
        // x = -1.0 → clamp to column 0.
        let c = g.cell_of(-1.0, 0.0);
        assert_eq!(c, 0, "negative x should clamp to column 0");
        let c2 = g.cell_of(0.0, -1.0);
        assert_eq!(c2, 0, "negative y should clamp to row 0");
    }

    /// Walled world: cell_of with x/y >= WORLD_SIZE clamps to last cell.
    #[test]
    fn cell_of_clamps_above() {
        let g = grid();
        // x = WORLD_SIZE = 1200.0 → floor(120) = 120 → clamp(119) → last column.
        let c = g.cell_of(WORLD_SIZE, 0.0);
        assert_eq!(
            c,
            HASH_DIM - 1,
            "x == WORLD_SIZE should clamp to last column"
        );
        let c2 = g.cell_of(0.0, WORLD_SIZE);
        assert_eq!(
            c2,
            (HASH_DIM - 1) * HASH_DIM,
            "y == WORLD_SIZE should clamp to last row"
        );
    }

    /// S32: the cached cells[] array matches a direct recompute of cell_of for
    /// every creature after rebuild.
    #[test]
    fn cell_of_cache_matches_recompute() {
        let xs = vec![0.0, 4.9, 5.1, 100.0, 299.9, 595.0];
        let ys = vec![0.0, 4.9, 5.1, 200.0, 300.0, 595.0];
        let mut g = grid();
        g.rebuild(&xs, &ys);
        assert_eq!(
            g.cells.len(),
            xs.len(),
            "cells length must equal creature count"
        );
        for k in 0..xs.len() {
            let expected = g.cell_of(xs[k], ys[k]);
            assert_eq!(
                g.cells[k] as usize, expected,
                "cells[{k}] = {} != recomputed {} for ({}, {})",
                g.cells[k], expected, xs[k], ys[k]
            );
        }
    }

    #[test]
    fn rebuild_indexes_correctly() {
        let xs = vec![0.0, 4.9, 5.1, 595.0];
        let ys = vec![0.0, 4.9, 5.1, 595.0];
        let mut g = grid();
        g.rebuild(&xs, &ys);
        // Collect all (cell, idx) by scanning
        let mut found_idx0 = false;
        g.for_each_in_radius(0.0, 0.0, 1.0, |i| {
            if i == 0 || i == 1 {
                found_idx0 = true;
            }
        });
        assert!(found_idx0);
        // The far corner creature is reachable when we query its cell.
        let mut found_corner = false;
        g.for_each_in_radius(595.0, 595.0, 1.0, |i| {
            if i == 3 {
                found_corner = true;
            }
        });
        assert!(found_corner);
    }

    #[test]
    fn radius_query_includes_all_within_box() {
        let xs = vec![100.0, 110.0, 120.0, 200.0];
        let ys = vec![100.0, 100.0, 100.0, 100.0];
        let mut g = grid();
        g.rebuild(&xs, &ys);
        let mut hits = vec![];
        g.for_each_in_radius(110.0, 100.0, 15.0, |i| hits.push(i));
        hits.sort();
        assert_eq!(hits, vec![0, 1, 2]);
    }

    #[test]
    fn rebuild_twice_with_different_positions_is_correct() {
        // First build: 3 creatures clustered near (0,0).
        let xs = vec![1.0, 2.0, 3.0];
        let ys = vec![1.0, 2.0, 3.0];
        let mut g = grid();
        g.rebuild(&xs, &ys);

        // Second build: same creatures moved to the opposite corner.
        let xs2 = vec![590.0, 591.0, 592.0];
        let ys2 = vec![590.0, 591.0, 592.0];
        g.rebuild(&xs2, &ys2);

        // After the second rebuild, the near-origin cell must be empty
        // and the far-corner cell must contain all three creatures.
        let mut near = vec![];
        g.for_each_in_radius(0.0, 0.0, 1.0, |i| near.push(i));
        assert!(
            near.is_empty(),
            "near-origin cell should be empty after rebuild; got {near:?}"
        );

        let mut far = vec![];
        g.for_each_in_radius(595.0, 595.0, 10.0, |i| far.push(i));
        far.sort();
        assert_eq!(
            far,
            vec![0, 1, 2],
            "far-corner cell should contain all three creatures after the second rebuild"
        );
    }

    /// Walled world: a creature near x=595 should NOT be found by a query centered
    /// at x=5 with small radius — there is no seam wrap.
    #[test]
    fn for_each_in_radius_no_seam_wrap() {
        let xs = vec![5.0, 595.0];
        let ys = vec![300.0, 300.0];
        let mut g = grid();
        g.rebuild(&xs, &ys);

        let mut found_b = false;
        g.for_each_in_radius(5.0, 300.0, 15.0, |i| {
            if i == 1 {
                found_b = true;
            }
        });
        assert!(
            !found_b,
            "walled world: creature at x=595 must NOT be found by query at x=5 r=15"
        );
    }

    /// Walled world: for_each_in_radius at world edge does not panic when
    /// the query box extends outside [0, WORLD_SIZE).
    #[test]
    fn for_each_in_radius_edge_query_no_panic() {
        let xs = vec![1.0, 599.0];
        let ys = vec![300.0, 300.0];
        let mut g = grid();
        g.rebuild(&xs, &ys);

        // Query that would go below 0 on x.
        let mut hits = vec![];
        g.for_each_in_radius(1.0, 300.0, 8.0, |i| hits.push(i));
        assert!(
            hits.contains(&0),
            "creature at x=1 must be found near left edge"
        );

        // Query that would go above WORLD_SIZE on x.
        let mut hits2 = vec![];
        g.for_each_in_radius(599.0, 300.0, 8.0, |i| hits2.push(i));
        assert!(
            hits2.contains(&1),
            "creature at x=599 must be found near right edge"
        );
    }
}
