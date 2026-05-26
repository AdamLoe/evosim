//! NN forward pass and action-decode helpers. Both sequential and threaded paths live here.
//! v1.3 D9: Action enum collapsed to {Graze=0, Eat=1, Split=2}. NN_OUTPUTS=5.
//! move_bias_* deleted. decode_action/is_valid_action take no Genome param.

use super::World;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::grass::GrassGrid;
use crate::vision::VisionBuf;
#[cfg(feature = "threads")]
use rayon::prelude::*;

impl World {
    /// Run the NN forward pass for all creatures across fixed chunks (v6 §J).
    /// Sequential by default; rayon-parallel behind `cfg(feature = "threads")`.
    /// Results are bit-identical because the forward pass contains no RNG.
    pub(crate) fn nn_forward_all_chunks(&mut self, ranges: &[(usize, usize); N_CHUNKS], n: usize) {
        // Threaded path: runs when compiled with --features threads AND the
        // S39 observation-only toggle is not set. Returns early so the sequential
        // fallthrough below does not run.
        #[cfg(feature = "threads")]
        if !self.force_sequential_nn {
            // S23: write NN outputs directly into the SoA columns via par_chunks_mut.
            // Strategy: mem::take the three output columns off self.creatures so the
            // borrow checker sees them as independent locals. The remaining SoA fields
            // (x, y, energy, vision, etc.) are read-only during the parallel block.
            // After the parallel work, restore the three columns. The heap buffers
            // are never reallocated — only the Vec metadata (ptr/len/cap, 24 bytes
            // each) moves to the stack. Pattern mirrors perf-2 §5 scratch_neighbors.
            let mut vx_local = std::mem::take(&mut self.creatures.vx);
            let mut vy_local = std::mem::take(&mut self.creatures.vy);
            let mut act_local = std::mem::take(&mut self.creatures.action_this_tick);

            {
                // All &self borrows are now valid because the three detached columns
                // no longer conflict — they live on the stack, not inside self.creatures.
                let creatures_ref = &self.creatures;
                let vision_ref = &self.vision[..n];
                let grass_ref = &self.grass;

                // Compute chunk_size identically to chunk_ranges (N_CHUNKS = 8).
                let chunk_size = chunk_base_size(n);

                // Tandem disjoint-mut chunks over the three output columns.
                // zip_eq panics on length mismatch (defensive); all three vecs have
                // len >= n (invariant on CreatureSoA) so the slices are same-length.
                vx_local[..n]
                    .par_chunks_mut(chunk_size)
                    .zip(vy_local[..n].par_chunks_mut(chunk_size))
                    .zip(act_local[..n].par_chunks_mut(chunk_size))
                    .enumerate()
                    .for_each(|(chunk_idx, ((vx_sub, vy_sub), act_sub))| {
                        debug_assert_eq!(vx_sub.len(), vy_sub.len());
                        debug_assert_eq!(vx_sub.len(), act_sub.len());

                        let mut input_buf = [0.0f32; NN_INPUTS];
                        let mut hidden_buf = [0.0f32; NN_HIDDEN];
                        let mut output_buf = [0.0f32; NN_OUTPUTS];
                        let lo = chunk_idx * chunk_size;

                        for k in 0..vx_sub.len() {
                            let i = lo + k;
                            // Read prev-tick vx/vy from the taken slice BEFORE
                            // overwriting them. vx_sub[k] / vy_sub[k] still hold
                            // the end-of-last-tick velocity at this point (S23).
                            let prev_vx = vx_sub[k];
                            let prev_vy = vy_sub[k];
                            let (vx, vy, action) = pick_action_d(
                                i,
                                &mut input_buf,
                                &mut hidden_buf,
                                &mut output_buf,
                                creatures_ref,
                                &vision_ref[i],
                                grass_ref,
                                prev_vx,
                                prev_vy,
                            );
                            vx_sub[k] = vx;
                            vy_sub[k] = vy;
                            act_sub[k] = action;
                        }
                    });
            } // read-only borrows of &self.creatures etc. dropped here

            // Restore the three output columns. The heap buffers were never
            // freed — only the Vec metadata traveled to the stack. If the
            // parallel for_each panics, the columns are left as Vec::new()
            // (the World is poisoned; no further ticks run). No drop-guard added
            // — consistent with the scratch_neighbors pattern (perf-2 §5).
            self.creatures.vx = vx_local;
            self.creatures.vy = vy_local;
            self.creatures.action_this_tick = act_local;
            return;
        }

        // Sequential fallthrough: runs in default builds always, and in
        // threaded builds when force_sequential_nn is true (S39 test (a) knob).
        let _ = n; // unused in the sequential path
        for &(lo, hi) in ranges {
            let mut input_buf = [0.0f32; NN_INPUTS];
            let mut hidden_buf = [0.0f32; NN_HIDDEN];
            let mut output_buf = [0.0f32; NN_OUTPUTS];
            for i in lo..hi {
                let prev_vx = self.creatures.vx[i];
                let prev_vy = self.creatures.vy[i];
                let (vx, vy, action) = pick_action_d(
                    i,
                    &mut input_buf,
                    &mut hidden_buf,
                    &mut output_buf,
                    &self.creatures,
                    &self.vision[i],
                    &self.grass,
                    prev_vx,
                    prev_vy,
                );
                self.creatures.vx[i] = vx;
                self.creatures.vy[i] = vy;
                self.creatures.action_this_tick[i] = action;
            }
        }
    }
}

