//! 32-input NN sensors (v1.5 S5b).
//!
//! Three pub(crate) helpers that fill the wall/creature/grass-sector slots of
//! `build_nn_input`:
//!
//! * `compute_wall_proximity` — pure arithmetic, N/S/E/W intensities.
//! * `compute_creature_proximity_sectors` — spatial-grid scan with bilinear
//!   sector splitting at 45° boundaries. Per-sector aggregation uses `max` so
//!   two adjacent neighbors in the same sector don't double-fire.
//! * `compute_grass_density_sectors` — AABB cell walk with per-row bitset
//!   gating and a precomputed sector LUT (eliminates `atan2`). Each cell's
//!   contribution is weighted by the cell area × density / dist², normalized
//!   against a saturation constant so 1.0 means "this sector saturated".
//!
//! Sector convention (world-aligned, clockwise from north):
//!   0 = N (0°), 1 = NE (45°), 2 = E (90°), 3 = SE (135°),
//!   4 = S (180°), 5 = SW (225°), 6 = W (270°), 7 = NW (315°).
//!
//! Angle of a target relative to the creature is `atan2(dx, -dy)` (clockwise
//! from north) so positive-y world ("south") yields ~180° and positive-x
//! ("east") yields ~90°. Sector index = `floor(angle / 45°) % 8`.

use std::sync::OnceLock;

use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grass::GrassGrid;
use crate::grid::SpatialGrid;

/// Precomputed cell-offset starburst for `compute_creature_proximity_sectors`.
///
/// Each entry is `(dxo, dyo, min_dist_sq)` where:
///   - `(dxo, dyo)` is an integer cell offset from the creature's home hash
///     cell, both in `[-MAX_OFFSET, MAX_OFFSET]`.
///   - `min_dist_sq` is the squared *lower bound* on the world-unit distance
///     from the creature to anything in that cell. Specifically:
///         min_dist = sqrt((max(0, |dxo|-1) * HASH_CELL)² +
///                         (max(0, |dyo|-1) * HASH_CELL)²)
///     The `-1` accounts for the creature sitting anywhere inside its home
///     cell — the home cell and any 8-neighbor overlap on at least one axis.
///
/// The list is sorted by `min_dist_sq` ascending, with `(|dxo|, |dyo|, dxo,
/// dyo)` as the deterministic tiebreak. Cells whose `min_dist >= PROXIMITY_RANGE`
/// are excluded — they can't contribute.
///
/// The walk consumer (creature proximity) iterates this list in order,
/// maintains per-sector `best_intensity`, and breaks when every sector has
/// been touched and the current cell's max possible intensity (`1 -
/// min_dist/range`) is `≤ min(best_intensity)`.
fn proximity_starburst() -> &'static [(i32, i32, f32)] {
    static CELL: OnceLock<Vec<(i32, i32, f32)>> = OnceLock::new();
    CELL.get_or_init(|| {
        let range = PROXIMITY_RANGE;
        // Max cell offset whose lower-bound min_dist can still be < range.
        // min_dist ≥ (|dxo| - 1) * HASH_CELL when on-axis, so the largest
        // useful |dxo| is `ceil(range / HASH_CELL) + 1`.
        let max_off = ((range / HASH_CELL).ceil() as i32) + 1;
        let mut out: Vec<(i32, i32, f32)> =
            Vec::with_capacity(((2 * max_off + 1) * (2 * max_off + 1)) as usize);
        let cell = HASH_CELL;
        let range2 = range * range;
        for dyo in -max_off..=max_off {
            for dxo in -max_off..=max_off {
                let mdx = (dxo.unsigned_abs() as i32 - 1).max(0) as f32 * cell;
                let mdy = (dyo.unsigned_abs() as i32 - 1).max(0) as f32 * cell;
                let min_dist_sq = mdx * mdx + mdy * mdy;
                if min_dist_sq >= range2 {
                    continue;
                }
                out.push((dxo, dyo, min_dist_sq));
            }
        }
        out.sort_by(|a, b| {
            a.2.partial_cmp(&b.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.abs().cmp(&b.0.abs()))
                .then(a.1.abs().cmp(&b.1.abs()))
                .then(a.0.cmp(&b.0))
                .then(a.1.cmp(&b.1))
        });
        out
    })
}

