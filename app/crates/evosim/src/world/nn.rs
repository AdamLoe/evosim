//! NN forward pass and action-decode helpers. Both sequential and threaded paths live here.
//! v2.1 P2: Action enum is {Graze=0, Attack=1, Split=2}. NN_OUTPUTS=5.
//! BiomeDir, CurrCellPenalty, and CurrBiomeType are removed; no penalty args.
//! v1.5 S5b: build_nn_input rewritten to semantic layout; vision deleted.

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
        let timed = self.profile.enabled();
        // Reset the per-tick aggregates so this tick's chunks accumulate from 0.
        self.nn_stats.reset_tick();
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
                let layout_ref = &self.nn_input_layout;
                let energy_max = self.sliders.energy_max;
                let split_threshold = self.sliders.split_threshold;
                // v2.0 Wave 3a: species mode + the initiator-only mating cost
                // (= the split energy gift). Both shared read-only across chunks.
                let species_mode = self.sliders.species_mode;
                let mating_cost = self.sliders.split_gift;
                let max_age = self.sliders.max_age;
                let world_size = self.dims.world_size;
                let stats_ref = std::sync::Arc::clone(&self.nn_stats);

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

                            // v2.0: input buffer sized to the MAX_NN_INPUTS
                            // ceiling; only `layout.width()` lanes are active.
                            let mut input_buf = [0.0f32; MAX_NN_INPUTS];
                            // v1.12: stack scratch sized to the hard upper
                            // bound (NN_MAX_HIDDEN_WIDTH * 4B = 1 KB each).
                            // The forward pass uses `..max_width()` slices.
                            let mut scratch_a = [0.0f32; NN_MAX_HIDDEN_WIDTH];
                            let mut scratch_b = [0.0f32; NN_MAX_HIDDEN_WIDTH];
                            let mut output_buf = [0.0f32; NN_OUTPUTS];
                            let lo = chunk_idx * chunk_size;

                            // Always-on: 2 clock reads/chunk. Records which
                            // worker ran this chunk + how many creatures it
                            // processed, regardless of profiler state.
                            let chunk_start_us = crate::profiler::clock_now_us_threadsafe();
                            let mut chunk_pick = PickTimings::default();

                            for k in 0..vx_sub.len() {
                                let i = lo + k;
                                let prev_vx = vx_sub[k];
                                let prev_vy = vy_sub[k];
                                let pick_t = if timed { Some(&mut chunk_pick) } else { None };
                                let (vx, vy, action, argmax_pre) = pick_action_d(
                                    i,
                                    layout_ref,
                                    species_mode,
                                    &mut input_buf,
                                    &mut scratch_a,
                                    &mut scratch_b,
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
                                    mating_cost,
                                    max_age,
                                    world_size,
                                    pick_t,
                                );
                                vx_sub[k] = vx;
                                vy_sub[k] = vy;
                                act_sub[k] = action;
                                arg_sub[k] = argmax_pre;
                            }

                            let chunk_end_us = crate::profiler::clock_now_us_threadsafe();
                            let worker_idx = rayon::current_thread_index().unwrap_or(0);
                            stats_ref.record_chunk_lite(
                                worker_idx,
                                chunk_start_us,
                                chunk_end_us,
                                vx_sub.len() as u64,
                            );
                            if timed {
                                stats_ref.record_chunk_subphases(
                                    vx_sub.len() as u64,
                                    chunk_pick.build_input_total_us,
                                    chunk_pick.build.other_us,
                                    chunk_pick.build.proximity_total_us,
                                    chunk_pick.build.creature_sectors_us,
                                    chunk_pick.build.grass_sectors_us,
                                    chunk_pick.forward_us,
                                    &chunk_pick.forward_layer_us[..],
                                );
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
            // v2.0 Wave 3a: species mode + the initiator-only mating cost.
            let species_mode = self.sliders.species_mode;
            let mating_cost = self.sliders.split_gift;
            let max_age = self.sliders.max_age;
            let world_size = self.dims.world_size;
            let layout = self.nn_input_layout.clone();
            for &(lo, hi) in ranges {
                let mut input_buf = [0.0f32; MAX_NN_INPUTS];
                let mut scratch_a = [0.0f32; NN_MAX_HIDDEN_WIDTH];
                let mut scratch_b = [0.0f32; NN_MAX_HIDDEN_WIDTH];
                let mut output_buf = [0.0f32; NN_OUTPUTS];
                // Always-on chunk telemetry (single virtual worker = idx 0).
                let chunk_start_us = crate::profiler::clock_now_us_threadsafe();
                let mut chunk_pick = PickTimings::default();
                for i in lo..hi {
                    let prev_vx = self.creatures.vx[i];
                    let prev_vy = self.creatures.vy[i];
                    let pick_t = if timed { Some(&mut chunk_pick) } else { None };
                    let (vx, vy, action, argmax_pre) = pick_action_d(
                        i,
                        &layout,
                        species_mode,
                        &mut input_buf,
                        &mut scratch_a,
                        &mut scratch_b,
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
                        mating_cost,
                        max_age,
                        world_size,
                        pick_t,
                    );
                    self.creatures.vx[i] = vx;
                    self.creatures.vy[i] = vy;
                    self.creatures.action_this_tick[i] = action;
                    self.scratch_argmax_pre[i] = argmax_pre;
                }
                let chunk_end_us = crate::profiler::clock_now_us_threadsafe();
                self.nn_stats
                    .record_chunk_lite(0, chunk_start_us, chunk_end_us, (hi - lo) as u64);
                if timed {
                    self.nn_stats.record_chunk_subphases(
                        (hi - lo) as u64,
                        chunk_pick.build_input_total_us,
                        chunk_pick.build.other_us,
                        chunk_pick.build.proximity_total_us,
                        chunk_pick.build.creature_sectors_us,
                        chunk_pick.build.grass_sectors_us,
                        chunk_pick.forward_us,
                        &chunk_pick.forward_layer_us[..],
                    );
                }
            }
        } // end #[cfg(not(feature = "threads"))]

        // Drain per-tick aggregates into the top-level `nn` profile tree as
        // sum-across-workers values. v1.7: this lives in the sibling `nn` root
        // (NOT under `tick`), and every parent row (`nn.build_input`,
        // `nn.proximity`, `nn.forward`) has its OWN per-worker bracket so the
        // panel's parent column is a real measurement, not a children rollup.
        // The wall-clock time the sim worker spent waiting for these workers
        // is recorded by the `tick.nn` RAII span in `World::step` — that one
        // stays a leaf in the `tick` tree.
        if timed {
            use std::sync::atomic::Ordering;
            // v1.7.2: each drained sample now carries an honest call count.
            // The `nn` root is bracketed once per chunk, every sub-phase under
            // `build_nn_input` runs once per creature, and the forward layers
            // run once per creature too. Reading both `_us` and `_calls`
            // atomics in matched pairs keeps the panel's `ms/call` column
            // pinned to real per-call cost.
            self.profile.record_under_root(
                "nn",
                "",
                self.nn_stats.tick_chunk_wall_us.load(Ordering::Relaxed) as u32,
                self.nn_stats.tick_chunk_wall_calls.load(Ordering::Relaxed) as u32,
            );
            self.profile.record_under_root(
                "nn",
                "build_input",
                self.nn_stats
                    .tick_build_input_total_us
                    .load(Ordering::Relaxed) as u32,
                self.nn_stats
                    .tick_build_input_total_calls
                    .load(Ordering::Relaxed) as u32,
            );
            self.profile.record_under_root(
                "nn",
                "build_input.other",
                self.nn_stats
                    .tick_build_input_other_us
                    .load(Ordering::Relaxed) as u32,
                self.nn_stats
                    .tick_build_input_other_calls
                    .load(Ordering::Relaxed) as u32,
            );
            self.profile.record_under_root(
                "nn",
                "build_input.proximity",
                self.nn_stats
                    .tick_proximity_total_us
                    .load(Ordering::Relaxed) as u32,
                self.nn_stats
                    .tick_proximity_total_calls
                    .load(Ordering::Relaxed) as u32,
            );
            self.profile.record_under_root(
                "nn",
                "build_input.proximity.creatures",
                self.nn_stats
                    .tick_proximity_creatures_us
                    .load(Ordering::Relaxed) as u32,
                self.nn_stats
                    .tick_proximity_creatures_calls
                    .load(Ordering::Relaxed) as u32,
            );
            self.profile.record_under_root(
                "nn",
                "build_input.proximity.grass",
                self.nn_stats
                    .tick_proximity_grass_us
                    .load(Ordering::Relaxed) as u32,
                self.nn_stats
                    .tick_proximity_grass_calls
                    .load(Ordering::Relaxed) as u32,
            );
            self.profile.record_under_root(
                "nn",
                "forward",
                self.nn_stats.tick_forward_us.load(Ordering::Relaxed) as u32,
                self.nn_stats.tick_forward_calls.load(Ordering::Relaxed) as u32,
            );
            // v1.12: emit `forward.l{k}` 1-based rows for every active matmul.
            // The legacy 32→48→24→5 topology has 3 matmuls → l1/l2/l3, same
            // names as pre-v1.12 so the perf panel renders identically.
            let matmul_count = self.nn_topology.matmul_count();
            for k in 0..matmul_count {
                let path = format!("forward.l{}", k + 1);
                self.profile.record_under_root(
                    "nn",
                    &path,
                    self.nn_stats.tick_forward_layer_us[k].load(Ordering::Relaxed) as u32,
                    self.nn_stats.tick_forward_layer_calls[k].load(Ordering::Relaxed) as u32,
                );
            }
        }
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

