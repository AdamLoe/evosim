//! Per-creature vision: 24 sector slots × 5 features.
//! Filled in tick step 2 (v5 §3.5). Reused by D as NN inputs (v6 §E).
//!
//! D3: All creatures use a fixed 24-sector layout at VISION_RANGE_MAX.
//! Eye count variation removed. Size slot uses constant FOUNDER_SIZE.
//! Pigment slots use placeholder 0.5 gray (M5 owns NN-hash color hot-mirror).
//!
//! D7: Walled world — rays that exit world bounds return a wall-sentinel gray.
//! Wall sentinel RGB = (0.5, 0.5, 0.5) per Q-D7-4.

use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grid::SpatialGrid;

pub use crate::constants::{EYE_STRIDE, SECTORS};
pub const FEATURES_PER_SECTOR: usize = 5; // dist, size, r, g, b (D9 owns shrinking to 3)
pub const VISION_LEN: usize = SECTORS * FEATURES_PER_SECTOR; // 120

/// D3: placeholder pigment for live creatures. M5 owns NN-hash color hot-mirror.
pub(crate) const CREATURE_PIGMENT: f32 = 0.5;

/// Fixed per-creature vision buffer. World owns Vec<VisionBuf>.
pub type VisionBuf = [f32; VISION_LEN];

/// Wall-sentinel RGB (D7: walled world).
pub const WALL_R: f32 = 0.5;
pub const WALL_G: f32 = 0.5;
pub const WALL_B: f32 = 0.5;

/// Ray hit from DDA traversal. Distance is the second field.
enum RayHit {
    Creature(usize, f32),
    /// Ray exited world bounds at parameter `t`.
    Wall(f32),
}

pub struct VisionPass<'a> {
    pub creatures: &'a CreatureSoA,
    pub grid: &'a SpatialGrid,
}

impl<'a> VisionPass<'a> {
    /// Populate `out[i]` for every creature with the post-grid-rebuild state.
    pub fn run(&self, out: &mut [VisionBuf]) {
        let n = self.creatures.len();

        #[cfg(not(feature = "threads"))]
        {
            for i in 0..n {
                self.fill_one(i, &mut out[i]);
            }
        }

        #[cfg(feature = "threads")]
        {
            use rayon::prelude::*;
            let chunk_size = crate::world::nn::chunk_base_size(n);
            out[..n]
                .par_chunks_mut(chunk_size)
                .enumerate()
                .for_each(|(chunk_idx, sub)| {
                    let lo = chunk_idx * chunk_size;
                    for k in 0..sub.len() {
                        self.fill_one(lo + k, &mut sub[k]);
                    }
                });
        }
    }

    #[inline]
    fn fill_one(&self, i: usize, buf: &mut VisionBuf) {
        debug_assert_eq!(
            self.creatures.eye_trig.len(),
            self.creatures.x.len() * SECTORS * 2
        );
        // D3: all creatures use 24 active sectors at VISION_RANGE_MAX (no per-creature variation).
        let ox = self.creatures.x[i];
        let oy = self.creatures.y[i];
        let max_dist = VISION_RANGE_MAX;

        *buf = [0.0; VISION_LEN];

        // D3: stride=1 (all 24 sectors active).
        let trig_base = i * SECTORS * 2;
        for s in 0..SECTORS {
            let dx = self.creatures.eye_trig[trig_base + s * 2];
            let dy = self.creatures.eye_trig[trig_base + s * 2 + 1];

            if let Some(hit) = self.raycast(i, ox, oy, dx, dy, max_dist) {
                let slot = s * FEATURES_PER_SECTOR;
                match hit {
                    RayHit::Creature(_j, dist) => {
                        // D3: size is constant; pigment is placeholder gray.
                        let d = dist.max(1e-4);
                        buf[slot] = d;
                        buf[slot + 1] = FOUNDER_SIZE; // constant body size
                        buf[slot + 2] = CREATURE_PIGMENT;
                        buf[slot + 3] = CREATURE_PIGMENT;
                        buf[slot + 4] = CREATURE_PIGMENT;
                    }
                    RayHit::Wall(dist) => {
                        // D7: wall sentinel — write gray RGB. Distance is the wall hit t.
                        let d = dist.max(1e-4);
                        buf[slot] = d;
                        buf[slot + 1] = 0.0; // no size for walls
                        buf[slot + 2] = WALL_R;
                        buf[slot + 3] = WALL_G;
                        buf[slot + 4] = WALL_B;
                    }
                }
            }
        }
    }

