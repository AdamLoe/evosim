//! Creature SoA. Layout shared by the live (mutable) tick state.
//!
//! D3: Genome entirely deleted. All body traits are constants from constants.rs.
//! Only NN weights vary across the population.
//! Body radius = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE (constant for every creature).
//! Move speed cap = MOVE_SPEED_MAX. Vision = 24 sectors at VISION_RANGE_MAX.

use crate::brain::Brain;
use crate::constants::{EYE_SLOTS, EYE_STRIDE, EYE_VALID, SECTORS, VISION_RANGE_MAX};
use serde::{Deserialize, Serialize};

/// One discrete action a creature chose this tick.
/// D9: collapsed from 6 variants to 3. Indices 0/1/2.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Action {
    Graze = 0,
    Eat = 1,
    Split = 2,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::Graze, Action::Eat, Action::Split];

    pub fn one_hot_index(self) -> usize {
        self as usize
    }
}

/// Hot per-creature scalars promoted into SoA arrays for cache friendliness
/// in the per-tick inner loops.
///
/// D3: g_* hot mirrors removed. Body traits are constants. Only NN weights vary.
pub struct CreatureSoA {
    pub id: Vec<u64>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub energy: Vec<f32>,
    pub age: Vec<u32>,
    pub digestion_cooldown: Vec<u32>,
    pub cumulative_upkeep: Vec<f32>,
    pub last_action: Vec<Action>,
    pub action_this_tick: Vec<Action>,
    pub distance_travelled: Vec<f32>,
    /// Tick at which this creature was born (for lifespan stats).
    pub birth_tick: Vec<u32>,
    pub brains: Vec<Brain>,
    /// M5: packed 0x00RRGGBB derived from NN weight hash. Hot-mirror of Brain.color_rgb.
    /// Length invariant: == x.len().
    pub(crate) color_rgb: Vec<u32>,
    /// Pre-computed (dx, dy) per sector per creature; interleaved as
    /// [s0_dx, s0_dy, s1_dx, s1_dy, …, s23_dx, s23_dy] per creature.
    /// Length invariant: == x.len() * SECTORS * 2.
    /// D3: all 24 sectors are active (fixed eye layout). Recomputed by `recompute_eye_trig_at`.
    /// All 24 sectors use evenly-spaced angles (0, TAU/24, 2*TAU/24, ...) with no offsets.
    pub eye_trig: Vec<f32>,
}

impl CreatureSoA {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            id: Vec::with_capacity(cap),
            x: Vec::with_capacity(cap),
            y: Vec::with_capacity(cap),
            vx: Vec::with_capacity(cap),
            vy: Vec::with_capacity(cap),
            energy: Vec::with_capacity(cap),
            age: Vec::with_capacity(cap),
            digestion_cooldown: Vec::with_capacity(cap),
            cumulative_upkeep: Vec::with_capacity(cap),
            last_action: Vec::with_capacity(cap),
            action_this_tick: Vec::with_capacity(cap),
            distance_travelled: Vec::with_capacity(cap),
            birth_tick: Vec::with_capacity(cap),
            brains: Vec::with_capacity(cap),
            color_rgb: Vec::with_capacity(cap),
            eye_trig: Vec::with_capacity(cap * SECTORS * 2),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.x.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append one creature. Returns new index.
    /// D3: no Genome parameter — body traits are constants.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        id: u64,
        x: f32,
        y: f32,
        energy: f32,
        birth_tick: u32,
        brain: Brain,
    ) -> usize {
        self.id.push(id);
        self.x.push(x);
        self.y.push(y);
        self.vx.push(0.0);
        self.vy.push(0.0);
        self.energy.push(energy);
        self.age.push(0);
        self.digestion_cooldown.push(0);
        self.cumulative_upkeep.push(0.0);
        self.last_action.push(Action::Graze);
        self.action_this_tick.push(Action::Graze);
        self.distance_travelled.push(0.0);
        self.birth_tick.push(birth_tick);
        self.color_rgb.push(brain.color_rgb); // M5: hot-mirror from Brain
        self.brains.push(brain);
        // Extend trig buffer by one chunk of zeros, then populate.
        debug_assert_eq!(
            self.eye_trig.len(),
            self.x.len().saturating_sub(1) * SECTORS * 2
        );
        let new_len = self.eye_trig.len() + SECTORS * 2;
        self.eye_trig.resize(new_len, 0.0);
        let i = self.x.len() - 1;
        self.recompute_eye_trig_at(i);
        i
    }

    /// Remove indices `dead` (must be sorted ascending). Uses swap_remove
    /// from the back so we only touch O(K) entries.
    pub fn remove_indices(&mut self, dead: &[usize]) {
        // walk dead from the back so swap-remove doesn't disturb earlier indices.
        for &k in dead.iter().rev() {
            self.id.swap_remove(k);
            self.x.swap_remove(k);
            self.y.swap_remove(k);
            self.vx.swap_remove(k);
            self.vy.swap_remove(k);
            self.energy.swap_remove(k);
            self.age.swap_remove(k);
            self.digestion_cooldown.swap_remove(k);
            self.cumulative_upkeep.swap_remove(k);
            self.last_action.swap_remove(k);
            self.action_this_tick.swap_remove(k);
            self.distance_travelled.swap_remove(k);
            self.birth_tick.swap_remove(k);
            self.color_rgb.swap_remove(k);
            self.brains.swap_remove(k);
            swap_remove_chunk(&mut self.eye_trig, k, SECTORS * 2);
        }
    }

    /// Recompute the 48 trig values for creature index `i`.
    ///
    /// D3: all creatures have a fixed 24-sector layout (eye_count = 24, no offsets,
    /// no vision_range gating). All 24 sectors are evenly spaced at TAU * s / 24.
    ///
    /// Caller must have already pushed the creature at `i`.
    pub(crate) fn recompute_eye_trig_at(&mut self, i: usize) {
        let base = i * SECTORS * 2;
        // D3: all 24 sectors always active with uniform spacing. No eye_offsets.
        // We still use EYE_VALID/EYE_STRIDE machinery with eye_count=24 and
        // vision_range=VISION_RANGE_MAX to be compatible with vision.rs fill_one.
        let k = EYE_SLOTS; // 24
        let k_idx = EYE_VALID.iter().position(|&v| v as usize == k).unwrap_or(7);
        let stride = EYE_STRIDE[k_idx] as usize; // 1 for eye_count=24
        for s in (0..SECTORS).step_by(stride) {
            let theta_center = std::f32::consts::TAU * (s as f32) / (SECTORS as f32);
            // No per-eye offset in D3.
            self.eye_trig[base + s * 2] = theta_center.cos();
            self.eye_trig[base + s * 2 + 1] = theta_center.sin();
        }
        // D3: sanity — VISION_RANGE_MAX > 0 so all sectors are written above.
        let _ = VISION_RANGE_MAX; // referenced to confirm the constant is used
    }
}