/// Sub-phase timings collected by `build_nn_input` when a `BuildTimings` is supplied.
///
/// All durations are in microseconds. The `other` bucket covers everything that
/// isn't a creature- or grass-sector scan: self/memory slots, wall arithmetic,
/// bilinear sampling, bias slot, padding. The two proximity scans are the
/// expensive parts and get their own buckets. v1.7 adds a `proximity_total_us`
/// parent bracket so the `nn.proximity` row in the panel reports its own
/// measured time, not a rollup of its children.
#[derive(Default)]
pub(crate) struct BuildTimings {
    pub other_us: u64,
    pub creature_sectors_us: u64,
    pub grass_sectors_us: u64,
    /// Wall-clock from the start of the creature-sector scan to the end of
    /// the grass-sector scan (i.e. the parent bracket of the two proximity
    /// children). v1.7: parent rows must be real measurements.
    pub proximity_total_us: u64,
}

/// Composable NN input-layout descriptor (v2.0).
///
/// The NN input vector is built from an ordered set of *groups*. Each group
/// occupies a contiguous slot range; `NnInputLayout` walks the active groups
/// in canonical order, assigns each a slot offset, and rounds the running
/// total up to the next multiple of 8 (the SIMD-chunk requirement) with a
/// trailing alignment pad. `build_nn_input` then writes each active group at
/// its computed offset into a `MAX_NN_INPUTS`-sized buffer.
///
/// For Wave 0 the only active configuration is the legacy layout
/// (`NnInputLayout::legacy()`), which reproduces the historical width-32 slot
/// order bit-for-bit. Future waves toggle groups (wrap walls 4→8,
/// creature sectors 8↔16) and the offsets + total width recompute here — no
/// hand-maintained slot constants.
///
/// Canonical group order (legacy, all active):
/// | group              | width | legacy slots |
/// |--------------------|-------|--------------|
/// | `SelfMemory`       | 8     | `[0..8)`     |
/// | `WallProximity`    | 4     | `[8..12)`    |
/// | `CreatureSectors`  | 8     | `[12..20)`   |
/// | `GrassSectors`     | 8     | `[20..28)`   |
/// | `ReservedPredator` | 1     | `[28]`       |
/// | `CurrGrass`        | 1     | `[29]`       |
/// | `Bias`             | 1     | `[30]`       |
/// | (trailing pad)     | 1     | `[31]`       |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NnInputGroup {
    SelfMemory,
    WallProximity,
    CreatureSectors,
    GrassSectors,
    /// v2.0.4 S6: far-band grass density sectors — 8 directional samples from
    /// the LOD pyramid at GRASS_FAR_MIP_LEVEL (radius ≈ GRASS_FAR_SIGHT_RADIUS).
    /// Only active when `grass_multisight` is on AND the layout has room
    /// (walled + species fills the budget; that combination falls back to
    /// single-band). Placed after `GrassSectors` in canonical order so it
    /// never shifts the near-band offset.
    GrassBandsFar,
    ReservedPredator,
    CurrGrass,
    Bias,
}

