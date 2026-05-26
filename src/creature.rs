//! Creature SoA. Layout shared by the live (mutable) tick state and the
//! double-buffered snapshot used by the persistence worker in F.26.
//!
//! Genome and Brain live in `genome.rs` / `brain.rs` and are stored as
//! per-creature `Vec` entries (AoS-within-SoA — fine because we touch the
//! whole struct on actions like split / mutation, not on per-trait inner
//! loops, except for the hot ones lifted out into the SoA arrays below).

use crate::brain::Brain;
use crate::constants::{EYE_STRIDE, EYE_VALID, SECTORS};
use crate::genome::Genome;
use serde::{Deserialize, Serialize};

/// One discrete action a creature chose this tick. v1.3 D9: collapsed to 3 variants.
/// Discriminants are stable serialisation keys — do NOT reorder.
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
/// in the per-tick inner loops. Everything else lives behind index-aligned
/// `genomes` / `brains` Vecs on the World.
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
    pub species_id: Vec<u32>,
    pub parent_species_id: Vec<u32>,
    pub last_action: Vec<Action>,
    pub action_this_tick: Vec<Action>,
    pub max_size_reached: Vec<f32>,
    pub distance_travelled: Vec<f32>,
    /// Tick at which this creature was born (for "last survivor" + lifespan stats).
    pub birth_tick: Vec<u32>,
    pub genomes: Vec<Genome>,
    pub brains: Vec<Brain>,
    /// Pre-computed (dx, dy) per sector per creature; interleaved as
    /// [s0_dx, s0_dy, s1_dx, s1_dy, …, s23_dx, s23_dy] per creature.
    /// Length invariant: == x.len() * SECTORS * 2.
    /// Inactive sectors are zero. Recomputed by `recompute_eye_trig_at`.
    /// Excluded from save (re-derived on push) and from snapshot_hash
    /// (would double-count the genome bytes it derives from).
    pub eye_trig: Vec<f32>,

    // Hot-field mirrors (perf-5). Each entry at index i is bit-identically
    // equal to genomes[i].<field>. Written only by `push`, `remove_indices`,
    // and `resync_hot_mirrors_at`. Read by every per-tick hot phase
    // (graze_step, energy_bookkeeping, apply_movement_and_repulsion,
    // build_nn_input, count_carrion_overlap,
    // eat_and_scavenge, VisionPass::fill_one).
    /// Mirror of genomes[i].size. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_size: Vec<f32>,
    /// Mirror of genomes[i].graze_efficiency. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_graze_eff: Vec<f32>,
    /// Mirror of genomes[i].eat_efficiency. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_eat_eff: Vec<f32>,
    /// Mirror of genomes[i].scavenge_efficiency. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_scav_eff: Vec<f32>,
    /// Mirror of genomes[i].move_speed. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_move_speed: Vec<f32>,
    /// Mirror of genomes[i].vision_range. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_vision_range: Vec<f32>,
    /// Mirror of genomes[i].eye_count. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_eye_count: Vec<u8>,
    /// Mirror of genomes[i].nose_count. Written ONLY by CreatureSoA::push,
    /// remove_indices, and resync_hot_mirrors_at. Read by hot tick paths.
    pub(crate) g_nose_count: Vec<u8>,
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
            species_id: Vec::with_capacity(cap),
            parent_species_id: Vec::with_capacity(cap),
            last_action: Vec::with_capacity(cap),
            action_this_tick: Vec::with_capacity(cap),
            max_size_reached: Vec::with_capacity(cap),
            distance_travelled: Vec::with_capacity(cap),
            birth_tick: Vec::with_capacity(cap),
            genomes: Vec::with_capacity(cap),
            brains: Vec::with_capacity(cap),
            eye_trig: Vec::with_capacity(cap * SECTORS * 2),
            g_size: Vec::with_capacity(cap),
            g_graze_eff: Vec::with_capacity(cap),
            g_eat_eff: Vec::with_capacity(cap),
            g_scav_eff: Vec::with_capacity(cap),
            g_move_speed: Vec::with_capacity(cap),
            g_vision_range: Vec::with_capacity(cap),
            g_eye_count: Vec::with_capacity(cap),
            g_nose_count: Vec::with_capacity(cap),
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
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        id: u64,
        x: f32,
        y: f32,
        energy: f32,
        species_id: u32,
        parent_species_id: u32,
        birth_tick: u32,
        genome: Genome,
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
        self.species_id.push(species_id);
        self.parent_species_id.push(parent_species_id);
        self.last_action.push(Action::Graze);
        self.action_this_tick.push(Action::Graze);
        self.max_size_reached.push(genome.size);
        self.distance_travelled.push(0.0);
        self.birth_tick.push(birth_tick);
        self.push_hot_mirrors(&genome); // perf-5: BEFORE genome move
        self.genomes.push(genome);
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
            self.species_id.swap_remove(k);
            self.parent_species_id.swap_remove(k);
            self.last_action.swap_remove(k);
            self.action_this_tick.swap_remove(k);
            self.max_size_reached.swap_remove(k);
            self.distance_travelled.swap_remove(k);
            self.birth_tick.swap_remove(k);
            self.genomes.swap_remove(k);
            self.brains.swap_remove(k);
            swap_remove_chunk(&mut self.eye_trig, k, SECTORS * 2);
            self.g_size.swap_remove(k);
            self.g_graze_eff.swap_remove(k);
            self.g_eat_eff.swap_remove(k);
            self.g_scav_eff.swap_remove(k);
            self.g_move_speed.swap_remove(k);
            self.g_vision_range.swap_remove(k);
            self.g_eye_count.swap_remove(k);
            self.g_nose_count.swap_remove(k);
        }
    }

    /// Push the seven hot mirror scalars from `g` onto the parallel Vecs.
    /// MUST be called exactly once per `genomes.push(...)`, immediately before it,
    /// to avoid moving `g` before borrowing it.
    fn push_hot_mirrors(&mut self, g: &Genome) {
        self.g_size.push(g.size);
        self.g_graze_eff.push(g.graze_efficiency);
        self.g_eat_eff.push(g.eat_efficiency);
        self.g_scav_eff.push(g.scavenge_efficiency);
        self.g_move_speed.push(g.move_speed);
        self.g_vision_range.push(g.vision_range);
        self.g_eye_count.push(g.eye_count);
        self.g_nose_count.push(g.nose_count);
    }

    /// Resync the seven hot mirror scalars at index `i` from `genomes[i]`.
    /// Must be called after any in-place edit of a hot genome field at `i`
    /// (e.g. test fixtures in world.rs) before any hot tick path is invoked.
    #[allow(dead_code)] // used only in #[cfg(test)] blocks in world.rs
    pub(crate) fn resync_hot_mirrors_at(&mut self, i: usize) {
        let g = &self.genomes[i];
        self.g_size[i] = g.size;
        self.g_graze_eff[i] = g.graze_efficiency;
        self.g_eat_eff[i] = g.eat_efficiency;
        self.g_scav_eff[i] = g.scavenge_efficiency;
        self.g_move_speed[i] = g.move_speed;
        self.g_vision_range[i] = g.vision_range;
        self.g_eye_count[i] = g.eye_count;
        self.g_nose_count[i] = g.nose_count;
    }

    /// Recompute the 48 trig values for creature index `i` from its genome.
    /// Caller must have already pushed the genome at `i`.
    ///
    /// Zeroes the full 48-slot window first. For zero-range / zero-eye creatures
    /// all 48 slots stay zero (matching `fill_one`'s short-circuit). For sighted
    /// creatures, active-sector slots are overwritten below.
    ///
    /// MUST also be called after any in-place edit of `genomes[i].eye_count` or
    /// `genomes[i].eye_offsets` (e.g. test fixtures in world.rs and vision.rs::tests).
    pub(crate) fn recompute_eye_trig_at(&mut self, i: usize) {
        let g = &self.genomes[i];
        let base = i * SECTORS * 2;
        // Zero the 48-slot window first; inactive sectors stay zero.
        for v in &mut self.eye_trig[base..base + SECTORS * 2] {
            *v = 0.0;
        }
        let k = g.eye_count as usize;
        if k == 0 || g.vision_range <= 0.0 {
            return;
        }
        let k_idx = EYE_VALID.iter().position(|&v| v as usize == k).unwrap_or(0);
        if k_idx == 0 {
            return;
        }
        let stride = EYE_STRIDE[k_idx] as usize;
        for s in (0..SECTORS).step_by(stride) {
            let active_index = s / stride;
            let offset = if active_index < g.eye_offsets.len() {
                g.eye_offsets[active_index]
            } else {
                0.0
            };
            let theta_center = std::f32::consts::TAU * (s as f32) / (SECTORS as f32);
            let theta_ray = theta_center + offset;
            self.eye_trig[base + s * 2] = theta_ray.cos();
            self.eye_trig[base + s * 2 + 1] = theta_ray.sin();
        }
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
    use crate::genome::{Genome, TraitMutationRates};
    use crate::rng::SimRng;

    #[test]
    fn eye_trig_matches_manual_compute_for_4_eyes() {
        let mut soa = CreatureSoA::with_capacity(1);
        let mut g = Genome::founder();
        g.eye_count = 4;
        g.vision_range = 80.0;
        g.eye_offsets = [
            0.1, 0.2, 0.3, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let brain = Brain::founder(&mut SimRng::from_u64(0));
        soa.push(1, 0.0, 0.0, 1.0, 0, 0, 0, g.clone(), brain);

        // eye_count=4 → stride=6, active sectors 0, 6, 12, 18.
        for (active_index, &s) in [0usize, 6, 12, 18].iter().enumerate() {
            let theta_center = std::f32::consts::TAU * (s as f32) / 24.0;
            let theta_ray = theta_center + g.eye_offsets[active_index];
            assert_eq!(soa.eye_trig[s * 2], theta_ray.cos(), "sector {s} dx");
            assert_eq!(soa.eye_trig[s * 2 + 1], theta_ray.sin(), "sector {s} dy");
        }
        // Inactive sectors must be zero.
        for s in 0..24 {
            if [0, 6, 12, 18].contains(&s) {
                continue;
            }
            assert_eq!(soa.eye_trig[s * 2], 0.0, "inactive sector {s} dx");
            assert_eq!(soa.eye_trig[s * 2 + 1], 0.0, "inactive sector {s} dy");
        }
    }

    #[test]
    fn eye_trig_len_invariant_after_remove() {
        let mut soa = CreatureSoA::with_capacity(4);
        let mut rng = SimRng::from_u64(1);
        for i in 0..4u64 {
            let g = Genome::founder();
            let b = Brain::founder(&mut rng);
            soa.push(i, i as f32 * 10.0, 0.0, 100.0, 0, 0, 0, g, b);
        }
        assert_eq!(soa.eye_trig.len(), 4 * SECTORS * 2);
        soa.remove_indices(&[1]);
        assert_eq!(soa.eye_trig.len(), 3 * SECTORS * 2);
        assert_eq!(soa.x.len(), 3);
    }

    // ---- perf-5 tests ----

    /// T1: hot mirrors match genomes after a sequence of pushes with mutations.
    #[test]
    fn hot_mirrors_match_genomes_after_births_and_mutations() {
        let mut soa = CreatureSoA::with_capacity(8);
        let mut rng = SimRng::from_u64(7);
        // Push 100 creatures, each with a different mutated genome.
        for k in 0..100 {
            let mut g = Genome::founder();
            g.mutation_rates = TraitMutationRates::uniform(1.0);
            for _ in 0..(k % 5) {
                g.mutate_in_place(&mut rng, 1.0);
            }
            let brain = Brain::founder(&mut rng);
            soa.push(k as u64, 0.0, 0.0, 1.0, 0, 0, 0, g, brain);
        }
        // Invariant check.
        for i in 0..soa.len() {
            let g = &soa.genomes[i];
            assert_eq!(soa.g_size[i], g.size, "size[{i}]");
            assert_eq!(soa.g_graze_eff[i], g.graze_efficiency, "graze_eff[{i}]");
            assert_eq!(soa.g_eat_eff[i], g.eat_efficiency, "eat[{i}]");
            assert_eq!(soa.g_scav_eff[i], g.scavenge_efficiency, "scav[{i}]");
            assert_eq!(soa.g_move_speed[i], g.move_speed, "move[{i}]");
            assert_eq!(soa.g_vision_range[i], g.vision_range, "vision[{i}]");
            assert_eq!(soa.g_eye_count[i], g.eye_count, "eye_count[{i}]");
            assert_eq!(soa.g_nose_count[i], g.nose_count, "nose_count[{i}]");
        }
    }

    /// T2: `remove_indices` keeps mirrors index-aligned with genomes.
    #[test]
    fn remove_indices_keeps_mirrors_in_step() {
        let mut soa = CreatureSoA::with_capacity(16);
        let mut rng = SimRng::from_u64(11);
        for k in 0..10u8 {
            let mut g = Genome::founder();
            g.size = (k as f32) + 1.0;
            g.eye_count = if k % 2 == 0 { 4 } else { 6 };
            soa.push(
                k as u64,
                0.0,
                0.0,
                1.0,
                0,
                0,
                0,
                g,
                Brain::founder(&mut rng),
            );
        }
        soa.remove_indices(&[1, 3, 5, 7]);
        assert_eq!(soa.len(), 6);
        for i in 0..soa.len() {
            assert_eq!(soa.g_size[i], soa.genomes[i].size);
            assert_eq!(soa.g_eye_count[i], soa.genomes[i].eye_count);
            assert_eq!(soa.g_nose_count[i], soa.genomes[i].nose_count);
        }
    }

    /// T3: save round-trip rebuilds hot mirrors bit-identically.
    #[test]
    fn save_round_trip_rebuilds_hot_mirrors() {
        use crate::world::World;
        let mut w = World::new("perf5-save-round-trip");
        for _ in 0..200 {
            w.tick_once();
        }
        let save = w.to_save_v1();
        let w2 = World::from_save_v1(save).expect("load");
        assert_eq!(w.creatures.len(), w2.creatures.len());
        for i in 0..w.creatures.len() {
            assert_eq!(
                w2.creatures.g_size[i], w.creatures.genomes[i].size,
                "g_size[{i}]"
            );
            assert_eq!(
                w2.creatures.g_graze_eff[i], w.creatures.genomes[i].graze_efficiency,
                "g_graze_eff[{i}]"
            );
            assert_eq!(
                w2.creatures.g_eat_eff[i], w.creatures.genomes[i].eat_efficiency,
                "g_eat_eff[{i}]"
            );
            assert_eq!(
                w2.creatures.g_scav_eff[i], w.creatures.genomes[i].scavenge_efficiency,
                "g_scav_eff[{i}]"
            );
            assert_eq!(
                w2.creatures.g_move_speed[i], w.creatures.genomes[i].move_speed,
                "g_move_speed[{i}]"
            );
            assert_eq!(
                w2.creatures.g_vision_range[i], w.creatures.genomes[i].vision_range,
                "g_vision_range[{i}]"
            );
            assert_eq!(
                w2.creatures.g_eye_count[i], w.creatures.genomes[i].eye_count,
                "g_eye_count[{i}]"
            );
            assert_eq!(
                w2.creatures.g_nose_count[i], w.creatures.genomes[i].nose_count,
                "g_nose_count[{i}]"
            );
        }
    }
}
