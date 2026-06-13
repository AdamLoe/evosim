//! v2.0.1 GRASS wave tests — organic spread and active-tile frontier.

use super::super::*;
use crate::constants::{WorldDims, GRASS_CELL_SIZE};
use crate::rng::SimRng;

// Grids are kept small so the debug-mode full-grid reference scan in the
// equivalence tests stays fast. Each is several tiles wide (tile = 32 cells).

// WRAPPED grid for disc-shape + most equivalence checks. 128² = 4×4 tiles.
const DIMS_WRAP: WorldDims = WorldDims {
    world_size: 640.0,
    wrap_world: true,
    grass_dim: 128,
    grass_cell_count: 128 * 128,
    hash_dim: 64,
    grass_cell_size: 5.0,
};

// WALLED grid for boundary-sensitive equivalence checks. 128² = 4×4 tiles.
const DIMS_WALLED: WorldDims = WorldDims {
    world_size: 640.0,
    wrap_world: false,
    grass_dim: 128,
    grass_cell_count: 128 * 128,
    hash_dim: 64,
    grass_cell_size: 5.0,
};

fn make_grid(dims: WorldDims) -> GrassGrid {
    let tpa = dims.grass_dim.div_ceil(GRASS_TILE_SIZE);
    let tile_words = (tpa * tpa).div_ceil(64);
    let n = dims.grass_cell_count;
    GrassGrid {
        dims,
        // v2.0.2 Stream 1a: density is `AtomicU8`, zero-filled.
        density: (0..n).map(|_| AtomicU8::new(0)).collect(),
        capacity: vec![GRASS_MAX; n],
        scratch: vec![0.0f32; n],
        row_has_density: vec![0u64; dims.grass_dim.div_ceil(64)],
        tiles_per_axis: tpa,
        tile_active: (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
        tile_active_next: (0..tile_words).map(|_| AtomicU64::new(0)).collect(),
        tile_class: vec![TILE_EMPTY; tpa * tpa],
        // v2.0.2 Stream 1b: these v2.0.1 equivalence/disc tests exercise the BLUR
        // path; default the grid to Blur so their assertions are unchanged.
        propagation: GrassPropagation::Blur,
        world_seed: 0,
        scatter_tick: 0,
        // v2.0.2 Stream 1d: scatter params default to working constants.
        scatter_params: ScatterParams::default(),
        disc: default_disc_table(),
        par_chunks_us: AtomicU64::new(0),
        chunks_mut_us: AtomicU64::new(0),
        row_body_us: AtomicU64::new(0),
        row_body_self_us: AtomicU64::new(0),
        row_body_calls: AtomicU64::new(0),
        dispatch_calls: AtomicU64::new(0),
        pyramid: GrassPyramid::new(dims.grass_dim),
        // v2.0.4 C2: profiling gate — off by default.
        profile_scatter: false,
        scatter_prev: Vec::new(),
        deterministic_science_mode: false,
        scatter_add_delta: Vec::new(),
        scatter_decay_delta: Vec::new(),
    }
}

fn seed_center(g: &mut GrassGrid) {
    let dim = g.dims.grass_dim;
    let c = (dim / 2) * dim + dim / 2;
    g.dset(c, GRASS_MAX);
    g.resync_active_from_density();
}

// ─── §1: organic disc-shape regression (NOT a diamond) ──────────────────────

/// Seed one centre cell, step many times with propagation, then measure the
/// frontier extent along the AXES vs along the DIAGONALS. A diamond (the old L1
/// kernel) has axis reach ≈ √2 × diagonal reach; a disc has them roughly equal.
/// We assert the ratio is close to 1 (organic) and far from √2 (diamond).
#[test]
fn spread_is_disc_not_diamond() {
    let mut g = make_grid(DIMS_WRAP);
    seed_center(&mut g);
    let dim = g.dims.grass_dim;
    let cx = (dim / 2) as i64;
    let cy = (dim / 2) as i64;

    // Strong propagation so the front advances visibly; modest logistic.
    for _ in 0..28 {
        g.compute_propagation(0.02, 0.5);
    }

    let thr = 0.01f32;
    let at = |ix: i64, iy: i64| -> f32 {
        let wx = ix.rem_euclid(dim as i64) as usize;
        let wy = iy.rem_euclid(dim as i64) as usize;
        g.dget(wy * dim + wx)
    };
    // Reach east along +x axis.
    let mut axis_reach = 0i64;
    for r in 1..(dim as i64 / 2) {
        if at(cx + r, cy) > thr {
            axis_reach = r;
        } else {
            break;
        }
    }
    // Reach along the +x/+y diagonal (Chebyshev step r,r).
    let mut diag_reach = 0i64;
    for r in 1..(dim as i64 / 2) {
        if at(cx + r, cy + r) > thr {
            diag_reach = r;
        } else {
            break;
        }
    }
    assert!(
        axis_reach > 3 && diag_reach > 3,
        "front should have advanced several cells; axis={axis_reach} diag={diag_reach}"
    );
    // Euclidean diagonal distance is √2·diag_reach. For a disc that should ≈
    // axis_reach. For a diamond, the diagonal Euclidean reach is ≈ axis_reach/√2
    // (corners cut), i.e. diag_reach ≈ axis_reach/√2 ≈ 0.707·axis_reach.
    let diag_euclid = (diag_reach as f32) * std::f32::consts::SQRT_2;
    let ratio = diag_euclid / axis_reach as f32;
    assert!(
        ratio > 0.85,
        "diagonal reach must be disc-like (≈ axis), got ratio {ratio} \
         (a diamond would be ≈0.71); axis={axis_reach} diag={diag_reach}"
    );
}

/// The all-zero neighbourhood must stay zero (blur of zero is zero → no
/// spontaneous spawn), preserved by the new kernel.
#[test]
fn empty_neighbourhood_stays_zero() {
    let mut g = make_grid(DIMS_WALLED);
    for _ in 0..20 {
        g.compute_propagation(0.5, 0.5);
    }
    assert!(g.density_u8_snapshot().iter().all(|&b| b == 0));
}

// ─── §2: active-tile == full-grid bit-equivalence ───────────────────────────

/// The frontier (active-tile) path must produce a BIT-IDENTICAL density field to
/// a full-grid reference scan over N ticks, on the same seed. Skipping
/// equilibrium tiles must be a pure optimization, never a value change.
fn assert_active_equals_full(dims: WorldDims, r: f32, k: f32, ticks: usize) {
    let mut seeded_rng = SimRng::from_u64(0xC0FFEE);
    // Build a non-trivial seeded field on both grids identically.
    let mut fast = make_grid(dims);
    let mut refg = make_grid(dims);
    let n = dims.grass_cell_count;
    for _ in 0..200 {
        let idx = seeded_rng.index(n);
        fast.dset(idx, GRASS_MAX);
        refg.dset(idx, GRASS_MAX);
    }
    fast.resync_active_from_density();

    for t in 0..ticks {
        fast.compute_propagation(r, k);
        refg.compute_propagation_full(r, k);
        assert_eq!(
            fast.density_u8_snapshot(),
            refg.density_u8_snapshot(),
            "active-tile path diverged from full-grid scan at tick {t}"
        );
    }
}

#[test]
fn active_tile_equals_full_grid_wrapped() {
    assert_active_equals_full(DIMS_WRAP, 0.02, 0.3, 30);
}

#[test]
fn active_tile_equals_full_grid_walled() {
    assert_active_equals_full(DIMS_WALLED, 0.02, 0.3, 30);
}

/// Equivalence must hold even with grazing (graze drains cells and must
/// reactivate the grazed tile so it regrows — the reference applies the same
/// drain then a full scan).
#[test]
fn active_tile_equals_full_grid_with_grazing() {
    let dims = DIMS_WRAP;
    let n = dims.grass_cell_count;
    let dim = dims.grass_dim;
    let mut fast = make_grid(dims);
    let mut refg = make_grid(dims);
    // Saturate a block so we have saturated interiors that grazing punches holes
    // in (the case where reactivation matters).
    for iy in 40..96 {
        for ix in 40..96 {
            fast.dset(iy * dim + ix, GRASS_MAX);
            refg.dset(iy * dim + ix, GRASS_MAX);
        }
    }
    fast.resync_active_from_density();
    refg.resync_active_from_density();

    let r = 0.02;
    let k = 0.3;
    for t in 0..25 {
        // Graze: drain the same cell on both grids each tick (inside the block).
        let cell = (50 + t) * dim + (50 + t);
        if cell < n {
            // fast: through consume (reactivates the tile)
            fast.consume(cell, 0.5, 1.0);
            // ref: drain directly using the same u8-byte arithmetic as consume()
            // (v2.0.2 Stream 1e: consume now works in u8 bytes, not f32, so the
            // reference must match byte-for-byte to keep active == full-grid).
            // chunk_bytes = encode_density(0.5) = round(0.5/1.0*255) = 128.
            let cur_byte = refg.dget_u8(cell);
            let chunk_bytes: u8 = 128; // encode_density(0.5).max(1)
            let taken_bytes = cur_byte.min(chunk_bytes);
            // dset(decode(new_byte)) is exact: encode(n/255*GRASS_MAX) = n.
            let new_byte = cur_byte - taken_bytes;
            let new_val = (new_byte as f32 / 255.0) * GRASS_MAX;
            refg.dset(cell, new_val);
        }
        fast.compute_propagation(r, k);
        refg.compute_propagation_full(r, k);
        assert_eq!(
            fast.density_u8_snapshot(),
            refg.density_u8_snapshot(),
            "active-tile + graze diverged from full-grid scan at tick {t}"
        );
    }
}

// ─── §2: single-thread == rayon determinism (same build, same seed) ─────────

/// Two grids with the same seed stepped identically must stay bit-identical.
/// Under `--features threads` this exercises the rayon dispatch; under default
/// features it is the sequential path. The active-list is built in sorted tile
/// order so the dispatch is order-independent → identical either way.
#[test]
fn propagation_is_deterministic_same_seed() {
    let dims = DIMS_WRAP;
    let n = dims.grass_cell_count;
    let mut a = make_grid(dims);
    let mut b = make_grid(dims);
    let mut rng = SimRng::from_u64(7);
    for _ in 0..300 {
        let idx = rng.index(n);
        a.dset(idx, GRASS_MAX);
        b.dset(idx, GRASS_MAX);
    }
    a.resync_active_from_density();
    b.resync_active_from_density();
    for t in 0..40 {
        a.compute_propagation(0.03, 0.4);
        b.compute_propagation(0.03, 0.4);
        assert_eq!(
            a.density_u8_snapshot(),
            b.density_u8_snapshot(),
            "non-deterministic at tick {t}"
        );
    }
}

// ─── consume / bilinear_sample contract preserved ───────────────────────────

/// `consume` drains the cell, returns proportional energy, and reactivates its tile.
#[test]
fn consume_drains_and_reactivates() {
    let dims = DIMS_WRAP;
    let mut g = make_grid(dims);
    let dim = dims.grass_dim;
    let cell = 50 * dim + 50;
    g.dset(cell, GRASS_MAX);
    g.resync_active_from_density();
    let got = g.consume(cell, 0.5, 10.0);
    assert!(
        (got - 10.0).abs() < 1e-6,
        "ripe consume returns full energy"
    );
    // Stage 3: byte-exact assertion.
    // encode(GRASS_MAX) = 255, encode(0.5) = 128; drain = 255 - 128 = 127.
    // decode(127) = 127/255 * GRASS_MAX ≠ exactly 0.5, but byte 127 IS the
    // exact stored value after consume() subtracts chunk_bytes=128 from 255.
    assert_eq!(
        g.dget_u8(cell),
        127u8,
        "cell must be drained to exactly byte 127 (255 - 128)"
    );
    let t = (50 / GRASS_TILE_SIZE) * g.tiles_per_axis + 50 / GRASS_TILE_SIZE;
    assert!(
        GrassGrid::bitset_get(&g.tile_active, t),
        "grazed tile must be reactivated for the next propagation pass"
    );
}

/// `bilinear_sample` still reads the f32 density field correctly (contract
/// unchanged by the wave).
#[test]
fn bilinear_sample_contract_unchanged() {
    let g = make_grid(DIMS_WRAP);
    g.dset(5, 0.42);
    let x = 5.5 * GRASS_CELL_SIZE;
    let y = 0.5 * GRASS_CELL_SIZE;
    // Stage 3: byte-exact assertion.
    // dset(5, 0.42) stores encode(0.42) = 107; decode(107) = 107/255 * GRASS_MAX.
    // bilinear_sample at cell center returns dget(5) exactly (no interpolation weight
    // on the zero-valued neighbours), so the sample must equal dget(5) to f32 epsilon.
    let expected = g.dget(5); // decode(encode(0.42)) = decode(107)
    assert_eq!(g.dget_u8(5), 107u8, "dset(5, 0.42) must store byte 107");
    assert!(
        (g.bilinear_sample(x, y) - expected).abs() < 1e-6,
        "bilinear_sample at cell center must equal dget(5) exactly; got {}",
        g.bilinear_sample(x, y)
    );
}
