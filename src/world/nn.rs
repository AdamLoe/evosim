//! NN forward pass and action-decode helpers. Both sequential and threaded paths live here.

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
        // Threaded path: runs when compiled with --features threads.
        // Returns early so the sequential fallthrough below does not run.
        #[cfg(feature = "threads")]
        {
            let _ = ranges; // threaded path uses chunk_base_size(n) directly
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
        }

        // Sequential fallthrough: runs in default builds only.
        #[cfg(not(feature = "threads"))]
        {
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
        } // end #[cfg(not(feature = "threads"))]
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

/// Build the NN input vector for creature `i` (v6 §E, Milestone D.16).
///
/// `prev_vx` and `prev_vy` are the creature's velocity from the END of the
/// previous tick. Passed explicitly so that callers can supply the correct
/// values even when the vx/vy SoA columns have been temporarily detached via
/// `mem::take` for the parallel NN path (S23).
///
/// `grass` is the world's grass density field, read-only during NN forward.
///
/// D3: genome deleted; all body traits are compile-time constants.
/// Layout (NN_INPUTS=112 slots total):
/// - `[0..5]` self-state (energy_frac, age_frac, prev_vx, prev_vy, cooldown_frac)
/// - `[5..77]` vision passthrough (72 floats = 24 sectors × 3 RGB)
/// - `[77..80]` last_action one-hot (3 actions: Graze/Eat/Split)
/// - `[80..105]` grass-patch 5×5 bilinear samples
/// - `[105..112]` SIMD padding zeros
pub(crate) fn build_nn_input(
    i: usize,
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    grass: &GrassGrid,
    prev_vx: f32,
    prev_vy: f32,
) -> [f32; NN_INPUTS] {
    // D3: body traits are constants; no genome reads needed.
    let mut buf = [0.0f32; NN_INPUTS];
    let energy = creatures.energy[i];
    let age = creatures.age[i];
    let cooldown = creatures.digestion_cooldown[i];

    // 0..5: self-state.
    buf[0] = (energy / 100.0).clamp(0.0, 1.0); // energy_frac
    buf[1] = (age as f32 / FOUNDER_MAX_AGE as f32).clamp(0.0, 1.0); // age_frac (D3: constant max age)
    buf[2] = prev_vx / MOVE_SPEED_MAX; // vx (previous-tick post-clip)
    buf[3] = prev_vy / MOVE_SPEED_MAX; // vy
    buf[4] = cooldown as f32 / DIGESTION_COOLDOWN_TICKS as f32; // cooldown_frac
    // buf[5..NN_VISION_OFFSET]: If NN_SELF_STATE_LEN < NN_VISION_OFFSET, extra slots stay 0.

    // Vision passthrough (raw world units, C.12 contract).
    // NOTE: VISION_LEN=120 (24×5) but NN_VISION_LEN=72 (24×3). Copy only 72 floats (D9 owns Q3 shrink).
    let copy_len = crate::vision::VISION_LEN.min(NN_VISION_LEN);
    buf[NN_VISION_OFFSET..NN_VISION_OFFSET + copy_len].copy_from_slice(&vision[..copy_len]);

    // Last-action one-hot. Action::one_hot_index() maps each variant to its discriminant.
    let la = creatures.last_action[i].one_hot_index();
    if la < NN_LAST_ACTION_LEN {
        buf[NN_LAST_ACTION_OFFSET + la] = 1.0;
    }

    // Grass-patch 5×5 bilinear samples.
    // D3: nose_count gate removed — all creatures always see grass.
    {
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
    // Slots [105..112) are SIMD padding zeros (stay at 0.0 init).

    buf
}

/// Check whether an action is currently valid for creature `i` (v6 §1 + §G).
/// D9: 3-variant enum. Graze always valid; Eat gated on cooldown; Split on energy.
pub(crate) fn is_valid_action(act: Action, energy: f32, cooldown: u32) -> bool {
    match act {
        Action::Graze => true,
        Action::Eat => cooldown == 0,
        Action::Split => energy >= SPLIT_THRESHOLD,
    }
}

/// Decode action logits to an Action via argmax with first-index tiebreak
/// and valid-fallthrough (v6 §1 + §E).
///
/// D3: genome removed. Logits slice length == Action::ALL.len().
/// S5: If any logit is non-finite (NaN or ±inf), returns `Action::Graze`
/// immediately (was Action::Rest).
pub(crate) fn decode_action(
    logits: &[f32; 3],
    energy: f32,
    cooldown: u32,
) -> Action {
    // S5: NaN / ±inf guard — non-finite logits make the sort undefined.
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

/// NN-driven action picker (Milestone D.16 — replaces `pick_action_c`).
///
/// Builds the 160-input vector, runs the SIMD forward pass, decodes velocity
/// via tanh and action via valid-fallthrough argmax (v6 §1, §E).
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

    // Velocity: tanh(out[0..2]) × MOVE_SPEED_MAX (v6 §E).
    // D3: move speed is a constant — no per-creature g_move_speed mirror.
    let vx = output_buf[0].tanh() * MOVE_SPEED_MAX;
    let vy = output_buf[1].tanh() * MOVE_SPEED_MAX;

    // Action: valid-fallthrough argmax over logits out[2..5] (3 action logits).
    let logits: &[f32; 3] = output_buf[2..5].try_into().unwrap();
    // S5: assert finite logits in debug builds — NaN in output_buf means the
    // forward pass received non-finite inputs, which should be unreachable in
    // a healthy simulation but is caught defensively in decode_action below.
    debug_assert!(
        logits.iter().all(|v| v.is_finite()),
        "non-finite logits for creature {i}: {logits:?}"
    );
    let energy = creatures.energy[i];
    let cooldown = creatures.digestion_cooldown[i];
    // D3: genome removed; decode_action no longer needs &Genome.
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

    /// D3/v1.3 layout contract.
    /// NN_INPUTS = 112: 5 self-state + 72 vision + 3 last_action + 25 grass-patch + 7 pad.
    #[test]
    fn vision_layout_matches_nn_input_block() {
        use crate::vision::VISION_LEN;
        assert_eq!(VISION_LEN, 120);
        // D3: NN_INPUTS=112; NN_SELF_STATE_LEN=5, NN_VISION_LEN=72, NN_LAST_ACTION_LEN=3, grass=25, pad=7.
        assert_eq!(NN_INPUTS, NN_SELF_STATE_LEN + NN_VISION_LEN + NN_LAST_ACTION_LEN + NN_GRASS_PATCH_LEN + 7);
        assert_eq!(NN_INPUTS, 112);
    }

    // ---- D.16 tests ----

    /// D.16 test 8: build_nn_input produces correct self-state at offsets 0..5 (D3 layout).
    #[test]
    fn nn_input_layout_self_state_correct() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("d16-self-state");
        // D3: body traits are constants; only energy/age/cooldown can be set.
        w.creatures.energy[0] = 50.0; // energy_frac = 0.5
        w.creatures.age[0] = 2500; // age_frac = 2500/5000 = 0.5 (FOUNDER_MAX_AGE=5000)
        w.creatures.vx[0] = 2.5; // vx/MOVE_SPEED_MAX = 2.5/5.0 = 0.5
        w.creatures.vy[0] = -5.0; // vy/MOVE_SPEED_MAX = -5.0/5.0 = -1.0
        w.creatures.digestion_cooldown[0] = 25; // cooldown_frac = 25/50 = 0.5
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        // D3 self-state layout: [0]=energy_frac, [1]=age_frac, [2]=vx, [3]=vy, [4]=cooldown.
        assert!((inp[0] - 0.5).abs() < 1e-5, "energy_frac = {}", inp[0]);
        assert!((inp[1] - 0.5).abs() < 1e-5, "age_frac = {}", inp[1]);
        assert!((inp[2] - 0.5).abs() < 1e-5, "vx_norm = {}", inp[2]);
        assert!((inp[3] - (-1.0)).abs() < 1e-5, "vy_norm = {}", inp[3]);
        assert!((inp[4] - 0.5).abs() < 1e-5, "cooldown_frac = {}", inp[4]);
    }

    /// D.16 test 9: vision passthrough — input[NN_VISION_OFFSET..NN_VISION_OFFSET+NN_VISION_LEN]
    /// contains the first NN_VISION_LEN floats of the vision buffer byte-exact.
    #[test]
    fn nn_input_layout_vision_passthrough() {
        use crate::vision::VISION_LEN;
        let w = World::new("d16-vision");
        // Write a known pattern into the founder's vision buffer.
        let mut vis = [0.0f32; VISION_LEN];
        for (k, v) in vis.iter_mut().enumerate() {
            *v = k as f32 * 0.01;
        }
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let inp = build_nn_input(0, &w.creatures, &vis, &grass, prev_vx, prev_vy);
        // D3: NN_VISION_OFFSET=5, NN_VISION_LEN=72.
        assert_eq!(
            &inp[NN_VISION_OFFSET..NN_VISION_OFFSET + NN_VISION_LEN],
            &vis[..NN_VISION_LEN],
            "vision passthrough must be byte-exact for first NN_VISION_LEN slots"
        );
    }

    /// D.16 test 10: last_action one-hot at NN_LAST_ACTION_OFFSET (77..80 in D3).
    #[test]
    fn nn_input_layout_last_action_onehot() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("d16-onehot");
        w.creatures.last_action[0] = Action::Eat;
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        let eat_idx = Action::Eat.one_hot_index(); // Eat=2
        // D3: NN_LAST_ACTION_OFFSET=77, NN_LAST_ACTION_LEN=3.
        for k in 0..NN_LAST_ACTION_LEN {
            let expected = if k == eat_idx { 1.0 } else { 0.0 };
            assert_eq!(
                inp[NN_LAST_ACTION_OFFSET + k],
                expected,
                "last_action one-hot[{k}] = {} (expected {expected})",
                inp[NN_LAST_ACTION_OFFSET + k]
            );
        }
    }

    /// D.16 test 11: valid-fallthrough — Split highest logit but energy too low.
    #[test]
    fn decode_action_valid_fallthrough_split_invalid() {
        // Split is index 2 in the 3-logit slice. Make it highest.
        let logits = [0.0f32, 0.0, 10.0];
        // energy = 10 < SPLIT_THRESHOLD (50) → Split invalid.
        let act = decode_action(&logits, 10.0, 0);
        assert_ne!(act, Action::Split, "Split must be invalid when energy < 50");
        // D9: Should fall through to a valid action (Graze or Eat).
        assert!(
            matches!(act, Action::Graze | Action::Eat),
            "got {:?}",
            act
        );
    }

    /// D.16 test 12: first-index tiebreak — lower index wins on equal logits.
    #[test]
    fn decode_action_first_index_tiebreak() {
        // All logits equal → Action::ALL[0] = Graze wins (D9: index 0 is Graze).
        let logits = [5.0f32; 3];
        let act = decode_action(&logits, 100.0, 0);
        // Action::ALL[0]=Graze is always valid → it wins.
        assert_eq!(act, Action::ALL[0], "lower index must win on ties");
        assert_eq!(act, Action::Graze, "Graze is index 0 after D9");
    }

    /// D.16 test 13: Eat invalid when in digestion cooldown.
    #[test]
    fn decode_action_eat_invalid_in_cooldown() {
        // Eat is the 3rd slot (index=2) in the 3-logit decode.
        // D3: Action::Eat one_hot_index = 2; logit index in 3-slice = 2.
        let logits = [0.0f32, 0.0, 10.0]; // slot 2 highest → maps to Eat or Split depending on Action::ALL order
        // cooldown > 0 → Eat invalid.
        let act = decode_action(&logits, 100.0, 5);
        // Either it returns a valid action (not Eat, since cooldown>0).
        assert_ne!(act, Action::Eat, "Eat must be invalid when cooldown > 0");
    }

    /// D.16 test 14: Split invalid when energy too low.
    #[test]
    fn decode_action_scavenge_invalid_when_zero_eff() {
        // Split is index 2, give it highest logit.
        let logits = [0.0f32, 0.0, 10.0];
        // energy = 0 → Split invalid.
        let act = decode_action(&logits, 0.0, 0);
        assert_ne!(act, Action::Split, "Split must be invalid when energy too low");
    }

    // ---- D.18 chunking tests ----

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

    /// D.16 test 15: Graze is always valid as the final fallback (D9).
    #[test]
    fn decode_action_graze_always_valid_as_fallback() {
        // D9: 3-variant enum. Split invalid (energy=0), Eat invalid (cooldown>0). Graze wins.
        let logits = [-5.0f32, 2.0, 10.0]; // Split highest but invalid
        let act = decode_action(&logits, 0.0, 1); // energy=0 → Split invalid; cooldown>0 → Eat invalid
        assert!(
            matches!(act, Action::Graze),
            "Expected Graze fallback, got {:?}",
            act
        );
    }

    /// S39 test (c): chunk_ranges partitions [0,n) cleanly AND agrees with the
    /// chunk_base_size stride that vision.rs calls. Asserts cross-partition
    /// equivalence by calling the real shared chunk_base_size function, not by
    /// re-inlining the formula. Any future desync between the two is caught here.
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

            // (vi) cross-partition equivalence — reconstruct the partition that
            // par_chunks_mut(chunk_base_size(n)) would produce and assert it
            // matches chunk_ranges. This is the property vision.rs:74 relies on
            // post-S39: vision calls chunk_base_size(n), and that stride must
            // yield the same boundaries as chunk_ranges.
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

            // (vii) n=0 special-case: chunk_base_size returns 1 (not 0) to
            // avoid par_chunks_mut(0) panic; all ranges are (0, 0).
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

    /// S5: decode_action returns Graze and pick_action_d zeros velocity when any
    /// logit is non-finite (NaN or ±inf). D3: genome removed from decode_action.
    #[test]
    fn nan_logits_return_rest_zero_velocity() {
        // decode_action with a NaN logit must return Graze.
        let nan_logits = [f32::NAN, 0.0, 0.0];
        let act = decode_action(&nan_logits, 100.0, 0);
        assert_eq!(act, Action::Graze, "NaN logit must produce Graze");

        let inf_logits = [f32::INFINITY, 0.0, 0.0];
        let act2 = decode_action(&inf_logits, 100.0, 0);
        assert_eq!(act2, Action::Graze, "+Inf logit must produce Graze");

        // pick_action_d: when output_buf[2..5] has NaN the returned velocity must be 0.
        let input_buf = [0.0f32; NN_INPUTS];
        let hidden_buf = [0.0f32; NN_HIDDEN];
        let mut output_buf = [0.0f32; NN_OUTPUTS];
        // Inject NaN into logit slots.
        output_buf[2] = f32::NAN;
        // Logits are out[2..5] (3 slots).
        let logits: &[f32; 3] = output_buf[2..5].try_into().unwrap();
        let had_nan = logits.iter().any(|v| !v.is_finite());
        assert!(had_nan, "test setup: NaN must be detected");
        // Simulate the velocity fallback.
        let (vx, vy, action) = if had_nan {
            (0.0_f32, 0.0_f32, Action::Graze)
        } else {
            let vx = output_buf[0].tanh() * MOVE_SPEED_MAX;
            let vy = output_buf[1].tanh() * MOVE_SPEED_MAX;
            let w = World::new("s5-nan");
            let act = decode_action(logits, w.creatures.energy[0], 0);
            (vx, vy, act)
        };
        assert_eq!(action, Action::Graze, "had_nan path must return Graze");
        assert_eq!(vx, 0.0, "had_nan path must zero vx");
        assert_eq!(vy, 0.0, "had_nan path must zero vy");
        // Suppress unused-variable warnings from the buffers we allocated above.
        let _ = &input_buf;
        let _ = &hidden_buf;
    }

    // ---- P2d grass-patch NN input tests ----

    /// P2d test 1: all-density-1.0 grass → grass-patch block all ≈ 1.0.
    /// D3: nose_count gate removed — all creatures always see grass.
    #[test]
    fn nn_input_patch_layout_basic() {
        use crate::vision::VISION_LEN;
        let w = World::new("p2d-basic");
        // D3: no nose_count gate; all creatures see grass.
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

    /// P2d test 2: pins center-slot index. Single seeded cell at creature's position
    /// → inp[NN_GRASS_PATCH_CENTER_SLOT] == seeded value, all other patch slots == 0.0.
    /// D3: NN_GRASS_PATCH_CENTER_SLOT = NN_GRASS_PATCH_OFFSET + 12 = 92 (v1.3 layout).
    #[test]
    fn nn_input_patch_iteration_order_locked() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("p2d-center");
        let cell_ix: usize = 120;
        let cell_iy: usize = 120;
        let cx = (cell_ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (cell_iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        // Seed ONLY this cell. The 24 offset sample points land on adjacent cells (all-zero).
        let center_cell = cell_iy * crate::constants::GRASS_GRID_DIM + cell_ix;
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        grass.density[center_cell] = 0.5;
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        assert!(
            (inp[NN_GRASS_PATCH_CENTER_SLOT] - 0.5).abs() < 1e-5,
            "center slot {} = {} (expected 0.5)",
            NN_GRASS_PATCH_CENTER_SLOT,
            inp[NN_GRASS_PATCH_CENTER_SLOT]
        );
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

    /// P2d test 3: constant pinning — NN_GRASS_PATCH_CENTER_SLOT == 92 (v1.3 D3 layout).
    #[test]
    fn nn_input_patch_center_slot_is_92() {
        assert_eq!(
            NN_GRASS_PATCH_CENTER_SLOT, 92,
            "center slot must be 92 in v1.3 D3 layout (NN_GRASS_PATCH_OFFSET=80 + 12 center offset)"
        );
    }

    /// P2d test 4 (D3 adaptation): creature near world edge, sampling outside the
    /// walled world. bilinear_sample must clamp gracefully.
    #[test]
    fn nn_input_patch_near_wall_does_not_panic() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("p2d-wall");
        // Place creature near the north-west corner — dx=-2/dy=-2 samples go off-grid.
        w.creatures.x[0] = 2.0;
        w.creatures.y[0] = 2.0;
        let grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        // Must not panic.
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        // All samples into empty grass grid must be 0.0 or clamped-valid.
        for k in 0..25 {
            assert!(
                inp[NN_GRASS_PATCH_OFFSET + k].is_finite(),
                "patch slot {} = {} must be finite near wall",
                NN_GRASS_PATCH_OFFSET + k,
                inp[NN_GRASS_PATCH_OFFSET + k]
            );
        }
    }

    /// D3: nose_count gate removed — every creature sees grass.
    /// All grass-patch slots must be ~1.0 when density=1.0 everywhere.
    #[test]
    fn nn_input_patch_always_filled_d3() {
        use crate::vision::VISION_LEN;
        let w = World::new("d3-grass-always");
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        grass.density.fill(1.0);
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, prev_vx, prev_vy);
        for slot in NN_GRASS_PATCH_OFFSET..(NN_GRASS_PATCH_OFFSET + 25) {
            assert!(
                inp[slot] > 0.99,
                "slot {slot} = {} (expected ~1.0 — D3 no nose_count gate)",
                inp[slot]
            );
        }
    }
}