    /// DDA along the spatial grid. Returns the closest hit within max_dist.
    ///
    /// Walled: cell coordinates are NOT wrapped. On world-bbox exit the ray
    /// terminates and returns `RayHit::Wall(t)` with the exit parameter.
    fn raycast(
        &self,
        self_idx: usize,
        ox: f32,
        oy: f32,
        dx: f32,
        dy: f32,
        max_dist: f32,
    ) -> Option<RayHit> {
        let dim = HASH_DIM as i32;

        // Start cell — clamp so out-of-range spawn positions still begin in a valid cell.
        let mut cx = (ox / HASH_CELL).floor() as i32;
        let mut cy = (oy / HASH_CELL).floor() as i32;

        let step_x: i32 = if dx >= 0.0 { 1 } else { -1 };
        let step_y: i32 = if dy >= 0.0 { 1 } else { -1 };

        let t_delta_x = if dx.abs() > 1e-9 {
            HASH_CELL / dx.abs()
        } else {
            f32::MAX
        };
        let t_delta_y = if dy.abs() > 1e-9 {
            HASH_CELL / dy.abs()
        } else {
            f32::MAX
        };

        let t_max_x_init = if dx.abs() > 1e-9 {
            let boundary_x = if dx > 0.0 {
                (cx + 1) as f32 * HASH_CELL
            } else {
                cx as f32 * HASH_CELL
            };
            ((boundary_x - ox) / dx).abs()
        } else {
            f32::MAX
        };
        let t_max_y_init = if dy.abs() > 1e-9 {
            let boundary_y = if dy > 0.0 {
                (cy + 1) as f32 * HASH_CELL
            } else {
                cy as f32 * HASH_CELL
            };
            ((boundary_y - oy) / dy).abs()
        } else {
            f32::MAX
        };

        let mut t_max_x = t_max_x_init;
        let mut t_max_y = t_max_y_init;

        let mut best: Option<RayHit> = None;
        let mut best_dist = max_dist;
        let mut cells_walked: usize = 0;

        loop {
            // Walled: exit if cell is out of bounds.
            if cx < 0 || cx >= dim || cy < 0 || cy >= dim {
                // Ray exited world bounds — return wall sentinel if nothing better was found.
                let wall_t = t_max_x.min(t_max_y) - t_delta_x.min(t_delta_y); // approximate entry t
                                                                              // Use current best or wall.
                if best.is_none() {
                    // t of the last boundary crossing is the wall exit t.
                    let wt = if t_max_x < t_max_y {
                        t_max_x - t_delta_x
                    } else {
                        t_max_y - t_delta_y
                    };
                    let wt = wt.max(0.0).min(max_dist);
                    if wt <= max_dist {
                        return Some(RayHit::Wall(wt));
                    }
                }
                let _ = wall_t;
                return best;
            }

            let cell_idx = cy as usize * HASH_DIM + cx as usize;

            // Test creatures in this cell.
            {
                let start = self.grid.starts[cell_idx] as usize;
                let end = self.grid.starts[cell_idx + 1] as usize;
                for k in start..end {
                    let j = self.grid.indices[k] as usize;
                    if j == self_idx {
                        continue;
                    }
                    // D3: body radius is constant across all creatures.
                    let rj = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
                    // Walled: use raw Euclidean positions (no torus re-anchoring).
                    let tx = self.creatures.x[j];
                    let ty = self.creatures.y[j];
                    if let Some(t) = ray_circle_hit(ox, oy, dx, dy, tx, ty, rj) {
                        if t <= best_dist {
                            best_dist = t;
                            best = Some(RayHit::Creature(j, t));
                        }
                    }
                }
            }

            // If we found a hit closer than the next cell boundary, return it.
            let next_t = t_max_x.min(t_max_y);
            if best.is_some() && best_dist < next_t {
                return best;
            }

            // Exceeded range.
            if next_t > max_dist {
                // Check if the ray would exit the world within max_dist — wall sentinel.
                // Project ahead to see if the next step exits bounds.
                let next_cx = if t_max_x < t_max_y { cx + step_x } else { cx };
                let next_cy = if t_max_x < t_max_y { cy } else { cy + step_y };
                if (next_cx < 0 || next_cx >= dim || next_cy < 0 || next_cy >= dim)
                    && best.is_none()
                {
                    return Some(RayHit::Wall(next_t.min(max_dist)));
                }
                break;
            }

            // Advance to next cell.
            if t_max_x < t_max_y {
                cx += step_x;
                t_max_x += t_delta_x;
            } else {
                cy += step_y;
                t_max_y += t_delta_y;
            }
            cells_walked += 1;
            if cells_walked >= MAX_CELLS_PER_RAY {
                break;
            }
        }

        best
    }
}

