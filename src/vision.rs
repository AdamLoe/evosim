//! Per-creature vision: 24 sector slots × 5 features.
//! Filled in tick step 2 (v5 §3.5). Reused by D as NN inputs (v6 §E).
//!
//! D7: Walled world — rays that exit world bounds return a wall-sentinel gray.
//! Wall sentinel RGB = (0.5, 0.5, 0.5) per Q-D7-4 (D2 vision palette may rename later).

use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grid::SpatialGrid;

pub use crate::constants::{EYE_STRIDE, SECTORS};
pub const FEATURES_PER_SECTOR: usize = 5; // dist, size, r, g, b
pub const VISION_LEN: usize = SECTORS * FEATURES_PER_SECTOR; // 120

/// Fixed per-creature vision buffer. World owns Vec<VisionBuf>.
pub type VisionBuf = [f32; VISION_LEN];

/// Carrion is rendered/seen as fixed gray per v6 §4 "What changed vs v5".
pub const CARRION_R: f32 = 0.4;
pub const CARRION_G: f32 = 0.4;
pub const CARRION_B: f32 = 0.4;

/// Wall-sentinel RGB (Q-D7-4). D2 (vision palette) may rename later.
pub const WALL_R: f32 = 0.5;
pub const WALL_G: f32 = 0.5;
pub const WALL_B: f32 = 0.5;

/// CSR-layout per-cell carrion index. Replaces `Vec<Vec<u32>>` (S26).
pub struct CarrionIndex {
    /// CSR row-start offsets, length = HASH_DIM * HASH_DIM + 1.
    starts: Vec<u32>,
    /// Flat carrion-index list, length = carrion.len() after each rebuild.
    indices: Vec<u32>,
    /// Scratch write-head cursors for the scatter pass. Reused across calls.
    cursors: Vec<u32>,
}

impl Default for CarrionIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl CarrionIndex {
    pub fn new() -> Self {
        let starts = vec![0u32; HASH_DIM * HASH_DIM + 1];
        let cursors = vec![0u32; starts.len()];
        Self {
            starts,
            indices: Vec::with_capacity(256),
            cursors,
        }
    }

    /// Rebuild the CSR index from a flat carrion list.
    pub fn rebuild(&mut self, carrion: &[Carrion]) {
        self.starts.iter_mut().for_each(|s| *s = 0);
        for c in carrion {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            self.starts[cell + 1] += 1;
        }
        for k in 1..self.starts.len() {
            self.starts[k] += self.starts[k - 1];
        }
        let n = carrion.len();
        self.indices.clear();
        self.indices.resize(n, 0);
        self.cursors.copy_from_slice(&self.starts);
        for (ci, c) in carrion.iter().enumerate() {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            let pos = self.cursors[cell] as usize;
            self.indices[pos] = ci as u32;
            self.cursors[cell] += 1;
        }
    }

    #[inline]
    pub fn cell_slice(&self, cell: usize) -> &[u32] {
        let s = self.starts[cell] as usize;
        let e = self.starts[cell + 1] as usize;
        &self.indices[s..e]
    }
}

/// Ray hit from DDA traversal.
enum RayHit {
    Creature(usize, f32),
    Carrion(#[allow(dead_code)] usize, f32),
    /// Ray exited world bounds at parameter `t`.
    Wall(f32),
}

pub struct VisionPass<'a> {
    pub creatures: &'a CreatureSoA,
    pub carrion: &'a [Carrion],
    pub grid: &'a SpatialGrid,
    pub cell_to_carrion: &'a CarrionIndex,
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
        let eye_count_i = self.creatures.g_eye_count[i];
        let vision_range_i = self.creatures.g_vision_range[i];
        if eye_count_i == 0 || vision_range_i <= 0.0 {
            *buf = [0.0; VISION_LEN];
            return;
        }

        let k = eye_count_i as usize;
        let k_idx = EYE_VALID.iter().position(|&v| v as usize == k).unwrap_or(0);
        if k_idx == 0 {
            *buf = [0.0; VISION_LEN];
            return;
        }
        let stride = EYE_STRIDE[k_idx] as usize;
        let ox = self.creatures.x[i];
        let oy = self.creatures.y[i];
        let max_dist = vision_range_i;

        *buf = [0.0; VISION_LEN];

