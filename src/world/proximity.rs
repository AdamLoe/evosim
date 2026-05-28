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

use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grass::GrassGrid;
use crate::grid::SpatialGrid;

/// Half-cell offset window used by `build_sector_lut`. The LUT covers integer
/// cell offsets in `-LUT_RADIUS..=+LUT_RADIUS` along each axis. At the current
/// `GRASS_CELL_SIZE = 5` this comfortably covers `PROXIMITY_RANGE = 20` (4
/// cells); a 16-cell pad leaves headroom for the Wave D drop to 1.25u (16
/// cells exactly).
pub(crate) const LUT_RADIUS: i32 = 16;
/// LUT side length (`2*LUT_RADIUS + 1 = 33`).
pub(crate) const LUT_DIM: usize = (LUT_RADIUS as usize) * 2 + 1;

/// Saturation constant for `compute_grass_density_sectors`. Tuned so a full
/// cap of GRASS_MAX-density cells right next to the creature pushes a sector
/// well into [0, 1]; clamping keeps the output bounded.
const GRASS_SECTOR_SAT: f32 = 4.0;

/// Build the (sector, weight) lookup table indexed by integer cell offset
/// `(dx, dy) ∈ [-LUT_RADIUS, LUT_RADIUS]² `. The stored sector is the
/// *primary* (nearest) sector; the weight is its share of the total. Consumers
/// distribute `1 - weight` to the adjacent sector — that side is computed at
/// consumer time so the LUT stays compact (33×33 entries × 5 bytes = 5.4 KB).
pub(crate) fn build_sector_lut() -> Vec<(u8, f32)> {
    let mut lut = vec![(0u8, 0.0f32); LUT_DIM * LUT_DIM];
    for dy in -LUT_RADIUS..=LUT_RADIUS {
        for dx in -LUT_RADIUS..=LUT_RADIUS {
            let (sector, weight) = if dx == 0 && dy == 0 {
                (0u8, 1.0f32)
            } else {
                // angle in radians, clockwise from north (+y down).
                let ang = (dx as f32).atan2(-dy as f32);
                // Map to [0, 2π).
                let two_pi = std::f32::consts::TAU;
                let ang = if ang < 0.0 { ang + two_pi } else { ang };
                // Sector width = 45° = π/4. Sector 0 is centered on north (0°),
                // so the boundary between sector s and s+1 is at (s + 0.5)*45°.
                let sector_f = ang / (two_pi / NN_SECTORS as f32);
                // primary sector = nearest center.
                let s_round = sector_f.round() as i32;
                let primary = s_round.rem_euclid(NN_SECTORS as i32) as u8;
                // distance to that center, in sector-widths; 0 = on center, 0.5 = on boundary.
                let frac = (sector_f - s_round as f32).abs();
                // Bilinear share for the primary sector: 1 at center, 0.5 on boundary.
                let weight = 1.0 - frac;
                (primary, weight)
            };
            let ix = (dx + LUT_RADIUS) as usize;
            let iy = (dy + LUT_RADIUS) as usize;
            lut[iy * LUT_DIM + ix] = (sector, weight);
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
    let range2 = range * range;
    grid.for_each_in_radius(x, y, range, |j| {
        if j == self_id {
            return;
        }
        let dx = creatures.x[j] - x;
        let dy = creatures.y[j] - y;
        let d2 = dx * dx + dy * dy;
        if d2 > range2 {
            return;
        }
        let d = d2.sqrt();
        let intensity = 1.0 - d / range;
        if intensity <= 0.0 {
            return;
        }
        let (primary, w) = sector_of(dx, dy);
        let adj = adjacent_sector(dx, dy, primary);
        let p_idx = primary as usize;
        let a_idx = adj as usize;
        let p_val = intensity * w;
        let a_val = intensity * (1.0 - w);
        if p_val > out[p_idx] {
            out[p_idx] = p_val;
        }
        if a_val > out[a_idx] {
            out[a_idx] = a_val;
        }
    });
}

/// Fill `out[0..8]` with grass-density intensities per sector. Walks the cells
/// inside an AABB of side `2*PROXIMITY_RANGE`, gated by the per-row bitset on
/// `grass`. Each cell's contribution is `density × cell_area / dist²`; the
/// sum per sector is normalized by `GRASS_SECTOR_SAT` and clamped to `[0, 1]`.
pub(crate) fn compute_grass_density_sectors(
    x: f32,
    y: f32,
    grass: &GrassGrid,
    sector_lut: &[(u8, f32)],
    out: &mut [f32; 8],
) {
    *out = [0.0f32; 8];
    let range = PROXIMITY_RANGE;
    let range2 = range * range;
    let cell = GRASS_CELL_SIZE;
    let inv_cell = 1.0 / cell;
    let cell_area = cell * cell;
    let dim = GRASS_GRID_DIM as i32;

    // Creature cell (clamped).
    let cx_cell = (x * inv_cell).floor() as i32;
    let cy_cell = (y * inv_cell).floor() as i32;
    let r_cells = (range * inv_cell).ceil() as i32 + 1;

    let iy_lo = (cy_cell - r_cells).max(0);
    let iy_hi = (cy_cell + r_cells).min(dim - 1);
    let ix_lo = (cx_cell - r_cells).max(0);
    let ix_hi = (cx_cell + r_cells).min(dim - 1);

    for iy in iy_lo..=iy_hi {
        if !grass.row_nonempty(iy as usize) {
            continue;
        }
        let row_off = iy as usize * GRASS_GRID_DIM;
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
            // LUT lookup by integer cell offset (signed).
            let lut_x = (ix - cx_cell).clamp(-LUT_RADIUS, LUT_RADIUS) + LUT_RADIUS;
            let lut_y = (iy - cy_cell).clamp(-LUT_RADIUS, LUT_RADIUS) + LUT_RADIUS;
            let (primary, w) = sector_lut[lut_y as usize * LUT_DIM + lut_x as usize];
            let adj = adjacent_sector(dx, dy, primary);
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
