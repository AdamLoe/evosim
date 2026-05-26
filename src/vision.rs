//! Per-creature vision: 24 sector slots × 5 features.
//! Filled in tick step 2 (v5 §3.5). Reused by D as NN inputs (v6 §E).
//!
//! D3: all creatures have a fixed 24-sector layout at VISION_RANGE_MAX.
//! Eye count variation removed — every creature uses all 24 sectors (stride=1).
//! Size slot uses constant FOUNDER_SIZE. Pigment slots use placeholder 0.5 gray
//! (M5 owns NN-hash color hot-mirror).
//!
//! Design decisions pinned here for Milestone D inheritance:
//! - Sector layout: all 24 sectors active (stride=1). Slot-center angle = 2π × s / 24.
//!   Eye_trig values are computed once at push time and recomputed on vision_range change.
//! - Inactive sectors write zeros — not applicable post-D3 (all sectors always active).
//! - Carrion is seen as fixed gray (0.4, 0.4, 0.4) regardless of dead
//!   creature's original pigment (v6 §4 "What changed vs v5").

use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grid::SpatialGrid;
use crate::torus::torus_delta;

pub use crate::constants::SECTORS;
pub const FEATURES_PER_SECTOR: usize = 5; // dist, size, r, g, b
pub const VISION_LEN: usize = SECTORS * FEATURES_PER_SECTOR; // 120

/// Fixed per-creature vision buffer. World owns Vec<VisionBuf>.
pub type VisionBuf = [f32; VISION_LEN];

/// Carrion is rendered/seen as fixed gray per v6 §4 "What changed vs v5".
pub const CARRION_R: f32 = 0.4;
pub const CARRION_G: f32 = 0.4;
pub const CARRION_B: f32 = 0.4;

/// D3: placeholder pigment for live creatures. M5 owns NN-hash color hot-mirror.
const CREATURE_PIGMENT: f32 = 0.5;

