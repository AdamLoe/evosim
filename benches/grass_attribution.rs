//! C0 attribution bench: isolate per-cell scatter cost components.
//!
//! Measures ns/cell for each lever at two fill levels:
//!   - frontier  : ~6.25% fill (16 384 seeded cells, 512×512 grid)
//!   - dense     : ~90% fill (236 029 seeded cells, same grid)
//!
//! Levers measured:
//!   baseline    : minimal loop over grassy cells, no alloc / instr / extra RNG
//!   +alloc      : per-tile Vec::with_capacity like production (C1)
//!   +instr      : per-tile Instant::now() + AtomicU64::fetch_add like production (C2)
//!   +rng_4hash  : 4 separate grass_hash_u64 calls per grassy cell (production path)
//!   +rng_1hash  : 1 hash, bit-sliced into 4 values (fused / C3 candidate)
//!   +rmw        : scatter_add / scatter_sub relaxed atomic RMW as production does
//!   +freeze     : the tile freeze loop alone (the O(tile) floor component)
//!   geom_skip   : geometric-skip spread sampler over the same active-tile set
//!
//! Native-rayon shim: each benchmark group has a *_par variant that fans the same
//! tile_body kernel across rayon threads, exposing false-sharing on the shared
//! AtomicU64 perf counters (the key threaded-only cost).
//!
//! Grid: world_size=2560 → grass_dim=512 → 512²=262 144 cells, 16×16=256 tiles.
//! GRASS_TILE_SIZE=32 → tiles are 32×32=1024 cells each (or smaller at edge).
//! Active tiles: all tiles containing at least one grassy cell. At 6.25% fill
//! approximately 64-120 tiles; at 90% fill essentially all 256 tiles.
//!
//! Per-cell cost = bench wall time / (active_cell_count). `active_cell_count` is
//! the number of grassy cells in active tiles, not total grid cells, because the
//! production kernel only iterates grassy cells in active tiles.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use evosim::constants::{WorldDims, GRASS_SPREAD_RADIUS};
use evosim::grass::{GrassGrid, GrassPropagation, ScatterParams, GRASS_TILE_SIZE};
use evosim::rng::{grass_hash_u64, grass_unit, SimRng};
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::Instant;

// ── helpers ──────────────────────────────────────────────────────────────────

const WORLD_SIZE: f32 = 2560.0;
const FRONTIER_SEEDS: u32 = 16_384; // ~6.25% of 262 144
const DENSE_SEEDS: u32 = 236_029; // ~90% of 262 144
const RNG_SEED: u64 = 0xDEAD_BEEF_1234_5678;
const GRASS_MAX: f32 = 1.0;

fn encode_u8(d: f32) -> u8 {
    let q = (d / GRASS_MAX).clamp(0.0, 1.0) * 255.0;
    (q + 0.5) as u8
}

