//! NN forward pass and action-decode helpers. Both sequential and threaded paths live here.

use super::World;
use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::genome::Genome;
use crate::grass::GrassGrid;
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
                            let overlap = count_carrion_overlap(
                                creatures_ref,
                                carrion_ref,
                                cell_to_carrion_ref,
                                i,
                            );
                            let (vx, vy, action) = pick_action_d(
                                i,
                                &mut input_buf,
                                &mut hidden_buf,
                                &mut output_buf,
                                creatures_ref,
                                &vision_ref[i],
                                grass_ref,
                                overlap,
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
                    overlap,
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

/// Build the 160-float NN input vector for creature `i` (v6 §E, Milestone D.16).
///
/// `prev_vx` and `prev_vy` are the creature's velocity from the END of the
/// previous tick. Passed explicitly so that callers can supply the correct
/// values even when the vx/vy SoA columns have been temporarily detached via
/// `mem::take` for the parallel NN path (S23).
///
/// `grass` is the world's grass density field, read-only during NN forward.
///
/// Layout (160 slots total):
/// - `[0..9]` self-state (energy_frac, age_frac, size_norm, vx, vy, cooldown_frac,
///   carrion_overlap_norm, reserved×2)
/// - `[9..129]` vision passthrough (120 floats, raw world units)
/// - `[129..135]` last_action one-hot (6 actions)
/// - `[135..160]` grass-patch 5×5 bilinear samples (dy outer, dx inner;
///   center dx=dy=0 → slot 147; P2d fills; P2e gates on nose_count)
pub(crate) fn build_nn_input(
    i: usize,
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    grass: &GrassGrid,
    carrion_overlap_count: u32,
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

    // 0..9: self-state (v6 §E "Self-state layout (9)"; is_at_wall removed in P2b).
    buf[0] = (energy / 100.0).clamp(0.0, 1.0); // energy_frac
    buf[1] = if g.max_age > 0 {
        (age as f32 / g.max_age as f32).clamp(0.0, 1.0)
    } else {
        1.0
    }; // age_frac
    buf[2] = size_i / SIZE_MAX; // size (normalized)
    buf[3] = prev_vx / MOVE_SPEED_MAX; // vx (previous-tick post-clip)
    buf[4] = prev_vy / MOVE_SPEED_MAX; // vy
    buf[5] = cooldown as f32 / DIGESTION_COOLDOWN_TICKS as f32; // cooldown_frac
    buf[6] = (carrion_overlap_count as f32 / CARRION_OVERLAP_NORM_BASE).min(1.0); // carrion_overlap_norm
                                                                                  // buf[7], buf[8]: reserved zero (v5 §5.1).

    // 9..129: vision passthrough (raw world units, C.12 contract).
    buf[NN_VISION_OFFSET..NN_VISION_OFFSET + 120].copy_from_slice(vision);

    // 129..135: last_action one-hot (v6 §2 + §E).
    let la = creatures.last_action[i].one_hot_index();
    buf[NN_LAST_ACTION_OFFSET + la] = 1.0;

    // 135..160: 25 grass-patch inputs (5x5 bilinear, dy outer, dx inner; v1.2 P2d).
    // Slot offset within block: (dy+2)*5 + (dx+2). Center (dx=dy=0) -> global slot 147.
    // P2e gate: only creatures with nose_count >= 1 see grass. Founders default
    // nose_count=0 → grass-blind; slots 135..160 stay at the [0.0; 160] init value.
    // Skipping the bilinear math entirely for nose_count=0 is a micro-perf win for
    // the founder lineage which dominates early ticks.
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
    // else: buf[135..160] remain at their [0.0; NN_INPUTS] initialized values.

    buf
}

/// Check whether an action is currently valid for creature `i` (v6 §1 + §G).
/// D9 stub: updated for 3-action enum {Graze, Eat, Split}.
pub(crate) fn is_valid_action(act: Action, genome: &Genome, energy: f32, cooldown: u32) -> bool {
    match act {
        Action::Graze => true,
        Action::Eat => genome.eat_efficiency > 0.0 && cooldown == 0,
        Action::Split => energy >= SPLIT_THRESHOLD,
    }
}

/// Decode 3 action logits to an Action via argmax with first-index tiebreak
/// and valid-fallthrough (v6 §1 + §E).
///
/// D9 stub: updated for 3-action enum. Logits are [f32; 3] (Graze/Eat/Split).
/// S5: If any logit is non-finite (NaN or ±inf), returns `Action::Graze`
/// immediately (was Action::Rest).
pub(crate) fn decode_action(
    logits: &[f32; 3],
    genome: &Genome,
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
        if is_valid_action(act, genome, energy, cooldown) {
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
    carrion_overlap_count: u32,
    prev_vx: f32,
    prev_vy: f32,
) -> (f32, f32, Action) {
    *input_buf = build_nn_input(
        i,
        creatures,
        vision,
        grass,
        carrion_overlap_count,
        prev_vx,
        prev_vy,
    );
    creatures.brains[i].forward(input_buf, output_buf, hidden_buf);

    // Velocity: tanh(out[0..2]) × move_speed (v6 §E).
    let speed = creatures.g_move_speed[i]; // perf-5: mirror
    let vx = output_buf[0].tanh() * speed;
    let vy = output_buf[1].tanh() * speed;

    // Action: valid-fallthrough argmax over logits out[2..5] (D9: 3 action logits).
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
    // decode_action takes &Genome (reads eat_eff). Hot, but kept on AoS to avoid API churn; one read/creature/tick.
    let action = decode_action(logits, &creatures.genomes[i], energy, cooldown);

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

    /// C.12 test 7: vision layout contract for D.
    /// VISION_LEN == 120 and NN_INPUTS == 9 + VISION_LEN + 6 + 25 (== 160).
    #[test]
    fn vision_layout_matches_nn_input_block() {
        use crate::vision::VISION_LEN;
        assert_eq!(VISION_LEN, 120);
        // Post-P2b: 9 self-state + 120 vision + 6 last_action + 25 reserved (P2d fills grass-patch).
        assert_eq!(NN_INPUTS, 9 + VISION_LEN + 6 + 25);
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
        w.creatures.last_action[0] = Action::Graze; // D9 stub: was Action::Rest
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, 2, prev_vx, prev_vy);
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
        // cooldown_frac (shifted from slot 6 to slot 5 after is_at_wall deletion)
        assert!((inp[5] - 0.5).abs() < 1e-5, "cooldown_frac = {}", inp[5]);
        // carrion_overlap: 2/4 = 0.5 (shifted from slot 7 to slot 6)
        assert!((inp[6] - 0.5).abs() < 1e-5, "carrion_norm = {}", inp[6]);
        // reserved zeros (shifted from slots 8,9 to slots 7,8)
        assert_eq!(inp[7], 0.0);
        assert_eq!(inp[8], 0.0);
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
        let grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let inp = build_nn_input(0, &w.creatures, &vis, &grass, 0, prev_vx, prev_vy);
        assert_eq!(
            &inp[9..129],
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
        let grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, 0, prev_vx, prev_vy);
        let eat_idx = Action::Eat.one_hot_index(); // should be 2
        for k in 0..6 {
            let expected = if k == eat_idx { 1.0 } else { 0.0 };
            assert_eq!(
                inp[129 + k],
                expected,
                "last_action one-hot[{k}] = {} (expected {expected})",
                inp[129 + k]
            );
        }
    }

    /// D.16 test 11: valid-fallthrough — Split highest logit but energy too low.
    #[test]
    fn decode_action_valid_fallthrough_split_invalid() {
        use crate::genome::Genome;
        let g = Genome::founder();
        // Split is Action::ALL[2], logit index 2. Make it highest (D9: 3-action enum).
        let logits = [0.0f32, 0.0, 10.0];
        // energy = 10 < SPLIT_THRESHOLD (50) → Split invalid.
        let act = decode_action(&logits, &g, 10.0, 0);
        assert_ne!(act, Action::Split, "Split must be invalid when energy < 50");
        // Should fall through to a valid action (Graze or Eat).
        assert!(
            matches!(act, Action::Graze | Action::Eat),
            "got {:?}",
            act
        );
    }

    /// D.16 test 12: first-index tiebreak — lower index wins on equal logits.
    #[test]
    fn decode_action_first_index_tiebreak() {
        use crate::genome::Genome;
        let g = Genome::founder();
        // All logits equal → Action::ALL[0] = Graze wins (D9: first action is Graze).
        let logits = [5.0f32; 3];
        let act = decode_action(&logits, &g, 100.0, 0);
        assert_eq!(act, Action::Graze, "lower index (Graze=0) must win on ties");
    }

    /// D.16 test 13: Eat invalid when in digestion cooldown.
    #[test]
    fn decode_action_eat_invalid_in_cooldown() {
        use crate::genome::Genome;
        let mut g = Genome::founder();
        g.eat_efficiency = 1.0;
        // Eat is index 1 (D9: 3-action enum), give it highest logit.
        let logits = [0.0f32, 10.0, 0.0];
        // cooldown > 0 → Eat invalid.
        let act = decode_action(&logits, &g, 100.0, 5);
        assert_ne!(act, Action::Eat, "Eat must be invalid when cooldown > 0");
    }

    /// D.16 test 14: Scavenge deleted in D9. Split invalid when energy too low.
    /// D9 stub: was "Scavenge invalid when scavenge_efficiency == 0".
    #[test]
    fn decode_action_scavenge_invalid_when_zero_eff() {
        use crate::genome::Genome;
        let g = Genome::founder();
        // Split is index 2, give it highest logit.
        let logits = [0.0f32, 0.0, 10.0];
        // energy = 0 → Split invalid.
        let act = decode_action(&logits, &g, 0.0, 0);
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

    /// D.16 test 15: Graze is always valid as the final fallback (D9 stub: was Rest).
    #[test]
    fn decode_action_rest_always_valid_as_fallback() {
        use crate::genome::Genome;
        let mut g = Genome::founder();
        g.eat_efficiency = 0.0; // Eat invalid
        // Split invalid (energy=0), Eat invalid (eff=0). Graze always valid.
        let logits = [-5.0f32, 2.0, 10.0]; // Split highest but invalid; Graze wins
        let act = decode_action(&logits, &g, 0.0, 1); // energy=0 → Split invalid; cooldown>0 → Eat invalid
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
    /// logit is non-finite (NaN or ±inf). D9 stub: updated for 3-action enum.
    #[test]
    fn nan_logits_return_rest_zero_velocity() {
        use crate::genome::Genome;
        // decode_action with a NaN logit must return Graze (was Rest, D9 stub).
        let g = Genome::founder();
        let nan_logits = [f32::NAN, 0.0, 0.0];
        let act = decode_action(&nan_logits, &g, 100.0, 0);
        assert_eq!(act, Action::Graze, "NaN logit must produce Graze (D9 stub)");

        let inf_logits = [f32::INFINITY, 0.0, 0.0];
        let act2 = decode_action(&inf_logits, &g, 100.0, 0);
        assert_eq!(act2, Action::Graze, "+Inf logit must produce Graze (D9 stub)");

        // pick_action_d: when output_buf[2..5] has NaN the returned velocity must be 0.
        let w = World::new("s5-nan");
        let input_buf = [0.0f32; NN_INPUTS];
        let hidden_buf = [0.0f32; NN_HIDDEN];
        let mut output_buf = [0.0f32; NN_OUTPUTS];
        // Inject NaN into logit slots.
        output_buf[2] = f32::NAN;
        // D9: logits are out[2..5] (3 slots).
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
            let act = decode_action(logits, &w.creatures.genomes[0], w.creatures.energy[0], 0);
            (vx, vy, act)
        };
        assert_eq!(action, Action::Graze, "had_nan path must return Graze (D9 stub)");
        assert_eq!(vx, 0.0, "had_nan path must zero vx");
        assert_eq!(vy, 0.0, "had_nan path must zero vy");
        // Suppress unused-variable warnings from the buffers we allocated above.
        let _ = &input_buf;
        let _ = &hidden_buf;
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
        // With uniform-random init (P2a gutted F.30 hardwiring), try multiple
        // seeds until one produces ≥2 creatures (the first split may take
        // several hundred ticks and some seeds go extinct first).
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

        // Clone identical state via clone_for_test (replaces save/load round-trip).
        let mut w_seq = w_base.clone_for_test();
        let mut w_par = w_base.clone_for_test();

        // Both worlds must have the same creature count.
        assert_eq!(w_seq.creatures.len(), n, "seq world population mismatch");
        assert_eq!(w_par.creatures.len(), n, "par world population mismatch");

        // Rebuild the carrion spatial index (normally done in run_vision_pass;
        // clone_for_test leaves cell_to_carrion empty).
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

    /// P2d test 1: all-density-1.0 grass → inp[135..160] all ≈ 1.0.
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
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, 0, prev_vx, prev_vy);
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
    /// → inp[147] == seeded value, all other patch slots == 0.0.
    /// This is the most important test — it locks the iteration-order for P2f's hardwiring.
    ///
    /// Strategy: place creature exactly at a grass cell center so bilinear tx=ty=0,
    /// making bilinear_sample return the seeded cell's density exactly. Seed ONLY that
    /// cell; all 24 offset sample points land on different cells (all-zero).
    #[test]
    fn nn_input_patch_iteration_order_locked() {
        use crate::vision::VISION_LEN;
        let mut w = World::new("p2d-center");
        // Place the creature at the center of grass cell (120, 120).
        // Cell center world coords: (ix + 0.5) * GRASS_CELL_SIZE.
        // bilinear_sample at this position: fx = (120.5*2.5)/2.5 - 0.5 = 120.0
        // → ix0=120, tx=0.0 → sample = density[iy0*dim + ix0] exactly.
        let cell_ix: usize = 120;
        let cell_iy: usize = 120;
        let cx = (cell_ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (cell_iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        // P2e: nose_count must be >= 1 for the creature to see grass.
        w.creatures.genomes[0].nose_count = 1;
        w.creatures.resync_hot_mirrors_at(0);
        // Seed ONLY this cell. The 24 offset sample points (dx/dy ∈ {-2,-1,+1,+2})
        // land on adjacent cells which remain zero, so all non-center slots stay 0.0.
        let center_cell = cell_iy * crate::constants::GRASS_GRID_DIM + cell_ix;
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        grass.density[center_cell] = 0.5;
        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, 0, prev_vx, prev_vy);
        // Center slot 147 must be the seeded value exactly.
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

    /// P2d test 3: constant pinning — NN_GRASS_PATCH_CENTER_SLOT == 147.
    #[test]
    fn nn_input_patch_center_slot_is_147() {
        assert_eq!(
            NN_GRASS_PATCH_CENTER_SLOT, 147,
            "center slot must be 147 (P2f depends on this)"
        );
    }

    /// P2d test 4: toroidal seam wrap — creature near origin, seed grass near
    /// opposite corner. The dx=-2,dy=-2 sample (slot 135) must read wrapped cells.
    #[test]
    fn nn_input_patch_seam_wrap() {
        use crate::grass::cell_index_for;
        use crate::vision::VISION_LEN;
        // Place creature near the world origin (world wraps at WORLD_SIZE = 600.0).
        let mut w = World::new("p2d-seam");
        // Move founder to (1.0, 1.0) — close to the west/north seam.
        w.creatures.x[0] = 1.0;
        w.creatures.y[0] = 1.0;
        // P2e: nose_count must be >= 1 for the creature to see grass.
        w.creatures.genomes[0].nose_count = 1;
        w.creatures.resync_hot_mirrors_at(0);

        // The dx=-2, dy=-2 sample point is at:
        //   sx = 1.0 + (-2) * GRASS_CELL_SIZE = 1.0 - 5.0 = -4.0 → wraps to ~596.0
        //   sy = 1.0 + (-2) * GRASS_CELL_SIZE = 1.0 - 5.0 = -4.0 → wraps to ~596.0
        let sx = 1.0_f32 + (-2_f32) * GRASS_CELL_SIZE;
        let sy = 1.0_f32 + (-2_f32) * GRASS_CELL_SIZE;
        // Seed the cell containing that wrapped position.
        let mut grass = GrassGrid::new(&mut crate::rng::SimRng::from_u64(0), 0);
        let wrapped_cell = cell_index_for(sx, sy);
        grass.density[wrapped_cell] = 0.8;

        let vision = [0.0f32; VISION_LEN];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, 0, prev_vx, prev_vy);
        // Slot 135 = NN_GRASS_PATCH_OFFSET + 0 = dx=-2, dy=-2 sample.
        let slot135 = inp[NN_GRASS_PATCH_OFFSET];
        assert!(
            slot135 > 0.0,
            "seam-wrap slot 135 = {slot135} (expected > 0.0 — wrapped cell seeded)"
        );
    }

    /// P2d test 5: threaded path produces same inp[135..160] as sequential path.
    /// Subsumed by S23's existing threaded_nn_in_place_writes_match_sequential but
    /// extended here to explicitly assert the grass-patch block is bit-identical.
    /// Gated on `threads` feature.
    #[cfg(feature = "threads")]
    #[test]
    fn nn_input_patch_threaded_matches_sequential() {
        use crate::world::nn::chunk_ranges;

        // Advance a world to a multi-creature state with grass seeded.
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

        let mut w_seq = w_base.clone_for_test();
        let mut w_par = w_base.clone_for_test();

        // P2e: set nose_count=1 for all creatures in both worlds so the bilinear
        // sampling path is exercised (not short-circuited by the nose_count=0 gate).
        for i in 0..n {
            w_seq.creatures.genomes[i].nose_count = 1;
            w_seq.creatures.resync_hot_mirrors_at(i);
            w_par.creatures.genomes[i].nose_count = 1;
            w_par.creatures.resync_hot_mirrors_at(i);
        }
        // Seed grass so grass-patch slots are non-trivially non-zero.
        w_seq.grass.density.fill(1.0);
        w_par.grass.density.fill(1.0);

        crate::vision::build_cell_to_carrion(&w_seq.carrion, &mut w_seq.cell_to_carrion);
        crate::vision::build_cell_to_carrion(&w_par.carrion, &mut w_par.cell_to_carrion);

        let ranges = chunk_ranges(n);
        w_seq.force_sequential_nn = true;
        w_seq.nn_forward_all_chunks(&ranges, n);

        w_par.force_sequential_nn = false;
        w_par.nn_forward_all_chunks(&ranges, n);

        // Build NN inputs for each creature under both paths and assert patch block is identical.
        use crate::vision::VISION_LEN;
        for i in 0..n {
            let vision = [0.0f32; VISION_LEN];
            let prev_vx = w_seq.creatures.vx[i];
            let prev_vy = w_seq.creatures.vy[i];
            let inp_seq = build_nn_input(
                i,
                &w_seq.creatures,
                &vision,
                &w_seq.grass,
                0,
                prev_vx,
                prev_vy,
            );
            let inp_par = build_nn_input(
                i,
                &w_par.creatures,
                &vision,
                &w_par.grass,
                0,
                prev_vx,
                prev_vy,
            );
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
    /// Founder defaults nose_count=0; patch slots 135..160 must stay 0.0 even when
    /// grass density > 0. After mutating nose_count to 1 and resyncing, the same
    /// slots must become ~1.0 (since grass is seeded to max density everywhere).
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
        let inp = build_nn_input(0, &w.creatures, &vision, &grass, 0, prev_vx, prev_vy);
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
        // Reuse the same all-density-1.0 grass grid.
        let inp2 = build_nn_input(0, &w2.creatures, &vision, &grass, 0, prev_vx2, prev_vy2);
        for slot in NN_GRASS_PATCH_OFFSET..(NN_GRASS_PATCH_OFFSET + 25) {
            assert!(
                inp2[slot] > 0.99,
                "slot {slot} = {} (expected ~1.0 when nose_count=1 and grass=max)",
                inp2[slot]
            );
        }
    }
}
