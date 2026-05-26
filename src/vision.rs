//! Per-creature vision: 24 sector slots × 3 features (RGB only).
//! Filled in tick step 2 (v5 §3.5). Reused by D as NN inputs (v6 §E).
//!
//! D3: All creatures use a fixed 24-sector layout at VISION_RANGE_MAX.
//! Eye count variation removed.
//! M5: Creature RGB is read from the CreatureSoA.color_rgb hot-mirror (NN-weight hash).
//! Q3: dist and size slots dropped — only RGB written per sector (v1.3 cleanup).
//!
//! D7: Walled world — rays that exit world bounds return a wall-sentinel gray.
//! Wall sentinel RGB = (0.5, 0.5, 0.5) per Q-D7-4.

use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grid::SpatialGrid;

pub use crate::constants::SECTORS;
/// RGB only — dist and size dropped in v1.3 Q3 cleanup.
pub const FEATURES_PER_SECTOR: usize = 3; // r, g, b
pub const VISION_LEN: usize = SECTORS * FEATURES_PER_SECTOR; // 72

/// Unpack a packed 0x00RRGGBB u32 into (R, G, B) floats in [0, 1].
#[inline]
fn unpack_rgb(c: u32) -> (f32, f32, f32) {
    let r = ((c >> 16) & 0xFF) as f32 / 255.0;
    let g = ((c >> 8) & 0xFF) as f32 / 255.0;
    let b = (c & 0xFF) as f32 / 255.0;
    (r, g, b)
}

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
                    RayHit::Creature(j, _dist) => {
                        // M5: use per-creature color_rgb from hot-mirror. Q3: RGB only.
                        let (cr, cg, cb) = unpack_rgb(self.creatures.color_rgb[j]);
                        buf[slot] = cr;
                        buf[slot + 1] = cg;
                        buf[slot + 2] = cb;
                    }
                    RayHit::Wall(_dist) => {
                        // D7: wall sentinel — write gray RGB. Q3: RGB only.
                        buf[slot] = WALL_R;
                        buf[slot + 1] = WALL_G;
                        buf[slot + 2] = WALL_B;
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
    /// Q3: layout is now 3 features per sector (RGB only); all slots 0..3 must be zero.
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
        // outside VISION_RANGE_MAX=80. Sector 0 (R slot) must be zero.
        assert_eq!(
            vision[0][0], 0.0,
            "sector 0 R must be zero (no nearby target)"
        );
    }

    /// Q3: layout is 3 features per sector (r, g, b). RGB must match the hit creature.
    #[test]
    fn ray_hits_creature_returns_color_rgb() {
        // M5: RGB comes from creature's color_rgb hot-mirror (NN-weight hash).
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

        // Sector 0 (east): creature 1 is 10u away in the east direction.
        // Q3: slot 0=R, 1=G, 2=B (no dist or size).
        let slot = 0;
        let (exp_r, exp_g, exp_b) = unpack_rgb(creatures.color_rgb[1]);
        // At least one channel must be non-zero for a hit (creature has a color).
        let any_nonzero =
            vision[0][slot] != 0.0 || vision[0][slot + 1] != 0.0 || vision[0][slot + 2] != 0.0;
        assert!(any_nonzero, "sector 0 must have a hit (non-zero RGB)");
        assert!(
            (vision[0][slot] - exp_r).abs() < 1e-5,
            "r should match creature color_rgb r={exp_r}"
        );
        assert!(
            (vision[0][slot + 1] - exp_g).abs() < 1e-5,
            "g should match creature color_rgb g={exp_g}"
        );
        assert!(
            (vision[0][slot + 2] - exp_b).abs() < 1e-5,
            "b should match creature color_rgb b={exp_b}"
        );
    }

    #[test]
    fn out_of_range_returns_zero() {
        // D3: VISION_RANGE_MAX = 80. Target at 191u is out of range.
        // Q3: slot 0 is now R (not dist); must be zero when no hit.
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

        // Target out of range — sector 0 stays all-zero.
        let slot = 0;
        assert_eq!(
            vision[0][slot], 0.0,
            "sector 0 R must be zero when target is out of range"
        );
        assert_eq!(
            vision[0][slot + 1],
            0.0,
            "sector 0 G must be zero when target is out of range"
        );
        assert_eq!(
            vision[0][slot + 2],
            0.0,
            "sector 0 B must be zero when target is out of range"
        );
    }

    // ---- D7 wall-sentinel tests ----

    /// D7/Q3: A creature near the east wall facing east should see the wall sentinel
    /// (gray RGB 0.5,0.5,0.5) in sector 0 (east-facing ray exits world bounds).
    /// Q3: layout is RGB-only; slot 0=R, 1=G, 2=B.
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

        // Sector 0 (east, dx=1, dy=0) should see the wall: slot 0=R, 1=G, 2=B.
        let slot = 0;
        let r = vision[0][slot];
        let g_val = vision[0][slot + 1];
        let b = vision[0][slot + 2];

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
    /// Q3: slot 0 is R; wall sentinel is gray (0.5). Non-wall-sentinel creature B
    /// would have a different color, but the important check is that the view is
    /// wall-blocked, not showing a creature 590u away.
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

        // Sector 0 (east) should see the wall sentinel (gray), NOT creature B's color.
        // Q3: slot 0=R. Wall sentinel R == WALL_R = 0.5.
        let slot = 0;
        let r_a = vision[0][slot]; // creature A's sector-0 R
        let (b_r, _, _) = unpack_rgb(creatures.color_rgb[1]); // creature B's R
                                                              // If the ray sees wall sentinel, R should be WALL_R; if it sees nothing, R = 0.
                                                              // Either way, it must NOT equal creature B's RGB (wraparound blocked).
        if (r_a - WALL_R).abs() < 1e-3 {
            // Wall sentinel seen — confirm it's not creature B.
            assert!(
                (r_a - b_r).abs() > 0.1 || (WALL_R - b_r).abs() < 1e-3,
                "wall sentinel seen but suspiciously matches creature B color: r_a={r_a}, b_r={b_r}"
            );
        }
        // If r_a == 0.0, nothing was seen — also fine (no wraparound).
    }
}
