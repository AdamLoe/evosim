//! Brain — fixed-shape NN: 112 inputs → 24 hidden (ReLU) → 5 outputs (v1.3 D9).
//!
//! Milestone D: SIMD forward pass (`wide::f32x8`) + geometric-skip mutation
//! per v5 §5.3 / v6 §E. Weight layout: row-major, hidden-unit-contiguous
//! (see D.15 spec and DECISIONS).

use crate::constants::*;
use crate::rng::SimRng;
use serde::{Deserialize, Serialize};
use std::hash::Hasher;
use twox_hash::XxHash64;
use wide::f32x8;

// Compile-time layout assertions.
const _: () = assert!(NN_INPUTS == 112);
const _: () = assert!(NN_HIDDEN == 24);
const _: () = assert!(NN_OUTPUTS == 5);
const _: () = assert!(
    NN_INPUTS.is_multiple_of(8),
    "NN_INPUTS must be multiple of 8 for SIMD chunks"
);
const _: () = assert!(
    NN_HIDDEN.is_multiple_of(8),
    "NN_HIDDEN must be multiple of 8 for output matmul"
);
// NOTE: NN_OUTPUTS (5) does NOT need SIMD alignment — only row stride matters.
const _: () = assert!(NN_WEIGHT_COUNT == NN_INPUTS * NN_HIDDEN + NN_HIDDEN * NN_OUTPUTS);
const _: () = assert!(
    NN_WEIGHT_COUNT == 2808,
    "v1.3 D9: NN_WEIGHT_COUNT must be 2808 (112*24 + 24*5)"
);

/// Number of 8-wide SIMD chunks in the input vector (14).
const INPUT_CHUNKS: usize = NN_INPUTS / 8; // 112/8 = 14
/// Number of 8-wide SIMD chunks in the hidden vector (3).
const HIDDEN_CHUNKS: usize = NN_HIDDEN / 8; // 24/8 = 3

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Brain {
    /// NN weight vector. Length = NN_WEIGHT_COUNT = 2808 (v1.3).
    /// WARNING: do NOT mutate weights after construction without recomputing
    /// `weight_hash` and `color_rgb` — only `child_from` and `founder` are
    /// correct write sites (M5).
    pub weights: Vec<f32>,
    pub nn_mutation_rate: f32,
    /// Packed 0x00RRGGBB derived from `weight_hash` low 24 bits. Cached at birth.
    pub color_rgb: u32,
    /// Full XxHash64(weights, seed=0). Cached at birth; color_rgb = low 24 bits.
    pub weight_hash: u64,
}

/// Hash a weight slice via XxHash64 with seed 0.
/// Stable across runs and compiler versions (not DefaultHasher).
fn hash_weights(weights: &[f32]) -> u64 {
    let mut h = XxHash64::with_seed(0);
    // SAFETY: f32 is Plain Old Data; we read the raw bytes for hashing only.
    // No alignment hazard — `as_ptr()` from a Vec<f32> is always 4-byte aligned
    // and `from_raw_parts` with `size_of_val` stays within the allocation.
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            weights.as_ptr() as *const u8,
            std::mem::size_of_val(weights),
        )
    };
    h.write(bytes);
    h.finish()
}

/// Truncate a 64-bit hash to a packed 24-bit RGB color (0x00RRGGBB).
fn color_from_hash(h: u64) -> u32 {
    (h & 0x00FF_FFFF) as u32
}

impl Brain {
    /// Pure uniform-random init (v1.3 D8: F.30 hardwiring deleted).
    pub fn founder(rng: &mut SimRng) -> Self {
        let mut weights = vec![0.0_f32; NN_WEIGHT_COUNT];
        for w in &mut weights {
            *w = rng.uniform(-NN_INIT_RANGE, NN_INIT_RANGE);
        }
        let weight_hash = hash_weights(&weights);
        let color_rgb = color_from_hash(weight_hash);
        Self {
            weights,
            nn_mutation_rate: NN_MUT_RATE_DEFAULT,
            color_rgb,
            weight_hash,
        }
    }

    #[cfg(test)]
    pub fn zero() -> Self {
        Self {
            weights: vec![0.0; NN_WEIGHT_COUNT],
            nn_mutation_rate: NN_MUT_RATE_DEFAULT,
            color_rgb: 0,
            weight_hash: 0,
        }
    }