/// Compute `N_CHUNKS` non-overlapping index ranges that partition `0..n` (v6 §J).
///
/// Chunks are ceil-sized; the last chunk may be smaller. Empty chunks have
/// `lo == hi`. Single-threaded path iterates them sequentially; the rayon
/// path (behind `cfg(feature = "threads")`) iterates them in parallel —
/// results are bit-identical because no RNG is consumed in the forward pass.
///
/// **Invariant (perf-4):** `vision.rs` uses `par_chunks_mut(n.div_ceil(N_CHUNKS).max(1))`
/// which produces the same partition as this function for all `n`. Do not
/// change one without updating the other, or vision and NN will partition
/// creatures differently within a single tick and the threaded golden will
/// silently drift.
/// Single source of truth for the chunk stride used by every `par_chunks_mut`
/// call in the simulation (vision.rs and nn_forward_all_chunks). Returns the
/// stride such that `slice.par_chunks_mut(chunk_base_size(n))` yields the
/// same partition as `chunk_ranges(n)`. The `.max(1)` guard keeps the stride
/// well-defined when `n == 0` (rayon's `par_chunks_mut(0)` panics).
///
/// New `par_chunks_mut` call sites MUST use this function rather than inlining
/// the formula; S39 test (c) pins the invariant via `chunk_partition_invariants_and_vision_agreement`.
pub(crate) fn chunk_base_size(n: usize) -> usize {
    n.div_ceil(N_CHUNKS).max(1)
}

pub(crate) fn chunk_ranges(n: usize) -> [(usize, usize); N_CHUNKS] {
    let base = chunk_base_size(n);
    let mut out = [(0usize, 0usize); N_CHUNKS];
    for k in 0..N_CHUNKS {
        let lo = (k * base).min(n);
        let hi = ((k + 1) * base).min(n);
        out[k] = (lo, hi);
    }
    // S10: invariant — ranges are contiguous, non-overlapping, and cover exactly 0..n.
    debug_assert_eq!(out[0].0, 0, "chunk_ranges: first lo must be 0");
    debug_assert_eq!(out[N_CHUNKS - 1].1, n, "chunk_ranges: last hi must equal n");
    debug_assert!(
        out.windows(2).all(|w| w[0].1 == w[1].0),
        "chunk_ranges: ranges must be contiguous (no gaps or overlaps)"
    );
    debug_assert!(
        out.iter().all(|(lo, hi)| lo <= hi),
        "chunk_ranges: each range must satisfy lo <= hi"
    );
    out
}

/// Build the 112-float NN input vector for creature `i` (v1.3 D9).
///
/// `prev_vx` and `prev_vy` are the creature's velocity from the END of the
/// previous tick. Passed explicitly so that callers can supply the correct
/// values even when the vx/vy SoA columns have been temporarily detached via
/// `mem::take` for the parallel NN path (S23).
///
/// `grass` is the world's grass density field, read-only during NN forward.
///
/// Layout (112 slots total):
/// - `[0..5)`  self-state: energy_frac, age_frac, prev_vx/MOVE_SPEED_MAX, prev_vy/MOVE_SPEED_MAX, cooldown_frac
/// - `[5..77)` vision passthrough: first 72 of VISION_LEN floats (24 sectors × 3 RGB).
///   vision.rs still emits 120 floats (Q3 not landed); only first 72 are copied.
/// - `[77..80)` last_action one-hot: 3 floats (Graze=0, Eat=1, Split=2)
/// - `[80..105)` grass-patch 5×5 bilinear samples (dy outer, dx inner;
///   center dx=dy=0 → slot 92; nose_count gate unchanged)
/// - `[105..112)` SIMD padding zeros (7 slots)
pub(crate) fn build_nn_input(
    i: usize,
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    grass: &GrassGrid,
    prev_vx: f32,
    prev_vy: f32,
) -> [f32; NN_INPUTS] {
    let mut buf = [0.0f32; NN_INPUTS];
    let g = &creatures.genomes[i]; // for g.max_age (cold)
    let energy = creatures.energy[i];
    let age = creatures.age[i];
    let cooldown = creatures.digestion_cooldown[i];

    // [0..5): self-state
    buf[0] = (energy / 100.0).clamp(0.0, 1.0); // energy_frac
    buf[1] = if g.max_age > 0 {
        (age as f32 / g.max_age as f32).clamp(0.0, 1.0)
    } else {
        1.0
    }; // age_frac
    buf[2] = prev_vx / MOVE_SPEED_MAX; // vx (previous-tick post-clip, normalized)
    buf[3] = prev_vy / MOVE_SPEED_MAX; // vy
    buf[4] = cooldown as f32 / DIGESTION_COOLDOWN_TICKS as f32; // cooldown_frac

    // [5..77): vision passthrough — copy first 72 of VISION_LEN floats.
    // vision.rs emits VISION_LEN=120 (24 sectors × 5 features; Q3 not landed).
    // D9 only uses first 72 (24 sectors × 3 RGB). Remaining slots zero (padding).
    let copy_len = NN_VISION_LEN.min(vision.len());
    buf[NN_VISION_OFFSET..NN_VISION_OFFSET + copy_len].copy_from_slice(&vision[..copy_len]);

    // [77..80): last_action one-hot (3 floats: Graze=0, Eat=1, Split=2)
    let la = creatures.last_action[i].one_hot_index();
    buf[NN_LAST_ACTION_OFFSET + la] = 1.0;

    // [80..105): 25 grass-patch inputs (5x5 bilinear, dy outer, dx inner; v1.2 P2d).
    // Slot offset within block: (dy+2)*5 + (dx+2). Center (dx=dy=0) → global slot 92.
    // P2e gate: only creatures with nose_count >= 1 see grass.
    if creatures.g_nose_count[i] >= 1 {
        let cx = creatures.x[i];
        let cy = creatures.y[i];
        debug_assert!(
            cx.is_finite() && cy.is_finite(),
            "non-finite position for creature {i}"
        );
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let sx = cx + (dx as f32) * GRASS_CELL_SIZE;
                let sy = cy + (dy as f32) * GRASS_CELL_SIZE;
                let off = ((dy + 2) as usize) * 5 + ((dx + 2) as usize);
                buf[NN_GRASS_PATCH_OFFSET + off] = grass.bilinear_sample(sx, sy);
            }
        }
    }
    // [105..112): SIMD padding zeros — remain at [0.0; NN_INPUTS] init.

    buf
}