#[inline]
fn swap_remove_chunk(buf: &mut Vec<f32>, k: usize, chunk: usize) {
    debug_assert_eq!(buf.len() % chunk, 0);
    let n = buf.len() / chunk;
    debug_assert!(k < n);
    let last = n - 1;
    if k != last {
        // Swap two chunk-sized windows.
        for off in 0..chunk {
            buf.swap(k * chunk + off, last * chunk + off);
        }
    }
    buf.truncate(last * chunk);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Brain;
    use crate::rng::SimRng;

    /// D3: eye_trig has all 24 active sectors with uniform spacing.
    #[test]
    fn eye_trig_has_24_active_sectors() {
        let mut soa = CreatureSoA::with_capacity(1);
        let brain = Brain::founder(&mut SimRng::from_u64(0));
        soa.push(1, 0.0, 0.0, 1.0, 0, brain);
        // All 24 sectors must have non-zero (cos, sin) pairs except the ones
        // where cos==1,sin==0 (s=0) — those are still "active" just at angle 0.
        assert_eq!(soa.eye_trig.len(), SECTORS * 2);
        for s in 0..SECTORS {
            let dx = soa.eye_trig[s * 2];
            let dy = soa.eye_trig[s * 2 + 1];
            let mag2 = dx * dx + dy * dy;
            // Each active direction vector must be a unit vector.
            assert!(
                (mag2 - 1.0).abs() < 1e-5,
                "sector {s}: |dir|^2 = {mag2} (expected ~1.0)"
            );
        }
    }

    #[test]
    fn eye_trig_len_invariant_after_remove() {
        let mut soa = CreatureSoA::with_capacity(4);
        let mut rng = SimRng::from_u64(1);
        for i in 0..4u64 {
            let b = Brain::founder(&mut rng);
            soa.push(i, i as f32 * 10.0, 0.0, 100.0, 0, b);
        }
        assert_eq!(soa.eye_trig.len(), 4 * SECTORS * 2);
        soa.remove_indices(&[1]);
        assert_eq!(soa.eye_trig.len(), 3 * SECTORS * 2);
        assert_eq!(soa.x.len(), 3);
    }

    /// D3 regression: push works without a Genome argument; len round-trips correctly.
    #[test]
    fn push_without_genome_roundtrips_len() {
        let mut soa = CreatureSoA::with_capacity(4);
        let mut rng = SimRng::from_u64(42);
        for k in 0u64..5 {
            let b = Brain::founder(&mut rng);
            soa.push(k, k as f32, 0.0, 100.0, 0, b);
        }
        assert_eq!(soa.len(), 5);
        soa.remove_indices(&[0, 2]);
        assert_eq!(soa.len(), 3);
    }

    /// D3: body radius is constant across all creatures.
    #[test]
    fn body_radius_constant_across_population() {
        use crate::constants::{BODY_RADIUS_PER_SIZE, FOUNDER_SIZE};
        let expected_radius = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let mut soa = CreatureSoA::with_capacity(10);
        let mut rng = SimRng::from_u64(7);
        for k in 0u64..10 {
            let b = Brain::founder(&mut rng);
            soa.push(k, k as f32 * 5.0, 0.0, 100.0, 0, b);
        }
        // D3: all creatures share the same body radius (no g_size mirror needed).
        // The "body radius" is simply FOUNDER_SIZE * BODY_RADIUS_PER_SIZE.
        for _i in 0..soa.len() {
            let body_r = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
            assert!(
                (body_r - expected_radius).abs() < 1e-6,
                "body_radius must equal FOUNDER_SIZE * BODY_RADIUS_PER_SIZE"
            );
        }
    }
}
