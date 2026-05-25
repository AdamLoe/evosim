//! NN forward pass and action-decode helpers. Both sequential and threaded paths live here.

use super::World;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::genome::Genome;
use crate::vision::VisionBuf;
#[cfg(feature = "threads")]
use rayon::prelude::*;

impl World {
    /// Run the NN forward pass for all creatures across fixed chunks (v6 §J).
    /// Sequential by default; rayon-parallel behind `cfg(feature = "threads")`.
    /// Results are bit-identical because the forward pass contains no RNG.
    pub(crate) fn nn_forward_all_chunks(&mut self, ranges: &[(usize, usize); N_CHUNKS], n: usize) {
        let _ = n; // used in threads path; suppress unused-variable in default build
        #[cfg(not(feature = "threads"))]
        {
            for &(lo, hi) in ranges {
                let mut input_buf = [0.0f32; NN_INPUTS];
                let mut hidden_buf = [0.0f32; NN_HIDDEN];
                let mut output_buf = [0.0f32; NN_OUTPUTS];
                for i in lo..hi {
                    let overlap = self.count_carrion_overlap(i);
                    let is_at_wall = self.compute_is_at_wall(i);
                    let (vx, vy, action) = pick_action_d(
                        i,
                        &mut input_buf,
                        &mut hidden_buf,
                        &mut output_buf,
                        &self.creatures,
                        &self.vision[i],
                        overlap,
                        is_at_wall,
                    );
                    self.creatures.vx[i] = vx;
                    self.creatures.vy[i] = vy;
                    self.creatures.action_this_tick[i] = action;
                }
            }
        }

        #[cfg(feature = "threads")]
        {
            // Collect (vx, vy, action) per creature into a flat Vec,
            // then drain back into SoA. Avoids shared-mutable-SoA hazards.
            // Read-only refs are Sync; write happens after join.
            let creatures_ref = &self.creatures;
            let vision_ref = &self.vision[..n];
            let carrion_ref = &self.carrion;
            let cell_to_carrion_ref = &self.cell_to_carrion;

            // Produce results in chunk order (deterministic).
            let results: Vec<(f32, f32, Action)> = ranges
                .par_iter()
                .flat_map(|&(lo, hi)| {
                    let mut input_buf = [0.0f32; NN_INPUTS];
                    let mut hidden_buf = [0.0f32; NN_HIDDEN];
                    let mut output_buf = [0.0f32; NN_OUTPUTS];
                    (lo..hi)
                        .map(|i| {
                            // Inline carrion overlap + is_at_wall for the threaded path.
                            let xi = creatures_ref.x[i];
                            let yi = creatures_ref.y[i];
                            let ri = creatures_ref.g_size[i] * BODY_RADIUS_PER_SIZE; // perf-5: mirror
                            let r2 = ri * ri;
                            let cx_cell = (xi / HASH_CELL).floor() as i32;
                            let cy_cell = (yi / HASH_CELL).floor() as i32;
                            let dim = HASH_DIM as i32;
                            let mut overlap = 0u32;
                            for dy in -1i32..=1 {
                                for dx in -1i32..=1 {
                                    let nx = cx_cell + dx;
                                    let ny = cy_cell + dy;
                                    if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                                        continue;
                                    }
                                    let cell_idx = ny as usize * HASH_DIM + nx as usize;
                                    for &ci in &cell_to_carrion_ref[cell_idx] {
                                        let c = &carrion_ref[ci as usize];
                                        let ddx = c.x - xi;
                                        let ddy = c.y - yi;
                                        if ddx * ddx + ddy * ddy <= r2 {
                                            overlap += 1;
                                        }
                                    }
                                }
                            }
                            let near = xi.min(yi).min(WORLD_SIZE - xi).min(WORLD_SIZE - yi);
                            let is_at_wall = if near < ri + WALL_THRESHOLD_PAD {
                                1.0f32
                            } else {
                                0.0f32
                            };
                            pick_action_d(
                                i,
                                &mut input_buf,
                                &mut hidden_buf,
                                &mut output_buf,
                                creatures_ref,
                                &vision_ref[i],
                                overlap,
                                is_at_wall,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect();

            for (i, (vx, vy, action)) in results.into_iter().enumerate() {
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
pub(crate) fn chunk_ranges(n: usize) -> [(usize, usize); N_CHUNKS] {
    let base = n.div_ceil(N_CHUNKS);
    let mut out = [(0usize, 0usize); N_CHUNKS];
    for k in 0..N_CHUNKS {
        let lo = (k * base).min(n);
        let hi = ((k + 1) * base).min(n);
        out[k] = (lo, hi);
    }
    out
}

/// Build the 136-float NN input vector for creature `i` (v6 §E, Milestone D.16).
///
/// Layout:
/// - `[0..10]` self-state (energy_frac, age_frac, size, vx, vy, is_at_wall,
///   cooldown_frac, carrion_overlap_norm, reserved×2)
/// - `[10..130]` vision passthrough (raw, C.12 VisionBuf)
/// - `[130..136]` last_action one-hot
pub(crate) fn build_nn_input(
    i: usize,
    creatures: &CreatureSoA,
    vision: &VisionBuf,
    carrion_overlap_count: u32,
    is_at_wall_flag: f32,
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
    buf[3] = creatures.vx[i] / MOVE_SPEED_MAX; // vx (previous-tick post-clip)
    buf[4] = creatures.vy[i] / MOVE_SPEED_MAX; // vy
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
        Action::Rest | Action::Photosynth | Action::Signal => true,
        Action::Eat => genome.eat_efficiency > 0.0 && cooldown == 0,
        Action::Scavenge => genome.scavenge_efficiency > 0.0,
        Action::Split => energy >= SPLIT_THRESHOLD,
    }
}

/// Decode 6 action logits to an Action via argmax with first-index tiebreak
/// and valid-fallthrough (v6 §1 + §E).
pub(crate) fn decode_action(
    logits: &[f32; 6],
    genome: &Genome,
    energy: f32,
    cooldown: u32,
) -> Action {
    use std::cmp::Ordering;
    // Sort Action::ALL indices by logit DESC, first-index tiebreak on ties.
    let mut order = [0u8, 1, 2, 3, 4, 5];
    order.sort_by(|&a, &b| {
        logits[b as usize]
            .partial_cmp(&logits[a as usize])
            .unwrap_or(Ordering::Equal) // NaN treated as equal → index tiebreak
            .then(a.cmp(&b)) // lower index wins on tie (stable first-index)
    });
    for &k in &order {
        let act = Action::ALL[k as usize];
        if is_valid_action(act, genome, energy, cooldown) {
            return act;
        }
    }
    Action::Rest // defensive fallback (Rest/Photosynth/Signal are always valid)
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
) -> (f32, f32, Action) {
    *input_buf = build_nn_input(i, creatures, vision, carrion_overlap_count, is_at_wall_flag);
    creatures.brains[i].forward(input_buf, output_buf, hidden_buf);

    // Velocity: tanh(out[0..2]) × move_speed (v6 §E).
    let speed = creatures.g_move_speed[i]; // perf-5: mirror
    let vx = output_buf[0].tanh() * speed;
    let vy = output_buf[1].tanh() * speed;

    // Action: valid-fallthrough argmax over logits out[2..8] (v6 §1).
    let logits: &[f32; 6] = output_buf[2..8].try_into().unwrap();
    let energy = creatures.energy[i];
    let cooldown = creatures.digestion_cooldown[i];
    // decode_action takes &Genome (reads eat_eff/scav_eff). Hot, but kept on AoS to avoid API churn; one read/creature/tick.
    let action = decode_action(logits, &creatures.genomes[i], energy, cooldown);

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
        let inp = build_nn_input(0, &w.creatures, &vision, 2, 0.0);
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
        let inp = build_nn_input(0, &w.creatures, &vis, 0, 0.0);
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
        let inp = build_nn_input(0, &w.creatures, &vision, 0, 0.0);
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
                Action::Rest | Action::Photosynth | Action::Eat | Action::Scavenge | Action::Signal
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
        let logits = [-5.0f32, 2.0, -3.0, -3.0, 10.0, 3.0]; // Split>Signal>Photo>...
        let act = decode_action(&logits, &g, 0.0, 1); // energy=0 → Split invalid; cooldown>0 → Eat invalid
                                                      // Photosynth (idx 1, logit=2) and Signal (idx 5, logit=3) are both valid;
                                                      // Signal has higher logit so it wins.
        assert!(
            matches!(act, Action::Photosynth | Action::Signal | Action::Rest),
            "Expected always-valid action, got {:?}",
            act
        );
    }
}
