//! Spatial hash grid for neighbor queries. 5u cells over a 600×600 world
//! → 120×120 = 14,400 cells. Single shared structure for raycasts and
//! repulsion (v5 §3.3).

use crate::constants::*;

pub struct SpatialGrid {
    /// Flattened cell index → contiguous slice of creature indices.
    /// `starts[k]` is the index of the first creature in cell `k`,
    /// `starts[k+1]` is past-the-end. `indices` holds them in cell order.
    pub(crate) starts: Vec<u32>,
    pub(crate) indices: Vec<u32>,
    /// Scratch cursors for the scatter pass of `rebuild`.
    /// Length matches `starts` (HASH_DIM * HASH_DIM + 1 = 14 401).
    /// Reused across rebuilds via `copy_from_slice(&starts)` to
    /// avoid the per-tick allocation that `starts.clone()` would
    /// otherwise cost (~57 kB × 3 rebuilds/tick = ~173 kB/tick).
    /// Must always be the same length as `starts`; resize both in lockstep.
    cursors: Vec<u32>,
    /// Per-creature cached cell index, computed once in `rebuild`.
    /// S32: avoids re-calling `cell_of(x, y)` twice per creature (count pass
    /// and write pass). Length == creature count after each rebuild.
    /// Downstream consumers may read `grid.cells[i]` instead of re-calling `cell_of`.
    pub(crate) cells: Vec<u32>,
}

impl Default for SpatialGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialGrid {
    pub fn new() -> Self {
        let starts = vec![0; HASH_DIM * HASH_DIM + 1];
        let cursors = vec![0; starts.len()];
        Self {
            starts,
            indices: Vec::with_capacity(2048),
            cursors,
            cells: Vec::with_capacity(2048),
        }
    }

    #[inline]
    pub fn cell_of(x: f32, y: f32) -> usize {
        // Toroidal world: wrap cell index modulo HASH_DIM via rem_euclid.
        // Integer rem_euclid is bit-stable; avoids float % surprises.
        // See v1.2 grass mechanic brief.
        let ix = (x / HASH_CELL).floor() as i32;
        let iy = (y / HASH_CELL).floor() as i32;
        let ix = ix.rem_euclid(HASH_DIM as i32) as usize;
        let iy = iy.rem_euclid(HASH_DIM as i32) as usize;
        iy * HASH_DIM + ix
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
            let c = Self::cell_of(xs[k], ys[k]);
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

    /// Iterate creature indices in cells inside a bounding box around
    /// `(x, y)` with the given radius. Caller must filter by exact distance.
    pub fn for_each_in_radius(&self, x: f32, y: f32, radius: f32, mut f: impl FnMut(usize)) {
        let lo_x = (((x - radius) / HASH_CELL).floor() as i32).max(0) as usize;
        let hi_x =
            (((x + radius) / HASH_CELL).floor() as i32).clamp(0, HASH_DIM as i32 - 1) as usize;
        let lo_y = (((y - radius) / HASH_CELL).floor() as i32).max(0) as usize;
        let hi_y =
            (((y + radius) / HASH_CELL).floor() as i32).clamp(0, HASH_DIM as i32 - 1) as usize;
        for iy in lo_y..=hi_y {
            for ix in lo_x..=hi_x {
                let c = iy * HASH_DIM + ix;
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

    /// Toroidal: cell_of with negative x/y wraps to the far edge (seam wrap).
    #[test]
    fn cell_of_wraps_negative() {
        // x = -1.0 → floor(-0.2) = -1 → rem_euclid(120) = 119 → cell column 119.
        let c = SpatialGrid::cell_of(-1.0, 0.0);
        assert_eq!(c, HASH_DIM - 1, "negative x should wrap to last column");
        let c2 = SpatialGrid::cell_of(0.0, -1.0);
        assert_eq!(
            c2,
            (HASH_DIM - 1) * HASH_DIM,
            "negative y should wrap to last row"
        );
    }

    /// Toroidal: cell_of with x/y >= WORLD_SIZE wraps to the near edge.
    #[test]
    fn cell_of_wraps_above() {
        // x = WORLD_SIZE = 600.0 → floor(120) = 120 → rem_euclid(120) = 0 → column 0.
        let c = SpatialGrid::cell_of(WORLD_SIZE, 0.0);
        assert_eq!(c, 0, "x == WORLD_SIZE should wrap to column 0");
        let c2 = SpatialGrid::cell_of(0.0, WORLD_SIZE);
        assert_eq!(c2, 0, "y == WORLD_SIZE should wrap to row 0");
    }

    /// S32: the cached cells[] array matches a direct recompute of cell_of for
    /// every creature after rebuild.
    #[test]
    fn cell_of_cache_matches_recompute() {
        let xs = vec![0.0, 4.9, 5.1, 100.0, 299.9, 595.0];
        let ys = vec![0.0, 4.9, 5.1, 200.0, 300.0, 595.0];
        let mut g = SpatialGrid::new();
        g.rebuild(&xs, &ys);
        assert_eq!(
            g.cells.len(),
            xs.len(),
            "cells length must equal creature count"
        );
        for k in 0..xs.len() {
            let expected = SpatialGrid::cell_of(xs[k], ys[k]);
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
        let mut g = SpatialGrid::new();
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
        let mut g = SpatialGrid::new();
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
        let mut g = SpatialGrid::new();
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
}