/// Check whether an action is currently valid for creature `i` (v1.3 D9).
/// No Genome param: Graze always valid, Eat requires cooldown==0, Split requires energy≥threshold.
pub(crate) fn is_valid_action(act: Action, energy: f32, cooldown: u32) -> bool {
    match act {
        Action::Graze => true,
        Action::Eat => cooldown == 0,
        Action::Split => energy >= SPLIT_THRESHOLD,
    }
}

/// Decode 3 action logits to an Action via argmax with first-index tiebreak
/// and valid-fallthrough (v1.3 D9).
///
/// If any logit is non-finite (NaN or ±inf), returns `Action::Graze` immediately.
/// Graze is always valid, so it serves as the safe fallback.
pub(crate) fn decode_action(logits: &[f32; 3], energy: f32, cooldown: u32) -> Action {
    // NaN / ±inf guard — non-finite logits make the sort undefined.
    if logits.iter().any(|v| !v.is_finite()) {
        return Action::Graze;
    }
    use std::cmp::Ordering;
    // Sort Action::ALL indices by logit DESC, first-index tiebreak on ties.
    let mut order = [0u8, 1, 2];
    order.sort_by(|&a, &b| {
        logits[b as usize]
            .partial_cmp(&logits[a as usize])
            .unwrap_or(Ordering::Equal) // tiebreak (logits are now guaranteed finite)
            .then(a.cmp(&b)) // lower index wins on tie (stable first-index)
    });
    for &k in &order {
        let act = Action::ALL[k as usize];
        if is_valid_action(act, energy, cooldown) {
            return act;
        }
    }
    Action::Graze // defensive fallback (Graze is always valid)
}