/// Lossy relaxed-atomic ADD (mirrors production scatter_add).
#[inline]
fn scatter_add(cell: &AtomicU8, add_byte: u8, cap_byte: u8) -> bool {
    let cur = cell.load(Ordering::Relaxed);
    let next = cur.saturating_add(add_byte).min(cap_byte);
    if next != cur {
        cell.store(next, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Lossy relaxed-atomic SUB (mirrors production scatter_sub).
#[inline]
fn scatter_sub(cell: &AtomicU8, sub_byte: u8) -> bool {
    let cur = cell.load(Ordering::Relaxed);
    let next = cur.saturating_sub(sub_byte);
    if next != cur {
        cell.store(next, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Build a GrassGrid with `seed_count` random cells set to full grass.
fn make_grid(seed_count: u32) -> GrassGrid {
    let dims = WorldDims::from_world_size(WORLD_SIZE, false);
    let mut rng = SimRng::from_u64(RNG_SEED);
    let mut g = GrassGrid::new(&mut rng, seed_count, dims);
    g.set_propagation(GrassPropagation::Scatter);
    g.world_seed = 42;
    g.scatter_tick = 1;
    g
}

/// Enumerate active tiles in a GrassGrid. Returns (tile_idx, ix0, iy0, ix1, iy1).
fn active_tiles(g: &GrassGrid) -> Vec<(usize, usize, usize, usize, usize)> {
    let dim = g.dims.grass_dim;
    let tpa = g.tiles_per_axis;
    let ntiles = tpa * tpa;
    (0..ntiles)
        .filter_map(|t| {
            // Check tile_active bitset via public density (no private bitset access).
            // We detect "active" by checking if the tile has any grassy cell;
            // this mirrors `resync_active_from_density` semantics for benchmarking.
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
            // Check if any cell in this tile is grassy.
            let has_grass = (iy0..iy1).any(|iy| {
                (ix0..ix1).any(|ix| g.dget_u8(iy * dim + ix) > 0)
            });
            if has_grass {
                Some((t, ix0, iy0, ix1, iy1))
            } else {
                None
            }
        })
        .collect()
}

/// Count total grassy cells across all active tiles (the denominator for ns/cell).
fn count_grassy_cells(g: &GrassGrid, tiles: &[(usize, usize, usize, usize, usize)]) -> u64 {
    let dim = g.dims.grass_dim;
    tiles.iter().map(|&(_, ix0, iy0, ix1, iy1)| {
        let mut n = 0u64;
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                if g.dget_u8(iy * dim + ix) > 0 {
                    n += 1;
                }
            }
        }
        n
    }).sum()
}

// ── DiscTable (minimal subset — band+pick sampling, mirrors production) ──────

struct DiscTable {
    offsets: Vec<(i32, i32)>,
    band_end: [usize; 3],
    cum: [f32; 3],
}

impl DiscTable {
    fn build() -> Self {
        let radius = GRASS_SPREAD_RADIUS;
        let ring1 = 0.70_f32;
        let ring2 = 0.22_f32;
        let ring3 = 0.08_f32;
        let rf = radius as f32 + 1e-3;
        let mut bands: [Vec<(i32, i32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                if dist > rf {
                    continue;
                }
                let band = if dist <= 1.5 { 0 } else if dist <= 2.5 { 1 } else { 2 };
                bands[band].push((dx, dy));
            }
        }
        let raw = [
            if bands[0].is_empty() { 0.0 } else { ring1.max(0.0) },
            if bands[1].is_empty() { 0.0 } else { ring2.max(0.0) },
            if bands[2].is_empty() { 0.0 } else { ring3.max(0.0) },
        ];
        let total: f32 = raw.iter().sum();
        let norm = if total > 0.0 {
            [raw[0] / total, raw[1] / total, raw[2] / total]
        } else {
            [1.0 / 3.0; 3]
        };
        let cum = [norm[0], norm[0] + norm[1], norm[0] + norm[1] + norm[2]];
        let band_end = [
            bands[0].len(),
            bands[0].len() + bands[1].len(),
            bands[0].len() + bands[1].len() + bands[2].len(),
        ];
        let mut offsets = Vec::with_capacity(band_end[2]);
        offsets.extend_from_slice(&bands[0]);
        offsets.extend_from_slice(&bands[1]);
        offsets.extend_from_slice(&bands[2]);
        Self { offsets, band_end, cum }
    }

    #[inline]
    fn sample(&self, band_roll: f32, pick_roll: f32) -> Option<(i32, i32)> {
        if self.offsets.is_empty() {
            return None;
        }
        let band = if band_roll < self.cum[0] {
            0
        } else if band_roll < self.cum[1] {
            1
        } else {
            2
        };
        let (lo, hi) = match band {
            0 => (0, self.band_end[0]),
            1 => (self.band_end[0], self.band_end[1]),
            _ => (self.band_end[1], self.band_end[2]),
        };
        let (lo, hi) = if lo >= hi { (0, self.offsets.len()) } else { (lo, hi) };
        let span = hi - lo;
        let k = ((pick_roll * span as f32) as usize).min(span - 1);
        Some(self.offsets[lo + k])
    }
}

// ── Shared counters struct (for instrumentation variant) ─────────────────────

struct SharedCounters {
    body_self_us: AtomicU64,
    body_calls: AtomicU64,
}

impl SharedCounters {
    fn new() -> Self {
        Self {
            body_self_us: AtomicU64::new(0),
            body_calls: AtomicU64::new(0),
        }
    }
}

// ── Kernel variants ───────────────────────────────────────────────────────────
//
// Each variant is a fn that takes a prepared GrassGrid snapshot and returns the
// number of processed (grassy) cells — so the caller can normalise to ns/cell.
// All variants are MEASUREMENT ONLY; no production behavior is changed.
//
// Naming: the variant accumulates cost components. The *difference* between
// adjacent variants isolates a single lever.
//
// baseline:   freeze loop only (reading each cell, skipping zeros)
// +alloc:     baseline + per-tile Vec::with_capacity(tw*th)
// +instr:     +alloc + per-tile Instant::now() + AtomicU64::fetch_add (×2)
// +rng_4hash: +instr + 4 grass_hash_u64 calls per grassy cell
// +rng_1hash: +instr + 1 grass_hash_u64 + bit-slice (fused alternative)
// +rmw:       +rng_4hash + scatter_add / scatter_sub (the actual RMW writes)
// full_prod:  +rmw + spread target resolution + tile activation bitset writes
//             (as close to production tile_body as we can get from outside)
// geom_skip:  +instr + geometric-skip spread (alternative to per-cell roll)

/// VARIANT 0 — freeze loop only.
/// Iterates every cell in the tile, loads the AtomicU8, counts grassy cells.
/// No heap alloc; no RNG; no RMW. Represents the irreducible O(tile) floor.
fn variant_freeze(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)], dim: usize) -> u64 {
    let mut grassy = 0u64;
    for &(_, ix0, iy0, ix1, iy1) in tiles {
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                let s = density[iy * dim + ix].load(Ordering::Relaxed);
                if s > 0 {
                    grassy += 1;
                    black_box(s);
                }
            }
        }
    }
    grassy
}