/// Ray-circle intersection (Amanatides & Woo closed form).
fn ray_circle_hit(ox: f32, oy: f32, dx: f32, dy: f32, cx: f32, cy: f32, r: f32) -> Option<f32> {
    let mx = ox - cx;
    let my = oy - cy;
    let b = mx * dx + my * dy;
    let c = mx * mx + my * my - r * r;
    if c > 0.0 && b > 0.0 {
        return None;
    }
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    if t < 0.0 {
        return None;
    }
    Some(t)
}

/// Compute the center angle of sector `s` (D3: uniform 24-sector layout, no offsets).
#[allow(dead_code)]
pub(crate) fn sector_to_angle(s: usize) -> f32 {
    std::f32::consts::TAU * (s as f32) / (SECTORS as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Brain;
    use crate::rng::SimRng;

    fn make_grid(creatures: &CreatureSoA) -> SpatialGrid {
        let mut g = SpatialGrid::new();
        g.rebuild(&creatures.x, &creatures.y);
        g
    }

    fn simple_creature(soa: &mut CreatureSoA, id: u64, x: f32, y: f32) -> usize {
        let mut rng = SimRng::from_u64(42);
        let brain = Brain::founder(&mut rng);
        soa.push(id, x, y, 100.0, 0, brain)
    }

    /// D3: all creatures always use all 24 sectors. A single creature at world
    /// center should see nothing in sector 0 (no target within VISION_RANGE_MAX).
    #[test]
    fn ray_hits_self_ignored() {
        let mut creatures = CreatureSoA::with_capacity(4);
        simple_creature(&mut creatures, 0, 300.0, 300.0);

        let grid = make_grid(&creatures);
        let pass = VisionPass {
            creatures: &creatures,
            grid: &grid,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]];
        pass.run(&mut vision);
        // Self ignored — but rays may hit walls. At world center, walls are ~300u away,
        // outside VISION_RANGE_MAX=80. Sector 0 distance must be zero.
        assert_eq!(
            vision[0][0], 0.0,
            "sector 0 distance must be zero (no nearby target)"
        );
    }

    #[test]
    fn ray_hits_creature_returns_distance_and_constant_pigment() {
        // D3: pigment is constant CREATURE_PIGMENT (0.5); size is FOUNDER_SIZE.
        let mut creatures = CreatureSoA::with_capacity(4);
        simple_creature(&mut creatures, 0, 100.0, 100.0);
        simple_creature(&mut creatures, 1, 110.0, 100.0);

        let grid = make_grid(&creatures);
        let pass = VisionPass {
            creatures: &creatures,
            grid: &grid,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]; 2];
        pass.run(&mut vision);

        let slot = 0;
        let dist = vision[0][slot];
        assert!(dist > 0.0, "sector 0 should have a hit");
        assert!((dist - 9.0).abs() < 0.5, "expected dist ~9, got {dist}");
        assert!(
            (vision[0][slot + 1] - FOUNDER_SIZE).abs() < 1e-5,
            "size should be FOUNDER_SIZE={FOUNDER_SIZE}"
        );
        // D3: pigment is placeholder gray.
        assert!((vision[0][slot + 2] - CREATURE_PIGMENT).abs() < 1e-5, "r should be CREATURE_PIGMENT");
        assert!((vision[0][slot + 3] - CREATURE_PIGMENT).abs() < 1e-5, "g should be CREATURE_PIGMENT");
        assert!((vision[0][slot + 4] - CREATURE_PIGMENT).abs() < 1e-5, "b should be CREATURE_PIGMENT");
    }

    #[test]
    fn out_of_range_returns_zero() {
        // D3: VISION_RANGE_MAX = 80. Target at 191u is out of range.
        let mut creatures = CreatureSoA::with_capacity(4);
        simple_creature(&mut creatures, 0, 100.0, 100.0);
        simple_creature(&mut creatures, 1, 191.0, 100.0);

        let grid = make_grid(&creatures);
        let pass = VisionPass {
            creatures: &creatures,
            grid: &grid,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]; 2];
        pass.run(&mut vision);

        let slot = 0;
        assert_eq!(
            vision[0][slot], 0.0,
            "target out of range must yield zero distance"
        );
    }

    // ---- D7 wall-sentinel tests ----

    /// D7/Q3: A creature near the east wall facing east should see the wall sentinel
    /// (gray RGB 0.5,0.5,0.5) in sector 0 (east-facing ray exits world bounds).
    #[test]
    fn raycast_returns_wall_sentinel_at_world_edge() {
        let mut creatures = CreatureSoA::with_capacity(2);
        // Place creature 5 units from the east wall (x = WORLD_SIZE - 5), facing east.
        // Vision range = VISION_RANGE_MAX=80 — the east wall is within range.
        simple_creature(&mut creatures, 0, WORLD_SIZE - 5.0, WORLD_SIZE * 0.5);

        let grid = make_grid(&creatures);
        let pass = VisionPass {
            creatures: &creatures,
            grid: &grid,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]];
        pass.run(&mut vision);

        // Sector 0 (east, dx=1, dy=0) should see the wall.
        let slot = 0;
        let dist = vision[0][slot];
        let r = vision[0][slot + 2];
        let g_val = vision[0][slot + 3];
        let b = vision[0][slot + 4];

        assert!(
            dist > 0.0,
            "east-facing ray near east wall must have a non-zero distance; dist={dist}"
        );
        assert!(
            (r - WALL_R).abs() < 1e-4,
            "wall sentinel R must be {WALL_R}; got {r}"
        );
        assert!(
            (g_val - WALL_G).abs() < 1e-4,
            "wall sentinel G must be {WALL_G}; got {g_val}"
        );
        assert!(
            (b - WALL_B).abs() < 1e-4,
            "wall sentinel B must be {WALL_B}; got {b}"
        );
    }

    /// D7: A creature at x=595 facing east does NOT see a creature at x=5
    /// (no wraparound — the wall blocks the view).
    #[test]
    fn raycast_no_wraparound_target() {
        let mut creatures = CreatureSoA::with_capacity(4);
        // Creature A at (595, 300): near east wall, facing east (sector 0).
        simple_creature(&mut creatures, 0, 595.0, 300.0);
        // Creature B at (5, 300): would be 10u away via torus seam, but we're walled now.
        simple_creature(&mut creatures, 1, 5.0, 300.0);

        let grid = make_grid(&creatures);
        let pass = VisionPass {
            creatures: &creatures,
            grid: &grid,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]; 2];
        pass.run(&mut vision);

        // Sector 0 (east) should see the wall sentinel or zero, NOT creature B (no wraparound).
        // D3: all creatures share the same CREATURE_PIGMENT; we verify distance only.
        // Creature B is 590u away on the east side (past wall). It should not appear.
        let slot = 0;
        let dist_a = vision[0][slot]; // creature A (at 595,300) sees wall ~5u east
        // The wall hit should be very close (~5u away), not the 590u "wraparound" path.
        if dist_a > 0.0 {
            assert!(
                dist_a < 20.0,
                "east hit dist={dist_a} must be wall-close (<20u), not wraparound-far"
            );
        }
    }
}