impl NnInputGroup {
    /// Canonical group order. The legacy layout activates every group; future
    /// waves choose a subset / wider variants from this same ordering.
    const ORDER: [NnInputGroup; 8] = [
        NnInputGroup::SelfMemory,
        NnInputGroup::WallProximity,
        NnInputGroup::CreatureSectors,
        NnInputGroup::GrassSectors,
        NnInputGroup::GrassBandsFar,
        NnInputGroup::ReservedPredator,
        NnInputGroup::CurrGrass,
        NnInputGroup::Bias,
    ];

    /// Position of this group in the canonical order.
    fn rank(self) -> usize {
        Self::ORDER
            .iter()
            .position(|&g| g == self)
            .expect("every group is in ORDER")
    }
}

/// True iff `active`'s groups are strictly increasing in canonical rank (i.e.
/// a subsequence of `NnInputGroup::ORDER`, no repeats, in order).
fn is_canonical_subsequence(active: &[(NnInputGroup, usize)]) -> bool {
    active.windows(2).all(|w| w[0].0.rank() < w[1].0.rank())
}

/// Resolved layout: per-active-group `(group, offset)` in canonical order plus
/// the SIMD-aligned total width. Construct once (cheap) and reuse; for Wave 0
/// the descriptor is the legacy layout.
#[derive(Clone, Debug)]
pub(crate) struct NnInputLayout {
    /// `(group, offset, width)` for each active group, in canonical order.
    groups: Vec<(NnInputGroup, usize, usize)>,
    /// Total active width, padded up to a multiple of 8 (≤ MAX_NN_INPUTS).
    width: usize,
}

impl NnInputLayout {
    /// Build a layout from `(group, width)` pairs given in canonical order.
    /// Offsets are assigned by packing groups contiguously; the total is then
    /// rounded up to the next multiple of 8 with a trailing alignment pad.
    ///
    /// The active list must be a subsequence of `NnInputGroup::ORDER` (each
    /// group appears at most once, in canonical order) — this keeps offsets
    /// deterministic as future waves toggle groups on/off.
    fn from_groups(active: &[(NnInputGroup, usize)]) -> Self {
        debug_assert!(
            is_canonical_subsequence(active),
            "NN input groups must be a canonical-order subsequence of NnInputGroup::ORDER"
        );
        let mut groups = Vec::with_capacity(active.len());
        let mut off = 0usize;
        for &(g, w) in active {
            groups.push((g, off, w));
            off += w;
        }
        // SIMD-chunk alignment: round the unpadded width up to a multiple of 8.
        let width = off.div_ceil(8) * 8;
        debug_assert!(
            width <= MAX_NN_INPUTS,
            "NN input layout width {width} exceeds MAX_NN_INPUTS {MAX_NN_INPUTS}"
        );
        Self { groups, width }
    }

    /// Active layout, driven by construction settings (wrap + species +
    /// grass_multisight).
    ///
    /// Drops `ReservedPredator`, `BiomeDir`, `CurrCellPenalty`, and
    /// `CurrBiomeType` entirely. Includes `WallProximity` (4 slots) ONLY when
    /// walled (`wrap_world == false`).
    ///
    /// `CreatureSectors` is **8** in single-pool and **16** in `species_mode`.
    /// When `grass_multisight` is ON, `GrassBandsFar` (8 slots) is appended
    /// after `GrassSectors`; all 8 configurations now fit within MAX_NN_INPUTS.
    ///
    /// Width table (base = SelfMemory 8 + GrassSectors 8 + CurrGrass 1 +
    /// Bias 1 = 18):
    ///
    /// Single-band (grass_multisight=false):
    /// | wrap | species | real | pad |
    /// |------|---------|------|-----|
    /// | on   | off     | 26   | 32  |
    /// | off  | off     | 30   | 32  |
    /// | on   | on      | 34   | 40  |
    /// | off  | on      | 38   | 40  |
    ///
    /// Multi-band (grass_multisight=true, GrassBandsFar +8):
    /// | wrap | species | real | pad |
    /// |------|---------|------|-----|
    /// | on   | off     | 34   | 40  |
    /// | off  | off     | 38   | 40  |
    /// | on   | on      | 42   | 48  |
    /// | off  | on      | 46   | 48  |
    ///
    /// All 8 combinations fit within MAX_NN_INPUTS=48 (no fallback needed).
    pub(crate) fn for_settings(
        wrap_world: bool,
        species_mode: bool,
        grass_multisight: bool,
    ) -> Self {
        let creature_sectors = if species_mode {
            NN_CREATURE_SECTORS_SPECIES
        } else {
            NN_SECTORS
        };
        // v2.1 P2: all 8 combinations fit; no fallback needed.
        let base_real = 8 // SelfMemory
            + if !wrap_world { 4 } else { 0 } // WallProximity
            + creature_sectors               // CreatureSectors (8 or 16)
            + NN_SECTORS                     // GrassSectors (8)
            + 1 + 1; // CurrGrass + Bias
        let include_far_band = grass_multisight && (base_real + NN_SECTORS <= MAX_NN_INPUTS);

        let mut groups: Vec<(NnInputGroup, usize)> = Vec::with_capacity(8);
        groups.push((NnInputGroup::SelfMemory, 8));
        if !wrap_world {
            groups.push((NnInputGroup::WallProximity, 4));
        }
        groups.push((NnInputGroup::CreatureSectors, creature_sectors));
        groups.push((NnInputGroup::GrassSectors, NN_SECTORS));
        if include_far_band {
            groups.push((NnInputGroup::GrassBandsFar, NN_SECTORS));
        }
        groups.push((NnInputGroup::CurrGrass, 1));
        groups.push((NnInputGroup::Bias, 1));
        let layout = Self::from_groups(&groups);
        debug_assert!(
            layout.width() <= MAX_NN_INPUTS,
            "NN input layout width {} exceeds MAX_NN_INPUTS {}",
            layout.width(),
            MAX_NN_INPUTS
        );
        layout
    }