/// VARIANT 1 — freeze loop + per-tile Vec alloc (mirrors production grass.rs:1655).
fn variant_freeze_alloc(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)], dim: usize) -> u64 {
    let mut grassy = 0u64;
    for &(_, ix0, iy0, ix1, iy1) in tiles {
        let tw = ix1 - ix0;
        let th = iy1 - iy0;
        let mut src: Vec<u8> = Vec::with_capacity(tw * th);
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                src.push(density[iy * dim + ix].load(Ordering::Relaxed));
            }
        }
        for &s in &src {
            if s > 0 {
                grassy += 1;
                black_box(s);
            }
        }
    }
    grassy
}

/// VARIANT 2 — freeze + alloc + instrumentation (Instant + fetch_add).
fn variant_freeze_alloc_instr(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)], dim: usize,
    counters: &SharedCounters) -> u64 {
    let mut grassy = 0u64;
    for &(_, ix0, iy0, ix1, iy1) in tiles {
        let t0 = Instant::now();
        let tw = ix1 - ix0;
        let th = iy1 - iy0;
        let mut src: Vec<u8> = Vec::with_capacity(tw * th);
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                src.push(density[iy * dim + ix].load(Ordering::Relaxed));
            }
        }
        for &s in &src {
            if s > 0 {
                grassy += 1;
                black_box(s);
            }
        }
        counters.body_self_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        counters.body_calls.fetch_add(1, Ordering::Relaxed);
    }
    grassy
}

