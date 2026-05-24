//! Brain — fixed-shape NN.
//!
//! Milestone B: holds zero-weight stubs so we can carry a Brain field on
//! Creature and not retrofit everything later. Milestone D installs SIMD
//! forward pass and geometric-skip mutation per v5 §5.3.

use crate::constants::*;
use crate::rng::SimRng;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Brain {
    pub weights: Vec<f32>, // length = NN_WEIGHT_COUNT
    pub nn_mutation_rate: f32,
}

impl Brain {
    pub fn founder(rng: &mut SimRng) -> Self {
        let mut weights = vec![0.0_f32; NN_WEIGHT_COUNT];
        for w in &mut weights {
            *w = rng.uniform(-NN_INIT_RANGE, NN_INIT_RANGE);
        }
        Self {
            weights,
            nn_mutation_rate: NN_MUT_RATE_DEFAULT,
        }
    }

    pub fn zero() -> Self {
        Self {
            weights: vec![0.0; NN_WEIGHT_COUNT],
            nn_mutation_rate: NN_MUT_RATE_DEFAULT,
        }
    }

    /// Child gets parent weights, then geometric-skip mutation per v6 §E.
    pub fn child_from(parent: &Brain, rng: &mut SimRng, sigma: f32) -> Self {
        let mut child = parent.clone();
        let p = parent.nn_mutation_rate.clamp(0.0, 1.0);
        if p > 0.0 && sigma > 0.0 {
            let n = child.weights.len();
            let mut i = rng.geom_skip(p);
            while i < n {
                child.weights[i] += rng.normal() * sigma;
                let step = rng.geom_skip(p);
                i = i.saturating_add(step).saturating_add(1);
            }
        }
        // mutation-rate drift: 0.5%/birth, ±20% jitter
        if rng.unit() < MUT_RATE_OF_RATES {
            let jitter = 1.0 + rng.symm() * MUT_RATE_JITTER;
            child.nn_mutation_rate = (child.nn_mutation_rate * jitter).clamp(0.0, 1.0);
        }
        child
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn founder_weight_layout_correct() {
        let mut rng = SimRng::from_u64(1);
        let b = Brain::founder(&mut rng);
        assert_eq!(b.weights.len(), NN_WEIGHT_COUNT);
        assert!(b.weights.iter().all(|w| w.is_finite()));
        assert!(b.weights.iter().any(|w| *w != 0.0));
    }

    #[test]
    fn child_mutation_zero_rate_is_identity() {
        let mut rng = SimRng::from_u64(1);
        let mut parent = Brain::founder(&mut rng);
        parent.nn_mutation_rate = 0.0;
        let child = Brain::child_from(&parent, &mut rng, 0.02);
        assert_eq!(parent.weights, child.weights);
    }

    #[test]
    fn child_mutation_perturbs_some_weights() {
        let mut rng = SimRng::from_u64(1);
        let mut parent = Brain::founder(&mut rng);
        parent.nn_mutation_rate = 0.1;
        let child = Brain::child_from(&parent, &mut rng, 0.02);
        let diffs = parent
            .weights
            .iter()
            .zip(child.weights.iter())
            .filter(|(a, b)| a != b)
            .count();
        // Expected ≈ 0.1 × 3360 ≈ 336. Just confirm "some".
        assert!(diffs > 100 && diffs < 800, "diffs = {diffs}");
    }
}
