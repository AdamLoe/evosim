//! v2.0.3 Stage-3 scatter battery — focused per-feature deterministic tests.
//!
//! Lives in its OWN file (merge-hazard rule), wired as a child module of `grass`
//! so it can reach crate-internal fields/fns (`dget_u8`, `dset`, `set_propagation`,
//! `resync_active_from_density`, `scatter_tick`, `scatter_params`, etc.).
//!
//! All tests use `GrassPropagation::Scatter` and a fixed `world_seed` so they are
//! deterministic on a single thread (the scatter RNG is keyed on
//! `(world_seed, cell, tick)` via SplitMix64 — no cross-cell state).

use super::*;
use crate::constants::{
    Biome, WorldDims, GRASS_CAPACITY_DESERT, GRASS_CAPACITY_PLAINS, GRASS_CAPACITY_WATER, GRASS_MAX,
};
use crate::rng::SimRng;

// ── Grid helpers ────────────────────────────────────────────────────────────

/// Small wrapped grid: 128² = 4×4 tiles of 32 cells each.
/// Wrapped so spread has no walls to special-case; large enough to avoid
/// the patch touching itself through the seam for the battery run lengths.
const DIMS: WorldDims = WorldDims {
    world_size: 640.0,
    wrap_world: true,
    grass_dim: 128,
    grass_cell_count: 128 * 128,
    hash_dim: 64,
    grass_cell_size: 5.0,
};

const WORLD_SEED: u32 = 0xDEAD_5678;

/// Build a scatter grid with a custom biome map and params, seeded at
/// world_seed = WORLD_SEED. Caller populates density and calls
/// `resync_active_from_density()` + `mark_all_tiles_dirty()` afterwards.
fn make_scatter_grid_biome(biome: Option<&[u8]>, params: ScatterParams) -> GrassGrid {
    let mut rng = SimRng::from_u64(0xCAFE_BABE);
    let mut g = GrassGrid::new_with_capacity(&mut rng, 0, DIMS, biome);
    g.set_propagation(GrassPropagation::Scatter);
    g.world_seed = WORLD_SEED;
    g.scatter_params = params;
    g
}

/// Build an all-plains scatter grid with default scatter params.
fn make_scatter_grid() -> GrassGrid {
    make_scatter_grid_biome(None, ScatterParams::default())
}

/// Run N scatter ticks (advancing scatter_tick to match).
fn tick_n(g: &mut GrassGrid, n: u32) {
    let start = g.scatter_tick;
    for t in 0..n {
        g.scatter_tick = start + t;
        g.compute_propagation(0.0, 0.0); // r/k unused on scatter path
    }
}

/// Sum all u8 bytes as u64 (cheap total-density proxy).
fn total_bytes(g: &GrassGrid) -> u64 {
    g.density_u8_snapshot().iter().map(|&b| b as u64).sum()
}

// ── Test (a): disc_footprint_is_round ───────────────────────────────────────