/// VARIANT 3 — freeze + alloc + instr + 4 hashes per grassy cell (production RNG).
fn variant_rng_4hash(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)], dim: usize,
    world_seed: u32, tick: u32, counters: &SharedCounters,
    decay_pct: f32, spread_pct: f32) -> u64 {
    let mut grassy = 0u64;
    for &(_, ix0, iy0, ix1, iy1) in tiles {
        let t0 = Instant::now();
        let tw = ix1 - ix0;
        let th = iy1 - iy0;
        let mut src: Vec<u8> = Vec::with_capacity(tw * th);
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                src.push(density[iy * dim + ix].load(Ordering::Relaxed));
            }
        }
        for ly in 0..(th) {
            let iy = iy0 + ly;
            for lx in 0..(tw) {
                let ix = ix0 + lx;
                let s = src[ly * tw + lx];
                if s == 0 {
                    continue;
                }
                grassy += 1;
                let c = iy * dim + ix;
                let cid = c as u64;
                // Decay roll: salt 1
                let _decay = grass_unit(world_seed, cid, tick, 1) < decay_pct;
                // Spread gate: salt 2, band: salt 3, pick: salt 4
                let _spread = grass_unit(world_seed, cid, tick, 2) < spread_pct;
                let _band = grass_unit(world_seed, cid, tick, 3);
                let _pick = grass_unit(world_seed, cid, tick, 4);
                black_box((_decay, _spread, _band, _pick));
            }
        }
        counters.body_self_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        counters.body_calls.fetch_add(1, Ordering::Relaxed);
    }
    grassy
}

/// VARIANT 4 — freeze + alloc + instr + 1 hash bit-sliced into 4 (fused RNG, C3 candidate).
/// One grass_hash_u64 call; extract four 16-bit sub-words as uniform [0,1) draws.
fn variant_rng_1hash(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)], dim: usize,
    world_seed: u32, tick: u32, counters: &SharedCounters,
    decay_pct: f32, spread_pct: f32) -> u64 {
    let mut grassy = 0u64;
    for &(_, ix0, iy0, ix1, iy1) in tiles {
        let t0 = Instant::now();
        let tw = ix1 - ix0;
        let th = iy1 - iy0;
        let mut src: Vec<u8> = Vec::with_capacity(tw * th);
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                src.push(density[iy * dim + ix].load(Ordering::Relaxed));
            }
        }
        for ly in 0..(th) {
            let iy = iy0 + ly;
            for lx in 0..(tw) {
                let ix = ix0 + lx;
                let s = src[ly * tw + lx];
                if s == 0 {
                    continue;
                }
                grassy += 1;
                let c = iy * dim + ix;
                let cid = c as u64;
                // One hash, bit-sliced into 4 16-bit draws.
                let h = grass_hash_u64(world_seed, cid, tick, 1);
                let scale16 = 1.0 / (1u32 << 16) as f32;
                let r0 = ((h & 0xFFFF) as f32) * scale16;
                let r1 = (((h >> 16) & 0xFFFF) as f32) * scale16;
                let r2 = (((h >> 32) & 0xFFFF) as f32) * scale16;
                let r3 = (((h >> 48) & 0xFFFF) as f32) * scale16;
                let _decay  = r0 < decay_pct;
                let _spread = r1 < spread_pct;
                let _band   = r2;
                let _pick   = r3;
                black_box((_decay, _spread, _band, _pick));
            }
        }
        counters.body_self_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        counters.body_calls.fetch_add(1, Ordering::Relaxed);
    }
    grassy
}

