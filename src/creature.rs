//! Creature SoA. Layout shared by the live (mutable) tick state and the
//! double-buffered snapshot used by the persistence worker in F.26.
//!
//! Genome and Brain live in `genome.rs` / `brain.rs` and are stored as
//! per-creature `Vec` entries (AoS-within-SoA — fine because we touch the
//! whole struct on actions like split / mutation, not on per-trait inner
//! loops, except for the hot ones lifted out into the SoA arrays below).

use crate::brain::Brain;
use crate::genome::Genome;
use serde::{Deserialize, Serialize};

/// One discrete action a creature chose this tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Action {
    Rest = 0,
    Photosynth = 1,
    Eat = 2,
    Scavenge = 3,
    Split = 4,
    Signal = 5,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::Rest,
        Action::Photosynth,
        Action::Eat,
        Action::Scavenge,
        Action::Split,
        Action::Signal,
    ];

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
        }
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }
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
        self.last_action.push(Action::Rest);
        self.action_this_tick.push(Action::Rest);
        self.max_size_reached.push(genome.size);
        self.distance_travelled.push(0.0);
        self.birth_tick.push(birth_tick);
        self.genomes.push(genome);
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
            self.species_id.swap_remove(k);
            self.parent_species_id.swap_remove(k);
            self.last_action.swap_remove(k);
            self.action_this_tick.swap_remove(k);
            self.max_size_reached.swap_remove(k);
            self.distance_travelled.swap_remove(k);
            self.birth_tick.swap_remove(k);
            self.genomes.swap_remove(k);
            self.brains.swap_remove(k);
        }
    }
}