/// Seed ONE cell in the centre of the grid, run many ticks with
/// spread_pct = 1.0 and decay_pct = 0.0 so the patch ONLY grows.
/// Measure the max reach along the cardinal axes vs along the 45° diagonals.
/// For the precomputed DISC table the reach should be approximately equal
/// (round footprint). A diamond (L∞ / L1 kernel) would have diagonal
/// Euclidean reach ≈ 0.71 × axis reach.
#[test]
fn disc_footprint_is_round() {
    let dim = DIMS.grass_dim;
    // Pure-spread params: no decay, high spread probability, many ticks.
    let params = ScatterParams {
        decay_pct: 0.0,
        decay_amount: 0.0,
        spread_pct: 1.0,
        spread_amount: GRASS_MAX, // deposit max amount so cells light up fast
        ..ScatterParams::default()
    };
    let mut g = make_scatter_grid_biome(None, params);

    // Seed centre cell.
    let cx = dim / 2;
    let cy = dim / 2;
    let center = cy * dim + cx;
    g.dset(center, GRASS_MAX);
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    // 60 ticks: enough for the front to advance several cell-radii.
    tick_n(&mut g, 60);

    let thr_byte: u8 = 1; // any nonzero byte counts as "reached"

    // Measure +x axis reach.
    let mut axis_reach_x = 0i64;
    for r in 1..(dim as i64 / 2) {
        let nx = ((cx as i64 + r).rem_euclid(dim as i64)) as usize;
        if g.dget_u8(cy * dim + nx) >= thr_byte {
            axis_reach_x = r;
        } else {
            break;
        }
    }

    // Measure +y axis reach.
    let mut axis_reach_y = 0i64;
    for r in 1..(dim as i64 / 2) {
        let ny = ((cy as i64 + r).rem_euclid(dim as i64)) as usize;
        if g.dget_u8(ny * dim + cx) >= thr_byte {
            axis_reach_y = r;
        } else {
            break;
        }
    }

    // Measure +x/+y diagonal reach (Chebyshev step).
    let mut diag_reach = 0i64;
    for r in 1..(dim as i64 / 2) {
        let nx = ((cx as i64 + r).rem_euclid(dim as i64)) as usize;
        let ny = ((cy as i64 + r).rem_euclid(dim as i64)) as usize;
        if g.dget_u8(ny * dim + nx) >= thr_byte {
            diag_reach = r;
        } else {
            break;
        }
    }

    // Take the mean axis reach so asymmetric grids still pass.
    let axis_reach = ((axis_reach_x + axis_reach_y) as f32 / 2.0).max(1.0);
    assert!(
        axis_reach > 2.0 && diag_reach > 2,
        "front must advance >2 cells; axis_x={axis_reach_x} axis_y={axis_reach_y} diag={diag_reach}"
    );

    // For a round disc the diagonal Euclidean distance = diag_reach × √2
    // should ≈ axis_reach. Ratio > 0.8 rejects diamonds (which give ≈ 0.71).
    let diag_euclid = diag_reach as f32 * std::f32::consts::SQRT_2;
    let ratio = diag_euclid / axis_reach;
    assert!(
        ratio > 0.80,
        "footprint must be disc-like (ratio > 0.80 rejects diamond ≈ 0.71); \
         ratio={ratio:.3} axis={axis_reach:.1} diag={diag_reach}"
    );
}

// ── Test (b): decay_floor_reaches_zero ──────────────────────────────────────

/// A lone cell with no inflow (surrounded by empty cells in a big grid, with
/// spread disabled) decays all the way to byte 0. Decay must NOT stall above 0.
#[test]
fn decay_floor_reaches_zero() {
    let dim = DIMS.grass_dim;
    // Pure-decay params: no spread so the cell gets no refill.
    let params = ScatterParams {
        decay_pct: 1.0,                        // guaranteed to roll decay every tick
        decay_amount: GRASS_MAX / 255.0 * 2.0, // 2 bytes/tick so ~128 ticks to zero
        spread_pct: 0.0,
        spread_amount: 0.0,
        ..ScatterParams::default()
    };
    let mut g = make_scatter_grid_biome(None, params);

    let cell = (dim / 2) * dim + (dim / 2);
    g.dset(cell, GRASS_MAX); // byte 255
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    // 300 ticks is far more than enough to drain 255 bytes at 2 bytes/tick.
    tick_n(&mut g, 300);

    assert_eq!(
        g.dget_u8(cell),
        0u8,
        "isolated cell must decay to byte 0 over 300 ticks; stuck at byte {}",
        g.dget_u8(cell)
    );

    // No spontaneous spawn: every cell in the grid must be 0 (spread_pct=0).
    let any_nonzero = g.density_u8_snapshot().iter().any(|&b| b > 0);
    assert!(
        !any_nonzero,
        "no spontaneous spawn: all cells must be 0 when spread is disabled"
    );
}

// ── Test (c): persistence_no_collapse ───────────────────────────────────────