    /// SIMD forward pass using `wide::f32x8`.
    ///
    /// Weight layout (row-major, v6 §E / D.15 spec):
    /// - `weights[0 .. NN_INPUTS*NN_HIDDEN]`: input→hidden, row = hidden unit.
    ///   `w_ih(h, i) = weights[h * NN_INPUTS + i]`.
    /// - `weights[NN_INPUTS*NN_HIDDEN ..]`: hidden→output, row = output unit.
    ///   `w_ho(o, h) = weights[NN_INPUTS*NN_HIDDEN + o * NN_HIDDEN + h]`.
    ///
    /// No biases (spec weight count matches no-bias exactly).
    /// No activation on outputs (tanh + argmax applied at call site).
    pub fn forward(
        &self,
        input: &[f32; NN_INPUTS],
        output: &mut [f32; NN_OUTPUTS],
        hidden: &mut [f32; NN_HIDDEN],
    ) {
        // --- Layer 1: input → hidden (ReLU) ---
        // 14 SIMD chunks × 24 hidden units.
        let w_ih = &self.weights[..NN_INPUTS * NN_HIDDEN];
        for h in 0..NN_HIDDEN {
            let row = &w_ih[h * NN_INPUTS..h * NN_INPUTS + NN_INPUTS];
            let mut acc = f32x8::ZERO;
            for c in 0..INPUT_CHUNKS {
                let xv = f32x8::from(&input[c * 8..c * 8 + 8]);
                let wv = f32x8::from(&row[c * 8..c * 8 + 8]);
                acc += xv * wv;
            }
            let sum: f32 = acc.reduce_add();
            hidden[h] = sum.max(0.0); // ReLU
        }

        // --- Layer 2: hidden → output (no activation) ---
        // 3 SIMD chunks × 5 output units (per-output dot product approach A).
        let w_ho = &self.weights[NN_INPUTS * NN_HIDDEN..];
        for o in 0..NN_OUTPUTS {
            let row = &w_ho[o * NN_HIDDEN..o * NN_HIDDEN + NN_HIDDEN];
            let mut acc = f32x8::ZERO;
            for c in 0..HIDDEN_CHUNKS {
                let hv = f32x8::from(&hidden[c * 8..c * 8 + 8]);
                let wv = f32x8::from(&row[c * 8..c * 8 + 8]);
                acc += hv * wv;
            }
            output[o] = acc.reduce_add();
        }
    }