/// VARIANT 5 — freeze + alloc + instr + 4 hashes + RMW (scatter_add/scatter_sub).
/// Includes the actual RMW atomics on the density field. No spread-target resolution
/// (skips the disc sample and tile activation) — isolates RMW cost increment.
fn variant_rng_4hash_rmw(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)], dim: usize,
    world_seed: u32, tick: u32, counters: &SharedCounters,
    decay_pct: f32, spread_pct: f32,
    decay_byte: u8, spread_byte: u8) -> u64 {
    let cap_byte = 255u8; // all-plains cap
    let mut grassy = 0u64;
    for &(_, ix0, iy0, ix1, iy1) in tiles {
        let t0 = Instant::now();
        let tw = ix1 - ix0;
        let th = iy1 - iy0;
        let mut src: Vec<u8> = Vec::with_capacity(tw * th);
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                src.push(density[iy * dim + ix].load(Ordering::Relaxed));
            }
        }
        for ly in 0..(th) {
            let iy = iy0 + ly;
            for lx in 0..(tw) {
                let ix = ix0 + lx;
                let s = src[ly * tw + lx];
                if s == 0 {
                    continue;
                }
                grassy += 1;
                let c = iy * dim + ix;
                let cid = c as u64;
                if decay_byte > 0 && grass_unit(world_seed, cid, tick, 1) < decay_pct {
                    scatter_sub(&density[c], decay_byte.max(1));
                }
                if spread_byte > 0 && grass_unit(world_seed, cid, tick, 2) < spread_pct {
                    let _band = grass_unit(world_seed, cid, tick, 3);
                    let _pick = grass_unit(world_seed, cid, tick, 4);
                    // Apply RMW to a dummy adjacent cell (same tile, wrapped) to keep cost realistic.
                    // In production this resolves a disc target; here we just write to (c+1) % len.
                    let tc = (c + 1) % density.len();
                    scatter_add(&density[tc], spread_byte.max(1), cap_byte);
                }
            }
        }
        counters.body_self_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        counters.body_calls.fetch_add(1, Ordering::Relaxed);
    }
    grassy
}

/// VARIANT 6 — geometric-skip spread sampler (C4 candidate).
/// Uses a Geometric(spread_pct) gap to skip non-spreading cells.
/// Decay is still per-cell; only the spread gate is replaced by skip.
/// Compares against variant_rng_4hash to isolate the skip benefit.
fn variant_geom_skip(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)], dim: usize,
    world_seed: u32, tick: u32, counters: &SharedCounters,
    decay_pct: f32, spread_pct: f32) -> u64 {
    let mut grassy = 0u64;
    let disc = DiscTable::build();
    let dimi = dim as i64;
    for &(t, ix0, iy0, ix1, iy1) in tiles {
        let t0 = Instant::now();
        let _tw = ix1 - ix0;
        let _th = iy1 - iy0;
        // Build a list of grassy cells in this tile (the compact list C4 requires).
        let mut grassy_cells: Vec<(usize, usize, usize)> = Vec::new(); // (lx, ly, c)
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                let s = density[iy * dim + ix].load(Ordering::Relaxed);
                if s > 0 {
                    grassy_cells.push((ix - ix0, iy - iy0, iy * dim + ix));
                }
            }
        }
        let n = grassy_cells.len();
        grassy += n as u64;
        // Decay: still per-grassy-cell (salt 1).
        for &(lx, ly, c) in &grassy_cells {
            let cid = c as u64;
            let _decay = grass_unit(world_seed, cid, tick, 1) < decay_pct;
            black_box((_decay, lx, ly));
        }
        // Spread: geometric-skip over the grassy list (distribution-exact for the gate).
        // gap ~ Geometric(spread_pct): gap = floor(ln(U)/ln(1-spread_pct)).
        if spread_pct > 0.0 && !grassy_cells.is_empty() {
            let ln1mp = (1.0_f32 - spread_pct).ln();
            // Use a per-tile seeded LCG-style counter for the skip RNG to avoid SimRng.
            // We use grass_hash_u64 with a tile-scoped id and advancing sub-salt.
            let mut skip_salt: u32 = 1000;
            let mut idx: usize = {
                // Initial gap before the first event.
                let u = (grass_hash_u64(world_seed, t as u64, tick, skip_salt) >> 40) as u32;
                let uf = (u as f32) / ((1u32 << 24) as f32);
                let uf = uf.max(f32::MIN_POSITIVE);
                skip_salt += 1;
                (uf.ln() / ln1mp).floor() as usize
            };
            while idx < n {
                let (lx, ly, c) = grassy_cells[idx];
                let cid = c as u64;
                let band_roll = grass_unit(world_seed, cid, tick, 3);
                let pick_roll = grass_unit(world_seed, cid, tick, 4);
                if let Some((dx, dy)) = disc.sample(band_roll, pick_roll) {
                    let gx = (ix0 as i64 + lx as i64 + dx as i64).rem_euclid(dimi);
                    let gy = (iy0 as i64 + ly as i64 + dy as i64).rem_euclid(dimi);
                    let tc = gy as usize * dim + gx as usize;
                    black_box(tc);
                }
                // Next gap.
                let u = (grass_hash_u64(world_seed, t as u64, tick, skip_salt) >> 40) as u32;
                let uf = (u as f32) / ((1u32 << 24) as f32);
                let uf = uf.max(f32::MIN_POSITIVE);
                skip_salt += 1;
                let gap = (uf.ln() / ln1mp).floor() as usize;
                idx = idx.saturating_add(gap + 1);
            }
        }
        counters.body_self_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        counters.body_calls.fetch_add(1, Ordering::Relaxed);
    }
    grassy
}

