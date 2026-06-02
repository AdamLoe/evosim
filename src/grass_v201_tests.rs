//! v2.0.1 GRASS wave tests — organic spread (#5) + active-tile frontier &
//! dirty-tile snapshot perf (#6). Kept in its OWN file (merge-hazard rule;
//! never appended to the shared `grass::tests` module).
//!
//! Scope note: §3's *u16 internal storage* migration was deferred (it forces
//! edits far outside the permitted file scope — see the wave report). The §3
//! snapshot win (re-quantize only dirty tiles, both slots stay valid) IS
//! implemented on the f32 path and is exercised here by the dirty-tile-quantize
//! equivalence + both-slots-valid tests; `bilinear_sample`/`consume` parity is
//! covered against a full reference quantize. When u16 lands, the parity test
//! below should be tightened to the u16 LSB tolerance.

use super::*;
use crate::constants::WorldDims;
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
};

// WALLED grid for boundary-sensitive equivalence checks. 128² = 4×4 tiles.
const DIMS_WALLED: WorldDims = WorldDims {
    world_size: 640.0,
    wrap_world: false,
    grass_dim: 128,
    grass_cell_count: 128 * 128,
    hash_dim: 64,
};

fn make_grid(dims: WorldDims) -> GrassGrid {
    let tpa = dims.grass_dim.div_ceil(GRASS_TILE_SIZE);
    let tile_words = (tpa * tpa).div_ceil(64);
    let n = dims.grass_cell_count;
    GrassGrid {
        dims,
        density: vec![0.0f32; n],
        capacity: vec![GRASS_MAX; n],
        scratch: vec![0.0f32; n],
        row_has_density: vec![0u64; dims.grass_dim.div_ceil(64)],
        tiles_per_axis: tpa,
        tile_active: vec![0u64; tile_words],
        tile_active_next: vec![0u64; tile_words],
        tile_dirty: [vec![0u64; tile_words], vec![0u64; tile_words]],
        tile_class: vec![TILE_EMPTY; tpa * tpa],
        par_chunks_us: AtomicU64::new(0),
        chunks_mut_us: AtomicU64::new(0),
        row_body_us: AtomicU64::new(0),
        row_body_self_us: AtomicU64::new(0),
        row_body_calls: AtomicU64::new(0),
        dispatch_calls: AtomicU64::new(0),
    }
}

fn seed_center(g: &mut GrassGrid) {
    let dim = g.dims.grass_dim;
    let c = (dim / 2) * dim + dim / 2;
    g.density[c] = GRASS_MAX;
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();
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
        g.density[wy * dim + wx]
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
    assert!(g.density.iter().all(|&d| d == 0.0));
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
        fast.density[idx] = GRASS_MAX;
        refg.density[idx] = GRASS_MAX;
    }
    fast.resync_active_from_density();
    fast.mark_all_tiles_dirty();

    for t in 0..ticks {
        fast.compute_propagation(r, k);
        refg.compute_propagation_full(r, k);
        assert_eq!(
            fast.density, refg.density,
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
            fast.density[iy * dim + ix] = GRASS_MAX;
            refg.density[iy * dim + ix] = GRASS_MAX;
        }
    }
    fast.resync_active_from_density();
    fast.mark_all_tiles_dirty();
    refg.resync_active_from_density();

    let r = 0.02;
    let k = 0.3;
    for t in 0..25 {
        // Graze: drain the same cell on both grids each tick (inside the block).
        let cell = (50 + t) * dim + (50 + t);
        if cell < n {
            // fast: through consume (reactivates the tile)
            fast.consume(cell, 0.5, 1.0);
            // ref: drain directly + resync the whole active set from density
            let cur = refg.density[cell];
            refg.density[cell] = (cur - cur.min(0.5)).max(0.0);
        }
        fast.compute_propagation(r, k);
        refg.compute_propagation_full(r, k);
        assert_eq!(
            fast.density, refg.density,
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
        a.density[idx] = GRASS_MAX;
        b.density[idx] = GRASS_MAX;
    }
    a.resync_active_from_density();
    b.resync_active_from_density();
    for t in 0..40 {
        a.compute_propagation(0.03, 0.4);
        b.compute_propagation(0.03, 0.4);
        assert_eq!(a.density, b.density, "non-deterministic at tick {t}");
    }
}

// ─── §3: dirty-tile snapshot quantize parity + both-slots-valid ─────────────

/// Full reference quantize (every cell), matching the legacy `quantize_grass_into`.
fn full_quantize(g: &GrassGrid) -> Vec<u8> {
    let inv_max = 1.0 / GRASS_MAX;
    g.density
        .iter()
        .map(|&d| {
            let q = (d * inv_max).clamp(0.0, 1.0) * 255.0;
            (q + 0.5) as u8
        })
        .collect()
}

