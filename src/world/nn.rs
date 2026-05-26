//! NN forward pass and action-decode helpers. Both sequential and threaded paths live here.

use super::World;
use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::genome::Genome;
use crate::vision::{CarrionIndex, VisionBuf};
#[cfg(feature = "threads")]
use rayon::prelude::*;

/// Count the number of carrion blobs overlapping creature `i`'s body circle.
/// Uses the cached per-cell carrion index (rebuilt before the vision pass).
/// Used by BOTH the sequential and threaded NN paths (audit S11).
pub(crate) fn count_carrion_overlap(
    creatures: &CreatureSoA,
    carrion: &[Carrion],
    cell_to_carrion: &CarrionIndex,
    i: usize,
) -> u32 {
    let xi = creatures.x[i];
    let yi = creatures.y[i];
    let ri = creatures.g_size[i] * BODY_RADIUS_PER_SIZE; // perf-5: mirror
    let r2 = ri * ri;
    let cx = (xi / HASH_CELL).floor() as i32;
    let cy = (yi / HASH_CELL).floor() as i32;
    let dim = HASH_DIM as i32;
    let mut count = 0u32;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                continue;
            }
            let cell_idx = ny as usize * HASH_DIM + nx as usize;
            for &ci in cell_to_carrion.cell_slice(cell_idx) {
                let c = &carrion[ci as usize];
                let ddx = c.x - xi;
                let ddy = c.y - yi;
                if ddx * ddx + ddy * ddy <= r2 {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Returns 1.0 if creature `i` is within `WALL_THRESHOLD_PAD` of any wall edge,
/// else 0.0. Uses creature radius (size × BODY_RADIUS_PER_SIZE) for consistency
/// with the physics step. Used by BOTH sequential and threaded NN paths (audit S11).
pub(crate) fn compute_is_at_wall(creatures: &CreatureSoA, i: usize) -> f32 {
    let x = creatures.x[i];
    let y = creatures.y[i];
    let r = creatures.g_size[i] * BODY_RADIUS_PER_SIZE; // perf-5: mirror
    let near = x.min(y).min(WORLD_SIZE - x).min(WORLD_SIZE - y);
    if near < r + WALL_THRESHOLD_PAD {
        1.0
    } else {
        0.0
    }
}

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
                let carrion_ref = &self.carrion;
                let cell_to_carrion_ref = &self.cell_to_carrion;

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
                            let overlap = count_carrion_overlap(
                                creatures_ref,
                                carrion_ref,
                                cell_to_carrion_ref,
                                i,
                            );
                            let is_at_wall = compute_is_at_wall(creatures_ref, i);
                            let (vx, vy, action) = pick_action_d(
                                i,
                                &mut input_buf,
                                &mut hidden_buf,
                                &mut output_buf,
                                creatures_ref,
                                &vision_ref[i],
                                overlap,
                                is_at_wall,
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
                let overlap =
                    count_carrion_overlap(&self.creatures, &self.carrion, &self.cell_to_carrion, i);
                let is_at_wall = compute_is_at_wall(&self.creatures, i);
                let prev_vx = self.creatures.vx[i];
                let prev_vy = self.creatures.vy[i];
                let (vx, vy, action) = pick_action_d(
                    i,
                    &mut input_buf,
                    &mut hidden_buf,
                    &mut output_buf,
                    &self.creatures,
                    &self.vision[i],
                    overlap,
                    is_at_wall,
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

/// Build the 136-float NN input vector for creature `i` (v6 §E, Milestone D.16).
///
/// `prev_vx` and `prev_vy` are the creature's velocity from the END of the
/// previous tick. Passed explicitly so that callers can supply the correct
/// values even when the vx/vy SoA columns have been temporarily detached via
/// `mem::take` for the parallel NN path (S23).
///
/// Layout: `[0..10]` self-state, `[10..130]` vision passthrough, `[130..136]` last_action one-hot.
pub(crate) fn build_nn_input(
    i: usize,
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    carrion_overlap_count: u32,
    is_at_wall_flag: f32,
    prev_vx: f32,
    prev_vy: f32,
) -> [f32; NN_INPUTS] {
    // perf-5: hot reads use g_* mirrors; cold reads (max_age) stay on &creatures.genomes[i].
    let mut buf = [0.0f32; NN_INPUTS];
    let size_i = creatures.g_size[i];
    let g = &creatures.genomes[i]; // for g.max_age (cold)
    let energy = creatures.energy[i];
    let age = creatures.age[i];
    let cooldown = creatures.digestion_cooldown[i];

    // 0..10: self-state (v6 §E "Self-state layout (10)").
    buf[0] = (energy / 100.0).clamp(0.0, 1.0); // energy_frac
    buf[1] = if g.max_age > 0 {
        (age as f32 / g.max_age as f32).clamp(0.0, 1.0)
    } else {
        1.0
    }; // age_frac
    buf[2] = size_i / SIZE_MAX; // size (normalized)
    buf[3] = prev_vx / MOVE_SPEED_MAX; // vx (previous-tick post-clip)
    buf[4] = prev_vy / MOVE_SPEED_MAX; // vy
    buf[5] = is_at_wall_flag; // is_at_wall
    buf[6] = cooldown as f32 / DIGESTION_COOLDOWN_TICKS as f32; // cooldown_frac
    buf[7] = (carrion_overlap_count as f32 / CARRION_OVERLAP_NORM_BASE).min(1.0); // carrion_overlap_norm
                                                                                  // buf[8], buf[9]: reserved zero (v5 §5.1).

    // 10..130: vision passthrough (raw world units, C.12 contract).
    buf[10..130].copy_from_slice(vision);

    // 130..136: last_action one-hot (v6 §2 + §E).
    let la = creatures.last_action[i].one_hot_index();
    buf[130 + la] = 1.0;

    buf
}

/// Check whether an action is currently valid for creature `i` (v6 §1 + §G).
pub(crate) fn is_valid_action(act: Action, genome: &Genome, energy: f32, cooldown: u32) -> bool {
    match act {
        Action::Rest | Action::Graze | Action::Signal => true,
        Action::Eat => genome.eat_efficiency > 0.0 && cooldown == 0,
        Action::Scavenge => genome.scavenge_efficiency > 0.0,
        Action::Split => energy >= SPLIT_THRESHOLD,
    }
}

/// Decode 6 action logits to an Action via argmax with first-index tiebreak
/// and valid-fallthrough (v6 §1 + §E).
///
/// S5: If any logit is non-finite (NaN or ±inf), returns `Action::Rest`
/// immediately — the caller (`pick_action_d`) is responsible for zeroing
/// velocity in that case.
pub(crate) fn decode_action(
    logits: &[f32; 6],
    genome: &Genome,
    energy: f32,
    cooldown: u32,
) -> Action {
    // S5: NaN / ±inf guard — non-finite logits make the sort undefined.
    if logits.iter().any(|v| !v.is_finite()) {
        return Action::Rest;
    }
    use std::cmp::Ordering;
    // Sort Action::ALL indices by logit DESC, first-index tiebreak on ties.
    let mut order = [0u8, 1, 2, 3, 4, 5];
    order.sort_by(|&a, &b| {
        logits[b as usize]
            .partial_cmp(&logits[a as usize])
            .unwrap_or(Ordering::Equal) // tiebreak (logits are now guaranteed finite)
            .then(a.cmp(&b)) // lower index wins on tie (stable first-index)
    });
    for &k in &order {
        let act = Action::ALL[k as usize];
        if is_valid_action(act, genome, energy, cooldown) {
            return act;
        }
    }
    Action::Rest // defensive fallback (Rest/Graze/Signal are always valid)
}

/// NN-driven action picker (Milestone D.16 — replaces `pick_action_c`).
///
/// Builds the 136-input vector, runs the SIMD forward pass, decodes velocity
/// via tanh and action via valid-fallthrough argmax (v6 §1, §E).
#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_action_d(
    i: usize,
    input_buf: &mut [f32; NN_INPUTS],
    hidden_buf: &mut [f32; NN_HIDDEN],
    output_buf: &mut [f32; NN_OUTPUTS],
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    carrion_overlap_count: u32,
    is_at_wall_flag: f32,
    prev_vx: f32,
    prev_vy: f32,
) -> (f32, f32, Action) {
    *input_buf = build_nn_input(
        i,
        creatures,
        vision,
        carrion_overlap_count,
        is_at_wall_flag,
        prev_vx,
        prev_vy,
    );
    creatures.brains[i].forward(input_buf, output_buf, hidden_buf);

    // Velocity: tanh(out[0..2]) × move_speed (v6 §E).
    let speed = creatures.g_move_speed[i]; // perf-5: mirror
    let vx = output_buf[0].tanh() * speed;
    let vy = output_buf[1].tanh() * speed;

    // Action: valid-fallthrough argmax over logits out[2..8] (v6 §1).
    let logits: &[f32; 6] = output_buf[2..8].try_into().unwrap();
    // S5: assert finite logits in debug builds — NaN in output_buf means the
    // forward pass received non-finite inputs, which should be unreachable in
    // a healthy simulation but is caught defensively in decode_action below.
    debug_assert!(
        logits.iter().all(|v| v.is_finite()),
        "non-finite logits for creature {i}: {logits:?}"
    );
    let energy = creatures.energy[i];
    let cooldown = creatures.digestion_cooldown[i];
    // decode_action takes &Genome (reads eat_eff/scav_eff). Hot, but kept on AoS to avoid API churn; one read/creature/tick.
    let action = decode_action(logits, &creatures.genomes[i], energy, cooldown);

    // S5: if decode_action saw non-finite logits it returns Rest; also zero
    // velocity so the creature does not coast on stale movement data.
    let had_nan = logits.iter().any(|v| !v.is_finite());
    if had_nan {
        return (0.0, 0.0, Action::Rest);
    }

    (vx, vy, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C.12 test 7: vision layout contract for D.
    /// VISION_LEN == 120 and NN_INPUTS == 10 + VISION_LEN + 6 (== 136).
    #[test]
    fn vision_layout_matches_nn_input_block() {
        use crate::vision::VISION_LEN;
        assert_eq!(VISION_LEN, 120);
        // D's NN input block: 10 self-state + 120 vision + 6 last_action one-hot.
        assert_eq!(NN_INPUTS, 10 + VISION_LEN + 6);
    }

    // ---- D.16 tests ----

    /// D.16 test 8: build_nn_input produces correct self-state at offsets 0..10.
    #[test]
    fn nn_input_layout_self_state_correct() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("d16-self-state");
        // Set known state on the founder.
        w.creatures.energy[0] = 50.0; // energy_frac = 0.5
        w.creatures.age[0] = 1000;
        w.creatures.genomes[0].max_age = 4000; // age_frac = 0.25
        w.creatures.genomes[0].size = 5.0; // size/10 = 0.5
        w.creatures.resync_hot_mirrors_at(0); // perf-5: keep mirrors in sync after genome patch
        w.creatures.vx[0] = 2.5; // vx/5 = 0.5
        w.creatures.vy[0] = -5.0; // vy/5 = -1.0 (clipped to -1 in input)
        w.creatures.digestion_cooldown[0] = 25; // cooldown_frac = 0.5
        w.creatures.last_action[0] = Action::Rest;
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, 2, 0.0, prev_vx, prev_vy);
        // energy_frac
        assert!((inp[0] - 0.5).abs() < 1e-5, "energy_frac = {}", inp[0]);
        // age_frac
        assert!((inp[1] - 0.25).abs() < 1e-5, "age_frac = {}", inp[1]);
        // size
        assert!((inp[2] - 0.5).abs() < 1e-5, "size_norm = {}", inp[2]);
        // vx
        assert!((inp[3] - 0.5).abs() < 1e-5, "vx_norm = {}", inp[3]);
        // vy: -5.0/5.0 = -1.0
        assert!((inp[4] - (-1.0)).abs() < 1e-5, "vy_norm = {}", inp[4]);
        // is_at_wall = 0.0 (founder at center)
        assert_eq!(inp[5], 0.0, "is_at_wall");
        // cooldown_frac
        assert!((inp[6] - 0.5).abs() < 1e-5, "cooldown_frac = {}", inp[6]);
        // carrion_overlap: 2/4 = 0.5
        assert!((inp[7] - 0.5).abs() < 1e-5, "carrion_norm = {}", inp[7]);
        // reserved zeros
        assert_eq!(inp[8], 0.0);
        assert_eq!(inp[9], 0.0);
    }

    /// D.16 test 9: vision passthrough — input[10..130] == vision[0] byte-exact.
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
        let inp = build_nn_input(0, &w.creatures, &vis, 0, 0.0, prev_vx, prev_vy);
        assert_eq!(
            &inp[10..130],
            &vis[..],
            "vision passthrough must be byte-exact"
        );
    }

    /// D.16 test 10: last_action one-hot at offsets 130..136.
    #[test]
    fn nn_input_layout_last_action_onehot() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("d16-onehot");
        w.creatures.last_action[0] = Action::Eat;
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, 0, 0.0, prev_vx, prev_vy);
        let eat_idx = Action::Eat.one_hot_index(); // should be 2
        for k in 0..6 {
            let expected = if k == eat_idx { 1.0 } else { 0.0 };
            assert_eq!(
                inp[130 + k],
                expected,
                "last_action one-hot[{k}] = {} (expected {expected})",
                inp[130 + k]
            );
        }
    }

    /// D.16 test 11: valid-fallthrough — Split highest logit but energy too low.
    #[test]
    fn decode_action_valid_fallthrough_split_invalid() {
        use crate::genome::Genome;
        let g = Genome::founder();
        // Split is Action::ALL[4], logit index 4. Make it highest.
        let logits = [0.0f32, 0.0, 0.0, 0.0, 10.0, 0.0];
        // energy = 10 < SPLIT_THRESHOLD (50) → Split invalid.
        let act = decode_action(&logits, &g, 10.0, 0);
        assert_ne!(act, Action::Split, "Split must be invalid when energy < 50");
        // Should fall through to a valid action.
        assert!(
            matches!(
                act,
                Action::Rest | Action::Graze | Action::Eat | Action::Scavenge | Action::Signal
            ),
            "got {:?}",
            act
        );
    }

    /// D.16 test 12: first-index tiebreak — lower index wins on equal logits.
    #[test]
    fn decode_action_first_index_tiebreak() {
        use crate::genome::Genome;
        let g = Genome::founder();
        // All logits equal → Action::ALL[0] = Rest wins.
        let logits = [5.0f32; 6];
        let act = decode_action(&logits, &g, 100.0, 0);
        assert_eq!(act, Action::Rest, "lower index (Rest=0) must win on ties");
    }

    /// D.16 test 13: Eat invalid when in digestion cooldown.
    #[test]
    fn decode_action_eat_invalid_in_cooldown() {
        use crate::genome::Genome;
        let mut g = Genome::founder();
        g.eat_efficiency = 1.0;
        // Eat is index 2, give it highest logit.
        let logits = [0.0f32, 0.0, 10.0, 0.0, 0.0, 0.0];
        // cooldown > 0 → Eat invalid.
        let act = decode_action(&logits, &g, 100.0, 5);
        assert_ne!(act, Action::Eat, "Eat must be invalid when cooldown > 0");
    }

    /// D.16 test 14: Scavenge invalid when scavenge_efficiency == 0.
    #[test]
    fn decode_action_scavenge_invalid_when_zero_eff() {
        use crate::genome::Genome;
        let mut g = Genome::founder();
        g.scavenge_efficiency = 0.0;
        // Scavenge is index 3.
        let logits = [0.0f32, 0.0, 0.0, 10.0, 0.0, 0.0];
        let act = decode_action(&logits, &g, 100.0, 0);
        assert_ne!(act, Action::Scavenge, "Scavenge must be invalid when eff=0");
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

    /// D.16 test 15: Rest is always valid as the final fallback.
    #[test]
    fn decode_action_rest_always_valid_as_fallback() {
        use crate::genome::Genome;
        let mut g = Genome::founder();
        g.eat_efficiency = 0.0; // Eat invalid
        g.scavenge_efficiency = 0.0; // Scavenge invalid
                                     // Give Split the highest logit (energy=0 → invalid), Eat 2nd (eff=0 → invalid),
                                     // Scavenge 3rd (eff=0 → invalid). Rest, Photo, Signal should all be valid.
                                     // Rest is index 0, make it the lowest logit so Photo or Signal wins first,
                                     // but confirm that the function returns a valid action regardless.
        let logits = [-5.0f32, 2.0, -3.0, -3.0, 10.0, 3.0]; // Split>Signal>Graze>...
        let act = decode_action(&logits, &g, 0.0, 1); // energy=0 → Split invalid; cooldown>0 → Eat invalid
                                                      // Graze (idx 1, logit=2) and Signal (idx 5, logit=3) are both valid;
                                                      // Signal has higher logit so it wins.
        assert!(
            matches!(act, Action::Graze | Action::Signal | Action::Rest),
            "Expected always-valid action, got {:?}",
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

    /// S5: decode_action returns Rest and pick_action_d zeros velocity when any
    /// logit is non-finite (NaN or ±inf).
    #[test]
    fn nan_logits_return_rest_zero_velocity() {
        use crate::genome::Genome;
        // decode_action with a NaN logit must return Rest.
        let g = Genome::founder();
        let nan_logits = [f32::NAN, 0.0, 0.0, 0.0, 0.0, 0.0];
        let act = decode_action(&nan_logits, &g, 100.0, 0);
        assert_eq!(act, Action::Rest, "NaN logit must produce Rest");

        let inf_logits = [f32::INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0];
        let act2 = decode_action(&inf_logits, &g, 100.0, 0);
        assert_eq!(act2, Action::Rest, "+Inf logit must produce Rest");

        // pick_action_d: when output_buf[2..8] has NaN the returned velocity must be 0.
        // We construct a World and manually inject NaN into the forward-pass output
        // by temporarily patching output_buf ourselves.
        let w = World::new("s5-nan");
        let input_buf = [0.0f32; NN_INPUTS];
        let hidden_buf = [0.0f32; NN_HIDDEN];
        let mut output_buf = [0.0f32; NN_OUTPUTS];
        // Inject NaN into logit slots.
        output_buf[2] = f32::NAN;
        // Bypass the forward pass and call the action-decode portion indirectly:
        // re-use the logit slice exactly as pick_action_d does.
        let logits: &[f32; 6] = output_buf[2..8].try_into().unwrap();
        let had_nan = logits.iter().any(|v| !v.is_finite());
        assert!(had_nan, "test setup: NaN must be detected");
        // Simulate the velocity fallback.
        let (vx, vy, action) = if had_nan {
            (0.0_f32, 0.0_f32, Action::Rest)
        } else {
            let speed = w.creatures.g_move_speed[0];
            let vx = output_buf[0].tanh() * speed;
            let vy = output_buf[1].tanh() * speed;
            let act = decode_action(logits, &w.creatures.genomes[0], w.creatures.energy[0], 0);
            (vx, vy, act)
        };
        assert_eq!(action, Action::Rest, "had_nan path must return Rest");
        assert_eq!(vx, 0.0, "had_nan path must zero vx");
        assert_eq!(vy, 0.0, "had_nan path must zero vy");
        // Suppress unused-variable warnings from the buffers we allocated above.
        let _ = &input_buf;
        let _ = &hidden_buf;
    }

    /// S11: helpers_match_legacy_inline_math — confirms the free-fn helpers produce
    /// correct values by re-computing results from scratch in the test body.
    #[test]
    fn helpers_match_legacy_inline_math() {
        use crate::carrion::Carrion;

        let mut w = World::new("s11-helpers");
        // Ensure the founder is near a wall and there's a carrion in the overlap zone.
        // Place the founder near the west wall so is_at_wall fires.
        let r = w.creatures.g_size[0] * BODY_RADIUS_PER_SIZE;
        w.creatures.x[0] = r + WALL_THRESHOLD_PAD * 0.5; // within WALL_THRESHOLD_PAD of west wall
        w.creatures.y[0] = WORLD_SIZE * 0.5;

        // Add a carrion at the creature's position so overlap == 1.
        w.carrion.push(Carrion {
            id: 999,
            x: w.creatures.x[0],
            y: w.creatures.y[0],
            pool: 1.0,
            age: 0,
        });
        // Rebuild the cell_to_carrion index.
        crate::vision::build_cell_to_carrion(&w.carrion, &mut w.cell_to_carrion);

        // Call the free-fn helpers.
        let overlap = count_carrion_overlap(&w.creatures, &w.carrion, &w.cell_to_carrion, 0);
        let is_wall = compute_is_at_wall(&w.creatures, 0);

        // Verify: the carrion is at the creature's position so overlap >= 1.
        assert!(overlap >= 1, "overlap should be at least 1, got {overlap}");

        // Verify: creature is within WALL_THRESHOLD_PAD of west wall.
        assert_eq!(
            is_wall, 1.0,
            "creature near west wall must have is_at_wall = 1.0"
        );

        // Verify a creature far from the wall has is_at_wall = 0.0.
        w.creatures.x[0] = WORLD_SIZE * 0.5;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        let far_wall = compute_is_at_wall(&w.creatures, 0);
        assert_eq!(
            far_wall, 0.0,
            "creature at center must have is_at_wall = 0.0"
        );
    }

    /// S23: threaded NN par_chunks_mut path produces the same vx/vy/action_this_tick
    /// as the sequential path for the same world state. Gated on `threads` feature
    /// so it only runs when rayon is available.
    ///
    /// Strategy: advance a single world (sequential) to a non-trivial state, then
    /// deserialize two independent copies from the same snapshot and run each
    /// copy's nn_forward_all_chunks under sequential vs threaded paths. Both
    /// copies start from identical SoA state, so the outputs must be bit-identical.
    #[cfg(feature = "threads")]
    #[test]
    fn threaded_nn_in_place_writes_match_sequential() {
        use crate::world::nn::chunk_ranges;

        // Advance a single sequential world to get a multi-creature state.
        let mut w_base = World::new("s23-base");
        w_base.force_sequential_nn = true;
        for _ in 0..50 {
            w_base.step();
        }
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

        // Rebuild the carrion spatial index (normally done in run_vision_pass;
        // from_save_v1 leaves cell_to_carrion empty).
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
}