// ── Parallel (rayon shim) variants ───────────────────────────────────────────
//
// Each par_ variant fans the tile loop across rayon threads. This makes the
// AtomicU64::fetch_add on shared counters contend across cores, revealing the
// false-sharing cost that is invisible to the sequential variants above.

fn variant_rng_4hash_par(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)],
    dim: usize, world_seed: u32, tick: u32, counters: &SharedCounters,
    decay_pct: f32, spread_pct: f32) -> u64 {
    tiles.par_iter().map(|&(_, ix0, iy0, ix1, iy1)| {
        let t0 = Instant::now();
        let tw = ix1 - ix0;
        let th = iy1 - iy0;
        let mut src: Vec<u8> = Vec::with_capacity(tw * th);
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                src.push(density[iy * dim + ix].load(Ordering::Relaxed));
            }
        }
        let mut grassy = 0u64;
        for ly in 0..(th) {
            let iy = iy0 + ly;
            for lx in 0..(tw) {
                let ix = ix0 + lx;
                let s = src[ly * tw + lx];
                if s == 0 {
                    continue;
                }
                grassy += 1;
                let c = iy * dim + ix;
                let cid = c as u64;
                let _r1 = grass_unit(world_seed, cid, tick, 1) < decay_pct;
                let _r2 = grass_unit(world_seed, cid, tick, 2) < spread_pct;
                let _r3 = grass_unit(world_seed, cid, tick, 3);
                let _r4 = grass_unit(world_seed, cid, tick, 4);
                black_box((_r1, _r2, _r3, _r4));
            }
        }
        counters.body_self_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        counters.body_calls.fetch_add(1, Ordering::Relaxed);
        grassy
    }).sum()
}

fn variant_freeze_alloc_instr_par(density: &[AtomicU8], tiles: &[(usize, usize, usize, usize, usize)],
    dim: usize, counters: &SharedCounters) -> u64 {
    tiles.par_iter().map(|&(_, ix0, iy0, ix1, iy1)| {
        let t0 = Instant::now();
        let tw = ix1 - ix0;
        let th = iy1 - iy0;
        let mut src: Vec<u8> = Vec::with_capacity(tw * th);
        for iy in iy0..iy1 {
            for ix in ix0..ix1 {
                src.push(density[iy * dim + ix].load(Ordering::Relaxed));
            }
        }
        let grassy = src.iter().filter(|&&s| s > 0).count() as u64;
        black_box(grassy);
        counters.body_self_us.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        counters.body_calls.fetch_add(1, Ordering::Relaxed);
        grassy
    }).sum()
}

// ── Benchmark groups ──────────────────────────────────────────────────────────

