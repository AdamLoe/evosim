//! Species registry. Full §12 distance metric lands in Milestone E; Milestone B
//! only needs the "founder species" entry so creatures can carry a species_id.
//!
//! D3: Genome removed from Species. anchor_genome replaced with just anchor_brain_weights.
//! species_distance now uses brain distance only (no body traits to compare).

use crate::brain::Brain;
use crate::constants::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Species {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub name: String,
    pub anchor_brain_weights: Vec<f32>,
    pub born_tick: u32,
    pub died_tick: Option<u32>,
    /// Counter for naming children-species. v6 §H: alternates letter/number per generation.
    pub child_count: u32,
    /// Depth in the lineage tree (0 = founder). Used to pick naming alphabet.
    pub depth: u32,
}

pub struct SpeciesRegistry {
    pub list: Vec<Species>,
    /// Next species id to allocate. Exposed `pub(crate)` so `snapshot_hash`
    /// can include it in the canonical hash without a visibility violation (S7).
    pub(crate) next_id: u32,
}

impl SpeciesRegistry {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
            next_id: 0,
        }
    }

    pub fn founder(&mut self, brain: &Brain, tick: u32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Species {
            id,
            parent_id: None,
            name: "Lineage A".to_string(),
            anchor_brain_weights: brain.weights.clone(),
            born_tick: tick,
            died_tick: None,
            child_count: 0,
            depth: 0,
        });
        id
    }

    /// Register a new species split off from `parent_id`. Returns new id.
    pub fn speciate(&mut self, parent_id: u32, anchor_brain_weights: Vec<f32>, tick: u32) -> u32 {
        let parent = &mut self.list[parent_id as usize];
        parent.child_count += 1;
        let n = parent.child_count;
        let depth = parent.depth + 1;
        let suffix = lineage_suffix(depth, n);
        let parent_name = parent.name.clone();
        let mut name = format!("{parent_name}{suffix}");
        if name.len() > 16 {
            name.truncate(15);
            name.push('…');
        }
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Species {
            id,
            parent_id: Some(parent_id),
            name,
            anchor_brain_weights,
            born_tick: tick,
            died_tick: None,
            child_count: 0,
            depth,
        });
        id
    }

    pub fn get(&self, id: u32) -> &Species {
        &self.list[id as usize]
    }

    /// Reconstruct a SpeciesRegistry from a persisted list + recomputed next_id.
    /// Called by F.26 `world_from_save_v1`.
    pub fn from_snapshot(list: Vec<Species>, next_id: u32) -> Self {
        Self { list, next_id }
    }
}

/// Lineage suffix rule from v6 §H: depth 1 → numeric ("1","2",…),
/// depth 2 → lowercase letter ("a","b",…), depth 3 → numeric, etc.
fn lineage_suffix(depth: u32, n: u32) -> String {
    if depth % 2 == 1 {
        format!("{n}")
    } else {
        // base-26 lower-letters: 1→a, 2→b, …, 26→z, 27→aa
        let mut s = String::new();
        let mut n = n;
        while n > 0 {
            let d = ((n - 1) % 26) as u8;
            s.insert(0, (b'a' + d) as char);
            n = (n - 1) / 26;
        }
        s
    }
}

/// D3: species_distance uses brain distance only (no body traits).
/// D3: SPECIES_W_BODY removed from constants.rs; only SPECIES_W_BRAIN remains.
pub fn species_distance(nn1: &[f32], nn2: &[f32]) -> f32 {
    debug_assert_eq!(nn1.len(), nn2.len());
    let mut sum_sq = 0.0_f32;
    for i in 0..nn1.len() {
        let d = nn1[i] - nn2[i];
        sum_sq += d * d;
    }
    let brain = (sum_sq / nn1.len() as f32).sqrt();
    SPECIES_W_BRAIN * brain
}

impl Default for SpeciesRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SimRng;

    #[test]
    fn lineage_naming_alternates() {
        let mut rng = SimRng::from_u64(0);
        let b = Brain::founder(&mut rng);
        let mut reg = SpeciesRegistry::new();
        let a = reg.founder(&b, 0);
        // first child of A → "A1"
        let a1 = reg.speciate(a, b.weights.clone(), 1);
        assert_eq!(reg.get(a1).name, "Lineage A1");
        // first child of A1 → "A1a"
        let a1a = reg.speciate(a1, b.weights.clone(), 2);
        assert_eq!(reg.get(a1a).name, "Lineage A1a");
        // second child of A → "A2"
        let a2 = reg.speciate(a, b.weights.clone(), 3);
        assert_eq!(reg.get(a2).name, "Lineage A2");
        // first child of A1a → "A1a1"
        let a1a1 = reg.speciate(a1a, b.weights.clone(), 4);
        assert_eq!(reg.get(a1a1).name, "Lineage A1a1");
    }

    #[test]
    fn identical_brains_have_zero_distance() {
        let mut rng = SimRng::from_u64(0);
        let b = Brain::founder(&mut rng);
        let d = species_distance(&b.weights, &b.weights);
        assert!(d < 1e-6, "d = {d}");
    }
}
