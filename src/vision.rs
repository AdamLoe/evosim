//! Per-creature vision: 24 sector slots × 5 features.
//! Filled in tick step 2 (v5 §3.5). Reused by D as NN inputs (v6 §E).
//!
//! Design decisions pinned here for Milestone D inheritance:
//! - Sector layout: for eye_count k > 0, active sectors are s ∈ {0, 24/k,
//!   2·24/k, ...}. Slot-center angle = 2π × s / 24. Effective ray angle =
//!   slot_center + eye_offsets[active_index] (per-slot additive offset).
//! - Inactive sectors write zeros, never skipped — 120-float buffer is always
//!   dense so D's NN sees a fixed layout.
//! - Carrion is seen as fixed gray (0.4, 0.4, 0.4) regardless of dead
//!   creature's original pigment (v6 §4 "What changed vs v5").

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

/// Ray hit from DDA traversal. Distance is the second field.
enum RayHit {
    Creature(usize, f32),
    Carrion(#[allow(dead_code)] usize, f32),
}

pub struct VisionPass<'a> {
    pub creatures: &'a CreatureSoA,
    pub carrion: &'a [Carrion],
    pub grid: &'a SpatialGrid,
    /// Per-cell carrion index (cell → list of carrion indices), rebuilt each pass.
    /// Size = HASH_DIM * HASH_DIM. Passed in from World's cached field.
    pub cell_to_carrion: &'a Vec<Vec<u32>>,
}

impl<'a> VisionPass<'a> {
    /// Populate `out[i]` for every creature with the post-grid-rebuild state.
    /// `out.len() >= creatures.len()` precondition.
    ///
    /// Sequential by default. Behind `cfg(feature = "threads")` the work is
    /// partitioned into at most `N_CHUNKS` disjoint creature ranges via
    /// rayon `par_chunks_mut` (mirrors the v6 §J chunking used by
    /// `nn_forward_all_chunks`). Vision is RNG-free and writes only to its
    /// own `out[i]`, so the parallel path is bit-identical to the sequential
    /// path (no sub-RNG plumbing required).
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
            // Fixed N_CHUNKS partitioning per v6 §J. div_ceil + max(1)
            // ensures rayon yields at most N_CHUNKS chunks even when
            // n < N_CHUNKS (the tail chunks just have len 0 / 1).
            // SAB-less browsers reach this code but rayon runs the work on the
            // calling thread (silent fallback per wasm-bindgen-rayon docs when
            // initThreadPool was never awaited).
            let chunk_size = n.div_ceil(crate::constants::N_CHUNKS).max(1);
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

    /// Fill `buf` for creature index `i`.
    #[inline]
    fn fill_one(&self, i: usize, buf: &mut VisionBuf) {
        // perf-5: hot reads use g_* mirrors; cold reads (eye_offsets, pigment_*) stay on &self.creatures.genomes[i].
        debug_assert_eq!(
            self.creatures.eye_trig.len(),
            self.creatures.x.len() * SECTORS * 2
        );
        let eye_count_i = self.creatures.g_eye_count[i];
        let vision_range_i = self.creatures.g_vision_range[i];
        // Short-circuit: blind or zero-range creature → all zeros.
        if eye_count_i == 0 || vision_range_i <= 0.0 {
            *buf = [0.0; VISION_LEN];
            return;
        }

        // Find eye_count position in EYE_VALID to get stride.
        let k = eye_count_i as usize;
        let k_idx = EYE_VALID.iter().position(|&v| v as usize == k).unwrap_or(0);
        if k_idx == 0 {
            // eye_count not in valid set → treat as blind
            *buf = [0.0; VISION_LEN];
            return;
        }
        let stride = EYE_STRIDE[k_idx] as usize; // 24 / k
        let ox = self.creatures.x[i];
        let oy = self.creatures.y[i];
        let max_dist = vision_range_i;

        // Zero the buffer first; inactive sectors stay zero.
        *buf = [0.0; VISION_LEN];

        let trig_base = i * SECTORS * 2;
        // Walk only active sectors using step_by(stride); read (dx, dy) from cache.
        for s in (0..SECTORS).step_by(stride) {
            let dx = self.creatures.eye_trig[trig_base + s * 2];
            let dy = self.creatures.eye_trig[trig_base + s * 2 + 1];

            if let Some(hit) = self.raycast(i, ox, oy, dx, dy, max_dist) {
                let slot = s * FEATURES_PER_SECTOR;
                match hit {
                    RayHit::Creature(j, dist) => {
                        let pigment = &self.creatures.genomes[j]; // for pigment_r/g/b only (cold)
                                                                  // Ensure dist is never near-zero (disambiguate from empty sector).
                        let d = dist.max(1e-4);
                        buf[slot] = d;
                        buf[slot + 1] = self.creatures.g_size[j]; // perf-5: mirror
                        buf[slot + 2] = pigment.pigment_r;
                        buf[slot + 3] = pigment.pigment_g;
                        buf[slot + 4] = pigment.pigment_b;
                    }
                    RayHit::Carrion(_, dist) => {
                        let d = dist.max(1e-4);
                        buf[slot] = d;
                        buf[slot + 1] = CARRION_RADIUS_FOR_VISION; // size proxy
                        buf[slot + 2] = CARRION_R;
                        buf[slot + 3] = CARRION_G;
                        buf[slot + 4] = CARRION_B;
                    }
                }
            }
        }
    }

