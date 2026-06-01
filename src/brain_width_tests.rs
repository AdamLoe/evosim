//! v2.0 Wave 0 — runtime NN input-width tests.
//!
//! These pin the foundational refactor that made the NN input width a runtime
//! field of `NnTopology` (fed as the first matmul's `fan_in`) with a
//! `MAX_NN_INPUTS = 48` ceiling, plus the composable `NnInputLayout` descriptor
//! that computes group offsets + total width.
//!
//! Lives in its OWN file/module (wired from `brain.rs` via `#[path]`) to avoid
//! the shared-`mod tests` merge hazard.

use crate::brain::{Activation, Brain, NnTopology};
use crate::constants::{MAX_NN_INPUTS, NN_INPUTS, NN_OUTPUTS, NN_SECTORS};
use crate::rng::SimRng;
use crate::world::nn::{NnInputGroup, NnInputLayout};

fn scratch_for(t: &NnTopology) -> (Vec<f32>, Vec<f32>) {
    let w = t.max_width();
    (vec![0.0; w], vec![0.0; w])
}

/// The exact legacy weight count must be unchanged by the refactor:
/// 32*48 + 48*24 + 24*5 = 2808 for the default 32-input topology.
#[test]
fn legacy_weight_count_unchanged() {
    let t = NnTopology::legacy();
    assert_eq!(t.input_width(), NN_INPUTS, "legacy width must stay 32");
    assert_eq!(
        t.weight_count(),
        2808,
        "legacy weight_count must equal the pre-refactor value 32*48 + 48*24 + 24*5"
    );
}

/// `weight_count` uses the runtime input width as the first matmul's fan_in.
#[test]
fn weight_count_scales_with_input_width() {
    // legacy hidden sizes [48, 24]; first matmul = input_width * 48.
    let hidden = vec![48usize, 24];
    let acts = vec![Activation::LReLU, Activation::LReLU];
    for &w in &[32usize, 40, 48] {
        let t = NnTopology::with_input_width(w, hidden.clone(), acts.clone()).unwrap();
        let expected = w * 48 + 48 * 24 + 24 * NN_OUTPUTS;
        assert_eq!(t.weight_count(), expected, "weight_count for input_width={w}");
        assert_eq!(t.input_width(), w);
    }
}

/// Synthesize topologies at the wider runtime widths (no settings produce these
/// yet) and confirm `founder` + `forward` run without panic and emit exactly
/// `NN_OUTPUTS` finite values. Exercises the MAX_NN_INPUTS-sized input path.
#[test]
fn founder_and_forward_at_widths_40_and_48() {
    for &width in &[40usize, 48] {
        let topology = NnTopology::with_input_width(
            width,
            vec![48, 24],
            vec![Activation::LReLU, Activation::LReLU],
        )
        .expect("width 40/48 must be a valid topology");

        let mut rng = SimRng::from_u64(0xC0FFEE + width as u64);
        let brain = Brain::founder(&mut rng, topology.clone());
        // First-matmul weights must be input_width * hidden0.
        assert_eq!(brain.weights.len(), topology.weight_count());

        // Max-sized input vector; only the first `width` lanes are read.
        let mut input = [0.0f32; MAX_NN_INPUTS];
        for (k, v) in input.iter_mut().enumerate() {
            *v = ((k as f32) * 0.123).sin();
        }
        let (mut sa, mut sb) = scratch_for(&topology);
        let mut output = [0.0f32; NN_OUTPUTS];
        brain.forward(&input, &mut output, &mut sa, &mut sb, None);

        assert_eq!(output.len(), NN_OUTPUTS, "output length must equal NN_OUTPUTS");
        assert!(
            output.iter().all(|v| v.is_finite()),
            "width {width}: forward output must be finite, got {output:?}"
        );
    }
}

/// A wider width's first-matmul fan_in actually differs from the legacy width,
/// proving the runtime width threads through to the weight buffer (not a
/// silently-32 path).
#[test]
fn wider_width_grows_first_matmul() {
    let legacy = NnTopology::with_input_width(
        32,
        vec![48, 24],
        vec![Activation::LReLU, Activation::LReLU],
    )
    .unwrap();
    let wide = NnTopology::with_input_width(
        48,
        vec![48, 24],
        vec![Activation::LReLU, Activation::LReLU],
    )
    .unwrap();
    // Only the first matmul differs: (48-32) * 48 extra weights.
    assert_eq!(wide.weight_count() - legacy.weight_count(), (48 - 32) * 48);
}