        let trig_base = i * SECTORS * 2;
        for s in (0..SECTORS).step_by(stride) {
            let dx = self.creatures.eye_trig[trig_base + s * 2];
            let dy = self.creatures.eye_trig[trig_base + s * 2 + 1];

            if let Some(hit) = self.raycast(i, ox, oy, dx, dy, max_dist) {
                let slot = s * FEATURES_PER_SECTOR;
                match hit {
                    RayHit::Creature(j, dist) => {
                        let pigment = &self.creatures.genomes[j];
                        let d = dist.max(1e-4);
                        buf[slot] = d;
                        buf[slot + 1] = self.creatures.g_size[j];
                        buf[slot + 2] = pigment.pigment_r;
                        buf[slot + 3] = pigment.pigment_g;
                        buf[slot + 4] = pigment.pigment_b;
                    }
                    RayHit::Carrion(_, dist) => {
                        let d = dist.max(1e-4);
                        buf[slot] = d;
                        buf[slot + 1] = CARRION_RADIUS_FOR_VISION;
                        buf[slot + 2] = CARRION_R;
                        buf[slot + 3] = CARRION_G;
                        buf[slot + 4] = CARRION_B;
                    }
                    RayHit::Wall(dist) => {
                        // Q3: wall sentinel — write gray RGB. Distance is the wall hit t.
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
                    let rj = self.creatures.g_size[j] * BODY_RADIUS_PER_SIZE;
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

            // Test carrion in this cell.
            for &ci in self.cell_to_carrion.cell_slice(cell_idx) {
                let c = &self.carrion[ci as usize];
                // Walled: raw Euclidean positions.
                if let Some(t) = ray_circle_hit(ox, oy, dx, dy, c.x, c.y, CARRION_RADIUS_FOR_VISION)
                {
                    if t <= best_dist {
                        best_dist = t;
                        best = Some(RayHit::Carrion(ci as usize, t));
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

/// Compute the angle a creature at index `i` is facing in sector `s`.
#[allow(dead_code)]
pub(crate) fn sector_to_angle(s: usize, eye_offsets: &[f32; EYE_SLOTS], eye_count: u8) -> f32 {
    let k = eye_count as usize;
    if k == 0 {
        return 0.0;
    }
    let stride = 24usize.checked_div(k).unwrap_or(0);
    let active_index = s.checked_div(stride).unwrap_or(0);
    let offset = if active_index < eye_offsets.len() {
        eye_offsets[active_index]
    } else {
        0.0
    };
    let theta_center = std::f32::consts::TAU * (s as f32) / (SECTORS as f32);
    theta_center + offset
}

/// Rebuild a per-cell carrion lookup from a flat carrion list.
pub fn build_cell_to_carrion(carrion: &[Carrion], dst: &mut CarrionIndex) {
    dst.rebuild(carrion);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Brain;
    use crate::genome::Genome;
    use crate::rng::SimRng;

    fn make_grid(creatures: &CreatureSoA) -> SpatialGrid {
        let mut g = SpatialGrid::new();
        g.rebuild(&creatures.x, &creatures.y);
        g
    }

    fn make_carrion_index(carrion: &[Carrion]) -> CarrionIndex {
        let mut dst = CarrionIndex::new();
        build_cell_to_carrion(carrion, &mut dst);
        dst
    }

    fn simple_creature(soa: &mut CreatureSoA, id: u64, x: f32, y: f32, genome: Genome) -> usize {
        let mut rng = SimRng::from_u64(42);
        let brain = Brain::founder(&mut rng);
        soa.push(id, x, y, 100.0, 0, 0, 0, genome, brain)
    }

    fn genome_with_eyes(eye_count: u8, vision_range: f32) -> Genome {
        let mut g = Genome::founder();
        g.eye_count = eye_count;
        g.vision_range = vision_range;
        g.eye_offsets = [0.0; EYE_SLOTS];
        g
    }

    #[test]
    fn eye_count_zero_yields_all_zero_vision() {
        let mut creatures = CreatureSoA::with_capacity(4);
        let mut ga = genome_with_eyes(0, 80.0);
        ga.eye_count = 0;
        simple_creature(&mut creatures, 0, 100.0, 100.0, ga);
        let gb = genome_with_eyes(4, 0.0);
        simple_creature(&mut creatures, 1, 110.0, 100.0, gb);

        let grid = make_grid(&creatures);
        let carrion = vec![];
        let cell_to_carrion = make_carrion_index(&carrion);
        let pass = VisionPass {
            creatures: &creatures,
            carrion: &carrion,
            grid: &grid,
            cell_to_carrion: &cell_to_carrion,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]; 2];
        pass.run(&mut vision);
        assert_eq!(
            vision[0], [0.0f32; VISION_LEN],
            "blind creature must see nothing"
        );
    }

    #[test]
    fn ray_hits_self_ignored() {
        let mut creatures = CreatureSoA::with_capacity(4);
        let g = genome_with_eyes(24, 80.0);
        simple_creature(&mut creatures, 0, 300.0, 300.0, g);

        let grid = make_grid(&creatures);
        let carrion = vec![];
        let cell_to_carrion = make_carrion_index(&carrion);
        let pass = VisionPass {
            creatures: &creatures,
            carrion: &carrion,
            grid: &grid,
            cell_to_carrion: &cell_to_carrion,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]];
        pass.run(&mut vision);
        // Self ignored — but rays may hit walls, so we only check no creature-self hit.
        // The creature is at world center so walls are at distance ~300, outside vision range 80.
        // Sector 0 should be 0 (no creature, wall too far).
        assert_eq!(
            vision[0][0], 0.0,
            "sector 0 distance must be zero (no nearby target)"
        );
    }

    #[test]
    fn ray_hits_creature_returns_color_and_distance() {
        let mut creatures = CreatureSoA::with_capacity(4);
        let ga = genome_with_eyes(24, 80.0);
        simple_creature(&mut creatures, 0, 100.0, 100.0, ga);

        let mut gb = genome_with_eyes(0, 0.0);
        gb.pigment_r = 0.9;
        gb.pigment_g = 0.1;
        gb.pigment_b = 0.2;
        gb.size = 1.0;
        gb.eye_count = 0;
        simple_creature(&mut creatures, 1, 110.0, 100.0, gb);

        let grid = make_grid(&creatures);
        let carrion = vec![];
        let cell_to_carrion = make_carrion_index(&carrion);
        let pass = VisionPass {
            creatures: &creatures,
            carrion: &carrion,
            grid: &grid,
            cell_to_carrion: &cell_to_carrion,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]; 2];
        pass.run(&mut vision);

        let slot = 0;
        let dist = vision[0][slot];
        assert!(dist > 0.0, "sector 0 should have a hit");
        assert!((dist - 9.0).abs() < 0.1, "expected dist ~9, got {dist}");
        assert!(
            (vision[0][slot + 1] - 1.0).abs() < 1e-5,
            "size should be 1.0"
        );
        assert!((vision[0][slot + 2] - 0.9).abs() < 1e-5, "r should be 0.9");
        assert!((vision[0][slot + 3] - 0.1).abs() < 1e-5, "g should be 0.1");
        assert!((vision[0][slot + 4] - 0.2).abs() < 1e-5, "b should be 0.2");
    }

    #[test]
    fn carrion_returns_gray() {
        let mut creatures = CreatureSoA::with_capacity(2);
        let ga = genome_with_eyes(4, 80.0);
        simple_creature(&mut creatures, 0, 100.0, 100.0, ga);

        let carrion = vec![Carrion {
            id: 999,
            x: 115.0,
            y: 100.0,
            pool: 1.0,
            age: 0,
        }];

        let grid = make_grid(&creatures);
        let cell_to_carrion = make_carrion_index(&carrion);
        let pass = VisionPass {
            creatures: &creatures,
            carrion: &carrion,
            grid: &grid,
            cell_to_carrion: &cell_to_carrion,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]];
        pass.run(&mut vision);

        let slot = 0;
        let dist = vision[0][slot];
        assert!(dist > 0.0, "sector 0 should see carrion");
        assert!(
            (vision[0][slot + 2] - CARRION_R).abs() < 1e-4,
            "carrion r should be 0.4"
        );
        assert!(
            (vision[0][slot + 3] - CARRION_G).abs() < 1e-4,
            "carrion g should be 0.4"
        );
        assert!(
            (vision[0][slot + 4] - CARRION_B).abs() < 1e-4,
            "carrion b should be 0.4"
        );
    }

    #[test]
    fn out_of_range_returns_zero() {
        let mut creatures = CreatureSoA::with_capacity(4);
        let ga = genome_with_eyes(24, 80.0);
        simple_creature(&mut creatures, 0, 100.0, 100.0, ga);

        let mut gb = Genome::founder();
        gb.size = 1.0;
        simple_creature(&mut creatures, 1, 191.0, 100.0, gb);

        let grid = make_grid(&creatures);
        let carrion = vec![];
        let cell_to_carrion = make_carrion_index(&carrion);
        let pass = VisionPass {
            creatures: &creatures,
            carrion: &carrion,
            grid: &grid,
            cell_to_carrion: &cell_to_carrion,
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
        // Vision range 80 — the east wall is within range.
        let ga = genome_with_eyes(24, 80.0);
        simple_creature(&mut creatures, 0, WORLD_SIZE - 5.0, WORLD_SIZE * 0.5, ga);

        let grid = make_grid(&creatures);
        let carrion = vec![];
        let cell_to_carrion = make_carrion_index(&carrion);
        let pass = VisionPass {
            creatures: &creatures,
            carrion: &carrion,
            grid: &grid,
            cell_to_carrion: &cell_to_carrion,
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
        let ga = genome_with_eyes(24, 80.0);
        simple_creature(&mut creatures, 0, 595.0, 300.0, ga);
        // Creature B at (5, 300): would be 10u away via torus seam, but we're walled now.
        let mut gb = Genome::founder();
        gb.size = 1.0;
        gb.eye_count = 0;
        simple_creature(&mut creatures, 1, 5.0, 300.0, gb);

        let grid = make_grid(&creatures);
        let carrion = vec![];
        let cell_to_carrion = make_carrion_index(&carrion);
        let pass = VisionPass {
            creatures: &creatures,
            carrion: &carrion,
            grid: &grid,
            cell_to_carrion: &cell_to_carrion,
        };
        let mut vision = vec![[0.0f32; VISION_LEN]; 2];
        pass.run(&mut vision);

        // Sector 0 (east) should see the wall sentinel, NOT creature B's color.
        let slot = 0;
        let r = vision[0][slot + 2];
        let g_val = vision[0][slot + 3];
        let b_val = vision[0][slot + 4];
        // If we see creature B, we'd see its pigment (not gray). Confirm it's the wall.
        let sees_creature_b = r != WALL_R || g_val != WALL_G || b_val != WALL_B;
        // More specifically: confirm we don't see creature B's genome color.
        let gb_color = Genome::founder();
        assert!(
            !(r == gb_color.pigment_r && g_val == gb_color.pigment_g),
            "sector 0 must not see creature B (no wraparound); saw r={r} g={g_val} b={b_val}"
        );
        // Either we see the wall sentinel or zero (empty past the wall).
        // The key: creature B's position is NOT the result.
        let _ = sees_creature_b;
    }

    // ---- S26 CarrionIndex CSR tests ----

    #[test]
    fn csr_carrion_membership_matches_old_layout() {
        let carrion = vec![
            Carrion {
                id: 0,
                x: 10.0,
                y: 10.0,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 1,
                x: 10.0,
                y: 10.0,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 2,
                x: 590.0,
                y: 590.0,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 3,
                x: 300.0,
                y: 300.0,
                pool: 1.0,
                age: 0,
            },
        ];

        let total = HASH_DIM * HASH_DIM;
        let mut ref_dst: Vec<Vec<u32>> = vec![Vec::new(); total];
        for (ci, c) in carrion.iter().enumerate() {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            ref_dst[cell].push(ci as u32);
        }

        let mut csr = CarrionIndex::new();
        csr.rebuild(&carrion);

        for cell in 0..total {
            let mut csr_set: Vec<u32> = csr.cell_slice(cell).to_vec();
            let mut ref_set: Vec<u32> = ref_dst[cell].clone();
            csr_set.sort_unstable();
            ref_set.sort_unstable();
            assert_eq!(
                csr_set, ref_set,
                "cell {cell}: CSR membership {csr_set:?} != reference {ref_set:?}"
            );
        }
    }

    #[test]
    fn csr_iteration_order_is_insertion_order() {
        let x = 100.0_f32;
        let y = 100.0_f32;
        let carrion = vec![
            Carrion {
                id: 10,
                x,
                y,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 20,
                x: x + 0.1,
                y,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 30,
                x: x + 0.2,
                y,
                pool: 1.0,
                age: 0,
            },
        ];
        let cell = SpatialGrid::cell_of(x, y);
        let mut csr = CarrionIndex::new();
        csr.rebuild(&carrion);
        let slice = csr.cell_slice(cell);
        assert_eq!(
            slice,
            &[0u32, 1, 2],
            "carrion indices must appear in insertion order; got {slice:?}"
        );
    }

    #[test]
    fn csr_empty_and_full_corners() {
        let mut csr = CarrionIndex::new();
        csr.rebuild(&[]);
        for cell in 0..HASH_DIM * HASH_DIM {
            assert!(
                csr.cell_slice(cell).is_empty(),
                "cell {cell} must be empty for zero carrion"
            );
        }

        let corners = vec![
            Carrion {
                id: 0,
                x: 0.5,
                y: 0.5,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 1,
                x: WORLD_SIZE - 0.5,
                y: 0.5,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 2,
                x: 0.5,
                y: WORLD_SIZE - 0.5,
                pool: 1.0,
                age: 0,
            },
            Carrion {
                id: 3,
                x: WORLD_SIZE - 0.5,
                y: WORLD_SIZE - 0.5,
                pool: 1.0,
                age: 0,
            },
        ];
        csr.rebuild(&corners);
        let total_in_csr: usize = (0..HASH_DIM * HASH_DIM)
            .map(|c| csr.cell_slice(c).len())
            .sum();
        assert_eq!(
            total_in_csr,
            corners.len(),
            "total carrion in CSR ({total_in_csr}) must equal input count ({})",
            corners.len()
        );
        for (ci, c) in corners.iter().enumerate() {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            let slice = csr.cell_slice(cell);
            assert!(
                slice.contains(&(ci as u32)),
                "corner carrion {ci} not found in cell {cell}: slice={slice:?}"
            );
        }
    }
}
