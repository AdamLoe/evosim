//! Spatial hash grid for neighbor queries. 5u cells over a 600×600 world
//! → 120×120 = 14,400 cells. Single shared structure for raycasts and
//! repulsion (v5 §3.3).

use crate::constants::*;

pub struct SpatialGrid {
    /// Flattened cell index → contiguous slice of creature indices.
    /// `starts[k]` is the index of the first creature in cell `k`,
    /// `starts[k+1]` is past-the-end. `indices` holds them in cell order.
    pub starts: Vec<u32>,
    pub indices: Vec<u32>,
}

impl Default for SpatialGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialGrid {
    pub fn new() -> Self {
        Self {
            starts: vec![0; HASH_DIM * HASH_DIM + 1],
            indices: Vec::with_capacity(2048),
        }
    }

    #[inline]
    pub fn cell_of(x: f32, y: f32) -> usize {
        let ix = ((x / HASH_CELL) as usize).min(HASH_DIM - 1);
        let iy = ((y / HASH_CELL) as usize).min(HASH_DIM - 1);
        iy * HASH_DIM + ix
    }

    /// Rebuild the index from positions. O(N + cells).
    pub fn rebuild(&mut self, xs: &[f32], ys: &[f32]) {
        let n = xs.len();
        self.starts.iter_mut().for_each(|s| *s = 0);
        for k in 0..n {
            let c = Self::cell_of(xs[k], ys[k]);
            self.starts[c + 1] += 1;
        }
        // prefix sum → offsets
        for k in 1..self.starts.len() {
            self.starts[k] += self.starts[k - 1];
        }
        self.indices.clear();
        self.indices.resize(n, 0);
        // scratch cursors (reuse a buffer to avoid alloc — store inline)
        let mut cursors = self.starts.clone();
        for k in 0..n {
            let c = Self::cell_of(xs[k], ys[k]);
            let pos = cursors[c] as usize;
            self.indices[pos] = k as u32;
            cursors[c] += 1;
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
}
