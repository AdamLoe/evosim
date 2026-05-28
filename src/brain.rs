//! Brain — fixed-shape NN: 32 inputs → 48 → 24 (Leaky ReLU) → 5 outputs (v1.5 S5b).
//!
//! 3-matmul pyramid, SIMD forward pass (`wide::f32x8`), geometric-skip mutation
//! per v5 §5.3 / v6 §E. Weight layout: row-major, hidden-unit-contiguous.
//! Leaky ReLU (slope 0.01) in hidden layers — sign-preservation gives smoother
//! mutation fitness than ReLU under no-gradient evolution.

use crate::constants::*;
use crate::profiler::clock_now_us_threadsafe;
use crate::rng::SimRng;
use crate::world::nn::PickTimings;
use serde::{Deserialize, Serialize};
use wide::f32x8;

// Compile-time layout assertions.
const _: () = assert!(NN_INPUTS == 32);
const _: () = assert!(NN_HIDDEN_1 == 48);
const _: () = assert!(NN_HIDDEN_2 == 24);
const _: () = assert!(NN_OUTPUTS == 5);
const _: () = assert!(
    NN_INPUTS.is_multiple_of(8),
    "NN_INPUTS must be multiple of 8 for SIMD chunks"
);
const _: () = assert!(
    NN_HIDDEN_1.is_multiple_of(8),
    "NN_HIDDEN_1 must be multiple of 8 for layer-2 matmul"
);
const _: () = assert!(
    NN_HIDDEN_2.is_multiple_of(8),
    "NN_HIDDEN_2 must be multiple of 8 for output matmul"
);
// NOTE: NN_OUTPUTS (5) does NOT need SIMD alignment — only row stride matters.
const _: () = assert!(
    NN_WEIGHT_COUNT
        == NN_INPUTS * NN_HIDDEN_1 + NN_HIDDEN_1 * NN_HIDDEN_2 + NN_HIDDEN_2 * NN_OUTPUTS
);
const _: () = assert!(
    NN_WEIGHT_COUNT == 2808,
    "v1.5 S5b: NN_WEIGHT_COUNT must be 2808 (32*48 + 48*24 + 24*5)"
);

/// Number of 8-wide SIMD chunks in the input vector (4).
const INPUT_CHUNKS: usize = NN_INPUTS / 8; // 32/8 = 4
/// Number of 8-wide SIMD chunks in hidden_1 (6).
const HIDDEN1_CHUNKS: usize = NN_HIDDEN_1 / 8; // 48/8 = 6
/// Number of 8-wide SIMD chunks in hidden_2 (3).
const HIDDEN2_CHUNKS: usize = NN_HIDDEN_2 / 8; // 24/8 = 3

/// Weight-vec slice offsets for the three layers.
const W1_LEN: usize = NN_INPUTS * NN_HIDDEN_1;
const W2_LEN: usize = NN_HIDDEN_1 * NN_HIDDEN_2;

