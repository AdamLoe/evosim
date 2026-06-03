//! v2.0.2 Stream 1b/1c — scatter kernel + frontier SMOKE test.
//!
//! Minor validation only (NOT the Stage-3 battery): build a small grid, select
//! the SCATTER kernel, tick ~50 times, and assert the field actually evolves and
//! stays sane. The exhaustive scatter battery (disc-footprint roundness, oracle
//! skip-exactness, decay-floor, persistence) is Stage 3.
//!
//! Lives in its OWN file (merge-hazard rule), wired as a child module of `grass`
//! so it can reach the crate-internal fields/fns it needs (`propagation`,
//! `scatter_tick`, `compute_propagation`, `resync_active_from_density`).

use super::*;
use crate::constants::{Biome, WorldDims, GRASS_CAPACITY_WATER};
use crate::world::biome::capacity_factor_from_u8;

// Small wrapped grid: 96² = 3×3 tiles (tile = 32). Wrapped so spread has no walls
// to special-case in this smoke check.
const DIMS: WorldDims = WorldDims {
    world_size: 480.0,
    wrap_world: true,
    grass_dim: 96,
    grass_cell_count: 96 * 96,
    hash_dim: 48,
};

/// Build a scatter grid: a partial-density plains patch (will grow + spread) and,
/// far away and isolated, a block of water cells at cap (sub-critical → erodes by
/// decay with no refill). Returns (grid, plains_patch_cells, water_block_cells).
fn build_scatter_grid() -> (GrassGrid, Vec<usize>, Vec<usize>) {
    let dim = DIMS.grass_dim;
    // Mostly plains; a scatter of ISOLATED water cells in the far quadrant, each
    // spaced ≥ 8 cells apart (> spread radius 3, so no water cell can ever spread
    // onto another). An isolated water cell is sub-critical: it has no grass
    // neighbour to refill it, and its own spread lands on plains (never back onto
    // itself), so it can only LOSE density via decay → it erodes over the run.
    let mut biome = vec![Biome::Plains as u8; DIMS.grass_cell_count];
    let mut water_cells_pos = Vec::new();
    let base = dim - 24;
    for gy in 0..3 {
        for gx in 0..3 {
            let wy = base + gy * 8;
            let wx = base + gx * 8;
            biome[wy * dim + wx] = Biome::Water as u8;
            water_cells_pos.push(wy * dim + wx);
        }
    }

    let mut rng = SimRng::from_u64(12345);
    let mut g = GrassGrid::new_with_capacity(&mut rng, 0, DIMS, Some(&biome));
    g.set_propagation(GrassPropagation::Scatter);
    g.world_seed = 0xC0FFEE;

    // Seed a dense plains patch (a 4×4 block) at partial density so the tiles
    // classify MIXED (active) and the patch can both grow inward and spread out.
    let cx = dim / 4;
    let cy = dim / 4;
    let mut patch = Vec::new();
    for py in cy..(cy + 4) {
        for px in cx..(cx + 4) {
            let c = py * dim + px;
            g.dset(c, 0.5); // plains cap = 1.0, so 0.5 is mid (MIXED tile)
            patch.push(c);
        }
    }

    // Seed each isolated water cell to its (tiny) water cap (byte 10). Using nine
    // cells makes the "decay fired somewhere" check robust against the per-cell
    // roll probability (P(no decay across 9 cells × 50 ticks) is negligible).
    let mut water_block = Vec::new();
    for &c in &water_cells_pos {
        g.dset(c, GRASS_CAPACITY_WATER);
        water_block.push(c);
    }

    // Sync the active/class state to the seeded field so the first scatter tick
    // processes the seeded tiles (production drives this incrementally).
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();
    (g, patch, water_block)
}

/// Sum the raw u8 density bytes across the whole field (cheap "total density").
fn total_density_u8(g: &GrassGrid) -> u64 {
    g.density_u8_snapshot().iter().map(|&b| b as u64).sum()
}

