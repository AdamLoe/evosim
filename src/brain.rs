//! Brain — runtime-shaped NN (v1.12 / v2.0).
//!
//! Runtime input width (a multiple of 8 in `[8, MAX_NN_INPUTS]`, default
//! `NN_INPUTS = 32`) + 5-output (vx, vy, action logits 0..2). The input width
//! is a field of `NnTopology`, mirroring how hidden layers are runtime-sized:
//! the first matmul takes it as `fan_in`. The semantic layout that produces a
//! given width lives in `world/nn.rs::build_nn_input`.
//! Hidden layers are configurable: 1..=NN_MAX_HIDDEN_LAYERS layers, each
//! width a multiple of 8 in [8, NN_MAX_HIDDEN_WIDTH], with a per-layer
//! activation drawn from `Activation`.
//!
//! Forward pass: SIMD matmul via `wide::f32x8`. Hidden→hidden matmuls run
//! the 4× output-tile kernel `matmul_tiled` (every hidden width is a
//! multiple of 4 because it's a multiple of 8). The last matmul into the
//! 5-output layer keeps the dedicated `matmul_output` kernel (NN_OUTPUTS
//! isn't a multiple of 4). Output projection — tanh on slots 0..2 — is a
//! fixed post-step that fires regardless of the user-chosen activation on
//! the last hidden layer.
//!
//! Mutation: geometric-skip Gaussian step driven by `MutationPolicy`'s
//! 8 buckets (v1.12 buckets editor in the NN rail tab). See `child_from`.

use crate::constants::*;
use crate::profiler::clock_now_us_threadsafe;
use crate::rng::SimRng;
use crate::world::nn::PickTimings;
use serde::{Deserialize, Serialize};
use wide::f32x8;

// v2.0 Wave 0 runtime input-width tests live in their own file/module to avoid
// the shared `mod tests` merge hazard.
#[cfg(test)]
#[path = "brain_width_tests.rs"]
mod brain_width_tests;

// Compile-time invariants that can't change at runtime. The active *input
// width* is validated at runtime in `NnTopology::new` (`input_width <=
// MAX_NN_INPUTS` and `input_width % 8 == 0`), since it is now a runtime field;
// only the legacy/default value and the output count are pinned here.
const _: () = assert!(NN_OUTPUTS == 5);
const _: () = assert!(
    NN_INPUTS.is_multiple_of(8),
    "NN_INPUTS (legacy default width) must be multiple of 8 for SIMD chunks"
);
const _: () = assert!(
    MAX_NN_INPUTS.is_multiple_of(8),
    "MAX_NN_INPUTS must be multiple of 8 for SIMD chunks"
);

/// Per-layer activation. `Linear` is identity; the others are pointwise.
/// The output layer ignores its `activations[..]` entry — it always runs the
/// fixed output projection (tanh on slots 0..2) instead.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Activation {
    LReLU,
    ReLU,
    Tanh,
    Sigmoid,
    Linear,
}

impl Activation {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "lrelu" => Activation::LReLU,
            "relu" => Activation::ReLU,
            "tanh" => Activation::Tanh,
            "sigmoid" => Activation::Sigmoid,
            "linear" => Activation::Linear,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Activation::LReLU => "lrelu",
            Activation::ReLU => "relu",
            Activation::Tanh => "tanh",
            Activation::Sigmoid => "sigmoid",
            Activation::Linear => "linear",
        }
    }
}

#[inline]
fn lrelu(x: f32) -> f32 {
    x.max(0.01 * x)
}

#[inline]
fn apply_activation(act: Activation, out: &mut [f32]) {
    match act {
        Activation::LReLU => {
            for v in out.iter_mut() {
                *v = lrelu(*v);
            }
        }
        Activation::ReLU => {
            for v in out.iter_mut() {
                *v = v.max(0.0);
            }
        }
        Activation::Tanh => {
            for v in out.iter_mut() {
                *v = v.tanh();
            }
        }
        Activation::Sigmoid => {
            for v in out.iter_mut() {
                *v = 1.0 / (1.0 + (-*v).exp());
            }
        }
        Activation::Linear => {}
    }
}