/// Half-cell offset window used by `build_sector_lut`. The LUT covers integer
/// cell offsets in `-LUT_RADIUS..=+LUT_RADIUS` along each axis. At v1.5 S6's
/// `GRASS_CELL_SIZE = 1.25` this covers `PROXIMITY_RANGE = 20` exactly
/// (20 / 1.25 = 16 cells).
pub(crate) const LUT_RADIUS: i32 = 16;
/// LUT side length (`2*LUT_RADIUS + 1 = 33`).
pub(crate) const LUT_DIM: usize = (LUT_RADIUS as usize) * 2 + 1;

/// Saturation constant for `compute_grass_density_sectors`. Tuned so a full
/// cap of GRASS_MAX-density cells right next to the creature pushes a sector
/// well into [0, 1]; clamping keeps the output bounded.
const GRASS_SECTOR_SAT: f32 = 4.0;

/// Build the (sector, weight) lookup table indexed by integer cell offset
/// `(dx, dy) ∈ [-LUT_RADIUS, LUT_RADIUS]² `. The stored tuple is
/// `(primary_sector, adjacent_sector, weight_to_primary)`. Consumers split a
/// contribution into `out[primary] += value * weight` and
/// `out[adj] += value * (1 - weight)` — no `atan2` in the hot path.
///
/// Precomputing `adjacent_sector` (instead of recomputing it from real dx/dy
/// at consumer time) is consistent with `weight` already being LUT-approximated:
/// both share the same ±1.8°-at-d=20u angle error from snapping the source
/// position to its containing cell center. Saves one `atan2` per cell.
///
/// Size: 33×33 entries × 8 bytes (with padding) ≈ 8.7 KB. Fits in L1.
pub(crate) fn build_sector_lut() -> Vec<(u8, u8, f32)> {
    let mut lut = vec![(0u8, 0u8, 0.0f32); LUT_DIM * LUT_DIM];
    let two_pi = std::f32::consts::TAU;
    let sector_width = two_pi / NN_SECTORS as f32;
    for dy in -LUT_RADIUS..=LUT_RADIUS {
        for dx in -LUT_RADIUS..=LUT_RADIUS {
            let entry = if dx == 0 && dy == 0 {
                // Degenerate: pin to sector 0 with full weight; adj is arbitrary
                // because (1 - weight) = 0 makes the adj column unused.
                (0u8, 1u8, 1.0f32)
            } else {
                // angle in radians, clockwise from north (+y down).
                let ang = (dx as f32).atan2(-dy as f32);
                let ang = if ang < 0.0 { ang + two_pi } else { ang };
                // Sector width = 45° = π/4. Sector 0 is centered on north (0°);
                // the boundary between sector s and s+1 is at (s + 0.5)*45°.
                let sector_f = ang / sector_width;
                let s_round = sector_f.round() as i32;
                let primary = s_round.rem_euclid(NN_SECTORS as i32) as u8;
                // Signed distance to primary center in sector-widths;
                // negative = source is rotationally before center → adj = primary-1.
                let signed_frac = sector_f - s_round as f32;
                let adj = if signed_frac >= 0.0 {
                    ((primary as i32 + 1).rem_euclid(NN_SECTORS as i32)) as u8
                } else {
                    ((primary as i32 - 1).rem_euclid(NN_SECTORS as i32)) as u8
                };
                let weight = 1.0 - signed_frac.abs();
                (primary, adj, weight)
            };
            let ix = (dx + LUT_RADIUS) as usize;
            let iy = (dy + LUT_RADIUS) as usize;
            lut[iy * LUT_DIM + ix] = entry;
        }
    }
    lut
}