/// After stepping with dirty-tile-only re-quantize into a single slot, that slot
/// must equal a full quantize of the current density (the dirty set must have
/// covered every changed cell).
#[test]
fn dirty_tile_quantize_matches_full() {
    let dims = DIMS_WRAP;
    let n = dims.grass_cell_count;
    let mut g = make_grid(dims);
    let mut rng = SimRng::from_u64(11);
    for _ in 0..150 {
        g.density[rng.index(n)] = GRASS_MAX;
    }
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    let mut slot0 = vec![0u8; n];
    // First write: full region (everything dirty at init).
    g.quantize_dirty_tiles_into(&mut slot0, 0);
    assert_eq!(
        slot0,
        full_quantize(&g),
        "initial slot0 must be full+correct"
    );

    for t in 0..30 {
        g.compute_propagation(0.02, 0.3);
        g.quantize_dirty_tiles_into(&mut slot0, 0);
        assert_eq!(
            slot0,
            full_quantize(&g),
            "dirty-tile slot0 diverged from full quantize at tick {t}"
        );
    }
}

/// BOTH ping-pong slots must remain valid after dirty-only writes that alternate
/// between slots: each slot, after being written this tick, equals a full
/// quantize of the current density. The per-slot dirty tracking must keep a tile
/// dirty for a slot until THAT slot has been refreshed.
#[test]
fn both_slots_valid_after_dirty_writes() {
    let dims = DIMS_WRAP;
    let n = dims.grass_cell_count;
    let mut g = make_grid(dims);
    let mut rng = SimRng::from_u64(13);
    for _ in 0..150 {
        g.density[rng.index(n)] = GRASS_MAX;
    }
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    let mut slots = [vec![0u8; n], vec![0u8; n]];
    // Prime both slots with a full write.
    g.quantize_dirty_tiles_into(&mut slots[0], 0);
    g.quantize_dirty_tiles_into(&mut slots[1], 1);

    for t in 0..30 {
        g.compute_propagation(0.02, 0.3);
        // The worker alternates slots each tick. Write only the slot for this
        // tick, then assert THAT slot is fully valid (the other holds last tick's
        // valid bytes, which is the ping-pong contract).
        let slot = t & 1;
        g.quantize_dirty_tiles_into(&mut slots[slot], slot);
        assert_eq!(
            slots[slot],
            full_quantize(&g),
            "slot {slot} invalid after dirty write at tick {t}"
        );
    }
}

// ─── §3: consume / bilinear_sample contract preserved ───────────────────────

/// `consume` still drains the cell and returns the proportional energy exactly,
/// and now also reactivates the cell's tile (so the snapshot re-quantizes it).
#[test]
fn consume_drains_and_marks_dirty() {
    let dims = DIMS_WRAP;
    let mut g = make_grid(dims);
    let dim = dims.grass_dim;
    let cell = 50 * dim + 50;
    g.density[cell] = GRASS_MAX;
    g.resync_active_from_density();
    // Clear dirty so we can observe the graze re-dirtying.
    for slot in 0..2 {
        for w in g.tile_dirty[slot].iter_mut() {
            *w = 0;
        }
    }
    let got = g.consume(cell, 0.5, 10.0);
    assert!(
        (got - 10.0).abs() < 1e-6,
        "ripe consume returns full energy"
    );
    assert!(
        (g.density[cell] - (GRASS_MAX - 0.5)).abs() < 1e-6,
        "cell drained"
    );
    let t = (50 / GRASS_TILE_SIZE) * g.tiles_per_axis + 50 / GRASS_TILE_SIZE;
    assert!(
        GrassGrid::bitset_get(&g.tile_dirty[0], t) && GrassGrid::bitset_get(&g.tile_dirty[1], t),
        "grazed tile must be marked dirty for both slots"
    );
    assert!(
        GrassGrid::bitset_get(&g.tile_active, t),
        "grazed tile must be reactivated for the next propagation pass"
    );
}

/// `bilinear_sample` still reads the f32 density field correctly (contract
/// unchanged by the wave).
#[test]
fn bilinear_sample_contract_unchanged() {
    let mut g = make_grid(DIMS_WRAP);
    g.density[5] = 0.42;
    let x = 5.5 * GRASS_CELL_SIZE;
    let y = 0.5 * GRASS_CELL_SIZE;
    assert!((g.bilinear_sample(x, y) - 0.42).abs() < 1e-5);
}