/// Validated topology: runtime input width + hidden-layer list.
///
/// `input_width` is the active NN input count — a multiple of 8 in
/// `[8, MAX_NN_INPUTS]`, default `NN_INPUTS = 32`. It feeds the first matmul
/// as `fan_in`. Outputs (`NN_OUTPUTS`) are implicit. Every entry in
/// `hidden_sizes` must be a multiple of 8 in [8, NN_MAX_HIDDEN_WIDTH], and
/// `hidden_sizes.len()` must be in [1, NN_MAX_HIDDEN_LAYERS].
/// `activations.len() == hidden_sizes.len()` — exactly one activation per
/// hidden layer. The output matmul is always `Linear`, followed by the fixed
/// output projection (tanh on slots 0..2); neither is user-configurable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NnTopology {
    /// Runtime input width. Defaults to `NN_INPUTS` when deserialized from a
    /// payload that predates the field (serde `default`), preserving legacy
    /// 32-input topology JSON.
    #[serde(default = "default_input_width")]
    input_width: usize,
    hidden_sizes: Vec<usize>,
    activations: Vec<Activation>,
}

fn default_input_width() -> usize {
    NN_INPUTS
}

impl NnTopology {
    /// Construct with the legacy/default input width (`NN_INPUTS = 32`) after
    /// validating. See `with_input_width` for a non-default width.
    pub fn new(hidden_sizes: Vec<usize>, activations: Vec<Activation>) -> Result<Self, String> {
        Self::with_input_width(NN_INPUTS, hidden_sizes, activations)
    }

    /// Construct after validating, with an explicit runtime input width.
    /// Returns `Err` describing the first rule violation; otherwise the
    /// topology is safe for `Brain::founder` and the SIMD matmul invariants.
    pub fn with_input_width(
        input_width: usize,
        hidden_sizes: Vec<usize>,
        activations: Vec<Activation>,
    ) -> Result<Self, String> {
        if !(8..=MAX_NN_INPUTS).contains(&input_width) {
            return Err(format!("input_width={input_width} out of [8, {MAX_NN_INPUTS}]"));
        }
        if !input_width.is_multiple_of(8) {
            return Err(format!(
                "input_width={input_width} not multiple of 8 (SIMD chunks)"
            ));
        }
        if hidden_sizes.is_empty() {
            return Err("need at least one hidden layer".into());
        }
        if hidden_sizes.len() > NN_MAX_HIDDEN_LAYERS {
            return Err(format!(
                "too many hidden layers: {} > {}",
                hidden_sizes.len(),
                NN_MAX_HIDDEN_LAYERS
            ));
        }
        if activations.len() != hidden_sizes.len() {
            return Err(format!(
                "activations.len() ({}) must equal hidden_sizes.len() ({}) — one per hidden layer",
                activations.len(),
                hidden_sizes.len()
            ));
        }
        for (idx, &w) in hidden_sizes.iter().enumerate() {
            if w < 8 || w > NN_MAX_HIDDEN_WIDTH {
                return Err(format!(
                    "hidden_sizes[{idx}]={w} out of [8, {NN_MAX_HIDDEN_WIDTH}]"
                ));
            }
            if !w.is_multiple_of(8) {
                return Err(format!("hidden_sizes[{idx}]={w} not multiple of 8"));
            }
        }
        Ok(Self {
            input_width,
            hidden_sizes,
            activations,
        })
    }

    /// Legacy 32→48→24→5 default. Used when the boot payload omits
    /// `nn_topology`.
    pub fn legacy() -> Self {
        Self::new(
            LEGACY_HIDDEN_SIZES.to_vec(),
            vec![Activation::LReLU, Activation::LReLU],
        )
        .expect("LEGACY_HIDDEN_SIZES + 2 lrelu must be a valid topology")
    }

    /// Active runtime input width (first-matmul `fan_in`).
    pub fn input_width(&self) -> usize {
        self.input_width
    }

