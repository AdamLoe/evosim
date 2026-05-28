//! NN forward pass and action-decode helpers. Both sequential and threaded paths live here.
//! v1.3 D9: Action enum collapsed to {Graze=0, Eat=1, Split=2}. NN_OUTPUTS=5.
//! decode_action/is_valid_action take no Genome param (genome removed in D3).
//! v1.5 S5b: build_nn_input rewritten to 32-input semantic layout; vision deleted.

use super::proximity;
use super::World;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::grass::GrassGrid;
use crate::grid::SpatialGrid;
#[cfg(feature = "threads")]
use rayon::prelude::*;

impl World {
    /// Run the NN forward pass for all creatures across fixed chunks (v6 §J).
    /// Sequential by default; rayon-parallel behind `cfg(feature = "threads")`.
    /// Results are bit-identical because the forward pass contains no RNG.
    pub(crate) fn nn_forward_all_chunks(&mut self, ranges: &[(usize, usize)], n: usize) {
        // Threaded path: runs when compiled with --features threads.
        // Returns early so the sequential fallthrough below does not run.
        #[cfg(feature = "threads")]
        {
            // Detach the output columns via mem::take so the parallel block can
            // mutate them disjointly while &self.creatures is borrowed read-only
            // for input reads. Restored after the parallel work.
            let mut vx_local = std::mem::take(&mut self.creatures.vx);
            let mut vy_local = std::mem::take(&mut self.creatures.vy);
            let mut act_local = std::mem::take(&mut self.creatures.action_this_tick);
            // v1.5 S3: argmax-pre column lives on World, not CreatureSoA.
            let mut argmax_pre_local = std::mem::take(&mut self.scratch_argmax_pre);
            // v1.5 S5b: per-creature sector accumulator scratch (8 floats each).
            let mut sector_local = std::mem::take(&mut self.scratch_sector_accum);

            {
                let creatures_ref = &self.creatures;
                let grass_ref = &self.grass;
                let grid_ref = &self.grid;
                let sector_lut_ref = &self.sector_lut[..];
                let energy_max = self.sliders.energy_max;
                let split_threshold = self.sliders.split_threshold;
                let max_age = self.sliders.max_age;

                let chunk_size = chunk_base_size_from_ranges(ranges, n);

                vx_local[..n]
                    .par_chunks_mut(chunk_size)
                    .zip(vy_local[..n].par_chunks_mut(chunk_size))
                    .zip(act_local[..n].par_chunks_mut(chunk_size))
                    .zip(argmax_pre_local[..n].par_chunks_mut(chunk_size))
                    .zip(sector_local[..n].par_chunks_mut(chunk_size))
                    .enumerate()
                    .for_each(
                        |(chunk_idx, ((((vx_sub, vy_sub), act_sub), arg_sub), sec_sub))| {
                            debug_assert_eq!(vx_sub.len(), vy_sub.len());
                            debug_assert_eq!(vx_sub.len(), act_sub.len());
                            debug_assert_eq!(vx_sub.len(), arg_sub.len());
                            debug_assert_eq!(vx_sub.len(), sec_sub.len());

                            let mut input_buf = [0.0f32; NN_INPUTS];
                            let mut hidden_buf_1 = [0.0f32; NN_HIDDEN_1];
                            let mut hidden_buf_2 = [0.0f32; NN_HIDDEN_2];
                            let mut output_buf = [0.0f32; NN_OUTPUTS];
                            let lo = chunk_idx * chunk_size;

                            for k in 0..vx_sub.len() {
                                let i = lo + k;
                                let prev_vx = vx_sub[k];
                                let prev_vy = vy_sub[k];
                                let (vx, vy, action, argmax_pre) = pick_action_d(
                                    i,
                                    &mut input_buf,
                                    &mut hidden_buf_1,
                                    &mut hidden_buf_2,
                                    &mut output_buf,
                                    creatures_ref,
                                    grass_ref,
                                    grid_ref,
                                    sector_lut_ref,
                                    &mut sec_sub[k],
                                    prev_vx,
                                    prev_vy,
                                    energy_max,
                                    split_threshold,
                                    max_age,
                                );
                                vx_sub[k] = vx;
                                vy_sub[k] = vy;
                                act_sub[k] = action;
                                arg_sub[k] = argmax_pre;
                            }
                        },
                    );
            } // read-only borrows of &self.creatures etc. dropped here

            self.creatures.vx = vx_local;
            self.creatures.vy = vy_local;
            self.creatures.action_this_tick = act_local;
            self.scratch_argmax_pre = argmax_pre_local;
            self.scratch_sector_accum = sector_local;
        }

        // Sequential fallthrough: runs in default builds only.
        #[cfg(not(feature = "threads"))]
        {
            let _ = n; // unused in the sequential path
            let energy_max = self.sliders.energy_max;
            let split_threshold = self.sliders.split_threshold;
            let max_age = self.sliders.max_age;
            for &(lo, hi) in ranges {
                let mut input_buf = [0.0f32; NN_INPUTS];
                let mut hidden_buf_1 = [0.0f32; NN_HIDDEN_1];
                let mut hidden_buf_2 = [0.0f32; NN_HIDDEN_2];
                let mut output_buf = [0.0f32; NN_OUTPUTS];
                for i in lo..hi {
                    let prev_vx = self.creatures.vx[i];
                    let prev_vy = self.creatures.vy[i];
                    let (vx, vy, action, argmax_pre) = pick_action_d(
                        i,
                        &mut input_buf,
                        &mut hidden_buf_1,
                        &mut hidden_buf_2,
                        &mut output_buf,
                        &self.creatures,
                        &self.grass,
                        &self.grid,
                        &self.sector_lut,
                        &mut self.scratch_sector_accum[i],
                        prev_vx,
                        prev_vy,
                        energy_max,
                        split_threshold,
                        max_age,
                    );
                    self.creatures.vx[i] = vx;
                    self.creatures.vy[i] = vy;
                    self.creatures.action_this_tick[i] = action;
                    self.scratch_argmax_pre[i] = argmax_pre;
                }
            }
        } // end #[cfg(not(feature = "threads"))]
    }
}