    /// Legacy width-32 layout: every group active at its historical width.
    /// Reproduces the pre-v2.0 slot order exactly (8/4/8/8/1/1/1 + 1 pad = 32).
    /// The computed offsets are pinned to the documented legacy slot
    /// constants (`NN_WALL_OFFSET`, …) so any drift is caught at construction.
    /// v2.0: this is the *calibration anchor* (drift/layout tests) — the active
    /// world layout comes from `for_settings`, which drops `ReservedPredator`.
    #[cfg(test)]
    pub(crate) fn legacy() -> Self {
        let layout = Self::from_groups(&[
            (NnInputGroup::SelfMemory, 8),
            (NnInputGroup::WallProximity, 4),
            (NnInputGroup::CreatureSectors, NN_SECTORS),
            (NnInputGroup::GrassSectors, NN_SECTORS),
            (NnInputGroup::ReservedPredator, 1),
            (NnInputGroup::CurrGrass, 1),
            (NnInputGroup::Bias, 1),
        ]);
        debug_assert_eq!(layout.width(), NN_INPUTS, "legacy layout width must be 32");
        debug_assert_eq!(layout.offset_of(NnInputGroup::SelfMemory), Some(0));
        debug_assert_eq!(
            layout.offset_of(NnInputGroup::WallProximity),
            Some(NN_WALL_OFFSET)
        );
        debug_assert_eq!(
            layout.offset_of(NnInputGroup::CreatureSectors),
            Some(NN_CREATURE_SECTOR_OFFSET)
        );
        debug_assert_eq!(
            layout.offset_of(NnInputGroup::GrassSectors),
            Some(NN_GRASS_SECTOR_OFFSET)
        );
        debug_assert_eq!(layout.offset_of(NnInputGroup::ReservedPredator), Some(28));
        debug_assert_eq!(
            layout.offset_of(NnInputGroup::CurrGrass),
            Some(NN_CURR_GRASS_SLOT)
        );
        debug_assert_eq!(layout.offset_of(NnInputGroup::Bias), Some(NN_BIAS_SLOT));
        layout
    }

    /// Test-only constructor: build a layout from an explicit canonical-order
    /// `(group, width)` list. Lets the width tests synthesize variant layouts
    /// (wider sectors, toggled-off groups) without a live group toggle.
    #[cfg(test)]
    pub(crate) fn from_groups_for_test(active: &[(NnInputGroup, usize)]) -> Self {
        Self::from_groups(active)
    }

    /// SIMD-aligned total input width (a multiple of 8 in [8, MAX_NN_INPUTS]).
    /// This is the value that must match the topology's `input_width`.
    pub(crate) fn width(&self) -> usize {
        self.width
    }

    /// Offset of `group` if active, else `None`.
    pub(crate) fn offset_of(&self, group: NnInputGroup) -> Option<usize> {
        self.groups
            .iter()
            .find(|(g, _, _)| *g == group)
            .map(|(_, off, _)| *off)
    }
}