/// Wall-proximity intensities — N/S/E/W. 1.0 at the wall, 0.0 at
/// ≥ `WALL_PROXIMITY_RANGE` units away. Pure arithmetic; no spatial-grid query.
#[inline]
pub(crate) fn compute_wall_proximity(x: f32, y: f32) -> [f32; 4] {
    let r = WALL_PROXIMITY_RANGE;
    let n = (1.0 - y / r).clamp(0.0, 1.0);
    let s = (1.0 - (WORLD_SIZE - y) / r).clamp(0.0, 1.0);
    let e = (1.0 - (WORLD_SIZE - x) / r).clamp(0.0, 1.0);
    let w = (1.0 - x / r).clamp(0.0, 1.0);
    [n, s, e, w]
}

/// Compute the world-aligned sector for a `(dx, dy)` displacement.
/// Returns `(primary_sector, weight_to_primary)`; the complement goes to the
/// adjacent sector — see `adjacent_sector`.
#[inline]
fn sector_of(dx: f32, dy: f32) -> (u8, f32) {
    let ang = dx.atan2(-dy);
    let two_pi = std::f32::consts::TAU;
    let ang = if ang < 0.0 { ang + two_pi } else { ang };
    let sector_f = ang / (two_pi / NN_SECTORS as f32);
    let s_round = sector_f.round() as i32;
    let primary = s_round.rem_euclid(NN_SECTORS as i32) as u8;
    let frac = (sector_f - s_round as f32).abs();
    (primary, 1.0 - frac)
}

/// Pick the adjacent sector (`±1`) that the bilinear split should bleed into,
/// based on which side of the primary's center the source point sits on.
#[inline]
fn adjacent_sector(dx: f32, dy: f32, primary: u8) -> u8 {
    let ang = dx.atan2(-dy);
    let two_pi = std::f32::consts::TAU;
    let ang = if ang < 0.0 { ang + two_pi } else { ang };
    let center = primary as f32 * (two_pi / NN_SECTORS as f32);
    // Wrap delta into (-π, π].
    let mut delta = ang - center;
    if delta > std::f32::consts::PI {
        delta -= two_pi;
    } else if delta < -std::f32::consts::PI {
        delta += two_pi;
    }
    let step: i32 = if delta >= 0.0 { 1 } else { -1 };
    ((primary as i32 + step).rem_euclid(NN_SECTORS as i32)) as u8
}