fn bench_attribution(c: &mut Criterion) {
    let params = ScatterParams::default();
    let decay_byte = encode_u8(params.decay_amount);
    let spread_byte = encode_u8(params.spread_amount);

    for (label, seed_count) in &[("frontier_6pct", FRONTIER_SEEDS), ("dense_90pct", DENSE_SEEDS)] {
        let base = make_grid(*seed_count);
        let dim = base.dims.grass_dim;
        let tiles = active_tiles(&base);
        let grassy_cells = count_grassy_cells(&base, &tiles);
        let total_cells: u64 = tiles.iter()
            .map(|&(_, ix0, iy0, ix1, iy1)| ((ix1 - ix0) * (iy1 - iy0)) as u64)
            .sum();
        eprintln!(
            "[{label}] active_tiles={}, grassy_cells={grassy_cells}, tile_cells={total_cells}",
            tiles.len()
        );

        let mut group = c.benchmark_group(format!("grass_attr_{label}"));
        // Shorter measurement to keep the bench run tractable.
        group.measurement_time(std::time::Duration::from_secs(3));
        group.sample_size(20);

        // ── freeze only (O(tile) floor) ─────────────────────────────────────
        group.bench_function("freeze", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            b.iter(|| {
                black_box(variant_freeze(&density, &tiles, dim))
            });
        });

        // ── freeze + alloc ──────────────────────────────────────────────────
        group.bench_function("freeze_alloc", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            b.iter(|| {
                black_box(variant_freeze_alloc(&density, &tiles, dim))
            });
        });

        // ── freeze + alloc + instrumentation ───────────────────────────────
        group.bench_function("freeze_alloc_instr", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            let counters = SharedCounters::new();
            b.iter(|| {
                black_box(variant_freeze_alloc_instr(&density, &tiles, dim, &counters))
            });
        });

        // ── + 4 hashes per grassy cell ──────────────────────────────────────
        group.bench_function("rng_4hash", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            let counters = SharedCounters::new();
            b.iter(|| {
                black_box(variant_rng_4hash(
                    &density, &tiles, dim, 42, 1, &counters,
                    params.decay_pct, params.spread_pct))
            });
        });

        // ── + 1 hash bit-sliced (fused alternative) ─────────────────────────
        group.bench_function("rng_1hash", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            let counters = SharedCounters::new();
            b.iter(|| {
                black_box(variant_rng_1hash(
                    &density, &tiles, dim, 42, 1, &counters,
                    params.decay_pct, params.spread_pct))
            });
        });

        // ── + RMW (scatter_add/scatter_sub) ─────────────────────────────────
        group.bench_function("rng_4hash_rmw", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            let counters = SharedCounters::new();
            b.iter(|| {
                black_box(variant_rng_4hash_rmw(
                    &density, &tiles, dim, 42, 1, &counters,
                    params.decay_pct, params.spread_pct,
                    decay_byte, spread_byte))
            });
        });

        // ── geometric-skip spread (C4 candidate) ────────────────────────────
        group.bench_function("geom_skip", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            let counters = SharedCounters::new();
            b.iter(|| {
                black_box(variant_geom_skip(
                    &density, &tiles, dim, 42, 1, &counters,
                    params.decay_pct, params.spread_pct))
            });
        });

        // ── par: freeze+alloc+instr (rayon shim — expose false-sharing) ─────
        group.bench_function("par_freeze_alloc_instr", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            let counters = SharedCounters::new();
            b.iter(|| {
                black_box(variant_freeze_alloc_instr_par(&density, &tiles, dim, &counters))
            });
        });

        // ── par: 4-hash (rayon shim — RNG + false-sharing) ──────────────────
        group.bench_function("par_rng_4hash", |b| {
            let density: Vec<AtomicU8> = base.density_u8_snapshot()
                .iter().map(|&v| AtomicU8::new(v)).collect();
            let counters = SharedCounters::new();
            b.iter(|| {
                black_box(variant_rng_4hash_par(
                    &density, &tiles, dim, 42, 1, &counters,
                    params.decay_pct, params.spread_pct))
            });
        });

        group.finish();
    }
}

criterion_group!(benches, bench_attribution);
criterion_main!(benches);