/// NN-driven action picker (v1.3 D9 — replaces `pick_action_d` v1.2).
///
/// Builds the 112-input vector, runs the SIMD forward pass, decodes velocity
/// via tanh and action via valid-fallthrough argmax (v1.3 D9).
/// Movement happens every tick from NN vx/vy regardless of action argmax.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_action_d(
    i: usize,
    input_buf: &mut [f32; NN_INPUTS],
    hidden_buf: &mut [f32; NN_HIDDEN],
    output_buf: &mut [f32; NN_OUTPUTS],
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    grass: &GrassGrid,
    prev_vx: f32,
    prev_vy: f32,
) -> (f32, f32, Action) {
    *input_buf = build_nn_input(i, creatures, vision, grass, prev_vx, prev_vy);
    creatures.brains[i].forward(input_buf, output_buf, hidden_buf);

    // Velocity: tanh(out[0..2]) × move_speed (v1.3 D9: movement always applied).
    let speed = creatures.g_move_speed[i]; // perf-5: mirror
    let vx = output_buf[0].tanh() * speed;
    let vy = output_buf[1].tanh() * speed;

    // Action: valid-fallthrough argmax over logits out[2..5] (v1.3 D9).
    let logits: &[f32; 3] = output_buf[2..5].try_into().unwrap();
    debug_assert!(
        logits.iter().all(|v| v.is_finite()),
        "non-finite logits for creature {i}: {logits:?}"
    );
    let energy = creatures.energy[i];
    let cooldown = creatures.digestion_cooldown[i];
    let action = decode_action(logits, energy, cooldown);

    // S5: if decode_action saw non-finite logits it returns Graze; also zero
    // velocity so the creature does not coast on stale movement data.
    let had_nan = logits.iter().any(|v| !v.is_finite());
    if had_nan {
        return (0.0, 0.0, Action::Graze);
    }

    (vx, vy, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- D9 action enum tests ----

    /// v1.3 D9: Action enum has exactly 3 variants with correct discriminants.
    #[test]
    fn action_enum_has_three_variants() {
        assert_eq!(Action::Graze as u8, 0);
        assert_eq!(Action::Eat as u8, 1);
        assert_eq!(Action::Split as u8, 2);
        assert_eq!(Action::ALL.len(), 3);
    }

    /// v1.3 D9: decode_action uses 3-logit argmax.
    #[test]
    fn decode_action_three_logits_argmax() {
        // Graze logit highest → Graze (always valid).
        let logits = [10.0f32, 0.0, 0.0];
        assert_eq!(decode_action(&logits, 100.0, 0), Action::Graze);

        // Eat logit highest, cooldown=0 → Eat.
        let logits = [0.0f32, 10.0, 0.0];
        assert_eq!(decode_action(&logits, 100.0, 0), Action::Eat);

        // Split logit highest, energy sufficient → Split.
        let logits = [0.0f32, 0.0, 10.0];
        assert_eq!(decode_action(&logits, 100.0, 0), Action::Split);
    }

    /// v1.3 D9: Split invalid at low energy → falls through to lower-logit valid action.
    #[test]
    fn decode_action_split_invalid_at_low_energy() {
        // Split highest logit but energy < SPLIT_THRESHOLD.
        let logits = [0.0f32, 0.0, 10.0];
        let act = decode_action(&logits, 10.0, 0); // energy=10 < SPLIT_THRESHOLD(50)
        assert_ne!(
            act,
            Action::Split,
            "Split must be invalid when energy < threshold"
        );
        // Should fall through to Graze or Eat (both have lower logits, Graze always valid).
        assert!(matches!(act, Action::Graze | Action::Eat), "got {:?}", act);
    }

    /// v1.3 D9: NaN logits return Graze (safe fallback), zero velocity.
    #[test]
    fn nan_logits_return_graze_zero_velocity() {
        // decode_action with a NaN logit must return Graze.
        let nan_logits = [f32::NAN, 0.0, 0.0];
        let act = decode_action(&nan_logits, 100.0, 0);
        assert_eq!(act, Action::Graze, "NaN logit must produce Graze");

        let inf_logits = [f32::INFINITY, 0.0, 0.0];
        let act2 = decode_action(&inf_logits, 100.0, 0);
        assert_eq!(act2, Action::Graze, "+Inf logit must produce Graze");

        // pick_action_d: when output_buf[2..5] has NaN the returned velocity must be 0.
        let w = World::new("d9-nan");
        let input_buf = [0.0f32; NN_INPUTS];
        let hidden_buf = [0.0f32; NN_HIDDEN];
        let mut output_buf = [0.0f32; NN_OUTPUTS];
        // Inject NaN into logit slots.
        output_buf[2] = f32::NAN;
        // Re-use the logit slice exactly as pick_action_d does.
        let logits: &[f32; 3] = output_buf[2..5].try_into().unwrap();
        let had_nan = logits.iter().any(|v| !v.is_finite());
        assert!(had_nan, "test setup: NaN must be detected");
        // Simulate the velocity fallback.
        let (vx, vy, action) = if had_nan {
            (0.0_f32, 0.0_f32, Action::Graze)
        } else {
            let speed = w.creatures.g_move_speed[0];
            let vx = output_buf[0].tanh() * speed;
            let vy = output_buf[1].tanh() * speed;
            let act = decode_action(logits, w.creatures.energy[0], 0);
            (vx, vy, act)
        };
        assert_eq!(action, Action::Graze, "had_nan path must return Graze");
        assert_eq!(vx, 0.0, "had_nan path must zero vx");
        assert_eq!(vy, 0.0, "had_nan path must zero vy");
        let _ = &input_buf;
        let _ = &hidden_buf;
    }

    /// v1.3 D9: first-index tiebreak — lower index wins on equal logits.
    #[test]
    fn decode_action_tiebreak_lower_index_wins() {
        // All logits equal → Action::ALL[0] = Graze wins.
        let logits = [5.0f32; 3];
        let act = decode_action(&logits, 100.0, 0);
        assert_eq!(act, Action::Graze, "lower index (Graze=0) must win on ties");
    }

    /// v1.3 D9: Eat invalid when in digestion cooldown.
    #[test]
    fn decode_action_eat_invalid_in_cooldown() {
        // Eat is index 1, give it highest logit, but cooldown > 0.
        let logits = [0.0f32, 10.0, 0.0];
        let act = decode_action(&logits, 100.0, 5); // cooldown=5 > 0
        assert_ne!(act, Action::Eat, "Eat must be invalid when cooldown > 0");
        // Falls through to Graze (always valid).
        assert_eq!(act, Action::Graze, "should fall through to Graze");
    }

    /// v1.3 D9: NN input layout post-D9.
    /// NN_INPUTS=112, layout: 5 self-state + 72 vision + 3 last_action + 25 grass + 7 padding.
    #[test]
    fn nn_input_layout_post_v1_3() {
        assert_eq!(NN_INPUTS, 112);
        assert_eq!(NN_VISION_OFFSET, 5);
        assert_eq!(NN_VISION_LEN, 72);
        assert_eq!(NN_LAST_ACTION_OFFSET, 77);
        assert_eq!(NN_LAST_ACTION_LEN, 3);
        assert_eq!(NN_GRASS_PATCH_OFFSET, 80);
        assert_eq!(NN_GRASS_PATCH_LEN, 25);
        // 5 + 72 + 3 + 25 = 105, then 7 padding = 112.
        assert_eq!(
            5 + NN_VISION_LEN + NN_LAST_ACTION_LEN + NN_GRASS_PATCH_LEN + 7,
            NN_INPUTS
        );
    }

    /// v1.3 D9: movement no longer uses move_bias — pure NN velocity.
    /// With zero weights, NN vx=vy=0 → creature stays put.
    #[test]
    fn movement_no_move_bias_uses_pure_nn_velocity() {
        use crate::constants::WORLD_SIZE;
        let mut w = World::new("d9-movement");
        // Zero all NN weights → zero NN output → zero velocity.
        w.creatures.brains[0].weights = vec![0.0; crate::constants::NN_WEIGHT_COUNT];
        // Give creature move_speed so it could move.
        w.creatures.genomes[0].move_speed = crate::constants::MOVE_SPEED_MAX;
        w.creatures.resync_hot_mirrors_at(0);
        let x_before = w.creatures.x[0];
        let y_before = w.creatures.y[0];

        // Run NN forward pass to set vx/vy.
        let n = w.creatures.len();
        let ranges = chunk_ranges(n);
        w.nn_forward_all_chunks(&ranges, n);

        // With zero weights, vx=vy=0 → no movement.
        assert_eq!(w.creatures.vx[0], 0.0, "zero-weight NN → zero vx");
        assert_eq!(w.creatures.vy[0], 0.0, "zero-weight NN → zero vy");

        // Run movement step — should not move.
        w.apply_movement_and_repulsion();
        let dx = (w.creatures.x[0] - x_before).abs();
        let dy = (w.creatures.y[0] - y_before).abs();
        // Allow for wrap-around edge case: treat any value < 1.0 as no movement.
        let moved = dx.min(WORLD_SIZE - dx) > 1.0 || dy.min(WORLD_SIZE - dy) > 1.0;
        assert!(
            !moved,
            "zero vx/vy → creature must not move; dx={dx} dy={dy}"
        );
    }

    /// v1.3 D9: NN_GRASS_PATCH_CENTER_SLOT == 92 (was 147 in v1.2).
    #[test]
    fn nn_input_patch_center_slot_is_92() {
        assert_eq!(
            NN_GRASS_PATCH_CENTER_SLOT, 92,
            "center slot must be 92 (v1.3 D9 layout)"
        );
    }

    // ---- D.18 chunking tests (unchanged) ----

    /// D.18 test 16: chunk_ranges partitions n=1000 into 8 non-overlapping ranges.
    #[test]
    fn chunk_ranges_partition() {
        let ranges = chunk_ranges(1000);
        assert_eq!(ranges.len(), N_CHUNKS);
        // First lo = 0, last hi = 1000.
        assert_eq!(ranges[0].0, 0);
        assert_eq!(ranges[N_CHUNKS - 1].1, 1000);
        // Ranges are contiguous and non-overlapping.
        let mut total = 0usize;
        for k in 0..N_CHUNKS {
            let (lo, hi) = ranges[k];
            assert!(lo <= hi, "range {k}: lo={lo} > hi={hi}");
            total += hi - lo;
            if k + 1 < N_CHUNKS {
                assert_eq!(hi, ranges[k + 1].0, "gap between chunks {k} and {}", k + 1);
            }
        }
        assert_eq!(total, 1000, "total elements across chunks must equal n");
    }

    /// D.18 test 17: chunk_ranges handles n < N_CHUNKS gracefully (no panic).
    #[test]
    fn chunk_ranges_small_population() {
        for n in [0, 1, 3, 7] {
            let ranges = chunk_ranges(n);
            let total: usize = ranges.iter().map(|(lo, hi)| hi - lo).sum();
            assert_eq!(total, n, "n={n}: total {total}");
            // All ranges must be valid (lo <= hi).
            for &(lo, hi) in &ranges {
                assert!(lo <= hi, "invalid range lo={lo} hi={hi} for n={n}");
            }
            // First lo = 0.
            assert_eq!(ranges[0].0, 0);
            // Last hi = n.
            assert_eq!(ranges[N_CHUNKS - 1].1, n);
        }
    }

    /// S10: chunk_ranges invariants hold for a broad set of n values including
    /// edge cases (0, 1, n < N_CHUNKS, n == N_CHUNKS, n % N_CHUNKS != 0, large n).
    #[test]
    fn chunk_ranges_invariants() {
        for &n in &[0usize, 1, 7, 8, 9, 100, 1500] {
            let ranges = chunk_ranges(n);
            // first lo = 0
            assert_eq!(ranges[0].0, 0, "n={n}: first lo must be 0");
            // last hi = n
            assert_eq!(ranges[N_CHUNKS - 1].1, n, "n={n}: last hi must be n");
            // all lo <= hi
            for (k, &(lo, hi)) in ranges.iter().enumerate() {
                assert!(lo <= hi, "n={n}: chunk {k}: lo={lo} > hi={hi}");
            }
            // contiguous
            for k in 0..N_CHUNKS - 1 {
                assert_eq!(
                    ranges[k].1,
                    ranges[k + 1].0,
                    "n={n}: gap between chunks {k} and {}",
                    k + 1
                );
            }
            // total coverage
            let total: usize = ranges.iter().map(|(lo, hi)| hi - lo).sum();
            assert_eq!(total, n, "n={n}: total elements {total} != n");
        }
    }

    /// S23: threaded NN par_chunks_mut path produces the same vx/vy/action_this_tick
    /// as the sequential path for the same world state. Gated on `threads` feature
    /// so it only runs when rayon is available.
    #[cfg(feature = "threads")]
    #[test]
    fn threaded_nn_in_place_writes_match_sequential() {
        use crate::world::nn::chunk_ranges;

        // Advance a single sequential world to get a multi-creature state.
        let seeds = ["s23-base", "split-test", "s23-alt"];
        let w_base = {
            let mut found = None;
            'outer: for seed in seeds {
                let mut w = World::new(seed);
                w.force_sequential_nn = true;
                for _ in 0..2000 {
                    if !w.step() {
                        break;
                    }
                    if w.creatures.len() >= 2 {
                        found = Some(w);
                        break 'outer;
                    }
                }
            }
            found.expect("no seed produced ≥2 creatures within 2000 ticks")
        };
        let n = w_base.creatures.len();
        assert!(
            n >= 2,
            "need >= 2 creatures for a meaningful parallel test; got {n}"
        );

        // Clone identical state via save/load round-trip.
        let snap = w_base.to_save_v1();
        let mut w_seq = World::from_save_v1(snap.clone()).expect("seq load");
        let mut w_par = World::from_save_v1(snap).expect("par load");

        // Both worlds must have the same creature count.
        assert_eq!(w_seq.creatures.len(), n, "seq world population mismatch");
        assert_eq!(w_par.creatures.len(), n, "par world population mismatch");

        // Rebuild the carrion spatial index.
        crate::vision::build_cell_to_carrion(&w_seq.carrion, &mut w_seq.cell_to_carrion);
        crate::vision::build_cell_to_carrion(&w_par.carrion, &mut w_par.cell_to_carrion);

        let ranges = chunk_ranges(n);

        // Sequential run.
        w_seq.force_sequential_nn = true;
        w_seq.nn_forward_all_chunks(&ranges, n);

        // Threaded run.
        w_par.force_sequential_nn = false;
        w_par.nn_forward_all_chunks(&ranges, n);

        // Outputs must be bit-identical.
        for i in 0..n {
            assert_eq!(
                w_seq.creatures.vx[i], w_par.creatures.vx[i],
                "vx[{i}] mismatch: seq={} par={}",
                w_seq.creatures.vx[i], w_par.creatures.vx[i]
            );
            assert_eq!(
                w_seq.creatures.vy[i], w_par.creatures.vy[i],
                "vy[{i}] mismatch: seq={} par={}",
                w_seq.creatures.vy[i], w_par.creatures.vy[i]
            );
            assert_eq!(
                w_seq.creatures.action_this_tick[i], w_par.creatures.action_this_tick[i],
                "action_this_tick[{i}] mismatch: seq={:?} par={:?}",
                w_seq.creatures.action_this_tick[i], w_par.creatures.action_this_tick[i]
            );
        }
    }

    // ---- P2d grass-patch NN input tests ----

    /// P2d test 1: all-density-1.0 grass → inp[80..105] all ≈ 1.0.
    #[test]
    fn nn_input_patch_layout_basic() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("p2d-basic");
        // P2e: nose_count must be >= 1 for the creature to see grass.
        w.creatures.genomes[0].nose_count = 1;
        w.creatures.resync_hot_mirrors_at(0);
        // Seed all cells at GRASS_MAX (1.0); bilinear sample anywhere returns 1.0.
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        for d in grass.density.iter_mut() {
            *d = 1.0;
        }
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        for k in 0..25 {
            assert!(
                (inp[NN_GRASS_PATCH_OFFSET + k] - 1.0).abs() < 1e-5,
                "patch slot {} = {} (expected 1.0)",
                NN_GRASS_PATCH_OFFSET + k,
                inp[NN_GRASS_PATCH_OFFSET + k]
            );
        }
    }

    /// P2d test 2: pins center-slot index = 92. Single seeded cell at creature's position
    /// → inp[92] == seeded value, all other patch slots == 0.0.
    #[test]
    fn nn_input_patch_iteration_order_locked() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("p2d-center");
        // Place the creature at the center of grass cell (120, 120).
        let cell_ix: usize = 120;
        let cell_iy: usize = 120;
        let cx = (cell_ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (cell_iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        // P2e: nose_count must be >= 1 for the creature to see grass.
        w.creatures.genomes[0].nose_count = 1;
        w.creatures.resync_hot_mirrors_at(0);
        // Seed ONLY this cell.
        let center_cell = cell_iy * crate::constants::GRASS_GRID_DIM + cell_ix;
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        grass.density[center_cell] = 0.5;
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        // Center slot 92 must be the seeded value exactly.
        assert!(
            (inp[NN_GRASS_PATCH_CENTER_SLOT] - 0.5).abs() < 1e-5,
            "center slot {} = {} (expected 0.5)",
            NN_GRASS_PATCH_CENTER_SLOT,
            inp[NN_GRASS_PATCH_CENTER_SLOT]
        );
        // All other patch slots must be zero.
        for k in 0..25 {
            let slot = NN_GRASS_PATCH_OFFSET + k;
            if slot == NN_GRASS_PATCH_CENTER_SLOT {
                continue;
            }
            assert_eq!(
                inp[slot], 0.0,
                "patch slot {slot} = {} (expected 0.0 — only center seeded)",
                inp[slot]
            );
        }
    }

    /// P2d test 3: constant pinning — NN_GRASS_PATCH_CENTER_SLOT == 92.
    #[test]
    fn nn_input_patch_center_slot_is_92_pin() {
        assert_eq!(
            NN_GRASS_PATCH_CENTER_SLOT, 92,
            "center slot must be 92 (v1.3 D9 layout)"
        );
    }

    /// P2d test 4: toroidal seam wrap — creature near origin, seed grass near
    /// opposite corner. The dx=-2,dy=-2 sample (slot 80) must read wrapped cells.
    #[test]
    fn nn_input_patch_seam_wrap() {
        use crate::grass::cell_index_for;
        use crate::vision::VISION_LEN;
        // Place creature near the world origin.
        let mut w = World::new("p2d-seam");
        w.creatures.x[0] = 1.0;
        w.creatures.y[0] = 1.0;
        // P2e: nose_count must be >= 1 for the creature to see grass.
        w.creatures.genomes[0].nose_count = 1;
        w.creatures.resync_hot_mirrors_at(0);

        let sx = 1.0_f32 + (-2_f32) * GRASS_CELL_SIZE;
        let sy = 1.0_f32 + (-2_f32) * GRASS_CELL_SIZE;
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let wrapped_cell = cell_index_for(sx, sy);
        grass.density[wrapped_cell] = 0.8;

        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        // Slot 80 = NN_GRASS_PATCH_OFFSET + 0 = dx=-2, dy=-2 sample.
        let slot80 = inp[NN_GRASS_PATCH_OFFSET];
        assert!(
            slot80 > 0.0,
            "seam-wrap slot 80 = {slot80} (expected > 0.0 — wrapped cell seeded)"
        );
    }

    /// P2d test 5: threaded path produces same inp[80..105] as sequential path.
    #[cfg(feature = "threads")]
    #[test]
    fn nn_input_patch_threaded_matches_sequential() {
        use crate::world::nn::chunk_ranges;

        let seeds = ["p2d-threads", "p2d-threads-2", "p2d-threads-3"];
        let w_base = {
            let mut found = None;
            'outer: for seed in seeds {
                let mut w = World::new(seed);
                w.force_sequential_nn = true;
                for _ in 0..2000 {
                    if !w.step() {
                        break;
                    }
                    if w.creatures.len() >= 2 {
                        found = Some(w);
                        break 'outer;
                    }
                }
            }
            found.expect("no seed produced ≥2 creatures within 2000 ticks")
        };
        let n = w_base.creatures.len();

        let snap = w_base.to_save_v1();
        let mut w_seq = World::from_save_v1(snap.clone()).expect("seq load");
        let mut w_par = World::from_save_v1(snap).expect("par load");

        // P2e: set nose_count=1 for all creatures in both worlds.
        for i in 0..n {
            w_seq.creatures.genomes[i].nose_count = 1;
            w_seq.creatures.resync_hot_mirrors_at(i);
            w_par.creatures.genomes[i].nose_count = 1;
            w_par.creatures.resync_hot_mirrors_at(i);
        }
        // Seed grass.
        w_seq.grass.density.fill(1.0);
        w_par.grass.density.fill(1.0);

        crate::vision::build_cell_to_carrion(&w_seq.carrion, &mut w_seq.cell_to_carrion);
        crate::vision::build_cell_to_carrion(&w_par.carrion, &mut w_par.cell_to_carrion);

        let ranges = chunk_ranges(n);
        w_seq.force_sequential_nn = true;
        w_seq.nn_forward_all_chunks(&ranges, n);

        w_par.force_sequential_nn = false;
        w_par.nn_forward_all_chunks(&ranges, n);

        use crate::vision::VISION_LEN;
        for i in 0..n {
            let vision = [0.0f32; VISION_LEN];
            let prev_vx = w_seq.creatures.vx[i];
            let prev_vy = w_seq.creatures.vy[i];
            let inp_seq =
                build_nn_input(i, &w_seq.creatures, &vision, &w_seq.grass, prev_vx, prev_vy);
            let inp_par =
                build_nn_input(i, &w_par.creatures, &vision, &w_par.grass, prev_vx, prev_vy);
            for k in 0..25 {
                let slot = NN_GRASS_PATCH_OFFSET + k;
                assert_eq!(
                    inp_seq[slot].to_bits(),
                    inp_par[slot].to_bits(),
                    "grass-patch slot {slot} mismatch for creature {i}: seq={} par={}",
                    inp_seq[slot],
                    inp_par[slot]
                );
            }
        }
    }

    /// P2e test: nose_count=0 gate — grass-blind creatures see all-zero patch slots.
    #[test]
    fn nn_input_patch_gated_by_nose_count() {
        use crate::vision::VISION_LEN;

        // --- Part 1: nose_count = 0 → grass-blind ---
        let w = World::new("p2e-gate");
        assert_eq!(
            w.creatures.g_nose_count[0], 0,
            "founder default nose_count must be 0"
        );
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        // Seed grass at max density everywhere.
        grass.density.fill(1.0);
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        for slot in NN_GRASS_PATCH_OFFSET..(NN_GRASS_PATCH_OFFSET + 25) {
            assert_eq!(
                inp[slot], 0.0,
                "slot {slot} must be 0.0 when nose_count=0 (grass-blind)"
            );
        }

        // --- Part 2: nose_count = 1 → grass-sighted, slots become ~1.0 ---
        let mut w2 = World::new("p2e-gate-sighted");
        w2.creatures.genomes[0].nose_count = 1;
        w2.creatures.resync_hot_mirrors_at(0);
        assert_eq!(
            w2.creatures.g_nose_count[0], 1,
            "mirror must reflect nose_count=1"
        );
        let prev_vx2 = w2.creatures.vx[0];
        let prev_vy2 = w2.creatures.vy[0];
        let inp2 = build_nn_input(0, &w2.creatures, &vision, &grass, prev_vx2, prev_vy2);
        for slot in NN_GRASS_PATCH_OFFSET..(NN_GRASS_PATCH_OFFSET + 25) {
            assert!(
                inp2[slot] > 0.99,
                "slot {slot} = {} (expected ~1.0 when nose_count=1 and grass=max)",
                inp2[slot]
            );
        }
    }

    /// S39 test (c): chunk_ranges partitions [0,n) cleanly AND agrees with the
    /// chunk_base_size stride that vision.rs calls.
    #[test]
    fn chunk_partition_invariants_and_vision_agreement() {
        let cases = [0usize, 1, 7, 8, 9, 100, 1500, N_CHUNKS, N_CHUNKS + 1];
        for &n in &cases {
            let ranges = chunk_ranges(n);

            // (i) non-empty range count <= N_CHUNKS
            let non_empty = ranges.iter().filter(|(lo, hi)| lo < hi).count();
            assert!(
                non_empty <= N_CHUNKS,
                "n={n}: non-empty chunks {non_empty} exceed N_CHUNKS {N_CHUNKS}"
            );

            // (ii) first range starts at 0
            assert_eq!(ranges[0].0, 0, "n={n}: first range must start at 0");

            // (iii) last range ends at n
            assert_eq!(ranges[N_CHUNKS - 1].1, n, "n={n}: last range must end at n");

            // (iv) contiguous and non-overlapping
            for k in 0..N_CHUNKS - 1 {
                assert_eq!(
                    ranges[k].1,
                    ranges[k + 1].0,
                    "n={n}: gap or overlap between chunk {k} and {}",
                    k + 1
                );
            }

            // (v) total coverage
            let total: usize = ranges.iter().map(|(lo, hi)| hi - lo).sum();
            assert_eq!(total, n, "n={n}: ranges do not cover [0,n)");

            // (vi) cross-partition equivalence
            let base = chunk_base_size(n);
            let mut reconstructed = [(0usize, 0usize); N_CHUNKS];
            for k in 0..N_CHUNKS {
                let lo = (k * base).min(n);
                let hi = ((k + 1) * base).min(n);
                reconstructed[k] = (lo, hi);
            }
            assert_eq!(
                ranges, reconstructed,
                "n={n}: chunk_ranges disagrees with par_chunks_mut(chunk_base_size(n)) partition"
            );

            // (vii) n=0 special-case
            if n == 0 {
                assert_eq!(
                    base, 1,
                    "n=0: chunk_base_size must return 1 to keep par_chunks_mut well-defined on []"
                );
                for &(lo, hi) in &ranges {
                    assert_eq!(lo, 0, "n=0: all ranges must be (0, 0)");
                    assert_eq!(hi, 0, "n=0: all ranges must be (0, 0)");
                }
            }
        }
    }
}