/// Fill `out[0..8]` with creature-proximity intensities per sector. Bilinear
/// split at sector boundaries (a hit at 50° contributes 0.78 to sector 1 (45°)
/// and 0.22 to sector 2 (90°)). Per-sector aggregation uses `max` so two
/// neighbors in the same sector don't push the channel above the single-
/// neighbor saturation.
///
/// Performance: instead of walking every cell in the `PROXIMITY_RANGE` bbox
/// (~256 cells at `HASH_CELL=2.5u`, `PROXIMITY_RANGE=20u`), we walk a
/// precomputed starburst list of cell offsets ordered by lower-bound distance
/// from the creature's home cell. Per sector we track the best intensity so
/// far; once every sector has been touched AND the current cell's max
/// possible intensity can no longer beat the worst surviving sector, we
/// break. In dense clumps the walk typically stops after the home cell + a
/// handful of neighbors, dropping the per-creature scan from O(K) to ~O(1).
pub(crate) fn compute_creature_proximity_sectors(
    x: f32,
    y: f32,
    self_id: usize,
    grid: &SpatialGrid,
    creatures: &CreatureSoA,
    out: &mut [f32; 8],
) {
    *out = [0.0f32; 8];
    let range = PROXIMITY_RANGE;
    let inv_range = 1.0 / range;
    let range2 = range * range;
    let cell = HASH_CELL;
    let inv_cell = 1.0 / cell;
    let dim = HASH_DIM as i32;

    let ix0 = (x * inv_cell).floor() as i32;
    let iy0 = (y * inv_cell).floor() as i32;

    // Per-sector best intensity found so far.
    let mut best = [0.0f32; 8];
    // Count of sectors with best > 0. The early-bail condition needs every
    // sector to have at least one hit before it can fire.
    let mut sectors_hit: u32 = 0;
    // Smallest best[] value among sectors we've already lit; any cell whose
    // max-achievable intensity is ≤ this can't improve any sector once all 8
    // are lit. Maintained incrementally so the per-cell test is one f32 cmp.
    let mut min_best: f32 = 0.0;

    for &(dxo, dyo, min_dist_sq) in proximity_starburst() {
        // Max intensity achievable from any creature in this cell.
        // = 1 - min_dist/range; once that ≤ 0 the rest of the starburst is
        // also out of range (offsets are sorted ascending by min_dist).
        let min_dist = min_dist_sq.sqrt();
        let max_intensity = 1.0 - min_dist * inv_range;
        if max_intensity <= 0.0 {
            break;
        }
        // Once every sector is lit AND no further cell can beat the worst
        // sector, we're done.
        if sectors_hit == 8 && max_intensity <= min_best {
            break;
        }

        let ix = ix0 + dxo;
        let iy = iy0 + dyo;
        if ix < 0 || ix >= dim || iy < 0 || iy >= dim {
            continue;
        }
        let c = iy as usize * HASH_DIM + ix as usize;
        let s = grid.starts[c] as usize;
        let e = grid.starts[c + 1] as usize;
        for &idx_u32 in &grid.indices[s..e] {
            let j = idx_u32 as usize;
            if j == self_id {
                continue;
            }
            let dx = creatures.x[j] - x;
            let dy = creatures.y[j] - y;
            let d2 = dx * dx + dy * dy;
            if d2 > range2 {
                continue;
            }
            let d = d2.sqrt();
            let intensity = 1.0 - d * inv_range;
            if intensity <= 0.0 {
                continue;
            }
            let (primary, w) = sector_of(dx, dy);
            let adj = adjacent_sector(dx, dy, primary);
            let p_val = intensity * w;
            let a_val = intensity * (1.0 - w);
            let p_idx = primary as usize;
            let a_idx = adj as usize;
            if p_val > best[p_idx] {
                if best[p_idx] == 0.0 {
                    sectors_hit += 1;
                }
                best[p_idx] = p_val;
            }
            if a_val > best[a_idx] {
                if best[a_idx] == 0.0 {
                    sectors_hit += 1;
                }
                best[a_idx] = a_val;
            }
        }

        // Recompute min_best lazily: only matters once we're trying to bail.
        if sectors_hit == 8 {
            let mut mb = best[0];
            for &v in &best[1..] {
                if v < mb {
                    mb = v;
                }
            }
            min_best = mb;
        }
    }

    *out = best;
}