/// CSR-layout per-cell carrion index. Replaces `Vec<Vec<u32>>` (S26).
///
/// Layout: `starts[c]..starts[c+1]` indexes into `indices` for cell `c`.
/// Rebuilt O(C + carrion_count) each tick before the vision pass via
/// `rebuild()`. `cursors` is a reusable scratch buffer for the scatter
/// pass; it is always the same length as `starts` and is never saved.
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
    /// O(cells + carrion_count). Reuses allocations; no heap traffic per call.
    pub fn rebuild(&mut self, carrion: &[Carrion]) {
        // Count pass: tally carrion per cell.
        self.starts.iter_mut().for_each(|s| *s = 0);
        for c in carrion {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            self.starts[cell + 1] += 1;
        }
        // Prefix sum → row-start offsets.
        for k in 1..self.starts.len() {
            self.starts[k] += self.starts[k - 1];
        }
        // Scatter pass: write carrion indices into flat array.
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

    /// Return the slice of carrion indices in `cell` (0..HASH_DIM*HASH_DIM).
    #[inline]
    pub fn cell_slice(&self, cell: usize) -> &[u32] {
        let s = self.starts[cell] as usize;
        let e = self.starts[cell + 1] as usize;
        &self.indices[s..e]
    }
}

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
    /// S26: CSR layout replaces Vec<Vec<u32>>.
    pub cell_to_carrion: &'a CarrionIndex,
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

    /// Fill `buf` for creature index `i`.
    /// D3: all 24 sectors always active (eye_count = 24 constant, stride = 1).
    /// Size uses FOUNDER_SIZE constant. Pigment uses placeholder gray.
    #[inline]
    fn fill_one(&self, i: usize, buf: &mut VisionBuf) {
        debug_assert_eq!(
            self.creatures.eye_trig.len(),
            self.creatures.x.len() * SECTORS * 2
        );
        // D3: all 24 sectors active at VISION_RANGE_MAX. No eye_count/vision_range gating.
        let max_dist = VISION_RANGE_MAX;
        let ox = self.creatures.x[i];
        let oy = self.creatures.y[i];

        // Zero the buffer first.
        *buf = [0.0; VISION_LEN];

        let trig_base = i * SECTORS * 2;
        // D3: stride = 1, all 24 sectors active.
        for s in 0..SECTORS {
            let dx = self.creatures.eye_trig[trig_base + s * 2];
            let dy = self.creatures.eye_trig[trig_base + s * 2 + 1];

            if let Some(hit) = self.raycast(i, ox, oy, dx, dy, max_dist) {
                let slot = s * FEATURES_PER_SECTOR;
                match hit {
                    RayHit::Creature(j, dist) => {
                        // D3: size = FOUNDER_SIZE constant; pigment = 0.5 placeholder gray.
                        let d = dist.max(1e-4);
                        buf[slot] = d;
                        buf[slot + 1] = FOUNDER_SIZE; // constant body size
                        buf[slot + 2] = CREATURE_PIGMENT; // M5: placeholder gray
                        buf[slot + 3] = CREATURE_PIGMENT;
                        buf[slot + 4] = CREATURE_PIGMENT;
                        // Suppress unused variable warning.
                        let _ = j;
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
    ///
    /// Toroidal: cell coordinates are wrapped via `rem_euclid(HASH_DIM)` on
    /// every step, so rays traverse seamlessly across world edges. Targets are
    /// re-anchored via `torus_delta` so `ray_circle_hit` always sees the
    /// shortest-path vector to each candidate. The old out-of-bounds break is
    /// removed; `MAX_CELLS_PER_RAY` already bounds the walk.
    ///
    /// See v1.2 grass mechanic brief (P1a §2.3).
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

        // Start cell — wrap with rem_euclid instead of clamp so the DDA
        // begins in the correct toroidal cell even for out-of-range positions.
        let mut cx = (ox / HASH_CELL).floor() as i32;
        let mut cy = (oy / HASH_CELL).floor() as i32;

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
            // Toroidal: wrap the raw cell coordinate before indexing.
            let wcx = cx.rem_euclid(dim) as usize;
            let wcy = cy.rem_euclid(dim) as usize;
            let cell_idx = wcy * HASH_DIM + wcx;

            // Test creatures in this cell.
            {
                let start = self.grid.starts[cell_idx] as usize;
                let end = self.grid.starts[cell_idx + 1] as usize;
                for k in start..end {
                    let j = self.grid.indices[k] as usize;
                    if j == self_idx {
                        continue;
                    }
                    // D3: body radius is constant — FOUNDER_SIZE * BODY_RADIUS_PER_SIZE.
                    let rj = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
                    // Re-anchor target to the shortest-path position relative to the ray origin.
                    let tx_raw = self.creatures.x[j];
                    let ty_raw = self.creatures.y[j];
                    let (ddx, ddy) = torus_delta(tx_raw - ox, ty_raw - oy);
                    let tx = ox + ddx;
                    let ty = oy + ddy;
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
                // Re-anchor carrion position to shortest-path.
                let (ddx, ddy) = torus_delta(c.x - ox, c.y - oy);
                let tx = ox + ddx;
                let ty = oy + ddy;
                if let Some(t) = ray_circle_hit(ox, oy, dx, dy, tx, ty, CARRION_RADIUS_FOR_VISION) {
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
            if t_max_x.min(t_max_y) > max_dist {
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
            // Toroidal: no out-of-bounds break; MAX_CELLS_PER_RAY bounds the walk.
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

/// Rebuild a per-cell carrion lookup from a flat carrion list.
/// Called once per tick by World before the vision pass (S26: CSR layout).
pub fn build_cell_to_carrion(carrion: &[Carrion], dst: &mut CarrionIndex) {
    dst.rebuild(carrion);
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

    fn make_carrion_index(carrion: &[Carrion]) -> CarrionIndex {
        let mut dst = CarrionIndex::new();
        build_cell_to_carrion(carrion, &mut dst);
        dst
    }

    /// D3: push a creature without genome — uses only Brain.
    fn simple_creature(soa: &mut CreatureSoA, id: u64, x: f32, y: f32) -> usize {
        let mut rng = SimRng::from_u64(42);
        let brain = Brain::founder(&mut rng);
        soa.push(id, x, y, 100.0, 0, 0, 0, brain)
    }

    #[test]
    fn ray_hits_self_ignored() {
        // Single creature with full vision — should see nothing (self-hit excluded).
        let mut creatures = CreatureSoA::with_capacity(4);
        simple_creature(&mut creatures, 0, 300.0, 300.0);

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
    fn ray_hits_creature_returns_distance_and_constant_size() {
        // Creature A at (100,100) with eye_count=24, vision_range=80.
        // Creature B at (110,100): 10 world units east. Both radius=FOUNDER_SIZE*BODY_RADIUS_PER_SIZE=1.
        // Expected dist = 10 - 1 (A radius) - 1 (B radius) = 8 (to surface).
        let mut creatures = CreatureSoA::with_capacity(4);
        simple_creature(&mut creatures, 0, 100.0, 100.0);
        simple_creature(&mut creatures, 1, 110.0, 100.0);

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

        // Sector 0 points east (theta = 0). D3: stride=1, all sectors active.
        let slot = 0; // sector 0 × FEATURES_PER_SECTOR
        let dist = vision[0][slot];
        // ray_circle_hit: mx = 100 - 110 = -10, my = 0; dx=1, dy=0.
        // b = -10, c = 100 - 1 = 99. disc = 100 - 99 = 1. t = 10 - 1 = 9.
        assert!(dist > 0.0, "sector 0 should have a hit");
        assert!((dist - 9.0).abs() < 0.1, "expected dist ~9, got {dist}");
        // D3: size slot uses FOUNDER_SIZE constant.
        assert!(
            (vision[0][slot + 1] - FOUNDER_SIZE).abs() < 1e-5,
            "size should be FOUNDER_SIZE={FOUNDER_SIZE}"
        );
        // D3: pigment slots use placeholder gray (0.5).
        assert!(
            (vision[0][slot + 2] - CREATURE_PIGMENT).abs() < 1e-5,
            "r should be 0.5"
        );
        assert!(
            (vision[0][slot + 3] - CREATURE_PIGMENT).abs() < 1e-5,
            "g should be 0.5"
        );
        assert!(
            (vision[0][slot + 4] - CREATURE_PIGMENT).abs() < 1e-5,
            "b should be 0.5"
        );

        // Other sectors (1..23) should all be zero (nothing else to see).
        for s in 1..SECTORS {
            let off = s * FEATURES_PER_SECTOR;
            assert_eq!(vision[0][off], 0.0, "sector {s} distance should be zero");
        }
    }

    #[test]
    fn carrion_returns_gray() {
        let mut creatures = CreatureSoA::with_capacity(2);
        // Creature A at (100, 100) with 24 active sectors.
        simple_creature(&mut creatures, 0, 100.0, 100.0);

        // Carrion directly east at (115, 100).
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

        // D3: stride=1, all sectors active. Sector 0 points east → should see carrion.
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
        // Target placed at distance 90 > VISION_RANGE_MAX (80) → sector should be zero.
        let mut creatures = CreatureSoA::with_capacity(4);
        simple_creature(&mut creatures, 0, 100.0, 100.0);
        simple_creature(&mut creatures, 1, 191.0, 100.0); // 91 units east

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

    // ---- Toroidal raycast tests ----

    /// P1a: A creature looking east from near the right seam (x=595) should
    /// see a target at x=5 via wrap-around (10 world units away via torus).
    #[test]
    fn raycast_sees_across_seam() {
        let mut creatures = CreatureSoA::with_capacity(4);
        // Creature A at (595, 300): facing east (sector 0, all sectors active in D3).
        simple_creature(&mut creatures, 0, 595.0, 300.0);
        // Creature B at (5, 300): 10 world units east via torus seam.
        simple_creature(&mut creatures, 1, 5.0, 300.0);

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

        // Sector 0 (east) should see creature B. Distance = 10 - 1 (A radius) - 1 (B radius) = 8.
        let slot = 0;
        let dist = vision[0][slot];
        assert!(
            dist > 0.0,
            "sector 0 should see creature across the seam; dist = {dist}"
        );
        // ray_circle_hit returns t measured from the ray origin (center of A).
        // With centers 10u apart via torus, B's radius = 1 → t = 10 - 1 = 9.
        assert!(
            (dist - 9.0).abs() < 0.5,
            "expected dist ≈ 9 (across seam), got {dist}"
        );
    }

    // ---- S26 CarrionIndex CSR tests ----

    /// S26 test 1: CSR membership matches a brute-force reference. For every
    /// cell in the grid, the set of carrion indices returned by `cell_slice`
    /// must equal the set that a Vec<Vec<u32>> build would have produced.
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
            }, // same cell as [0]
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

        // Build reference layout.
        let total = HASH_DIM * HASH_DIM;
        let mut ref_dst: Vec<Vec<u32>> = vec![Vec::new(); total];
        for (ci, c) in carrion.iter().enumerate() {
            let cell = SpatialGrid::cell_of(c.x, c.y);
            ref_dst[cell].push(ci as u32);
        }

        // Build CSR layout.
        let mut csr = CarrionIndex::new();
        csr.rebuild(&carrion);

        // Compare membership for every cell.
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

    /// S26 test 2: Carrion indices within each cell are stored in insertion
    /// (enumeration) order. Relies on the scatter pass walking `carrion` in
    /// forward order, mirroring the old Vec<Vec<u32>>::push order.
    #[test]
    fn csr_iteration_order_is_insertion_order() {
        // Place three carrion in the SAME cell so order matters.
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

    /// S26 test 3: empty carrion list and carrion at world corners produce
    /// valid (non-panicking), correct CSR state.
    #[test]
    fn csr_empty_and_full_corners() {
        // Empty list: all cells must be empty slices.
        let mut csr = CarrionIndex::new();
        csr.rebuild(&[]);
        for cell in 0..HASH_DIM * HASH_DIM {
            assert!(
                csr.cell_slice(cell).is_empty(),
                "cell {cell} must be empty for zero carrion"
            );
        }

        // Corner carrion: one blob at each corner of the world.
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
        // Each corner must be reachable.
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