/// A seeded plains patch under DEFAULT ScatterParams persists for 200 ticks.
/// Plains is super-critical (spread-in >> decay-out) so total density should
/// stay > 0 and ideally grow. The patch must NOT vanish.
#[test]
fn persistence_no_collapse() {
    let dim = DIMS.grass_dim;
    let mut g = make_scatter_grid(); // all-plains, default params

    // Seed a 6×6 block of plains cells at GRASS_MAX.
    let cx = dim / 2;
    let cy = dim / 2;
    let mut any_seeded = false;
    for dy in 0..6 {
        for dx in 0..6 {
            let nx = (cx + dx).min(dim - 1);
            let ny = (cy + dy).min(dim - 1);
            g.dset(ny * dim + nx, GRASS_MAX);
            any_seeded = true;
        }
    }
    assert!(any_seeded);
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    let total_before = total_bytes(&g);
    tick_n(&mut g, 200);
    let total_after = total_bytes(&g);

    assert!(
        total_after > 0,
        "plains patch must not collapse to zero after 200 ticks; total_before={total_before}"
    );
    // Plains is super-critical: net spread-in far exceeds net decay-out.
    // Total density should increase (or at worst hold) over 200 ticks.
    assert!(
        total_after >= total_before,
        "plains patch must persist or grow after 200 ticks; before={total_before} after={total_after}"
    );
}

// ── Test (d): biome_cap_clamp ────────────────────────────────────────────────

/// Over a long scatter run, no cell ever exceeds its biome cap byte.
/// Tests plains (cap=255), desert (cap=77), and water (cap=10) cells in the
/// same grid.
#[test]
fn biome_cap_clamp() {
    let dim = DIMS.grass_dim;

    // Build a mixed biome grid: most cells plains, a 4×4 desert block,
    // a 4×4 water block.
    let mut biome = vec![Biome::Plains as u8; DIMS.grass_cell_count];
    let desert_origin = (dim / 4, dim / 4);
    let water_origin = (3 * dim / 4, 3 * dim / 4);
    for dy in 0..4 {
        for dx in 0..4 {
            biome[(desert_origin.1 + dy) * dim + (desert_origin.0 + dx)] = Biome::Desert as u8;
            biome[(water_origin.1 + dy) * dim + (water_origin.0 + dx)] = Biome::Water as u8;
        }
    }

    let mut g = make_scatter_grid_biome(Some(&biome), ScatterParams::default());

    // Saturate each cell to its BIOME CAP (not GRASS_MAX) to stress the
    // scatter clamp: the spread kernel must not push any cell above its cap.
    for i in 0..DIMS.grass_cell_count {
        let cap_val = g.capacity[i];
        g.dset(i, cap_val);
    }
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    // 100 ticks.
    tick_n(&mut g, 100);

    // Plains cap = encode(1.0) = 255.
    // Desert cap = encode(0.30) = 77.
    // Water  cap = encode(0.04) = 10.
    let plains_cap_byte: u8 = {
        let q = GRASS_CAPACITY_PLAINS.clamp(0.0, 1.0) * 255.0;
        (q + 0.5) as u8
    };
    let desert_cap_byte: u8 = {
        let q = GRASS_CAPACITY_DESERT.clamp(0.0, 1.0) * 255.0;
        (q + 0.5) as u8
    };
    let water_cap_byte: u8 = {
        let q = GRASS_CAPACITY_WATER.clamp(0.0, 1.0) * 255.0;
        (q + 0.5) as u8
    };

    for i in 0..DIMS.grass_cell_count {
        let d_byte = g.dget_u8(i);
        let biome_tag = biome[i];
        let cap = match crate::world::biome::biome_from_u8(biome_tag) {
            Biome::Plains => plains_cap_byte,
            Biome::Desert => desert_cap_byte,
            Biome::Water => water_cap_byte,
        };
        assert!(
            d_byte <= cap,
            "cell {i} (biome={biome_tag}): byte {d_byte} exceeds cap byte {cap}"
        );
    }
}