#[inline]
fn lrelu(x: f32) -> f32 {
    x.max(0.01 * x)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Brain {
    /// NN weight vector. Length = NN_WEIGHT_COUNT = 6648 (v1.5 S5a, 112 inputs intact).
    pub weights: Vec<f32>,
    pub nn_mutation_rate: f32,
}

impl Brain {
    /// Pure uniform-random init with per-layer He ranges (v1.5 S5a).
    pub fn founder(rng: &mut SimRng) -> Self {
        let mut weights = vec![0.0_f32; NN_WEIGHT_COUNT];
        let (w1, rest) = weights.split_at_mut(W1_LEN);
        let (w2, w3) = rest.split_at_mut(W2_LEN);
        for w in w1.iter_mut() {
            *w = rng.uniform(-NN_INIT_RANGE_L1, NN_INIT_RANGE_L1);
        }
        for w in w2.iter_mut() {
            *w = rng.uniform(-NN_INIT_RANGE_L2, NN_INIT_RANGE_L2);
        }
        for w in w3.iter_mut() {
            *w = rng.uniform(-NN_INIT_RANGE_L3, NN_INIT_RANGE_L3);
        }
        Self {
            weights,
            nn_mutation_rate: NN_MUT_RATE_DEFAULT,
        }
    }

    #[cfg(test)]
    pub fn zero() -> Self {
        Self {
            weights: vec![0.0; NN_WEIGHT_COUNT],
            nn_mutation_rate: NN_MUT_RATE_DEFAULT,
        }
    }

    /// SIMD forward pass using `wide::f32x8` — 3-matmul pyramid.
    ///
    /// Weight layout (row-major, hidden-unit-contiguous per layer):
    /// - `weights[0 .. W1_LEN]`: input → hidden_1, row = hidden_1 unit.
    /// - `weights[W1_LEN .. W1_LEN+W2_LEN]`: hidden_1 → hidden_2, row = hidden_2 unit.
    /// - `weights[W1_LEN+W2_LEN ..]`: hidden_2 → output, row = output unit.
    ///
    /// Leaky ReLU after L1 and L2; no activation in L3 (tanh on velocity slots
    /// applied at call site).
    pub fn forward(
        &self,
        input: &[f32; NN_INPUTS],
        output: &mut [f32; NN_OUTPUTS],
        hidden_1: &mut [f32; NN_HIDDEN_1],
        hidden_2: &mut [f32; NN_HIDDEN_2],
        timings: Option<&mut PickTimings>,
    ) {
        let timed = timings.is_some();
        // --- Layer 1: input → hidden_1 (Leaky ReLU) ---
        let l1_start = if timed { clock_now_us_threadsafe() } else { 0 };
        let w1 = &self.weights[..W1_LEN];
        for h in 0..NN_HIDDEN_1 {
            let row = &w1[h * NN_INPUTS..h * NN_INPUTS + NN_INPUTS];
            let mut acc = f32x8::ZERO;
            for c in 0..INPUT_CHUNKS {
                let xv = f32x8::from(&input[c * 8..c * 8 + 8]);
                let wv = f32x8::from(&row[c * 8..c * 8 + 8]);
                acc += xv * wv;
            }
            hidden_1[h] = lrelu(acc.reduce_add());
        }
        let l1_end = if timed { clock_now_us_threadsafe() } else { 0 };

        // --- Layer 2: hidden_1 → hidden_2 (Leaky ReLU) ---
        let w2 = &self.weights[W1_LEN..W1_LEN + W2_LEN];
        for h in 0..NN_HIDDEN_2 {
            let row = &w2[h * NN_HIDDEN_1..h * NN_HIDDEN_1 + NN_HIDDEN_1];
            let mut acc = f32x8::ZERO;
            for c in 0..HIDDEN1_CHUNKS {
                let hv = f32x8::from(&hidden_1[c * 8..c * 8 + 8]);
                let wv = f32x8::from(&row[c * 8..c * 8 + 8]);
                acc += hv * wv;
            }
            hidden_2[h] = lrelu(acc.reduce_add());
        }
        let l2_end = if timed { clock_now_us_threadsafe() } else { 0 };

        // --- Layer 3: hidden_2 → output (tanh on velocity slots only) ---
        let w3 = &self.weights[W1_LEN + W2_LEN..];
        for o in 0..NN_OUTPUTS {
            let row = &w3[o * NN_HIDDEN_2..o * NN_HIDDEN_2 + NN_HIDDEN_2];
            let mut acc = f32x8::ZERO;
            for c in 0..HIDDEN2_CHUNKS {
                let hv = f32x8::from(&hidden_2[c * 8..c * 8 + 8]);
                let wv = f32x8::from(&row[c * 8..c * 8 + 8]);
                acc += hv * wv;
            }
            let sum = acc.reduce_add();
            output[o] = if o < 2 { sum.tanh() } else { sum };
        }
        let l3_end = if timed { clock_now_us_threadsafe() } else { 0 };

        if let Some(t) = timings {
            t.forward_l1_us = t
                .forward_l1_us
                .saturating_add(l1_end.saturating_sub(l1_start));
            t.forward_l2_us = t.forward_l2_us.saturating_add(l2_end.saturating_sub(l1_end));
            t.forward_l3_us = t.forward_l3_us.saturating_add(l3_end.saturating_sub(l2_end));
        }
    }

    /// Scalar (non-SIMD) forward pass — reference implementation for tests.
    #[cfg(test)]
    pub fn forward_scalar(
        &self,
        input: &[f32; NN_INPUTS],
        output: &mut [f32; NN_OUTPUTS],
        hidden_1: &mut [f32; NN_HIDDEN_1],
        hidden_2: &mut [f32; NN_HIDDEN_2],
    ) {
        let w1 = &self.weights[..W1_LEN];
        for h in 0..NN_HIDDEN_1 {
            let mut sum = 0.0f32;
            for i in 0..NN_INPUTS {
                sum += input[i] * w1[h * NN_INPUTS + i];
            }
            hidden_1[h] = lrelu(sum);
        }

        let w2 = &self.weights[W1_LEN..W1_LEN + W2_LEN];
        for h in 0..NN_HIDDEN_2 {
            let mut sum = 0.0f32;
            for j in 0..NN_HIDDEN_1 {
                sum += hidden_1[j] * w2[h * NN_HIDDEN_1 + j];
            }
            hidden_2[h] = lrelu(sum);
        }

        let w3 = &self.weights[W1_LEN + W2_LEN..];
        for o in 0..NN_OUTPUTS {
            let mut sum = 0.0f32;
            for h in 0..NN_HIDDEN_2 {
                sum += hidden_2[h] * w3[o * NN_HIDDEN_2 + h];
            }
            output[o] = if o < 2 { sum.tanh() } else { sum };
        }
    }

    /// Child gets parent weights, then geometric-skip mutation per v6 §E.
    /// `multiplier` is the `mutation_rate_multiplier` slider from v6 §K;
    /// it scales the NN mutation rate just as it scales body mutation rates.
    pub fn child_from(parent: &Brain, rng: &mut SimRng, sigma: f32, multiplier: f32) -> Self {
        let mut child = parent.clone();
        let p = (parent.nn_mutation_rate * multiplier).clamp(0.0, 1.0);
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

    /// v1.5 S5b: pyramid topology weight count = 32*48 + 48*24 + 24*5 = 2808.
    #[test]
    fn nn_weight_count_topology_pyramid() {
        assert_eq!(
            NN_WEIGHT_COUNT, 2808,
            "NN_WEIGHT_COUNT must be 32*48 + 48*24 + 24*5 = 2808"
        );
    }

    /// D9: NN_INPUTS padded to multiple of 8.
    #[test]
    fn nn_inputs_padded_to_multiple_of_8() {
        assert_eq!(
            NN_INPUTS % 8,
            0,
            "NN_INPUTS={NN_INPUTS} must be multiple of 8"
        );
    }

    #[test]
    fn child_mutation_zero_rate_is_identity() {
        let mut rng = SimRng::from_u64(1);
        let mut parent = Brain::founder(&mut rng);
        parent.nn_mutation_rate = 0.0;
        let child = Brain::child_from(&parent, &mut rng, 0.02, 1.0);
        assert_eq!(parent.weights, child.weights);
    }

    #[test]
    fn child_mutation_perturbs_some_weights() {
        let mut rng = SimRng::from_u64(1);
        let mut parent = Brain::founder(&mut rng);
        parent.nn_mutation_rate = 0.1;
        let child = Brain::child_from(&parent, &mut rng, 0.02, 1.0);
        let diffs = parent
            .weights
            .iter()
            .zip(child.weights.iter())
            .filter(|(a, b)| a != b)
            .count();
        // Expected ≈ 0.1 × 2808 ≈ 281. Just confirm "some but not all".
        assert!(diffs > 100 && diffs < 600, "diffs = {diffs}");
    }

    #[test]
    fn child_mutation_multiplier_scales_rate() {
        // With multiplier=0, mutation_rate becomes 0 → no weights change.
        let mut rng = SimRng::from_u64(42);
        let mut parent = Brain::founder(&mut rng);
        parent.nn_mutation_rate = 0.5; // high rate, but multiplier zeroes it
        let child = Brain::child_from(&parent, &mut rng, 0.02, 0.0);
        assert_eq!(parent.weights, child.weights);
    }

    // ---- Forward pass tests ----

    /// Zero weights → all outputs zero; non-zero weights → all outputs finite.
    #[test]
    fn forward_pass_output_shape() {
        let brain = Brain::zero();
        let input = [0.0f32; NN_INPUTS];
        let mut output = [0.0f32; NN_OUTPUTS];
        let mut h1 = [0.0f32; NN_HIDDEN_1];
        let mut h2 = [0.0f32; NN_HIDDEN_2];
        brain.forward(&input, &mut output, &mut h1, &mut h2, None);
        assert_eq!(output.len(), 5);
        assert!(
            output.iter().all(|&v| v == 0.0),
            "zero weights → zero output"
        );

        // Non-zero weights → finite outputs.
        let mut rng = SimRng::from_u64(99);
        let brain2 = Brain::founder(&mut rng);
        let input2 = [1.0f32; NN_INPUTS];
        let mut out2 = [0.0f32; NN_OUTPUTS];
        let mut h1b = [0.0f32; NN_HIDDEN_1];
        let mut h2b = [0.0f32; NN_HIDDEN_2];
        brain2.forward(&input2, &mut out2, &mut h1b, &mut h2b, None);
        assert_eq!(out2.len(), 5);
        assert!(
            out2.iter().all(|&v| v.is_finite()),
            "non-zero weights → finite"
        );
    }

    /// CRITICAL: SIMD and scalar agree within 1e-5 relative error.
    #[test]
    fn forward_pass_matches_scalar_reference() {
        let mut rng = SimRng::from_u64(12345);
        let brain = Brain::founder(&mut rng);

        let mut input = [0.0f32; NN_INPUTS];
        for v in &mut input {
            *v = rng.symm();
        }

        let mut out_simd = [0.0f32; NN_OUTPUTS];
        let mut h1_simd = [0.0f32; NN_HIDDEN_1];
        let mut h2_simd = [0.0f32; NN_HIDDEN_2];
        brain.forward(&input, &mut out_simd, &mut h1_simd, &mut h2_simd, None);

        let mut out_scalar = [0.0f32; NN_OUTPUTS];
        let mut h1_scalar = [0.0f32; NN_HIDDEN_1];
        let mut h2_scalar = [0.0f32; NN_HIDDEN_2];
        brain.forward_scalar(&input, &mut out_scalar, &mut h1_scalar, &mut h2_scalar);

        for o in 0..NN_OUTPUTS {
            let simd = out_simd[o];
            let scalar = out_scalar[o];
            let abs_err = (simd - scalar).abs();
            let ref_mag = scalar.abs().max(1e-6);
            let rel_err = abs_err / ref_mag;
            assert!(
                rel_err < 1e-5,
                "output[{o}]: simd={simd}, scalar={scalar}, rel_err={rel_err:.2e}"
            );
        }

        for h in 0..NN_HIDDEN_1 {
            let simd = h1_simd[h];
            let scalar = h1_scalar[h];
            let abs_err = (simd - scalar).abs();
            let ref_mag = scalar.abs().max(1e-6);
            let rel_err = abs_err / ref_mag;
            assert!(
                rel_err < 1e-5,
                "hidden_1[{h}]: simd={simd}, scalar={scalar}, rel_err={rel_err:.2e}"
            );
        }
        for h in 0..NN_HIDDEN_2 {
            let simd = h2_simd[h];
            let scalar = h2_scalar[h];
            let abs_err = (simd - scalar).abs();
            let ref_mag = scalar.abs().max(1e-6);
            let rel_err = abs_err / ref_mag;
            assert!(
                rel_err < 1e-5,
                "hidden_2[{h}]: simd={simd}, scalar={scalar}, rel_err={rel_err:.2e}"
            );
        }
    }

    /// Leaky ReLU preserves negative pre-activations at 1% slope (vs ReLU's hard zero).
    #[test]
    fn forward_pass_lrelu_preserves_negative_with_small_slope() {
        // Craft hidden_1 unit 0 with strongly negative pre-activation.
        // Row 0 of w1: all -1.0; input = all +1.0 → pre-activation = -NN_INPUTS.
        // After lrelu: 0.01 * -NN_INPUTS.
        let mut brain = Brain::zero();
        for i in 0..NN_INPUTS {
            brain.weights[i] = -1.0;
        }

        let input = [1.0f32; NN_INPUTS];
        let mut output = [0.0f32; NN_OUTPUTS];
        let mut h1 = [0.0f32; NN_HIDDEN_1];
        let mut h2 = [0.0f32; NN_HIDDEN_2];
        brain.forward(&input, &mut output, &mut h1, &mut h2, None);

        let expected = 0.01 * -(NN_INPUTS as f32);
        let err = (h1[0] - expected).abs();
        assert!(
            err < 1e-5,
            "Leaky ReLU must preserve negative pre-activation at slope 0.01: \
             hidden_1[0]={} expected={expected}",
            h1[0]
        );
        // Other hidden_1 units have zero row weights → pre-activation 0 → lrelu(0) = 0.
        for h in 1..NN_HIDDEN_1 {
            assert_eq!(h1[h], 0.0);
        }
    }

    /// No NaN on extreme inputs.
    #[test]
    fn forward_pass_no_nan_on_extreme_inputs() {
        let mut brain = Brain::zero();
        for w in &mut brain.weights {
            *w = 0.3;
        }
        for sign in [1.0f32, -1.0f32] {
            let input = [sign; NN_INPUTS];
            let mut output = [0.0f32; NN_OUTPUTS];
            let mut h1 = [0.0f32; NN_HIDDEN_1];
            let mut h2 = [0.0f32; NN_HIDDEN_2];
            brain.forward(&input, &mut output, &mut h1, &mut h2, None);
            assert!(
                output.iter().all(|v| v.is_finite()),
                "NaN/inf on extreme input (sign={sign})"
            );
        }
    }

    /// Identical inputs + weights → bit-identical outputs (no RNG / global state).
    #[test]
    fn forward_pass_deterministic() {
        let mut rng = SimRng::from_u64(7);
        let brain = Brain::founder(&mut rng);
        let mut input = [0.0f32; NN_INPUTS];
        for v in &mut input {
            *v = rng.symm();
        }

        let mut out1 = [0.0f32; NN_OUTPUTS];
        let mut h1a = [0.0f32; NN_HIDDEN_1];
        let mut h2a = [0.0f32; NN_HIDDEN_2];
        brain.forward(&input, &mut out1, &mut h1a, &mut h2a, None);

        let mut out2 = [0.0f32; NN_OUTPUTS];
        let mut h1b = [0.0f32; NN_HIDDEN_1];
        let mut h2b = [0.0f32; NN_HIDDEN_2];
        brain.forward(&input, &mut out2, &mut h1b, &mut h2b, None);

        assert_eq!(
            out1, out2,
            "forward pass must be bit-identical on same input"
        );
        assert_eq!(h1a, h1b, "hidden_1 must be bit-identical on same input");
        assert_eq!(h2a, h2b, "hidden_2 must be bit-identical on same input");
    }
}