/// Fill `out[0..8]` with grass-density intensities per sector. Walks the cells
/// inside an AABB of side `2*PROXIMITY_RANGE`, gated by the per-row bitset on
/// `grass`. Each cell's contribution is `density × cell_area / dist²`; the
/// sum per sector is normalized by `GRASS_SECTOR_SAT` and clamped to `[0, 1]`.
pub(crate) fn compute_grass_density_sectors(
    x: f32,
    y: f32,
    grass: &GrassGrid,
    sector_lut: &[(u8, u8, f32)],
    out: &mut [f32; 8],
) {
    *out = [0.0f32; 8];
    let range = GRASS_PROXIMITY_RANGE;
    let range2 = range * range;
    let cell = GRASS_CELL_SIZE;
    let inv_cell = 1.0 / cell;
    let cell_area = cell * cell;
    let dim = GRASS_GRID_DIM as i32;

    // Creature cell (in-bounds query positions are clamped at world edges; the
    // grid walk's lo/hi guards do the rest).
    let cx_cell = (x * inv_cell).floor() as i32;
    let cy_cell = (y * inv_cell).floor() as i32;
    // No `+ 1` slack: a cell with center at offset (r_cells, 0) is at
    // `r_cells * cell` world units away. With `r_cells = ceil(range / cell)`,
    // the farthest in-range cell is at offset `floor(range / cell)`, which
    // equals LUT_RADIUS at default params. Any cell at offset > LUT_RADIUS is
    // strictly past `range` and gets culled by `d2 > range2` — no LUT access
    // outside the ±LUT_RADIUS window.
    let r_cells = (range * inv_cell).ceil() as i32;
    debug_assert!(
        r_cells <= LUT_RADIUS,
        "r_cells={r_cells} > LUT_RADIUS={LUT_RADIUS}: bump LUT_RADIUS or shrink range"
    );

    let iy_lo = (cy_cell - r_cells).max(0);
    let iy_hi = (cy_cell + r_cells).min(dim - 1);
    let ix_lo = (cx_cell - r_cells).max(0);
    let ix_hi = (cx_cell + r_cells).min(dim - 1);

    for iy in iy_lo..=iy_hi {
        if !grass.row_nonempty(iy as usize) {
            continue;
        }
        let row_off = iy as usize * GRASS_GRID_DIM;
        let lut_y = (iy - cy_cell + LUT_RADIUS) as usize;
        for ix in ix_lo..=ix_hi {
            let d = grass.density[row_off + ix as usize];
            if d <= 0.0 {
                continue;
            }
            // Cell center in world-units.
            let cx = (ix as f32 + 0.5) * cell;
            let cy = (iy as f32 + 0.5) * cell;
            let dx = cx - x;
            let dy = cy - y;
            let d2 = dx * dx + dy * dy;
            if d2 > range2 {
                continue;
            }
            // LUT covers ±LUT_RADIUS along each axis; r_cells <= LUT_RADIUS
            // (guaranteed by the debug_assert above + per-frame constants) so
            // no clamp is needed and the lookup is pure arithmetic.
            let lut_x = (ix - cx_cell + LUT_RADIUS) as usize;
            debug_assert!(lut_x < LUT_DIM && lut_y < LUT_DIM);
            let (primary, adj, w) = sector_lut[lut_y * LUT_DIM + lut_x];
            // 1/d² weight, with a small floor to avoid singularity at d=0.
            let inv_d2 = 1.0 / d2.max(0.25);
            let contribution = d * cell_area * inv_d2;
            out[primary as usize] += contribution * w;
            out[adj as usize] += contribution * (1.0 - w);
        }
    }

    let inv_sat = 1.0 / GRASS_SECTOR_SAT;
    for s in out.iter_mut() {
        *s = (*s * inv_sat).clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_proximity_corner_is_near_one() {
        let p = compute_wall_proximity(1.0, 1.0);
        // N (y=1) and W (x=1) should be ~0.98; S/E ~0.
        assert!((p[0] - 0.98).abs() < 1e-3, "N = {}", p[0]);
        assert_eq!(p[1], 0.0, "S = {}", p[1]);
        assert_eq!(p[2], 0.0, "E = {}", p[2]);
        assert!((p[3] - 0.98).abs() < 1e-3, "W = {}", p[3]);
    }

    #[test]
    fn wall_proximity_far_from_walls_is_zero() {
        let p = compute_wall_proximity(WORLD_SIZE * 0.5, WORLD_SIZE * 0.5);
        for v in p {
            assert_eq!(v, 0.0, "interior must have all walls at 0");
        }
    }

    /// A point directly north (dx=0, dy=-r) lands cleanly in sector 0.
    #[test]
    fn sector_lut_cardinals_lock_in() {
        let (n, _) = sector_of(0.0, -1.0);
        let (e, _) = sector_of(1.0, 0.0);
        let (s, _) = sector_of(0.0, 1.0);
        let (w, _) = sector_of(-1.0, 0.0);
        assert_eq!(n, 0);
        assert_eq!(e, 2);
        assert_eq!(s, 4);
        assert_eq!(w, 6);
    }

    /// A point at the NE diagonal (45°) sits at the sector-1 center; weight ≈ 1.0.
    #[test]
    fn sector_lut_ne_diagonal_is_sector_one() {
        let (p, w) = sector_of(1.0, -1.0);
        assert_eq!(p, 1, "NE diagonal should be sector 1");
        assert!(
            w > 0.99,
            "weight at center of sector 1 should be ~1.0; got {w}"
        );
    }
}