/// Build the NN input vector for creature `i` into a `MAX_NN_INPUTS`-sized
/// buffer (v2.0 composable layout). Only the first `layout.width()` lanes are
/// active; the remainder are alignment padding left at 0.0.
///
/// For the legacy layout this reproduces the historical width-32 slot order
/// bit-for-bit (see `NnInputLayout`). The forward pass consumes exactly
/// `topology.input_width()` lanes, which must equal `layout.width()`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_nn_input(
    i: usize,
    layout: &NnInputLayout,
    species_mode: bool,
    creatures: &CreatureSoA,
    grass: &GrassGrid,
    grid: &SpatialGrid,
    sector_lut: &[(u8, u8, f32)],
    sector_scratch: &mut [f32; 16],
    prev_vx: f32,
    prev_vy: f32,
    energy_max: f32,
    max_age: u32,
    world_size: f32,
    timings: Option<&mut BuildTimings>,
) -> [f32; MAX_NN_INPUTS] {
    use crate::profiler::clock_now_us_threadsafe;
    let mut buf = [0.0f32; MAX_NN_INPUTS];
    let energy = creatures.energy[i];
    let age = creatures.age[i];
    let cooldown = creatures.digestion_cooldown[i];
    let x = creatures.x[i];
    let y = creatures.y[i];

    // t0: start of "other" work (self/memory + walls).
    let t0 = if timings.is_some() {
        clock_now_us_threadsafe()
    } else {
        0
    };

    // --- self/memory ---
    if let Some(o) = layout.offset_of(NnInputGroup::SelfMemory) {
        let energy_frac = (energy / energy_max.max(1.0)).clamp(0.0, 1.0);
        buf[o] = (1.0 - energy_frac).clamp(0.0, 1.0); // hunger
        let max_age_f = (max_age.max(1)) as f32;
        buf[o + 1] = (age as f32 / max_age_f).clamp(0.0, 1.0); // age_frac
        buf[o + 2] = prev_vx / MOVE_SPEED_MAX;
        buf[o + 3] = prev_vy / MOVE_SPEED_MAX;
        let la = creatures.last_action[i];
        buf[o + 4] = if matches!(la, Action::Graze) {
            1.0
        } else {
            0.0
        };
        // slot 5: is_last_attack (was is_last_eat before the Wave 2a rename).
        buf[o + 5] = if matches!(la, Action::Attack) {
            1.0
        } else {
            0.0
        };
        buf[o + 6] = (creatures.ticks_since_split[i] as f32 / max_age_f).clamp(0.0, 1.0);
        buf[o + 7] = if cooldown == 0 { 1.0 } else { 0.0 };
    }

    // --- wall_proximity N/S/E/W (present only in walled worlds) ---
    if let Some(o) = layout.offset_of(NnInputGroup::WallProximity) {
        let walls = proximity::compute_wall_proximity(x, y, world_size);
        buf[o] = walls[0];
        buf[o + 1] = walls[1];
        buf[o + 2] = walls[2];
        buf[o + 3] = walls[3];
    }

    // t1: handoff to creature-sector scan.
    let t1 = if timings.is_some() {
        clock_now_us_threadsafe()
    } else {
        0
    };

    // --- creature_proximity sectors ---
    // Single-pool: 8 unified sectors. species_mode: 16 sectors (8 same-species +
    // 8 other-species; v2.0 Wave 3a). The wider 16-slot `sector_scratch` holds
    // both blocks; in single-pool only the first 8 are used.
    if let Some(o) = layout.offset_of(NnInputGroup::CreatureSectors) {
        if species_mode {
            let self_species = creatures.species_id[i];
            proximity::compute_creature_proximity_sectors_species(
                x,
                y,
                i,
                self_species,
                grid,
                creatures,
                sector_scratch,
            );
            buf[o..o + NN_CREATURE_SECTORS_SPECIES]
                .copy_from_slice(&sector_scratch[..NN_CREATURE_SECTORS_SPECIES]);
        } else {
            let mut sec8 = [0.0f32; 8];
            proximity::compute_creature_proximity_sectors(x, y, i, grid, creatures, &mut sec8);
            buf[o..o + NN_SECTORS].copy_from_slice(&sec8[..NN_SECTORS]);
        }
    }

    let t2 = if timings.is_some() {
        clock_now_us_threadsafe()
    } else {
        0
    };

    // --- grass_density sectors ---
    if let Some(o) = layout.offset_of(NnInputGroup::GrassSectors) {
        let mut grass_sec = [0.0f32; 8];
        proximity::compute_grass_density_sectors(x, y, grass, sector_lut, &mut grass_sec);
        buf[o..o + NN_SECTORS].copy_from_slice(&grass_sec[..NN_SECTORS]);
    }

    let t3 = if timings.is_some() {
        clock_now_us_threadsafe()
    } else {
        0
    };

    // --- far-band grass density sectors (v2.0.4 S6) ---
    if let Some(o) = layout.offset_of(NnInputGroup::GrassBandsFar) {
        let mut far_sec = [0.0f32; 8];
        proximity::compute_grass_far_band_sectors(x, y, grass, &mut far_sec);
        buf[o..o + NN_SECTORS].copy_from_slice(&far_sec[..NN_SECTORS]);
    }

    // --- reserved-predator padding (explicit 0.0) ---
    if let Some(o) = layout.offset_of(NnInputGroup::ReservedPredator) {
        buf[o] = 0.0;
    }

    // --- curr_grass_density ---
    if let Some(o) = layout.offset_of(NnInputGroup::CurrGrass) {
        buf[o] = grass.bilinear_sample(x, y);
    }

    // --- bias constant ---
    if let Some(o) = layout.offset_of(NnInputGroup::Bias) {
        buf[o] = 1.0;
    }

    let t4 = if timings.is_some() {
        clock_now_us_threadsafe()
    } else {
        0
    };

    if let Some(t) = timings {
        // "other" = pre-proximity setup + post-proximity assembly (bilinear sample + bias + padding).
        t.other_us = t.other_us.saturating_add(t1.saturating_sub(t0));
        t.creature_sectors_us = t.creature_sectors_us.saturating_add(t2.saturating_sub(t1));
        t.grass_sectors_us = t.grass_sectors_us.saturating_add(t3.saturating_sub(t2));
        t.other_us = t.other_us.saturating_add(t4.saturating_sub(t3));
        // Parent bracket: full proximity portion (creature + grass scans). The
        // children sum here equals the parent because the bracket is exactly
        // (t3 - t1); v1.7 still records both — the panel reads the parent row
        // from this counter rather than rolling up its children's totals.
        t.proximity_total_us = t.proximity_total_us.saturating_add(t3.saturating_sub(t1));
    }

    buf
}

/// Mode-dependent validity parameters for the third action slot (v2.0 Wave 3a).
///
/// `Action::Split` is the enum variant for action[2] in BOTH modes (the NN logit
/// order is stable), but its *meaning* and validity differ:
///   * single-pool (`species_mode == false`): it's **Split** — valid iff
///     `energy >= split_threshold`. The mating fields are ignored.
///   * species_mode (`true`): it's **Mate** — valid iff the initiator is off
///     mating cooldown (`mating_cooldown == 0`) AND `energy >= mating_cost`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActionGate {
    pub species_mode: bool,
    pub split_threshold: f32,
    /// Initiator-only mating cost (= the split energy gift). Species mode only.
    pub mating_cost: f32,
    /// Initiator's current mating cooldown (ticks). Species mode only.
    pub mating_cooldown: u32,
}