/// Dynamic chunk count: `clamp(pop/32, MIN_CHUNKS, min(MAX_CHUNKS, workers))`.
/// `worker_count` is rayon's `current_num_threads()` under `--features threads`,
/// else 1.
pub(crate) fn dynamic_chunks(pop: usize, worker_count: usize) -> usize {
    let upper = MAX_CHUNKS.min(worker_count.max(1)).max(MIN_CHUNKS);
    (pop / 32).clamp(MIN_CHUNKS, upper)
}

/// Stride used by `par_chunks_mut`. `ranges` is the partition the sequential
/// path walks; this returns the chunk_size that reproduces it.
#[cfg_attr(not(feature = "threads"), allow(dead_code))]
pub(crate) fn chunk_base_size_from_ranges(ranges: &[(usize, usize)], n: usize) -> usize {
    // ceil-sized chunks: stride = first non-empty range length, or n.div_ceil(k).
    let k = ranges.len().max(1);
    n.div_ceil(k).max(1)
}

/// Compute non-overlapping index ranges that partition `0..n` (v6 §J).
pub(crate) fn chunk_ranges(n: usize, chunks: usize) -> Vec<(usize, usize)> {
    let chunks = chunks.max(1);
    let base = n.div_ceil(chunks).max(1);
    let mut out = Vec::with_capacity(chunks);
    for k in 0..chunks {
        let lo = (k * base).min(n);
        let hi = ((k + 1) * base).min(n);
        out.push((lo, hi));
    }
    debug_assert_eq!(out[0].0, 0, "chunk_ranges: first lo must be 0");
    debug_assert_eq!(
        out.last().unwrap().1,
        n,
        "chunk_ranges: last hi must equal n"
    );
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

/// Build the 32-input NN input vector for creature `i` (v1.5 S5b).
///
/// Layout (NN_INPUTS=32):
/// - `[0..8)`   self/memory
///   - `[0]` hunger = 1 - energy/energy_max
///   - `[1]` age_frac = age / max_age
///   - `[2]` prev_vx / MOVE_SPEED_MAX
///   - `[3]` prev_vy / MOVE_SPEED_MAX
///   - `[4]` is_last_graze (1.0 if last_action == Graze)
///   - `[5]` is_last_eat   (1.0 if last_action == Eat)
///   - `[6]` ticks_since_split / max_age (clamped [0,1])
///   - `[7]` cooldown_ready (1.0 when digestion_cooldown == 0)
/// - `[8..12)`  wall_proximity N/S/E/W (range WALL_PROXIMITY_RANGE)
/// - `[12..20)` creature_proximity 8 sectors (range PROXIMITY_RANGE)
/// - `[20..28)` grass_density 8 sectors (range PROXIMITY_RANGE)
/// - `[28]`     padding (0.0)            — reserved for predator-color (v1.6+)
/// - `[29]`     curr_grass_density (bilinear sample at body center)
/// - `[30]`     1.0 (bias-learning constant)
/// - `[31]`     padding (0.0)            — SIMD alignment to 32 = 4 × 8
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_nn_input(
    i: usize,
    creatures: &CreatureSoA,
    grass: &GrassGrid,
    grid: &SpatialGrid,
    sector_lut: &[(u8, f32)],
    sector_scratch: &mut [f32; 8],
    prev_vx: f32,
    prev_vy: f32,
    energy_max: f32,
    max_age: u32,
) -> [f32; NN_INPUTS] {
    let mut buf = [0.0f32; NN_INPUTS];
    let energy = creatures.energy[i];
    let age = creatures.age[i];
    let cooldown = creatures.digestion_cooldown[i];
    let x = creatures.x[i];
    let y = creatures.y[i];

    // --- [0..8) self/memory ---
    let energy_frac = (energy / energy_max.max(1.0)).clamp(0.0, 1.0);
    buf[0] = (1.0 - energy_frac).clamp(0.0, 1.0); // hunger
    let max_age_f = (max_age.max(1)) as f32;
    buf[1] = (age as f32 / max_age_f).clamp(0.0, 1.0); // age_frac
    buf[2] = prev_vx / MOVE_SPEED_MAX;
    buf[3] = prev_vy / MOVE_SPEED_MAX;
    let la = creatures.last_action[i];
    buf[4] = if matches!(la, Action::Graze) {
        1.0
    } else {
        0.0
    };
    buf[5] = if matches!(la, Action::Eat) { 1.0 } else { 0.0 };
    buf[6] = (creatures.ticks_since_split[i] as f32 / max_age_f).clamp(0.0, 1.0);
    buf[7] = if cooldown == 0 { 1.0 } else { 0.0 };

    // --- [8..12) wall_proximity ---
    let walls = proximity::compute_wall_proximity(x, y);
    buf[NN_WALL_OFFSET] = walls[0];
    buf[NN_WALL_OFFSET + 1] = walls[1];
    buf[NN_WALL_OFFSET + 2] = walls[2];
    buf[NN_WALL_OFFSET + 3] = walls[3];

    // --- [12..20) creature_proximity sectors ---
    proximity::compute_creature_proximity_sectors(x, y, i, grid, creatures, sector_scratch);
    buf[NN_CREATURE_SECTOR_OFFSET..NN_CREATURE_SECTOR_OFFSET + NN_SECTORS]
        .copy_from_slice(&sector_scratch[..NN_SECTORS]);

    // --- [20..28) grass_density sectors ---
    proximity::compute_grass_density_sectors(x, y, grass, sector_lut, sector_scratch);
    buf[NN_GRASS_SECTOR_OFFSET..NN_GRASS_SECTOR_OFFSET + NN_SECTORS]
        .copy_from_slice(&sector_scratch[..NN_SECTORS]);

    // --- [28] padding ---
    buf[28] = 0.0;

    // --- [29] curr_grass_density ---
    buf[NN_CURR_GRASS_SLOT] = grass.bilinear_sample(x, y);

    // --- [30] bias ---
    buf[NN_BIAS_SLOT] = 1.0;

    // --- [31] padding ---
    buf[31] = 0.0;

    buf
}

/// Check whether an action is currently valid for creature `i` (v6 §1 + §G).
/// D9: 3-variant enum. Graze always valid; Eat gated on cooldown; Split on energy.
pub(crate) fn is_valid_action(
    act: Action,
    energy: f32,
    cooldown: u32,
    split_threshold: f32,
) -> bool {
    match act {
        Action::Graze => true,
        Action::Eat => cooldown == 0,
        Action::Split => energy >= split_threshold,
    }
}

/// Decode action logits to an Action via argmax with first-index tiebreak
/// and valid-fallthrough (v6 §1 + §E).
pub(crate) fn decode_action(
    logits: &[f32; 3],
    energy: f32,
    cooldown: u32,
    split_threshold: f32,
) -> Action {
    if logits.iter().any(|v| !v.is_finite()) {
        return Action::Graze;
    }
    use std::cmp::Ordering;
    let mut order = [0u8, 1, 2];
    order.sort_by(|&a, &b| {
        logits[b as usize]
            .partial_cmp(&logits[a as usize])
            .unwrap_or(Ordering::Equal)
            .then(a.cmp(&b))
    });
    for &k in &order {
        let act = Action::ALL[k as usize];
        if is_valid_action(act, energy, cooldown, split_threshold) {
            return act;
        }
    }
    Action::Graze
}

/// NN-driven action picker (v1.5 S5b).
///
/// Builds the 32-input vector, runs the forward pass, decodes velocity via
/// tanh (applied inside Brain::forward on slots 0..2) and action via
/// valid-fallthrough argmax over logit slots 2..5.
///
/// Returns `(vx, vy, action, argmax_pre)`. `argmax_pre` is the index (0/1/2)
/// of the highest action logit BEFORE the validity-fallthrough step.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_action_d(
    i: usize,
    input_buf: &mut [f32; NN_INPUTS],
    hidden_buf_1: &mut [f32; NN_HIDDEN_1],
    hidden_buf_2: &mut [f32; NN_HIDDEN_2],
    output_buf: &mut [f32; NN_OUTPUTS],
    creatures: &CreatureSoA,
    grass: &GrassGrid,
    grid: &SpatialGrid,
    sector_lut: &[(u8, f32)],
    sector_scratch: &mut [f32; 8],
    prev_vx: f32,
    prev_vy: f32,
    energy_max: f32,
    split_threshold: f32,
    max_age: u32,
) -> (f32, f32, Action, u8) {
    *input_buf = build_nn_input(
        i,
        creatures,
        grass,
        grid,
        sector_lut,
        sector_scratch,
        prev_vx,
        prev_vy,
        energy_max,
        max_age,
    );
    creatures.brains[i].forward(input_buf, output_buf, hidden_buf_1, hidden_buf_2);

    // Velocity: out[0..2] are tanh-activated in Brain::forward.
    let vx = output_buf[0] * MOVE_SPEED_MAX;
    let vy = output_buf[1] * MOVE_SPEED_MAX;

    let logits: &[f32; 3] = output_buf[2..5].try_into().unwrap();
    debug_assert!(
        logits.iter().all(|v| v.is_finite()),
        "non-finite logits for creature {i}: {logits:?}"
    );

    let argmax_pre: u8 = if logits.iter().any(|v| !v.is_finite()) {
        0
    } else {
        let mut best = 0u8;
        for k in 1u8..3 {
            if logits[k as usize] > logits[best as usize] {
                best = k;
            }
        }
        best
    };

    let energy = creatures.energy[i];
    let cooldown = creatures.digestion_cooldown[i];
    let action = decode_action(logits, energy, cooldown, split_threshold);

    let had_nan = logits.iter().any(|v| !v.is_finite());
    if had_nan {
        return (0.0, 0.0, Action::Graze, argmax_pre);
    }

    (vx, vy, action, argmax_pre)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NN_INPUTS contract for the 32-input semantic layout (v1.5 S5b).
    #[test]
    fn nn_input_layout_size_is_32() {
        assert_eq!(NN_INPUTS, 32);
        assert_eq!(
            NN_INPUTS % 8,
            0,
            "NN_INPUTS must be padded to multiple of 8"
        );
    }

    /// Helper: build inputs for the founder of a fresh world.
    fn build_for_founder(w: &mut World) -> [f32; NN_INPUTS] {
        let mut scratch = [0.0f32; 8];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let energy_max = w.sliders.energy_max;
        let max_age = w.sliders.max_age;
        build_nn_input(
            0,
            &w.creatures,
            &w.grass,
            &w.grid,
            &w.sector_lut,
            &mut scratch,
            prev_vx,
            prev_vy,
            energy_max,
            max_age,
        )
    }

    /// Self-state slots: hunger and age_frac with known values.
    #[test]
    fn nn_input_layout_self_state() {
        let mut w = World::new("s5b-self");
        w.creatures.energy[0] = 50.0; // energy_frac=0.5 → hunger=0.5
        w.creatures.age[0] = w.sliders.max_age / 2; // age_frac=0.5
        w.sliders.energy_max = 100.0;
        let inp = build_for_founder(&mut w);
        assert!((inp[0] - 0.5).abs() < 1e-5, "hunger = {}", inp[0]);
        assert!((inp[1] - 0.5).abs() < 1e-5, "age_frac = {}", inp[1]);
    }

    /// Slot 28 is padding — always 0.0 (user override: no predator-color input).
    #[test]
    fn nn_input_slot_28_is_padding() {
        let mut w = World::new("s5b-pad28");
        let inp = build_for_founder(&mut w);
        assert_eq!(inp[28], 0.0, "slot 28 must be padding");
    }

    /// Slot 30 is the bias-learning constant — always 1.0.
    #[test]
    fn nn_input_slot_30_is_one() {
        let mut w = World::new("s5b-bias");
        let inp = build_for_founder(&mut w);
        assert_eq!(inp[NN_BIAS_SLOT], 1.0, "bias slot must be 1.0");
    }

    /// Wall-proximity at corner: N≈0.98, W≈0.98 (creature at (1,1), range 50).
    #[test]
    fn nn_input_layout_wall_proximity_corner() {
        let mut w = World::new("s5b-walls");
        w.creatures.x[0] = 1.0;
        w.creatures.y[0] = 1.0;
        let inp = build_for_founder(&mut w);
        assert!(
            (inp[NN_WALL_OFFSET] - 0.98).abs() < 1e-3,
            "N = {}",
            inp[NN_WALL_OFFSET]
        );
        assert!(
            (inp[NN_WALL_OFFSET + 3] - 0.98).abs() < 1e-3,
            "W = {}",
            inp[NN_WALL_OFFSET + 3]
        );
        // S/E should be 0 (creature is far from those walls).
        assert_eq!(inp[NN_WALL_OFFSET + 1], 0.0);
        assert_eq!(inp[NN_WALL_OFFSET + 2], 0.0);
    }

    /// Empty grass grid → all 8 grass-density sector slots are 0.
    #[test]
    fn nn_input_grass_sector_empty_grid_is_zero() {
        let mut w = World::new("s5b-grass-empty");
        w.grass.density.fill(0.0);
        w.grass.rebuild_row_bitset();
        let inp = build_for_founder(&mut w);
        for s in 0..NN_SECTORS {
            assert_eq!(
                inp[NN_GRASS_SECTOR_OFFSET + s],
                0.0,
                "grass sector {s} must be 0 on empty grid"
            );
        }
    }

    // ---- decode_action tests (kept; layout-independent) ----

    #[test]
    fn decode_action_valid_fallthrough_split_invalid() {
        let logits = [0.0f32, 0.0, 10.0];
        let act = decode_action(&logits, 10.0, 0, SPLIT_THRESHOLD);
        assert_ne!(act, Action::Split, "Split must be invalid when energy < 50");
        assert!(matches!(act, Action::Graze | Action::Eat), "got {:?}", act);
    }

    #[test]
    fn decode_action_first_index_tiebreak() {
        let logits = [5.0f32; 3];
        let act = decode_action(&logits, 100.0, 0, SPLIT_THRESHOLD);
        assert_eq!(act, Action::ALL[0], "lower index must win on ties");
        assert_eq!(act, Action::Graze, "Graze is index 0 after D9");
    }

    #[test]
    fn decode_action_eat_invalid_in_cooldown() {
        let logits = [0.0f32, 10.0, 0.0];
        let act = decode_action(&logits, 100.0, 5, SPLIT_THRESHOLD);
        assert_ne!(act, Action::Eat, "Eat must be invalid when cooldown > 0");
        assert_eq!(act, Action::Graze, "should fall through to Graze");
    }

    #[test]
    fn decode_action_split_invalid_when_low_energy() {
        let logits = [0.0f32, 0.0, 10.0];
        let act = decode_action(&logits, 0.0, 0, SPLIT_THRESHOLD);
        assert_ne!(
            act,
            Action::Split,
            "Split must be invalid when energy too low"
        );
    }

    #[test]
    fn decode_action_graze_always_valid_as_fallback() {
        let logits = [-5.0f32, 2.0, 10.0];
        let act = decode_action(&logits, 0.0, 1, SPLIT_THRESHOLD);
        assert!(
            matches!(act, Action::Graze),
            "Expected Graze fallback, got {:?}",
            act
        );
    }

    #[test]
    fn nan_logits_return_graze_via_decode() {
        let nan_logits = [f32::NAN, 0.0, 0.0];
        let act = decode_action(&nan_logits, 100.0, 0, SPLIT_THRESHOLD);
        assert_eq!(act, Action::Graze, "NaN logit must produce Graze");

        let inf_logits = [f32::INFINITY, 0.0, 0.0];
        let act2 = decode_action(&inf_logits, 100.0, 0, SPLIT_THRESHOLD);
        assert_eq!(act2, Action::Graze, "+Inf logit must produce Graze");
    }

    // ---- Dynamic chunking tests ----

    #[test]
    fn dynamic_chunks_boundaries() {
        assert_eq!(dynamic_chunks(0, 32), MIN_CHUNKS);
        assert_eq!(dynamic_chunks(1, 32), MIN_CHUNKS);
        assert_eq!(dynamic_chunks(32, 32), MIN_CHUNKS);
        assert_eq!(dynamic_chunks(128, 32), MIN_CHUNKS);
        assert_eq!(dynamic_chunks(160, 32), 5);
        assert_eq!(dynamic_chunks(512, 32), MAX_CHUNKS);
        assert_eq!(dynamic_chunks(2048, 32), MAX_CHUNKS);
        assert_eq!(dynamic_chunks(2048, 1), MIN_CHUNKS);
        assert_eq!(dynamic_chunks(2048, 6), 6);
        assert_eq!(dynamic_chunks(2048, 0), MIN_CHUNKS);
    }

    #[test]
    fn chunk_ranges_partition() {
        for &chunks in &[MIN_CHUNKS, 8usize, MAX_CHUNKS] {
            let ranges = chunk_ranges(1000, chunks);
            assert_eq!(ranges.len(), chunks);
            assert_eq!(ranges[0].0, 0);
            assert_eq!(ranges.last().unwrap().1, 1000);
            let mut total = 0usize;
            for k in 0..chunks {
                let (lo, hi) = ranges[k];
                assert!(lo <= hi, "range {k}: lo={lo} > hi={hi}");
                total += hi - lo;
                if k + 1 < chunks {
                    assert_eq!(hi, ranges[k + 1].0, "gap between chunks {k} and {}", k + 1);
                }
            }
            assert_eq!(total, 1000, "total elements across chunks must equal n");
        }
    }

    #[test]
    fn chunk_ranges_small_population() {
        for n in [0usize, 1, 3, 7] {
            let chunks = MIN_CHUNKS;
            let ranges = chunk_ranges(n, chunks);
            assert_eq!(ranges.len(), chunks);
            let total: usize = ranges.iter().map(|(lo, hi)| hi - lo).sum();
            assert_eq!(total, n, "n={n}: total {total}");
            for &(lo, hi) in &ranges {
                assert!(lo <= hi, "invalid range lo={lo} hi={hi} for n={n}");
            }
            assert_eq!(ranges[0].0, 0);
            assert_eq!(ranges.last().unwrap().1, n);
        }
    }

    #[test]
    fn chunk_ranges_invariants() {
        for &chunks in &[MIN_CHUNKS, 5usize, 8, 12, MAX_CHUNKS] {
            for &n in &[0usize, 1, 7, 8, 9, 32, 100, 128, 1500, 2048] {
                let ranges = chunk_ranges(n, chunks);
                assert_eq!(ranges.len(), chunks);
                assert_eq!(ranges[0].0, 0, "n={n} chunks={chunks}: first lo must be 0");
                assert_eq!(
                    ranges.last().unwrap().1,
                    n,
                    "n={n} chunks={chunks}: last hi must be n"
                );
                for (k, &(lo, hi)) in ranges.iter().enumerate() {
                    assert!(
                        lo <= hi,
                        "n={n} chunks={chunks}: chunk {k}: lo={lo} > hi={hi}"
                    );
                }
                for k in 0..chunks - 1 {
                    assert_eq!(
                        ranges[k].1,
                        ranges[k + 1].0,
                        "n={n} chunks={chunks}: gap between chunks {k} and {}",
                        k + 1
                    );
                }
                let total: usize = ranges.iter().map(|(lo, hi)| hi - lo).sum();
                assert_eq!(total, n, "n={n} chunks={chunks}: total {total} != n");
            }
        }
    }

    /// chunk_ranges partitions [0,n) cleanly and agrees with the par_chunks_mut
    /// stride derived from the same ranges.
    #[test]
    fn chunk_partition_invariants_and_stride_agreement() {
        let cases = [
            (0usize, MIN_CHUNKS),
            (1, MIN_CHUNKS),
            (7, MIN_CHUNKS),
            (32, MIN_CHUNKS),
            (128, MIN_CHUNKS),
            (1500, 8),
            (2048, MAX_CHUNKS),
        ];
        for &(n, chunks) in &cases {
            let ranges = chunk_ranges(n, chunks);
            assert_eq!(ranges.len(), chunks, "n={n}: range count must equal chunks");
            let total: usize = ranges.iter().map(|(lo, hi)| hi - lo).sum();
            assert_eq!(total, n, "n={n}: ranges do not cover [0,n)");
            let base = chunk_base_size_from_ranges(&ranges, n);
            let mut reconstructed = Vec::with_capacity(chunks);
            for k in 0..chunks {
                let lo = (k * base).min(n);
                let hi = ((k + 1) * base).min(n);
                reconstructed.push((lo, hi));
            }
            assert_eq!(
                ranges, reconstructed,
                "n={n}: chunk_ranges disagrees with par_chunks_mut(base) partition"
            );
            if n == 0 {
                assert_eq!(base, 1, "n=0: stride must be 1");
            }
        }
    }
}
