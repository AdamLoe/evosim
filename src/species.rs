//! Species registry. Full §12 distance metric lands in Milestone E; Milestone B
//! only needs the "founder species" entry so creatures can carry a species_id.

use crate::brain::Brain;
use crate::constants::*;
use crate::genome::Genome;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Species {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub name: String,
    pub anchor_genome: Genome,
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
    next_id: u32,
}

impl SpeciesRegistry {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
            next_id: 0,
        }
    }

    pub fn founder(&mut self, genome: &Genome, brain: &Brain, tick: u32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Species {
            id,
            parent_id: None,
            name: "Lineage A".to_string(),
            anchor_genome: genome.clone(),
            anchor_brain_weights: brain.weights.clone(),
            born_tick: tick,
            died_tick: None,
            child_count: 0,
            depth: 0,
        });
        id
    }

    /// Register a new species split off from `parent_id`. Returns new id.
    pub fn speciate(
        &mut self,
        parent_id: u32,
        anchor_genome: Genome,
        anchor_brain_weights: Vec<f32>,
        tick: u32,
    ) -> u32 {
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
            anchor_genome,
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

/// v5 §12 distance — used by Milestone E. Defined here so the species
/// module owns its own metric. Per-trait σ_t scales from v6 §H.
pub fn species_distance(g1: &Genome, g2: &Genome, nn1: &[f32], nn2: &[f32]) -> f32 {
    let body_sq = trait_body_distance_sq(g1, g2);
    let body = body_sq.sqrt();
    // brain distance: ||w1 - w2|| / sqrt(num_weights)
    debug_assert_eq!(nn1.len(), nn2.len());
    let mut sum_sq = 0.0_f32;
    for i in 0..nn1.len() {
        let d = nn1[i] - nn2[i];
        sum_sq += d * d;
    }
    let brain = (sum_sq / nn1.len() as f32).sqrt();
    SPECIES_W_BODY * body + SPECIES_W_BRAIN * brain
}

pub fn trait_body_distance_sq(g1: &Genome, g2: &Genome) -> f32 {
    let mut s = 0.0_f32;
    macro_rules! term {
        ($field:ident, $sigma:expr) => {{
            let d = (g1.$field as f32 - g2.$field as f32) / ($sigma);
            s += d * d;
        }};
    }
    term!(size, 1.0);
    term!(max_age, 2000.0);
    term!(photosynth_efficiency, 0.3);
    term!(eat_efficiency, 0.3);
    term!(scavenge_efficiency, 0.3);
    term!(move_speed, 1.0);
    term!(vision_range, 20.0);
    term!(armor, 0.2);
    term!(bite_reach, 0.5);
    term!(pigment_r, 0.15);
    term!(pigment_g, 0.15);
    term!(pigment_b, 0.15);
    // categorical jump cost for eye_count
    if g1.eye_count != g2.eye_count {
        s += SPECIES_EYE_JUMP_COST * SPECIES_EYE_JUMP_COST;
    }
    // eye-offset circular distance, only indices used by BOTH
    let n_shared = (g1.eye_count.min(g2.eye_count)) as usize;
    let two_pi = std::f32::consts::TAU;
    let sigma_eo = 0.5_f32;
    for i in 0..n_shared {
        let a = g1.eye_offsets[i];
        let b = g2.eye_offsets[i];
        let raw = (a - b).abs() % two_pi;
        let d = raw.min(two_pi - raw) / sigma_eo;
        s += d * d;
    }
    s
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
        let g = Genome::founder();
        let b = Brain::founder(&mut rng);
        let mut reg = SpeciesRegistry::new();
        let a = reg.founder(&g, &b, 0);
        // first child of A → "A1"
        let a1 = reg.speciate(a, g.clone(), b.weights.clone(), 1);
        assert_eq!(reg.get(a1).name, "Lineage A1");
        // first child of A1 → "A1a"
        let a1a = reg.speciate(a1, g.clone(), b.weights.clone(), 2);
        assert_eq!(reg.get(a1a).name, "Lineage A1a");
        // second child of A → "A2"
        let a2 = reg.speciate(a, g.clone(), b.weights.clone(), 3);
        assert_eq!(reg.get(a2).name, "Lineage A2");
        // first child of A1a → "A1a1"
        let a1a1 = reg.speciate(a1a, g.clone(), b.weights.clone(), 4);
        assert_eq!(reg.get(a1a1).name, "Lineage A1a1");
    }

    #[test]
    fn identical_genomes_have_zero_distance() {
        let mut rng = SimRng::from_u64(0);
        let g = Genome::founder();
        let b = Brain::founder(&mut rng);
        let d = species_distance(&g, &g, &b.weights, &b.weights);
        assert!(d < 1e-6, "d = {d}");
    }
}