    /// Scalar (non-SIMD) forward pass — used as the reference implementation
    /// in tests to verify SIMD correctness within 1e-5 relative error.
    #[cfg(test)]
    pub fn forward_scalar(
        &self,
        input: &[f32; NN_INPUTS],
        output: &mut [f32; NN_OUTPUTS],
        hidden: &mut [f32; NN_HIDDEN],
    ) {
        // Layer 1: input → hidden (ReLU).
        let w_ih = &self.weights[..NN_INPUTS * NN_HIDDEN];
        for h in 0..NN_HIDDEN {
            let mut sum = 0.0f32;
            for i in 0..NN_INPUTS {
                sum += input[i] * w_ih[h * NN_INPUTS + i];
            }
            hidden[h] = sum.max(0.0);
        }

        // Layer 2: hidden → output (no activation).
        let w_ho = &self.weights[NN_INPUTS * NN_HIDDEN..];
        for o in 0..NN_OUTPUTS {
            let mut sum = 0.0f32;
            for h in 0..NN_HIDDEN {
                sum += hidden[h] * w_ho[o * NN_HIDDEN + h];
            }
            output[o] = sum;
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
        // M5: recompute color on FINAL mutated weights (overwrites stale parent fields).
        child.weight_hash = hash_weights(&child.weights);
        child.color_rgb = color_from_hash(child.weight_hash);
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

    /// D9: NN_WEIGHT_COUNT == 2808 post-v1.3.
    #[test]
    fn nn_weight_count_post_v1_3() {
        assert_eq!(
            NN_WEIGHT_COUNT, 2808,
            "NN_WEIGHT_COUNT must be 112×24 + 24×5 = 2808"
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
        assert_eq!(parent.color_rgb, child.color_rgb);
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
        assert!(diffs > 50 && diffs < 600, "diffs = {diffs}");
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

    // ---- D.15 forward pass tests ----

    /// D.15 test 1: zero weights → all outputs zero; non-zero weights → all outputs finite.
    #[test]
    fn forward_pass_output_shape() {
        let brain = Brain::zero();
        let input = [0.0f32; NN_INPUTS];
        let mut output = [0.0f32; NN_OUTPUTS];
        let mut hidden = [0.0f32; NN_HIDDEN];
        brain.forward(&input, &mut output, &mut hidden);
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
        let mut hid2 = [0.0f32; NN_HIDDEN];
        brain2.forward(&input2, &mut out2, &mut hid2);
        assert_eq!(out2.len(), 5);
        assert!(
            out2.iter().all(|&v| v.is_finite()),
            "non-zero weights → finite"
        );
    }

    /// D.15 test 2 (CRITICAL): SIMD and scalar agree within 1e-5 relative error.
    #[test]
    fn forward_pass_matches_scalar_reference() {
        let mut rng = SimRng::from_u64(12345);
        let brain = Brain::founder(&mut rng);

        // Random input in [-1, 1].
        let mut input = [0.0f32; NN_INPUTS];
        for v in &mut input {
            *v = rng.symm();
        }

        let mut out_simd = [0.0f32; NN_OUTPUTS];
        let mut hid_simd = [0.0f32; NN_HIDDEN];
        brain.forward(&input, &mut out_simd, &mut hid_simd);

        let mut out_scalar = [0.0f32; NN_OUTPUTS];
        let mut hid_scalar = [0.0f32; NN_HIDDEN];
        brain.forward_scalar(&input, &mut out_scalar, &mut hid_scalar);

        for o in 0..NN_OUTPUTS {
            let simd = out_simd[o];
            let scalar = out_scalar[o];
            let abs_err = (simd - scalar).abs();
            // Use relative error; fall back to absolute if scalar is near zero.
            let ref_mag = scalar.abs().max(1e-6);
            let rel_err = abs_err / ref_mag;
            assert!(
                rel_err < 1e-5,
                "output[{o}]: simd={simd}, scalar={scalar}, rel_err={rel_err:.2e}"
            );
        }

        // Also check hidden layer agreement.
        for h in 0..NN_HIDDEN {
            let simd = hid_simd[h];
            let scalar = hid_scalar[h];
            let abs_err = (simd - scalar).abs();
            let ref_mag = scalar.abs().max(1e-6);
            let rel_err = abs_err / ref_mag;
            assert!(
                rel_err < 1e-5,
                "hidden[{h}]: simd={simd}, scalar={scalar}, rel_err={rel_err:.2e}"
            );
        }
    }

    /// D.15 test 3: ReLU clips negative hidden pre-activations → zero contribution.
    #[test]
    fn forward_pass_relu_clips_negative_hidden() {
        // Craft a brain so hidden unit 0 has strongly negative pre-activation.
        // All input→hidden row 0 weights = -1.0; input = all +1.0.
        // Pre-activation = -1 * 160 = -160 → after ReLU = 0.
        // Then make all hidden→output weights for unit 0 = +100 (large).
        // If ReLU is correct, the large weights shouldn't affect output.
        // Compare against a brain with hidden→output row 0 weights = 0.
        let mut brain_a = Brain::zero();
        for i in 0..NN_INPUTS {
            brain_a.weights[i] = -1.0; // row 0 of w_ih: all -1
        }
        // Set hidden→output weights for hidden unit 0 to +100 in brain_a.
        let ho_offset = NN_INPUTS * NN_HIDDEN;
        for o in 0..NN_OUTPUTS {
            brain_a.weights[ho_offset + o * NN_HIDDEN] = 100.0;
        }

        let brain_b = Brain::zero(); // all zeros → output zero regardless

        let input = [1.0f32; NN_INPUTS];
        let mut out_a = [0.0f32; NN_OUTPUTS];
        let mut hid_a = [0.0f32; NN_HIDDEN];
        brain_a.forward(&input, &mut out_a, &mut hid_a);

        let mut out_b = [0.0f32; NN_OUTPUTS];
        let mut hid_b = [0.0f32; NN_HIDDEN];
        brain_b.forward(&input, &mut out_b, &mut hid_b);

        // hidden[0] in brain_a should be 0 (ReLU of -160).
        assert_eq!(hid_a[0], 0.0, "ReLU must clip negative pre-activation to 0");
        // Since hidden[0] = 0, the large w_ho weights don't contribute.
        assert_eq!(out_a, out_b, "clipped hidden unit must not affect outputs");
    }

    /// D.15 test 4: no NaN on extreme inputs (all ±1, weights all 0.3).
    #[test]
    fn forward_pass_no_nan_on_extreme_inputs() {
        let mut brain = Brain::zero();
        for w in &mut brain.weights {
            *w = 0.3;
        }
        for sign in [1.0f32, -1.0f32] {
            let input = [sign; NN_INPUTS];
            let mut output = [0.0f32; NN_OUTPUTS];
            let mut hidden = [0.0f32; NN_HIDDEN];
            brain.forward(&input, &mut output, &mut hidden);
            assert!(
                output.iter().all(|v| v.is_finite()),
                "NaN/inf on extreme input (sign={sign})"
            );
        }
    }

    // ---- M5 color-hash tests ----

    /// M5: identical weight vectors produce identical color_rgb.
    #[test]
    fn identical_weights_produce_identical_color() {
        let mut rng = SimRng::from_u64(42);
        let b1 = Brain::founder(&mut rng);
        let mut b2 = b1.clone();
        // Recompute on same weights — should match.
        b2.weight_hash = hash_weights(&b2.weights);
        b2.color_rgb = color_from_hash(b2.weight_hash);
        assert_eq!(b1.color_rgb, b2.color_rgb);
        assert_eq!(b1.weight_hash, b2.weight_hash);
    }

    /// M5: a mutated child has a different color_rgb from its parent.
    #[test]
    fn mutated_child_changes_color() {
        let mut rng = SimRng::from_u64(7);
        let mut parent = Brain::founder(&mut rng);
        parent.nn_mutation_rate = 0.5; // high rate — almost certain to mutate
        let child = Brain::child_from(&parent, &mut rng, 0.1, 1.0);
        // With rate=0.5 and sigma=0.1, at least some weights will change → color differs.
        // (If by chance the hash collides, this would fail — but P(collision) ≈ 1/16M.)
        assert_ne!(
            parent.color_rgb, child.color_rgb,
            "mutated child must have different color_rgb"
        );
    }

    /// M5: zero-mutation child keeps parent color because weights are unchanged.
    #[test]
    fn zero_mutation_child_keeps_parent_color() {
        let mut rng = SimRng::from_u64(3);
        let mut parent = Brain::founder(&mut rng);
        parent.nn_mutation_rate = 0.0;
        let child = Brain::child_from(&parent, &mut rng, 0.02, 1.0);
        assert_eq!(
            parent.color_rgb, child.color_rgb,
            "zero-mutation child must keep parent color_rgb"
        );
        assert_eq!(parent.weight_hash, child.weight_hash);
    }

    /// M5: color_rgb always fits in 24 bits (high byte is zero).
    #[test]
    fn color_packs_into_24_bits() {
        let mut rng = SimRng::from_u64(99);
        for _ in 0..20 {
            let b = Brain::founder(&mut rng);
            assert_eq!(
                b.color_rgb & 0xFF00_0000,
                0,
                "color_rgb high byte must be zero (24-bit only)"
            );
        }
    }

    /// D.15 test 5: identical inputs + weights → bit-identical outputs (no RNG / global state).
    #[test]
    fn forward_pass_deterministic() {
        let mut rng = SimRng::from_u64(7);
        let brain = Brain::founder(&mut rng);
        let mut input = [0.0f32; NN_INPUTS];
        for v in &mut input {
            *v = rng.symm();
        }

        let mut out1 = [0.0f32; NN_OUTPUTS];
        let mut hid1 = [0.0f32; NN_HIDDEN];
        brain.forward(&input, &mut out1, &mut hid1);

        let mut out2 = [0.0f32; NN_OUTPUTS];
        let mut hid2 = [0.0f32; NN_HIDDEN];
        brain.forward(&input, &mut out2, &mut hid2);

        assert_eq!(
            out1, out2,
            "forward pass must be bit-identical on same input"
        );
        assert_eq!(
            hid1, hid2,
            "hidden layer must be bit-identical on same input"
        );
    }
}