    pub fn hidden_sizes(&self) -> &[usize] {
        &self.hidden_sizes
    }
    pub fn activations(&self) -> &[Activation] {
        &self.activations
    }
    pub fn hidden_count(&self) -> usize {
        self.hidden_sizes.len()
    }
    /// `hidden_count + 1` — total number of matmuls in a forward pass.
    pub fn matmul_count(&self) -> usize {
        self.hidden_sizes.len() + 1
    }
    /// Total weight count = Σ (fan_in × fan_out) for each matmul. The first
    /// matmul's `fan_in` is the runtime `input_width`.
    pub fn weight_count(&self) -> usize {
        let mut sum = 0;
        let mut fan_in = self.input_width;
        for &w in &self.hidden_sizes {
            sum += fan_in * w;
            fan_in = w;
        }
        sum + fan_in * NN_OUTPUTS
    }
    /// Largest layer width across hidden + output. Used to size scratch.
    pub fn max_width(&self) -> usize {
        let mut m = NN_OUTPUTS;
        for &w in &self.hidden_sizes {
            if w > m {
                m = w;
            }
        }
        m
    }
}

/// 4× output-tile SIMD matmul. `fan_in` MUST be multiple of 8; `fan_out`
/// MUST be multiple of 4. Used for hidden→hidden steps where both widths
/// are multiples of 8 (which implies multiple of 4). The output layer takes
/// the small dedicated kernel below because `NN_OUTPUTS=5` isn't 4-aligned.
#[inline(always)]
fn matmul_tiled(weights: &[f32], input: &[f32], output: &mut [f32], fan_in: usize, fan_out: usize) {
    debug_assert!(fan_in.is_multiple_of(8));
    debug_assert!(fan_out.is_multiple_of(4));
    debug_assert!(input.len() >= fan_in);
    debug_assert!(output.len() >= fan_out);
    debug_assert!(weights.len() >= fan_in * fan_out);
    let input_chunks = fan_in / 8;
    let mut h = 0;
    while h < fan_out {
        let r0 = h * fan_in;
        let r1 = r0 + fan_in;
        let r2 = r1 + fan_in;
        let r3 = r2 + fan_in;
        let row0 = &weights[r0..r1];
        let row1 = &weights[r1..r2];
        let row2 = &weights[r2..r3];
        let row3 = &weights[r3..r3 + fan_in];
        let mut a0 = f32x8::ZERO;
        let mut a1 = f32x8::ZERO;
        let mut a2 = f32x8::ZERO;
        let mut a3 = f32x8::ZERO;
        for c in 0..input_chunks {
            let off = c * 8;
            let xv = f32x8::from(&input[off..off + 8]);
            a0 += xv * f32x8::from(&row0[off..off + 8]);
            a1 += xv * f32x8::from(&row1[off..off + 8]);
            a2 += xv * f32x8::from(&row2[off..off + 8]);
            a3 += xv * f32x8::from(&row3[off..off + 8]);
        }
        output[h] = a0.reduce_add();
        output[h + 1] = a1.reduce_add();
        output[h + 2] = a2.reduce_add();
        output[h + 3] = a3.reduce_add();
        h += 4;
    }
}

/// Output-layer kernel. NN_OUTPUTS=5 hard-coded; `fan_in` is dynamic but
/// MUST be multiple of 8. Each output row reduces a SIMD-vectorised dot
/// product. Output projection (tanh on slots 0..2) lives in the caller, not
/// here, so the kernel stays a pure linear pass.
#[inline(always)]
fn matmul_output(weights: &[f32], input: &[f32], output: &mut [f32; NN_OUTPUTS], fan_in: usize) {
    debug_assert!(fan_in.is_multiple_of(8));
    debug_assert!(input.len() >= fan_in);
    debug_assert!(weights.len() >= fan_in * NN_OUTPUTS);
    let input_chunks = fan_in / 8;
    for o in 0..NN_OUTPUTS {
        let row = &weights[o * fan_in..o * fan_in + fan_in];
        let mut acc = f32x8::ZERO;
        for c in 0..input_chunks {
            let off = c * 8;
            acc += f32x8::from(&input[off..off + 8]) * f32x8::from(&row[off..off + 8]);
        }
        output[o] = acc.reduce_add();
    }
}

/// One mutation bucket. Per-birth, the parent's `MutationPolicy` samples one
/// bucket by `weight`; that bucket's `(rate, sigma)` then drive the geometric
/// -skip Gaussian step. `weight` is any non-negative float — it's normalised
/// at draw time. All-zero weights mean "no mutation at all" — child is a
/// bit-identical clone (v1.12 freeze-evolution behaviour).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Bucket {
    pub weight: f32,
    pub rate: f32,
    pub sigma: f32,
}

impl Bucket {
    pub const fn zero() -> Self {
        Self {
            weight: 0.0,
            rate: 0.0,
            sigma: 0.0,
        }
    }
}

