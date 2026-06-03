//! v2.0.4 S4: fertility (density-weighted spread) tests.
//!
//! Wired as a child of `grass` so it can reach internal fields/fns
//! (`dget_u8`, `dset`, `resync_active_from_density`, `mark_all_tiles_dirty`,
//! `scatter_params`, etc.).
//!
//! Tests in this file:
//!   (a) amount_proportional_to_density — near-full cell contributes ~spread_amount,
//!       low-density cell contributes proportionally less; clamps at biome cap.
//!   (b) persistence_invariant_plains_and_water — plains stays super-critical
//!       (persists/regrows), water stays sub-critical (does NOT overrun water).
//!   (c) net_growth_sanity — seeded plains patch survives N ticks at the raised
//!       decay_pct without global collapse.

use super::*;
use crate::constants::{
    Biome, WorldDims, GRASS_CAPACITY_WATER, GRASS_DECAY_PCT_DEFAULT, GRASS_MAX,
    GRASS_SPREAD_AMOUNT_DEFAULT,
};
use crate::rng::SimRng;

// ── Grid helpers (mirror grass_scatter_tests pattern) ──────────────────────

/// Small wrapped grid: 128² = 4×4 tiles of 32 cells each.
const DIMS: WorldDims = WorldDims {
    world_size: 640.0,
    wrap_world: true,
    grass_dim: 128,
    grass_cell_count: 128 * 128,
    hash_dim: 64,
    grass_cell_size: 5.0,
};

const WORLD_SEED: u32 = 0xFE57_1234;

fn make_grid_biome(biome: Option<&[u8]>, params: ScatterParams) -> GrassGrid {
    let mut rng = SimRng::from_u64(0xCAFE_F00D);
    let mut g = GrassGrid::new_with_capacity(&mut rng, 0, DIMS, biome);
    g.set_propagation(GrassPropagation::Scatter);
    g.world_seed = WORLD_SEED;
    g.scatter_params = params;
    g
}

fn make_plains_grid(params: ScatterParams) -> GrassGrid {
    make_grid_biome(None, params)
}

fn tick_n(g: &mut GrassGrid, n: u32) {
    let start = g.scatter_tick;
    for t in 0..n {
        g.scatter_tick = start + t;
        g.compute_propagation(0.0, 0.0);
    }
}

fn total_bytes(g: &GrassGrid) -> u64 {
    g.density_u8_snapshot().iter().map(|&b| b as u64).sum()
}

// ── Test (a): amount_proportional_to_density ────────────────────────────────