    /// DDA along the spatial grid. Returns the closest hit within max_dist.
    fn raycast(
        &self,
        self_idx: usize,
        ox: f32,
        oy: f32,
        dx: f32,
        dy: f32,
        max_dist: f32,
    ) -> Option<RayHit> {
        // Start cell (clamped to grid bounds).
        let mut cx = ((ox / HASH_CELL) as i32).clamp(0, HASH_DIM as i32 - 1);
        let mut cy = ((oy / HASH_CELL) as i32).clamp(0, HASH_DIM as i32 - 1);

        // Step directions (+1 or -1 per axis).
        let step_x: i32 = if dx >= 0.0 { 1 } else { -1 };
        let step_y: i32 = if dy >= 0.0 { 1 } else { -1 };

        // t_delta: how far along the ray per cell crossing.
        // Handle dx/dy == 0 with f32::MAX to disable that axis.
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

        // Initial t_max: distance to first cell boundary from origin.
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
                    let rj = self.creatures.g_size[j] * BODY_RADIUS_PER_SIZE; // perf-5: mirror
                    if let Some(t) =
                        ray_circle_hit(ox, oy, dx, dy, self.creatures.x[j], self.creatures.y[j], rj)
                    {
                        if t <= best_dist {
                            best_dist = t;
                            best = Some(RayHit::Creature(j, t));
                        }
                    }
                }
            }

            // Test carrion in this cell.
            for &ci in &self.cell_to_carrion[cell_idx] {
                let c = &self.carrion[ci as usize];
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

            // Advance to next cell.
            if t_max_x < t_max_y {
                cx += step_x;
                t_max_x += t_delta_x;
            } else {
                cy += step_y;
                t_max_y += t_delta_y;
            }

            // Bounds check.
            if cx < 0 || cx >= HASH_DIM as i32 || cy < 0 || cy >= HASH_DIM as i32 {
                break;
            }
            // Exceeded range.
            if t_max_x.min(t_max_y) > max_dist {
                break;
            }
            cells_walked += 1;
            if cells_walked >= MAX_CELLS_PER_RAY {
                break;
            }
        }

        // Return best found even if we broke early (it's within max_dist).
        best
    }
}

/// Ray-circle intersection (Amanatides & Woo closed form).
/// Returns t > 0 of first intersection, or None.
/// Origin-inside case returns None (don't see things we're already touching).
fn ray_circle_hit(ox: f32, oy: f32, dx: f32, dy: f32, cx: f32, cy: f32, r: f32) -> Option<f32> {
    let mx = ox - cx;
    let my = oy - cy;
    let b = mx * dx + my * dy;
    let c = mx * mx + my * my - r * r;
    if c > 0.0 && b > 0.0 {
        return None; // origin outside, ray points away
    }
    let disc = b * b - c;
    if disc < 0.0 {
        return None; // no intersection
    }
    let t = -b - disc.sqrt();
    if t < 0.0 {
        return None; // origin inside — treat as no hit
    }
    Some(t)
}

/// Compute the angle a creature at index `i` is facing in sector `s`,
/// accounting for eye_offsets. Used by the hardcoded picker.
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