/// 8 fixed mutation buckets. Default = three equal-weight buckets covering
/// no-mutation, small/medium drift, and big perturbation (1/3 each); buckets
/// 3..=7 are zero-weight reserves for user tuning.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutationPolicy {
    pub buckets: [Bucket; MUTATION_BUCKET_COUNT],
}

impl Default for MutationPolicy {
    fn default() -> Self {
        let mut buckets = [Bucket::zero(); MUTATION_BUCKET_COUNT];
        buckets[0] = Bucket { weight: 1.0, rate: 0.0, sigma: 0.0 };
        buckets[1] = Bucket { weight: 1.0, rate: 0.05, sigma: 0.05 };
        buckets[2] = Bucket { weight: 1.0, rate: 0.30, sigma: 0.20 };
        Self { buckets }
    }
}

/// v1.12: runtime-shaped NN weights + topology.
///
/// `weights` is one contiguous row-major SoA, hidden-unit-contiguous per
/// matmul: weights for matmul k of size `fan_in × fan_out` live in
/// `weights[off..off + fan_in*fan_out]` where `off` is the cumulative
/// previous size. `topology` carries the layer-size list so a child
/// inherits the parent's shape verbatim.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Brain {
    pub weights: Vec<f32>,
    pub topology: NnTopology,
}

impl Brain {
    /// Per-layer He-uniform init: `r = sqrt(6 / fan_in)`. Replaces the
    /// hard-coded `NN_INIT_RANGE_L1/L2/L3` constants — those are now
    /// computed inline from each matmul's fan_in.
    pub fn founder(rng: &mut SimRng, topology: NnTopology) -> Self {
        let total = topology.weight_count();
        let mut weights = vec![0.0f32; total];
        let mut off = 0;
        let mut fan_in = topology.input_width();
        for &fan_out in topology
            .hidden_sizes()
            .iter()
            .chain(std::iter::once(&NN_OUTPUTS))
        {
            let r = (6.0_f32 / fan_in as f32).sqrt();
            for w in weights[off..off + fan_in * fan_out].iter_mut() {
                *w = rng.uniform(-r, r);
            }
            off += fan_in * fan_out;
            fan_in = fan_out;
        }
        Self { weights, topology }
    }

    #[cfg(test)]
    pub fn zero_with(topology: NnTopology) -> Self {
        let total = topology.weight_count();
        Self {
            weights: vec![0.0; total],
            topology,
        }
    }

    #[cfg(test)]
    pub fn zero() -> Self {
        Self::zero_with(NnTopology::legacy())
    }