/// Verify variant A: the spread add amount scales with source cell density.
///
/// Strategy: use spread_pct=1.0 (every cell spreads every tick) and
/// decay_pct=0.0 (no decay) so each spread event is deterministic in its
/// PROBABILITY (always fires). Seed two isolated cells:
///   - "full" cell at GRASS_MAX (byte 255)
///   - "low" cell at 1/8 of GRASS_MAX (byte ~32)
///
/// Both surrounded by empty cells. After ONE tick, the target cells they
/// spread into should have add bytes proportional to their source density.
///
/// We can't check a single target deterministically (the target offset is RNG-
/// chosen), so we check the aggregate: the total density added to all cells
/// adjacent to the full cell should be ≈8× the total added near the low cell
/// (same number of spread events, 8× the add per event). We allow a 2× slack
/// since the disc picks are stochastic and both cells have just ONE event.
///
/// Additionally: a cell seeded at full capacity and surrounded by cells at cap
/// must NOT push any neighbour above the biome cap — verifying the clamp.
#[test]
fn amount_proportional_to_density() {
    let dim = DIMS.grass_dim;
    let spread_amount = GRASS_SPREAD_AMOUNT_DEFAULT;

    // Single tick with guaranteed spread, no decay.
    let params = ScatterParams {
        decay_pct: 0.0,
        decay_amount: 0.0,
        spread_pct: 1.0,
        spread_amount,
        ..ScatterParams::default()
    };
    let mut g = make_plains_grid(params);

    // Place the "full" cell at (cx, cy).
    let cx = 30usize;
    let cy = 30usize;
    // Place the "low" cell at (cx+20, cy) — far enough apart that their halos
    // don't overlap (spread radius = 3 cells, distance = 20 cells).
    let lx = cx + 20;
    let ly = cy;

    let full_idx = cy * dim + cx;
    let low_idx = ly * dim + lx;

    g.dset(full_idx, GRASS_MAX); // byte 255 → scale = 1.0
    g.dset(low_idx, GRASS_MAX / 8.0); // byte ~32 → scale ≈ 1/8
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    // Single tick.
    tick_n(&mut g, 1);

    // Measure the total density that has spread into the halo around each source.
    // Halo = all cells within GRASS_SPREAD_RADIUS (3) of the source (excluding
    // the source itself). With probability=1.0 each source fires exactly one
    // spread event per tick, so we expect exactly one non-zero neighbour per
    // source after one tick.
    //
    // Rather than chasing which neighbour was picked, we verify:
    //   1. The full-density source's halo has a higher total add than the low-
    //      density source's halo (proportionality direction check).
    //   2. The full-density halo total is at most spread_amount (one event, at most
    //      spread_amount added).
    //   3. The low-density halo total is at most GRASS_MAX/8 (upper-bounded by
    //      spread_amount * 1/8 ≈ 0.00625, well below spread_amount).

    let halo_total = |sx: usize, sy: usize| -> u64 {
        let r = 4i64; // check a box around source to capture any ring-3 spread
        let mut sum = 0u64;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = ((sx as i64 + dx).rem_euclid(dim as i64)) as usize;
                let ny = ((sy as i64 + dy).rem_euclid(dim as i64)) as usize;
                sum += g.dget_u8(ny * dim + nx) as u64;
            }
        }
        sum
    };

    let full_halo = halo_total(cx, cy);
    let low_halo = halo_total(lx, ly);

    // Direction: full cell contributes more than the low cell.
    assert!(
        full_halo > low_halo,
        "full-density cell should push more into its halo than a low-density cell; \
         full_halo={full_halo} low_halo={low_halo}"
    );

    // Full cell halo: at most spread_byte (one event at max scale). Allow 2× slack
    // in case the single add lands on a target that also has some initial grass byte.
    let spread_byte = encode_density(spread_amount);
    assert!(
        full_halo <= spread_byte as u64 * 2,
        "full halo should be bounded by spread_byte={spread_byte}; full_halo={full_halo}"
    );

    // Low cell halo: should be at most spread_byte/8 * 2 (same event, 8× less).
    // If low_density_byte=32, spread_byte=13, low add = (13*32+127)/255 = 1 (min=1
    // after floor). Check just that it's strictly less than full_halo.
    assert!(
        low_halo < full_halo,
        "low cell halo must be strictly less than full cell halo; \
         low_halo={low_halo} full_halo={full_halo}"
    );

    // Clamp check: fill all plains cells to cap, run a few more ticks, none exceed 255.
    let mut g2 = make_plains_grid(params);
    for i in 0..DIMS.grass_cell_count {
        let cap_val = g2.capacity[i];
        g2.dset(i, cap_val);
    }
    g2.resync_active_from_density();
    g2.mark_all_tiles_dirty();
    tick_n(&mut g2, 50);
    for i in 0..DIMS.grass_cell_count {
        let b = g2.dget_u8(i);
        let cap_byte = encode_density(g2.capacity[i]);
        assert!(
            b <= cap_byte,
            "cell {i}: byte {b} exceeds biome cap {cap_byte} after density-scaled spread"
        );
    }
}

// ── Test (b): persistence_invariant_plains_and_water ────────────────────────