/// Build a per-cell carrion lookup from a flat carrion list.
/// Returns a Vec<Vec<u32>> of size HASH_DIM * HASH_DIM.
/// Called once per tick by World before the vision pass.
pub fn build_cell_to_carrion(carrion: &[Carrion], dst: &mut Vec<Vec<u32>>) {
    // Ensure correct size and clear.
    let total = HASH_DIM * HASH_DIM;
    if dst.len() < total {
        dst.resize_with(total, Vec::new);
    }
    for cell in dst.iter_mut() {
        cell.clear();
    }
    for (ci, c) in carrion.iter().enumerate() {
        let cell = SpatialGrid::cell_of(c.x, c.y);
        if cell < total {
            dst[cell].push(ci as u32);
        }
    }
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

    fn make_carrion_index(carrion: &[Carrion]) -> Vec<Vec<u32>> {
        let mut dst = Vec::new();
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
        // Creature A: blind
        let mut ga = genome_with_eyes(0, 80.0);
        ga.eye_count = 0;
        simple_creature(&mut creatures, 0, 100.0, 100.0, ga);
        // Creature B: in front of A
        let gb = genome_with_eyes(4, 0.0); // irrelevant
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
        // Single creature with full vision — should see nothing (self-hit excluded).
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
        assert_eq!(vision[0], [0.0f32; VISION_LEN], "self must not be hit");
    }

    #[test]
    fn ray_hits_creature_returns_color_and_distance() {
        // Creature A at (100,100) with eye_count=24, vision_range=80.
        // Creature B at (110,100): 10 world units east. A's radius=1, B's radius=1.
        // Expected dist = 10 - 1 (A radius) - 1 (B radius) = 8 (to surface).
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

        // Sector 0 points east (theta = 0). With eye_count=24, stride=1,
        // every sector is active, so sector 0 is active.
        let slot = 0; // sector 0 × FEATURES_PER_SECTOR
        let dist = vision[0][slot];
        // ray_circle_hit: mx = 100 - 110 = -10, my = 0; dx=1, dy=0.
        // b = -10, c = 100 - 1 = 99. disc = 100 - 99 = 1. t = 10 - 1 = 9.
        assert!(dist > 0.0, "sector 0 should have a hit");
        assert!((dist - 9.0).abs() < 0.1, "expected dist ~9, got {dist}");
        assert!(
            (vision[0][slot + 1] - 1.0).abs() < 1e-5,
            "size should be 1.0"
        );
        assert!((vision[0][slot + 2] - 0.9).abs() < 1e-5, "r should be 0.9");
        assert!((vision[0][slot + 3] - 0.1).abs() < 1e-5, "g should be 0.1");
        assert!((vision[0][slot + 4] - 0.2).abs() < 1e-5, "b should be 0.2");

        // Other sectors (1..23) should all be zero (nothing else to see).
        for s in 1..SECTORS {
            let off = s * FEATURES_PER_SECTOR;
            assert_eq!(vision[0][off], 0.0, "sector {s} distance should be zero");
        }
    }

    #[test]
    fn carrion_returns_gray() {
        let mut creatures = CreatureSoA::with_capacity(2);
        // Creature A facing east with 4 eyes.
        let ga = genome_with_eyes(4, 80.0);
        simple_creature(&mut creatures, 0, 100.0, 100.0, ga);

        // Carrion directly east at (115, 100).
        let carrion = vec![Carrion {
            id: 999,
            x: 115.0,
            y: 100.0,
            pool: 1.0,
            age: 0,
            sun_cell: 0,
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

        // eye_count=4 → stride=6, active sectors: 0, 6, 12, 18.
        // Sector 0 points east → should see carrion.
        let slot = 0; // sector 0 × FEATURES_PER_SECTOR
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
        // Target placed at distance 90 > vision_range 80 → sector should be zero.
        let mut creatures = CreatureSoA::with_capacity(4);
        let ga = genome_with_eyes(24, 80.0); // range 80
        simple_creature(&mut creatures, 0, 100.0, 100.0, ga);

        let mut gb = Genome::founder();
        gb.size = 1.0;
        simple_creature(&mut creatures, 1, 191.0, 100.0, gb); // 91 units east

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

        let slot = 0; // sector 0 × FEATURES_PER_SECTOR
                      // 191 - 100 = 91 units; radius A=1, radius B=1 → surface-to-surface = 89 > 80.
        assert_eq!(
            vision[0][slot], 0.0,
            "target out of range must yield zero distance"
        );
    }
}