/// (a) no panic, (b) total density changes over the run, (c) field stays a valid
/// non-degenerate u8 field (not all-zero, not all-saturated), (d) the plains
/// patch grows/spreads to neighbours while the isolated water cell decays.
#[test]
fn scatter_smoke_evolves_grows_and_decays() {
    let dim = DIMS.grass_dim;
    let (mut g, patch, water_cells) = build_scatter_grid();

    // Snapshot the initial state for the grow/decay/change assertions.
    let total_before = total_density_u8(&g);
    let water_before: f32 = water_cells.iter().map(|&c| g.dget(c)).sum();

    // Count grass cells in the 8-neighbour fringe of the patch BEFORE (should be
    // ~0 — the patch is a tight block) so we can prove spread reached new cells.
    let mut fringe = std::collections::BTreeSet::new();
    for &c in &patch {
        let ix = c % dim;
        let iy = c / dim;
        for ddy in -2i64..=2 {
            for ddx in -2i64..=2 {
                let nx = (ix as i64 + ddx).rem_euclid(dim as i64) as usize;
                let ny = (iy as i64 + ddy).rem_euclid(dim as i64) as usize;
                let nc = ny * dim + nx;
                if !patch.contains(&nc) {
                    fringe.insert(nc);
                }
            }
        }
    }
    let fringe_grass_before = fringe.iter().filter(|&&c| g.dget(c) > 0.0).count();

    // Track the MINIMUM total water density reached across the run. Decay erodes
    // the isolated water cells; the surrounding plains halo they seed can later
    // refill them back toward cap (a real, plan-described dynamic — "grass creeps
    // one cell past the shore"), so the *end* total may recover. The robust,
    // faithful witness of "isolated water decays" is that the total DIPS below its
    // start at some point (decay fired and momentarily out-paced any refill).
    let mut water_min = water_before;

    // ~50 ticks of pure scatter (drive compute_propagation directly, advancing
    // the scatter RNG tick each step, exactly as the live World path does).
    for t in 0..50u32 {
        g.scatter_tick = t;
        g.compute_propagation(0.0, 0.0); // r/k unused on the scatter path
                                         // (a) no panic is implicit; also keep the row bitset coherent.
        g.rebuild_row_bitset();
        let water_now: f32 = water_cells.iter().map(|&c| g.dget(c)).sum();
        if water_now < water_min {
            water_min = water_now;
        }
    }

    let total_after = total_density_u8(&g);
    let fringe_grass_after = fringe.iter().filter(|&&c| g.dget(c) > 0.0).count();

    // (c) every byte is trivially in [0,255]; assert the field is NON-DEGENERATE:
    // not all-zero (it didn't collapse to nothing) and not all-saturated.
    let bytes = g.density_u8_snapshot();
    let any_grass = bytes.iter().any(|&b| b > 0);
    let any_below_max = bytes.iter().any(|&b| b < 255);
    assert!(
        any_grass,
        "scatter field must not be all-zero after 50 ticks"
    );
    assert!(
        any_below_max,
        "scatter field must not be fully saturated after 50 ticks"
    );

    // (b) total density actually changed over the run (the field is not frozen).
    assert_ne!(
        total_before, total_after,
        "total density must change over the scatter run (spread/decay active)"
    );

    // (d-grow/spread): the plains patch spread into previously-empty fringe cells.
    assert!(
        fringe_grass_after > fringe_grass_before,
        "plains patch must spread to new neighbour cells; before={fringe_grass_before} after={fringe_grass_after}"
    );

    // (d-decay): the isolated water cells decayed — their total density dipped
    // below the seeded value at some point in the run (decay fired and eroded
    // them; whether a plains halo later refills them is a separate dynamic).
    assert!(
        water_min < water_before,
        "isolated water cells must decay at some point; min={water_min} before={water_before}"
    );

    // Sanity: a water cell can never exceed its (tiny) cap via the scatter clamp.
    let water_cap = capacity_factor_from_u8(Biome::Water as u8) * GRASS_MAX;
    for &c in &water_cells {
        let d = g.dget(c);
        assert!(
            d <= water_cap + 1.0 / 255.0,
            "water density {d} exceeded water cap {water_cap}"
        );
    }
}