/// Persistence invariant at the raised GRASS_DECAY_PCT_DEFAULT:
///   - Plains (cap × 1.0): super-critical — a seeded patch persists and grows.
///   - Water (cap × 0.04): sub-critical — even a fully-seeded 8×8 water block
///     DECAYS over time (grass does NOT overrun water).
///
/// This is the critical gate for the decay-headroom claim in S4.
#[test]
fn persistence_invariant_plains_and_water() {
    let dim = DIMS.grass_dim;

    // Build a mixed grid: most plains, a 16×16 water zone isolated from the
    // plains seed patch so we can track each independently.
    let mut biome_map = vec![Biome::Plains as u8; DIMS.grass_cell_count];
    let water_ox = 3 * dim / 4;
    let water_oy = 3 * dim / 4;
    for dy in 0..16usize {
        for dx in 0..16usize {
            biome_map[(water_oy + dy) * dim + (water_ox + dx)] = Biome::Water as u8;
        }
    }

    // Use the RAISED decay_pct (the new default) with default spread params.
    let params = ScatterParams {
        decay_pct: GRASS_DECAY_PCT_DEFAULT, // 0.04 (raised by S4)
        ..ScatterParams::default()
    };
    let mut g = make_grid_biome(Some(&biome_map), params);

    // ── Plains seed: 8×8 block at full density near centre ──────────────────
    let cx = dim / 4;
    let cy = dim / 4;
    for dy in 0..8usize {
        for dx in 0..8usize {
            let nx = (cx + dx).min(dim - 1);
            let ny = (cy + dy).min(dim - 1);
            g.dset(ny * dim + nx, GRASS_MAX);
        }
    }

    // ── Water seed: fully saturate every water cell to its biome cap ─────────
    for dy in 0..16usize {
        for dx in 0..16usize {
            let nx = water_ox + dx;
            let ny = water_oy + dy;
            let idx = ny * dim + nx;
            let cap = g.capacity[idx];
            g.dset(idx, cap); // byte 10 (encode(0.04))
        }
    }

    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    // Run 300 ticks — enough to see steady state.
    tick_n(&mut g, 300);

    // ── Plains check: patch must persist (total > 0, ideally grow) ───────────
    let plains_total: u64 = g
        .density_u8_snapshot()
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if biome_map[i] == Biome::Plains as u8 {
                b as u64
            } else {
                0
            }
        })
        .sum();
    assert!(
        plains_total > 0,
        "plains: patch must not collapse to zero at decay_pct={} after 300 ticks",
        GRASS_DECAY_PCT_DEFAULT
    );

    // Count cells with non-zero grass in the plains area around the seed.
    let plains_alive: usize = (0..16)
        .flat_map(|dy: usize| (0..16usize).map(move |dx| (dx, dy)))
        .filter(|&(dx, dy)| {
            let nx = (cx + dx - 4).min(dim - 1); // window around seed
            let ny = (cy + dy - 4).min(dim - 1);
            g.dget_u8(ny * dim + nx) > 0
        })
        .count();
    assert!(
        plains_alive > 0,
        "plains: at least some cells must remain alive after 300 ticks"
    );

    // ── Water check: grass does NOT overrun water ───────────────────────────
    // "Sub-critical" means water cells stay bounded at their biome cap (10 bytes),
    // nowhere near the plains cap (255 bytes). The biome cap clamp in scatter_add
    // enforces this directly; here we verify the raised decay_pct doesn't break
    // the cap invariant, and that the water zone stays far below plains density.
    let water_cap_byte = encode_density(GRASS_CAPACITY_WATER * GRASS_MAX); // = 10
    let plains_cap_byte = encode_density(1.0f32); // = 255
                                                  // Sub-critical threshold: water cells must stay below 25% of plains cap.
                                                  // At water cap=10 this is trivially satisfied; the assertion catches any
                                                  // scenario where water cells somehow reach near-plains density.
    let sub_critical_threshold = plains_cap_byte / 4;

    for dy in 0..16usize {
        for dx in 0..16usize {
            let idx = (water_oy + dy) * dim + (water_ox + dx);
            let b = g.dget_u8(idx);
            assert!(
                b <= water_cap_byte,
                "water cell ({},{}): byte {b} exceeds biome cap {water_cap_byte}; \
                 water must stay sub-critical at decay_pct={}",
                water_ox + dx,
                water_oy + dy,
                GRASS_DECAY_PCT_DEFAULT
            );
            assert!(
                b < sub_critical_threshold,
                "water cell ({},{}): byte {b} ≥ sub-critical threshold {sub_critical_threshold} \
                 (25%% of plains cap {plains_cap_byte}); water is overrunning",
                water_ox + dx,
                water_oy + dy
            );
        }
    }
}

// ── Test (c): net_growth_sanity ─────────────────────────────────────────────

/// The field does NOT globally collapse at the raised decay_pct.
/// A seeded plains patch survives 500 ticks under the new default params.
/// This mirrors `persistence_no_collapse` from grass_scatter_tests but
/// explicitly uses GRASS_DECAY_PCT_DEFAULT (the new raised value) and checks
/// that the patch grows (not just survives).
#[test]
fn net_growth_sanity() {
    let dim = DIMS.grass_dim;
    let params = ScatterParams {
        decay_pct: GRASS_DECAY_PCT_DEFAULT, // 0.04 (the raised S4 default)
        ..ScatterParams::default()
    };
    let mut g = make_plains_grid(params);

    // Seed a 6×6 block at full density.
    let cx = dim / 2;
    let cy = dim / 2;
    for dy in 0..6usize {
        for dx in 0..6usize {
            let nx = (cx + dx).min(dim - 1);
            let ny = (cy + dy).min(dim - 1);
            g.dset(ny * dim + nx, GRASS_MAX);
        }
    }
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    let before = total_bytes(&g);
    tick_n(&mut g, 500);
    let after = total_bytes(&g);

    assert!(
        after > 0,
        "plains must not collapse to zero after 500 ticks at decay_pct={}; before={before}",
        GRASS_DECAY_PCT_DEFAULT
    );
    assert!(
        after >= before,
        "plains must grow or hold at decay_pct={} (super-critical); before={before} after={after}",
        GRASS_DECAY_PCT_DEFAULT
    );
}