/// Width validation: must be a multiple of 8 in [8, MAX_NN_INPUTS].
#[test]
fn input_width_validation_rejects_bad_widths() {
    let h = vec![8usize];
    let a = vec![Activation::LReLU];
    assert!(NnTopology::with_input_width(0, h.clone(), a.clone()).is_err());
    assert!(NnTopology::with_input_width(33, h.clone(), a.clone()).is_err()); // not mult of 8
    assert!(
        NnTopology::with_input_width(MAX_NN_INPUTS + 8, h.clone(), a.clone()).is_err(),
        "width above MAX_NN_INPUTS must be rejected"
    );
    // Valid boundaries.
    assert!(NnTopology::with_input_width(8, h.clone(), a.clone()).is_ok());
    assert!(NnTopology::with_input_width(MAX_NN_INPUTS, h, a).is_ok());
}

// ---- Composable input-layout descriptor ----

/// The legacy layout reproduces the historical width-32 slot order exactly.
#[test]
fn legacy_layout_reproduces_width_32_slots() {
    let l = NnInputLayout::legacy();
    assert_eq!(l.width(), 32, "legacy layout width must be 32");
    assert_eq!(l.offset_of(NnInputGroup::SelfMemory), Some(0));
    assert_eq!(l.offset_of(NnInputGroup::WallProximity), Some(8));
    assert_eq!(l.offset_of(NnInputGroup::CreatureSectors), Some(12));
    assert_eq!(l.offset_of(NnInputGroup::GrassSectors), Some(20));
    assert_eq!(l.offset_of(NnInputGroup::ReservedPredator), Some(28));
    assert_eq!(l.offset_of(NnInputGroup::CurrGrass), Some(29));
    assert_eq!(l.offset_of(NnInputGroup::Bias), Some(30));
    // The legacy layout's width must match the legacy topology's input_width.
    assert_eq!(l.width(), NnTopology::legacy().input_width());
}

/// Generalization proof (machinery only, no live group added): a hypothetical
/// layout that widens the creature-sector block from 8 to 16 sectors must
/// recompute every downstream offset and round the total up to the next
/// multiple of 8 — without any hand-edited slot constant.
#[test]
fn layout_recomputes_offsets_when_a_group_widens() {
    // 8 + 4 + 16 + 8 + 1 + 1 + 1 = 39 → padded to 40.
    let wide = NnInputLayout::from_groups_for_test(&[
        (NnInputGroup::SelfMemory, 8),
        (NnInputGroup::WallProximity, 4),
        (NnInputGroup::CreatureSectors, 2 * NN_SECTORS),
        (NnInputGroup::GrassSectors, NN_SECTORS),
        (NnInputGroup::ReservedPredator, 1),
        (NnInputGroup::CurrGrass, 1),
        (NnInputGroup::Bias, 1),
    ]);
    assert_eq!(wide.width(), 40, "39 real slots round up to 40");
    assert_eq!(wide.offset_of(NnInputGroup::CreatureSectors), Some(12));
    // GrassSectors now starts after the 16-wide creature block (12 + 16 = 28).
    assert_eq!(wide.offset_of(NnInputGroup::GrassSectors), Some(28));
    assert_eq!(wide.offset_of(NnInputGroup::ReservedPredator), Some(36));
    assert_eq!(wide.offset_of(NnInputGroup::Bias), Some(38));
    // The recomputed width is a valid runtime input_width for a topology.
    assert!(NnTopology::with_input_width(
        wide.width(),
        vec![48, 24],
        vec![Activation::LReLU, Activation::LReLU],
    )
    .is_ok());
}

/// A subset layout (group toggled off) still packs contiguously and aligns.
#[test]
fn layout_subset_packs_and_aligns() {
    // Drop the two reserved/scalar tail groups: 8 + 4 + 8 + 8 + 1(bias) = 29 → 32.
    let l = NnInputLayout::from_groups_for_test(&[
        (NnInputGroup::SelfMemory, 8),
        (NnInputGroup::WallProximity, 4),
        (NnInputGroup::CreatureSectors, NN_SECTORS),
        (NnInputGroup::GrassSectors, NN_SECTORS),
        (NnInputGroup::Bias, 1),
    ]);
    assert_eq!(l.width(), 32);
    assert_eq!(l.offset_of(NnInputGroup::Bias), Some(28));
    assert_eq!(l.offset_of(NnInputGroup::ReservedPredator), None);
    assert_eq!(l.offset_of(NnInputGroup::CurrGrass), None);
}