    /// SIMD forward pass. Layer loop alternates between two scratch buffers
    /// (`scratch_a`/`scratch_b`) supplied by the caller; both must hold at
    /// least `topology.max_width()` floats. Output projection (tanh on
    /// slots 0..2) fires after the final matmul.
    ///
    /// `layer_timings` is an optional per-matmul `(start, end)` accumulator
    /// pair — the caller writes per-layer wall clock there for the
    /// `nn.forward.l{k}` perf rows. Length must equal `topology.matmul_count()`
    /// when present.
    pub fn forward(
        &self,
        input: &[f32; MAX_NN_INPUTS],
        output: &mut [f32; NN_OUTPUTS],
        scratch_a: &mut [f32],
        scratch_b: &mut [f32],
        timings: Option<&mut PickTimings>,
    ) {
        debug_assert!(scratch_a.len() >= self.topology.max_width());
        debug_assert!(scratch_b.len() >= self.topology.max_width());
        let timed = timings.is_some();
        let hidden_sizes = self.topology.hidden_sizes();
        let activations = self.topology.activations();
        let matmul_count = self.topology.matmul_count();

        // Per-matmul timings collected as elapsed µs; folded into PickTimings
        // at the end.
        let mut layer_us = [0u64; NN_MAX_MATMULS];

        // Ping-pong buffers. The first matmul reads from `input` (only the
        // active `input_width` lanes), every subsequent matmul reads from the
        // previous output buffer. We start with output going to `scratch_a`,
        // then alternate.
        let mut weight_off = 0usize;
        let mut fan_in = self.topology.input_width();
        debug_assert!(input.len() >= fan_in);

        // ─── Hidden matmuls: input → h1, h1 → h2, ... ──────────────────
        let hidden_count = hidden_sizes.len();
        // Each iteration produces hidden layer k's outputs into `cur`.
        for k in 0..hidden_count {
            let fan_out = hidden_sizes[k];
            let t_start = if timed { clock_now_us_threadsafe() } else { 0 };
            let weights = &self.weights[weight_off..weight_off + fan_in * fan_out];
            // First matmul reads input; subsequent reads from whichever
            // scratch we last wrote.
            if k == 0 {
                matmul_tiled(
                    weights,
                    &input[..fan_in],
                    &mut scratch_a[..fan_out],
                    fan_in,
                    fan_out,
                );
                apply_activation(activations[k], &mut scratch_a[..fan_out]);
            } else if k % 2 == 1 {
                // Read from scratch_a, write to scratch_b.
                let (a, b) = (&*scratch_a, &mut *scratch_b);
                matmul_tiled(weights, &a[..fan_in], &mut b[..fan_out], fan_in, fan_out);
                apply_activation(activations[k], &mut scratch_b[..fan_out]);
            } else {
                // Read from scratch_b, write to scratch_a.
                let (b, a) = (&*scratch_b, &mut *scratch_a);
                matmul_tiled(weights, &b[..fan_in], &mut a[..fan_out], fan_in, fan_out);
                apply_activation(activations[k], &mut scratch_a[..fan_out]);
            }
            let t_end = if timed { clock_now_us_threadsafe() } else { 0 };
            layer_us[k] = t_end.saturating_sub(t_start);
            weight_off += fan_in * fan_out;
            fan_in = fan_out;
        }

        // ─── Output matmul: last_hidden → output. Output projection
        //     (tanh on slots 0..2) overrides the user's activation choice.
        let t_start = if timed { clock_now_us_threadsafe() } else { 0 };
        let last_hidden_buf: &[f32] = if (hidden_count - 1) % 2 == 0 {
            &scratch_a[..fan_in]
        } else {
            &scratch_b[..fan_in]
        };
        let weights = &self.weights[weight_off..weight_off + fan_in * NN_OUTPUTS];
        matmul_output(weights, last_hidden_buf, output, fan_in);
        // Output projection: tanh on velocity slots only. Action logits
        // (slots 2..5) stay raw.
        output[0] = output[0].tanh();
        output[1] = output[1].tanh();
        let t_end = if timed { clock_now_us_threadsafe() } else { 0 };
        layer_us[hidden_count] = t_end.saturating_sub(t_start);

        // Fold per-matmul wall clock into PickTimings using 1-based row
        // names so the panel's `nn.forward.l1/l2/l3` rows render identically
        // for the legacy 32→48→24→5 topology.
        if let Some(t) = timings {
            for k in 0..matmul_count {
                t.forward_layer_us[k] = t.forward_layer_us[k].saturating_add(layer_us[k]);
            }
        }
    }

    /// Scalar (non-SIMD) forward pass — reference implementation for tests.
    /// Mirrors `forward` semantics: per-layer activation + fixed output
    /// projection (tanh on slots 0..2).
    #[cfg(test)]
    pub fn forward_scalar(
        &self,
        input: &[f32; MAX_NN_INPUTS],
        output: &mut [f32; NN_OUTPUTS],
        scratch_a: &mut [f32],
        scratch_b: &mut [f32],
    ) {
        let hidden_sizes = self.topology.hidden_sizes();
        let activations = self.topology.activations();
        let mut weight_off = 0usize;
        let mut fan_in = self.topology.input_width();

        for (k, &fan_out) in hidden_sizes.iter().enumerate() {
            let weights = &self.weights[weight_off..weight_off + fan_in * fan_out];
            let (src, dst): (&[f32], &mut [f32]) = if k == 0 {
                (&input[..], scratch_a)
            } else if k % 2 == 1 {
                (&*scratch_a, scratch_b)
            } else {
                (&*scratch_b, scratch_a)
            };
            for h in 0..fan_out {
                let mut sum = 0.0f32;
                for i in 0..fan_in {
                    sum += src[i] * weights[h * fan_in + i];
                }
                dst[h] = sum;
            }
            apply_activation(activations[k], &mut dst[..fan_out]);
            weight_off += fan_in * fan_out;
            fan_in = fan_out;
        }

        let hidden_count = hidden_sizes.len();
        let last_hidden: &[f32] = if (hidden_count - 1) % 2 == 0 {
            &*scratch_a
        } else {
            &*scratch_b
        };
        let weights = &self.weights[weight_off..weight_off + fan_in * NN_OUTPUTS];
        for o in 0..NN_OUTPUTS {
            let mut sum = 0.0f32;
            for j in 0..fan_in {
                sum += last_hidden[j] * weights[o * fan_in + j];
            }
            output[o] = sum;
        }
        output[0] = output[0].tanh();
        output[1] = output[1].tanh();
    }

