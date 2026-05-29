//! Creature SoA. Layout shared by the live (mutable) tick state.
//!
//! D3: Genome entirely deleted. All body traits are constants from constants.rs.
//! Only NN weights vary across the population.
//! Body radius = CREATURE_SIZE * BODY_RADIUS_PER_SIZE (constant for every creature).
//! Move speed cap = MOVE_SPEED_MAX.
//! v1.5 S5b: vision raycast removed; `eye_trig`, `last2_action`, `prev_energy`
//! columns deleted (no consumer after the 32-input semantic rewrite).

use crate::brain::Brain;
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
    /// Ticks elapsed since the creature last performed a Split action (or
    /// since birth, if it has never split). Reset to 0 on the tick a parent
    /// splits; incremented in bookkeeping_tail. Drives NN input slot 6.
    pub ticks_since_split: Vec<u32>,
    pub brains: Vec<Brain>,
    /// v1.5 S3: per-creature action-EMA color channels in [0, 1].
    /// Green tracks Graze argmax; red tracks successful bites; blue tracks Split.
    /// Initialized to 0.0 at birth — color emerges from behavior.
    pub color_r: Vec<f32>,
    pub color_g: Vec<f32>,
    pub color_b: Vec<f32>,
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
            ticks_since_split: Vec::with_capacity(cap),
            brains: Vec::with_capacity(cap),
            color_r: Vec::with_capacity(cap),
            color_g: Vec::with_capacity(cap),
            color_b: Vec::with_capacity(cap),
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
        self.ticks_since_split.push(0);
        // v1.5 S3: color EMA starts at zero — emerges from action history.
        self.color_r.push(0.0);
        self.color_g.push(0.0);
        self.color_b.push(0.0);
        self.brains.push(brain);
        self.x.len() - 1
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
            self.ticks_since_split.swap_remove(k);
            self.color_r.swap_remove(k);
            self.color_g.swap_remove(k);
            self.color_b.swap_remove(k);
            self.brains.swap_remove(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Brain;
    use crate::rng::SimRng;

    /// v1.5 S5b: remove_indices keeps every SoA column length-consistent.
    #[test]
    fn remove_indices_preserves_column_lengths() {
        let mut soa = CreatureSoA::with_capacity(4);
        let mut rng = SimRng::from_u64(1);
        for i in 0..4u64 {
            let b = Brain::founder(&mut rng);
            soa.push(i, i as f32 * 10.0, 0.0, 100.0, 0, b);
        }
        assert_eq!(soa.len(), 4);
        soa.remove_indices(&[1]);
        assert_eq!(soa.len(), 3);
        assert_eq!(soa.x.len(), 3);
        assert_eq!(soa.color_r.len(), 3);
        assert_eq!(soa.ticks_since_split.len(), 3);
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

    /// v1.5 S3: color EMA channels initialize to 0.0 at birth.
    #[test]
    fn color_ema_zero_at_birth() {
        let mut soa = CreatureSoA::with_capacity(2);
        let mut rng = SimRng::from_u64(7);
        for k in 0u64..3 {
            let b = Brain::founder(&mut rng);
            soa.push(k, k as f32, 0.0, 100.0, 0, b);
        }
        for i in 0..soa.len() {
            assert_eq!(soa.color_r[i], 0.0, "color_r[{i}] must be 0 at birth");
            assert_eq!(soa.color_g[i], 0.0, "color_g[{i}] must be 0 at birth");
            assert_eq!(soa.color_b[i], 0.0, "color_b[{i}] must be 0 at birth");
        }
    }
}