impl ActionGate {
    /// Single-pool gate: only `split_threshold` matters.
    #[cfg(test)]
    pub(crate) fn single_pool(split_threshold: f32) -> Self {
        Self {
            species_mode: false,
            split_threshold,
            mating_cost: 0.0,
            mating_cooldown: 0,
        }
    }
}

/// Check whether an action is currently valid for a creature (v6 §1 + §G).
/// D9: 3-variant enum. Graze always valid; Attack gated on digestion cooldown;
/// action[2] is Split (single-pool: energy gate) or Mate (species: off
/// mating-cooldown + energy ≥ mating cost) per `gate`.
pub(crate) fn is_valid_action(act: Action, energy: f32, cooldown: u32, gate: &ActionGate) -> bool {
    match act {
        Action::Graze => true,
        Action::Attack => cooldown == 0,
        // action[2]: Split (single-pool) or Mate (species_mode).
        Action::Split => {
            if gate.species_mode {
                gate.mating_cooldown == 0 && energy >= gate.mating_cost
            } else {
                energy >= gate.split_threshold
            }
        }
    }
}

/// Decode action logits to an Action via argmax with first-index tiebreak
/// and valid-fallthrough (v6 §1 + §E). An invalid action[2] (Split/Mate) falls
/// through to Graze exactly as today.
pub(crate) fn decode_action(
    logits: &[f32; 3],
    energy: f32,
    cooldown: u32,
    gate: &ActionGate,
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
        if is_valid_action(act, energy, cooldown, gate) {
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
/// Aggregated sub-phase timings produced by `pick_action_d` when a
/// `PickTimings` is supplied. `build` covers the input-build phases;
/// `forward_us` covers the NN forward pass. Per-creature accumulators
/// are summed into a chunk-level total before being merged into the
/// world's `NnStats`.
///
/// v1.7 adds `build_input_total_us` — the bracket around the whole
/// `build_nn_input` call so the `nn.build_input` parent row in the perf panel
/// is a real measurement, not a children rollup.
#[derive(Default)]
pub(crate) struct PickTimings {
    pub build: BuildTimings,
    /// Wall-clock bracket around the entire `build_nn_input` call (parent of
    /// `nn.build_input.other` plus the proximity-scan subtree).
    pub build_input_total_us: u64,
    pub forward_us: u64,
    /// v1.12: per-matmul wall clock. Length cap = `NN_MAX_MATMULS` (8 hidden
    /// + 1 output). Only the first `topology.matmul_count()` slots are written
    ///   per pass; the rest stay zero and the drain skips them.
    pub forward_layer_us: [u64; NN_MAX_MATMULS],
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_action_d(
    i: usize,
    layout: &NnInputLayout,
    species_mode: bool,
    input_buf: &mut [f32; MAX_NN_INPUTS],
    scratch_a: &mut [f32; NN_MAX_HIDDEN_WIDTH],
    scratch_b: &mut [f32; NN_MAX_HIDDEN_WIDTH],
    output_buf: &mut [f32; NN_OUTPUTS],
    creatures: &CreatureSoA,
    grass: &GrassGrid,
    grid: &SpatialGrid,
    sector_lut: &[(u8, u8, f32)],
    sector_scratch: &mut [f32; 16],
    prev_vx: f32,
    prev_vy: f32,
    energy_max: f32,
    split_threshold: f32,
    mating_cost: f32,
    max_age: u32,
    world_size: f32,
    mut timings: Option<&mut PickTimings>,
) -> (f32, f32, Action, u8) {
    use crate::profiler::clock_now_us_threadsafe;
    let timed = timings.is_some();
    let build_arg = timings.as_deref_mut().map(|t| &mut t.build);
    let t_build_start = if timed { clock_now_us_threadsafe() } else { 0 };
    *input_buf = build_nn_input(
        i,
        layout,
        species_mode,
        creatures,
        grass,
        grid,
        sector_lut,
        sector_scratch,
        prev_vx,
        prev_vy,
        energy_max,
        max_age,
        world_size,
        build_arg,
    );
    let t_build_end = if timed { clock_now_us_threadsafe() } else { 0 };
    if let Some(t) = timings.as_deref_mut() {
        t.build_input_total_us = t
            .build_input_total_us
            .saturating_add(t_build_end.saturating_sub(t_build_start));
    }
    let t_fwd_start = if timed { clock_now_us_threadsafe() } else { 0 };
    creatures.brains[i].forward(
        input_buf,
        output_buf,
        &mut scratch_a[..],
        &mut scratch_b[..],
        timings.as_deref_mut(),
    );
    let t_fwd_end = if timed { clock_now_us_threadsafe() } else { 0 };
    if let Some(t) = timings {
        t.forward_us = t
            .forward_us
            .saturating_add(t_fwd_end.saturating_sub(t_fwd_start));
    }

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
    // action[2] gate: Split (single-pool) or Mate (species_mode). The mating
    // gate is initiator-only (off mating cooldown + energy ≥ mating cost).
    let gate = ActionGate {
        species_mode,
        split_threshold,
        mating_cost,
        mating_cooldown: creatures.mating_cooldown[i],
    };
    let action = decode_action(logits, energy, cooldown, &gate);

    let had_nan = logits.iter().any(|v| !v.is_finite());
    if had_nan {
        return (0.0, 0.0, Action::Graze, argmax_pre);
    }

    (vx, vy, action, argmax_pre)
}

/// One labeled NN input entry: group name, per-slot label, and value.
/// Serialized to JSON for `creature_nn_inspect_json`.
#[derive(serde::Serialize)]
pub(crate) struct LabeledInput {
    pub group: &'static str,
    pub label: &'static str,
    pub value: f32,
}

/// Compass direction labels for 8-sector groups (N, NE, E, SE, S, SW, W, NW).
/// Sector order matches the proximity module: 0=N, 1=NE, 2=E, 3=SE, 4=S, 5=SW, 6=W, 7=NW.
const COMPASS8: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
/// Cardinal direction labels for 4-slot WallProximity (order: N, S, E, W — matches
/// `compute_wall_proximity` return layout in `proximity.rs`).
const CARDINAL4_WALL: [&str; 4] = ["N", "S", "E", "W"];

/// Build the full labeled NN input vector for creature `i` and run one
/// `Brain::forward`, returning:
///   - `inputs`: a `Vec<LabeledInput>` of length `layout.width()` (active lanes
///     only — pad lanes are omitted from the vec).
///   - `output_buf`: the raw 5-element forward output `[vx_raw, vy_raw, l0, l1, l2]`.
///
/// Used by `creature_nn_inspect_json` in `wasm_api/mod.rs`. Read-only; does NOT
/// write back to any SoA field.
///
/// v2.1 P1 (Stream A): single-creature on-demand inspect. Always uses the world's
/// live `nn_input_layout` so the group set reflects construction settings exactly.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_labeled_nn_inspect(
    i: usize,
    layout: &NnInputLayout,
    species_mode: bool,
    creatures: &crate::creature::CreatureSoA,
    grass: &crate::grass::GrassGrid,
    grid: &crate::grid::SpatialGrid,
    sector_lut: &[(u8, u8, f32)],
    energy_max: f32,
    max_age: u32,
    world_size: f32,
) -> (Vec<LabeledInput>, [f32; crate::constants::NN_OUTPUTS]) {
    // Build the raw input buffer (same path as the tick).
    let prev_vx = creatures.vx[i];
    let prev_vy = creatures.vy[i];
    let mut sector_scratch = [0.0f32; 16];
    let raw = build_nn_input(
        i,
        layout,
        species_mode,
        creatures,
        grass,
        grid,
        sector_lut,
        &mut sector_scratch,
        prev_vx,
        prev_vy,
        energy_max,
        max_age,
        world_size,
        None,
    );

    // Annotate each active slot with its group + label.
    let mut inputs: Vec<LabeledInput> = Vec::with_capacity(layout.width());

    for &(group, off, width) in &layout.groups {
        match group {
            NnInputGroup::SelfMemory => {
                const LABELS: [&str; 8] = [
                    "hunger",
                    "age_frac",
                    "prev_vx",
                    "prev_vy",
                    "last_graze",
                    "last_attack",
                    "ticks_since_split_frac",
                    "cooldown_ready",
                ];
                for k in 0..width {
                    inputs.push(LabeledInput {
                        group: "SelfMemory",
                        label: LABELS[k],
                        value: raw[off + k],
                    });
                }
            }
            NnInputGroup::WallProximity => {
                for k in 0..width {
                    inputs.push(LabeledInput {
                        group: "WallProximity",
                        label: CARDINAL4_WALL[k],
                        value: raw[off + k],
                    });
                }
            }
            NnInputGroup::CreatureSectors => {
                // Single-pool: 8 sectors; species: 16 (8 same + 8 other).
                if width == 8 {
                    for k in 0..8 {
                        inputs.push(LabeledInput {
                            group: "CreatureSectors",
                            label: COMPASS8[k],
                            value: raw[off + k],
                        });
                    }
                } else {
                    // species mode: 8 same-species, then 8 other-species
                    for k in 0..8 {
                        inputs.push(LabeledInput {
                            group: "CreatureSectors_same",
                            label: COMPASS8[k],
                            value: raw[off + k],
                        });
                    }
                    for k in 0..8 {
                        inputs.push(LabeledInput {
                            group: "CreatureSectors_other",
                            label: COMPASS8[k],
                            value: raw[off + 8 + k],
                        });
                    }
                }
            }
            NnInputGroup::GrassSectors => {
                for k in 0..width {
                    inputs.push(LabeledInput {
                        group: "GrassSectors",
                        label: COMPASS8[k],
                        value: raw[off + k],
                    });
                }
            }
            NnInputGroup::GrassBandsFar => {
                for k in 0..width {
                    inputs.push(LabeledInput {
                        group: "GrassBandsFar",
                        label: COMPASS8[k],
                        value: raw[off + k],
                    });
                }
            }
            NnInputGroup::ReservedPredator => {
                inputs.push(LabeledInput {
                    group: "ReservedPredator",
                    label: "reserved",
                    value: raw[off],
                });
            }
            NnInputGroup::CurrGrass => {
                inputs.push(LabeledInput {
                    group: "CurrGrass",
                    label: "density",
                    value: raw[off],
                });
            }
            NnInputGroup::Bias => {
                inputs.push(LabeledInput {
                    group: "Bias",
                    label: "bias",
                    value: raw[off],
                });
            }
        }
    }

    // Run one forward pass.
    let mut scratch_a = [0.0f32; crate::constants::NN_MAX_HIDDEN_WIDTH];
    let mut scratch_b = [0.0f32; crate::constants::NN_MAX_HIDDEN_WIDTH];
    let mut output_buf = [0.0f32; crate::constants::NN_OUTPUTS];
    creatures.brains[i].forward(&raw, &mut output_buf, &mut scratch_a, &mut scratch_b, None);

    (inputs, output_buf)
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
    fn build_for_founder(w: &mut World) -> [f32; MAX_NN_INPUTS] {
        let mut scratch = [0.0f32; 16];
        let prev_vx = w.creatures.vx[0];
        let prev_vy = w.creatures.vy[0];
        let energy_max = w.sliders.energy_max;
        let max_age = w.sliders.max_age;
        let world_size = w.dims.world_size;
        let species_mode = w.sliders.species_mode;
        let layout = w.nn_input_layout.clone();
        build_nn_input(
            0,
            &layout,
            species_mode,
            &w.creatures,
            &w.grass,
            &w.grid,
            &w.sector_lut,
            &mut scratch,
            prev_vx,
            prev_vy,
            energy_max,
            max_age,
            world_size,
            None,
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

    /// v2.1 P2: the SIMD trailing-pad lanes are always 0.0. The walled
    /// single-pool multisight layout is width 40.
    ///
    /// Layout: SelfMemory(8) + WallProximity(4) + CreatureSectors(8) +
    ///   GrassSectors(8) + GrassBandsFar(8) + CurrGrass(1) + Bias(1)
    ///   = 38 real → padded to 40.
    #[test]
    fn nn_input_trailing_pad_is_zero() {
        let mut w = World::new("s5b-pad");
        let inp = build_for_founder(&mut w);
        assert_eq!(
            w.nn_input_layout.width(),
            40,
            "walled single-pool multisight width is 40 (v2.1 P2)"
        );
        for s in 39..40 {
            assert_eq!(inp[s], 0.0, "trailing SIMD pad lane {s} must be 0.0");
        }
    }

    /// The bias-learning constant lane is always 1.0 (at the layout's Bias
    /// offset — slot 37 in the walled single-pool multisight layout:
    /// 8+4+8+8+8+1 = 37 before the bias slot.
    #[test]
    fn nn_input_bias_is_one() {
        let mut w = World::new("s5b-bias");
        let bias_off = w
            .nn_input_layout
            .offset_of(NnInputGroup::Bias)
            .expect("Bias group must be active");
        let inp = build_for_founder(&mut w);
        assert_eq!(inp[bias_off], 1.0, "bias slot must be 1.0");
        assert_eq!(
            bias_off, 37,
            "walled single-pool multisight layout puts Bias at 37"
        );
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
        w.grass.fill_density(0.0);
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

    #[test]
    fn nn_input_grass_sector_ignores_current_cell() {
        let mut w = World::new_with_sliders(
            "grass-sector-current-cell",
            crate::world::DevSliders {
                founder_count: 1,
                grass_multisight: false,
                ..Default::default()
            },
        );
        w.grass.fill_density(0.0);
        let x = w.creatures.x[0];
        let y = w.creatures.y[0];
        let ix = (x / w.dims.grass_cell_size) as usize;
        let iy = (y / w.dims.grass_cell_size) as usize;
        let idx = iy.min(w.dims.grass_dim - 1) * w.dims.grass_dim + ix.min(w.dims.grass_dim - 1);
        w.grass.dset(idx, GRASS_MAX);
        w.grass.rebuild_row_bitset();

        let inp = build_for_founder(&mut w);
        let grass_off = w
            .nn_input_layout
            .offset_of(NnInputGroup::GrassSectors)
            .expect("GrassSectors must be active");
        for s in 0..NN_SECTORS {
            assert_eq!(
                inp[grass_off + s],
                0.0,
                "current grass cell must not light grass sector {s}"
            );
        }
        let curr_off = w
            .nn_input_layout
            .offset_of(NnInputGroup::CurrGrass)
            .expect("CurrGrass must be active");
        assert!(
            inp[curr_off] > 0.0,
            "current grass cell must still be visible through CurrGrass"
        );
    }

    // ---- decode_action tests (kept; layout-independent) ----

    #[test]
    fn decode_action_valid_fallthrough_split_invalid() {
        let logits = [0.0f32, 0.0, 10.0];
        let act = decode_action(
            &logits,
            10.0,
            0,
            &ActionGate::single_pool(SPLIT_THRESHOLD_DEFAULT),
        );
        assert_ne!(act, Action::Split, "Split must be invalid when energy < 50");
        assert!(
            matches!(act, Action::Graze | Action::Attack),
            "got {:?}",
            act
        );
    }

    #[test]
    fn decode_action_first_index_tiebreak() {
        let logits = [5.0f32; 3];
        let act = decode_action(
            &logits,
            100.0,
            0,
            &ActionGate::single_pool(SPLIT_THRESHOLD_DEFAULT),
        );
        assert_eq!(act, Action::ALL[0], "lower index must win on ties");
        assert_eq!(act, Action::Graze, "Graze is index 0 after D9");
    }

    #[test]
    fn decode_action_attack_invalid_in_cooldown() {
        let logits = [0.0f32, 10.0, 0.0];
        let act = decode_action(
            &logits,
            100.0,
            5,
            &ActionGate::single_pool(SPLIT_THRESHOLD_DEFAULT),
        );
        assert_ne!(
            act,
            Action::Attack,
            "Attack must be invalid when cooldown > 0"
        );
        assert_eq!(act, Action::Graze, "should fall through to Graze");
    }

    #[test]
    fn decode_action_split_invalid_when_low_energy() {
        let logits = [0.0f32, 0.0, 10.0];
        let act = decode_action(
            &logits,
            0.0,
            0,
            &ActionGate::single_pool(SPLIT_THRESHOLD_DEFAULT),
        );
        assert_ne!(
            act,
            Action::Split,
            "Split must be invalid when energy too low"
        );
    }

    #[test]
    fn decode_action_graze_always_valid_as_fallback() {
        let logits = [-5.0f32, 2.0, 10.0];
        let act = decode_action(
            &logits,
            0.0,
            1,
            &ActionGate::single_pool(SPLIT_THRESHOLD_DEFAULT),
        );
        assert!(
            matches!(act, Action::Graze),
            "Expected Graze fallback, got {:?}",
            act
        );
    }

    #[test]
    fn nan_logits_return_graze_via_decode() {
        let nan_logits = [f32::NAN, 0.0, 0.0];
        let act = decode_action(
            &nan_logits,
            100.0,
            0,
            &ActionGate::single_pool(SPLIT_THRESHOLD_DEFAULT),
        );
        assert_eq!(act, Action::Graze, "NaN logit must produce Graze");

        let inf_logits = [f32::INFINITY, 0.0, 0.0];
        let act2 = decode_action(
            &inf_logits,
            100.0,
            0,
            &ActionGate::single_pool(SPLIT_THRESHOLD_DEFAULT),
        );
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