    /// v1.12: child = parent weights + geometric-skip Gaussian step driven by
    /// a bucket sampled from `policy`.
    ///
    /// Algorithm:
    /// - `total = sum(bucket.weight)`. If `total <= 0`, return parent clone
    ///   verbatim — this is the user-facing "freeze evolution" mode.
    /// - Otherwise draw `u = rng.unit() * total`, walk buckets to pick one
    ///   (cumulative-weight scan).
    /// - Apply geometric-skip with `(bucket.rate * multiplier, bucket.sigma)`.
    ///
    /// `multiplier` is the global `mutation_rate_multiplier` slider; it scales
    /// the chosen bucket's rate.
    pub fn child_from(
        parent: &Brain,
        rng: &mut SimRng,
        policy: &MutationPolicy,
        multiplier: f32,
    ) -> Self {
        let mut child = parent.clone();
        let total: f32 = policy.buckets.iter().map(|b| b.weight.max(0.0)).sum();
        if !(total > 0.0) {
            return child;
        }
        // Pick a bucket by weighted draw.
        let mut u = rng.unit() * total;
        let mut chosen = &policy.buckets[0];
        for b in policy.buckets.iter() {
            let w = b.weight.max(0.0);
            if u < w {
                chosen = b;
                break;
            }
            u -= w;
        }
        let p = (chosen.rate * multiplier).clamp(0.0, 1.0);
        let sigma = chosen.sigma;
        if p > 0.0 && sigma > 0.0 {
            let n = child.weights.len();
            let mut i = rng.geom_skip(p);
            while i < n {
                child.weights[i] += rng.normal() * sigma;
                let step = rng.geom_skip(p);
                i = i.saturating_add(step).saturating_add(1);
            }
        }
        child
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scratch(t: &NnTopology) -> (Vec<f32>, Vec<f32>) {
        let w = t.max_width();
        (vec![0.0; w], vec![0.0; w])
    }

    #[test]
    fn founder_weight_layout_correct() {
        let mut rng = SimRng::from_u64(1);
        let topo = NnTopology::legacy();
        let expected = topo.weight_count();
        let b = Brain::founder(&mut rng, topo);
        assert_eq!(b.weights.len(), expected);
        assert!(b.weights.iter().all(|w| w.is_finite()));
        assert!(b.weights.iter().any(|w| *w != 0.0));
    }

    /// Legacy pyramid topology weight count = 32*48 + 48*24 + 24*5 = 2808.
    #[test]
    fn nn_weight_count_legacy_pyramid() {
        let t = NnTopology::legacy();
        assert_eq!(t.weight_count(), 2808);
    }

    /// D9: NN_INPUTS padded to multiple of 8.
    #[test]
    fn nn_inputs_padded_to_multiple_of_8() {
        assert_eq!(NN_INPUTS % 8, 0);
    }

    fn single_bucket_policy(rate: f32, sigma: f32) -> MutationPolicy {
        let mut p = MutationPolicy::default();
        p.buckets[0] = Bucket {
            weight: 1.0,
            rate,
            sigma,
        };
        for b in p.buckets.iter_mut().skip(1) {
            *b = Bucket::zero();
        }
        p
    }

    #[test]
    fn child_mutation_zero_rate_is_identity() {
        let mut rng = SimRng::from_u64(1);
        let parent = Brain::founder(&mut rng, NnTopology::legacy());
        let policy = single_bucket_policy(0.0, 0.02);
        let child = Brain::child_from(&parent, &mut rng, &policy, 1.0);
        assert_eq!(parent.weights, child.weights);
    }

    #[test]
    fn child_mutation_perturbs_some_weights() {
        let mut rng = SimRng::from_u64(1);
        let parent = Brain::founder(&mut rng, NnTopology::legacy());
        let policy = single_bucket_policy(0.1, 0.02);
        let child = Brain::child_from(&parent, &mut rng, &policy, 1.0);
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
        let mut rng = SimRng::from_u64(42);
        let parent = Brain::founder(&mut rng, NnTopology::legacy());
        let policy = single_bucket_policy(0.5, 0.02);
        let child = Brain::child_from(&parent, &mut rng, &policy, 0.0);
        assert_eq!(parent.weights, child.weights);
    }

    /// v1.12: all-zero bucket weights → child is a bit-identical clone, no RNG
    /// draws. User-facing freeze-evolution mode.
    #[test]
    fn child_mutation_all_zero_weights_is_clone() {
        let mut rng = SimRng::from_u64(7);
        let parent = Brain::founder(&mut rng, NnTopology::legacy());
        let policy = MutationPolicy {
            buckets: [Bucket::zero(); MUTATION_BUCKET_COUNT],
        };
        let child = Brain::child_from(&parent, &mut rng, &policy, 1.0);
        assert_eq!(parent.weights, child.weights);
    }

    /// v1.12: a 9:1 weight ratio between a zero-rate bucket and a high-rate
    /// bucket gives ≈ 10% of children any mutations.
    #[test]
    fn child_mutation_weight_ratio_selects_bucket() {
        let mut rng = SimRng::from_u64(13);
        let parent = Brain::founder(&mut rng, NnTopology::legacy());
        let mut policy = MutationPolicy {
            buckets: [Bucket::zero(); MUTATION_BUCKET_COUNT],
        };
        policy.buckets[0] = Bucket {
            weight: 9.0,
            rate: 0.0,
            sigma: 0.0,
        };
        policy.buckets[1] = Bucket {
            weight: 1.0,
            rate: 0.5,
            sigma: 0.05,
        };

        let trials = 400;
        let mut mutated = 0;
        for _ in 0..trials {
            let child = Brain::child_from(&parent, &mut rng, &policy, 1.0);
            if child.weights != parent.weights {
                mutated += 1;
            }
        }
        assert!(
            mutated > 20 && mutated < 80,
            "mutated children = {mutated} / {trials}; expected ~40"
        );
    }

    // ---- Forward pass tests ----

    /// Zero weights → all outputs zero; non-zero weights → all outputs finite.
    /// Also exercises both even- and odd-hidden-count topologies (the layer
    /// loop's parity-driven ping-pong).
    #[test]
    fn forward_pass_output_shape() {
        for topology in [
            NnTopology::legacy(),                                                // 2 hidden, even
            NnTopology::new(vec![32], vec![Activation::LReLU]).unwrap(),         // 1 hidden, odd
            NnTopology::new(
                vec![64, 32, 16, 8],
                vec![
                    Activation::LReLU,
                    Activation::Tanh,
                    Activation::ReLU,
                    Activation::Sigmoid,
                ],
            )
            .unwrap(), // 4 hidden, even
        ] {
            let (mut sa, mut sb) = make_scratch(&topology);
            let brain = Brain::zero_with(topology);
            let input = [0.0f32; MAX_NN_INPUTS];
            let mut output = [0.0f32; NN_OUTPUTS];
            brain.forward(&input, &mut output, &mut sa, &mut sb, None);
            assert!(output.iter().all(|&v| v == 0.0), "zero weights → zero output");
        }

        let mut rng = SimRng::from_u64(99);
        let topology = NnTopology::legacy();
        let (mut sa, mut sb) = make_scratch(&topology);
        let brain = Brain::founder(&mut rng, topology);
        let input = [1.0f32; MAX_NN_INPUTS];
        let mut output = [0.0f32; NN_OUTPUTS];
        brain.forward(&input, &mut output, &mut sa, &mut sb, None);
        assert!(output.iter().all(|&v| v.is_finite()));
    }

    /// CRITICAL: SIMD and scalar agree within 1e-5 relative error across
    /// three topologies (parameterised drift-guard from v1.12 plan).
    #[test]
    fn forward_pass_matches_scalar_reference_multiple_topologies() {
        let topologies = [
            NnTopology::legacy(),
            NnTopology::new(
                vec![64, 32, 16],
                vec![Activation::LReLU, Activation::LReLU, Activation::LReLU],
            )
            .unwrap(),
            NnTopology::new(vec![8], vec![Activation::LReLU]).unwrap(),
        ];

        for topology in topologies {
            let mut rng = SimRng::from_u64(12345);
            let brain = Brain::founder(&mut rng, topology.clone());
            let mut input = [0.0f32; MAX_NN_INPUTS];
            for v in &mut input {
                *v = rng.symm();
            }

            let (mut sa_simd, mut sb_simd) = make_scratch(&topology);
            let (mut sa_scal, mut sb_scal) = make_scratch(&topology);

            let mut out_simd = [0.0f32; NN_OUTPUTS];
            let mut out_scalar = [0.0f32; NN_OUTPUTS];
            brain.forward(&input, &mut out_simd, &mut sa_simd, &mut sb_simd, None);
            brain.forward_scalar(&input, &mut out_scalar, &mut sa_scal, &mut sb_scal);

            for o in 0..NN_OUTPUTS {
                let abs_err = (out_simd[o] - out_scalar[o]).abs();
                let mag = out_scalar[o].abs().max(1e-6);
                let rel_err = abs_err / mag;
                assert!(
                    rel_err < 1e-5,
                    "topology {:?}: output[{o}] simd={} scalar={} rel_err={:.2e}",
                    topology.hidden_sizes(),
                    out_simd[o],
                    out_scalar[o],
                    rel_err
                );
            }
        }
    }

    /// Leaky ReLU preserves negative pre-activations at 1% slope.
    #[test]
    fn forward_pass_lrelu_preserves_negative_with_small_slope() {
        let topology = NnTopology::legacy();
        let (mut sa, mut sb) = make_scratch(&topology);
        let h1_width = topology.hidden_sizes()[0];
        let input_width = topology.input_width();
        let mut brain = Brain::zero_with(topology);
        // Row 0 of layer-1 weights (input_width entries) all -1.0. Input all
        // +1.0 → pre-activation = -input_width = -NN_INPUTS for legacy.
        for i in 0..input_width {
            brain.weights[i] = -1.0;
        }

        let input = [1.0f32; MAX_NN_INPUTS];
        let mut output = [0.0f32; NN_OUTPUTS];
        brain.forward(&input, &mut output, &mut sa, &mut sb, None);

        let expected = 0.01 * -(NN_INPUTS as f32);
        let err = (sa[0] - expected).abs();
        assert!(
            err < 1e-5,
            "Leaky ReLU must preserve negative pre-activation at slope 0.01: \
             h1[0]={} expected={expected}",
            sa[0]
        );
        for h in 1..h1_width {
            assert_eq!(sa[h], 0.0);
        }
    }

    /// No NaN on extreme inputs.
    #[test]
    fn forward_pass_no_nan_on_extreme_inputs() {
        let topology = NnTopology::legacy();
        let (mut sa, mut sb) = make_scratch(&topology);
        let mut brain = Brain::zero_with(topology);
        for w in &mut brain.weights {
            *w = 0.3;
        }
        for sign in [1.0f32, -1.0f32] {
            let input = [sign; MAX_NN_INPUTS];
            let mut output = [0.0f32; NN_OUTPUTS];
            brain.forward(&input, &mut output, &mut sa, &mut sb, None);
            assert!(
                output.iter().all(|v| v.is_finite()),
                "NaN/inf on extreme input (sign={sign})"
            );
        }
    }

    /// Identical inputs + weights → bit-identical outputs.
    #[test]
    fn forward_pass_deterministic() {
        let mut rng = SimRng::from_u64(7);
        let topology = NnTopology::legacy();
        let brain = Brain::founder(&mut rng, topology.clone());
        let (mut sa1, mut sb1) = make_scratch(&topology);
        let (mut sa2, mut sb2) = make_scratch(&topology);
        let mut input = [0.0f32; MAX_NN_INPUTS];
        for v in &mut input {
            *v = rng.symm();
        }

        let mut out1 = [0.0f32; NN_OUTPUTS];
        let mut out2 = [0.0f32; NN_OUTPUTS];
        brain.forward(&input, &mut out1, &mut sa1, &mut sb1, None);
        brain.forward(&input, &mut out2, &mut sa2, &mut sb2, None);

        assert_eq!(out1, out2);
    }
}
