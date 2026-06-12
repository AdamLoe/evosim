//! Biome generation tests.
//! Movement penalty tests removed (penalties gone); genome-modulation tests
//! removed (genome gone). Biome no longer contributes a direct NN input; grass
//! density is the selection signal.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard.

use super::biome::generate_biome_grid;
use crate::constants::{WorldDims, WORLD_SIZE_DEFAULT};

/// Count each biome tag in a grid → (plains, water, desert).
fn counts(grid: &[u8]) -> (usize, usize, usize) {
    let mut p = 0;
    let mut w = 0;
    let mut d = 0;
    for &b in grid {
        match b {
            1 => w += 1,
            2 => d += 1,
            _ => p += 1,
        }
    }
    (p, w, d)
}

/// Determinism: the same `world_seed` produces a byte-identical biome grid
/// across two independent generations.
#[test]
fn biome_grid_is_deterministic_for_same_seed() {
    let dims = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
    let a = generate_biome_grid(0xDEAD_BEEF, &dims);
    let b = generate_biome_grid(0xDEAD_BEEF, &dims);
    assert_eq!(a, b, "same world_seed must yield byte-identical biome grid");
}

/// A different `world_seed` produces a different biome grid.
#[test]
fn biome_grid_differs_across_seeds() {
    let dims = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
    let a = generate_biome_grid(1, &dims);
    let b = generate_biome_grid(2, &dims);
    assert_ne!(
        a, b,
        "different world_seed must yield a different biome grid"
    );
}

/// Proportions: plains is the plurality; water and desert are each non-trivial.
/// Checked across several seeds so we're not asserting a lucky single draw.
#[test]
fn biome_proportions_are_plains_majority_with_nontrivial_water_and_desert() {
    let dims = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
    let total = dims.grass_cell_count as f64;
    for seed in [1u32, 7, 42, 1000, 123_456] {
        let grid = generate_biome_grid(seed, &dims);
        let (p, w, d) = counts(&grid);
        let pf = p as f64 / total;
        let wf = w as f64 / total;
        let df = d as f64 / total;
        assert!(
            p > w && p > d,
            "seed {seed}: plains must be the plurality (p={pf:.3} w={wf:.3} d={df:.3})"
        );
        assert!(
            wf > 0.02,
            "seed {seed}: water must be non-trivial (w={wf:.3})"
        );
        assert!(
            df > 0.02,
            "seed {seed}: desert must be non-trivial (d={df:.3})"
        );
    }
}

/// Wrap-aware generation: a wrap-on grid differs from the same-seed wrap-off
/// grid (toroidal distance changes blob membership near the seam). Both stay
/// plains-majority.
#[test]
fn biome_wrap_aware_generation_changes_seam_cells() {
    let dims_wrap = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
    let dims_wall = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, false);
    let wrap = generate_biome_grid(99, &dims_wrap);
    let wall = generate_biome_grid(99, &dims_wall);
    assert_ne!(
        wrap, wall,
        "wrap-aware toroidal distance must change membership vs walled"
    );
    let (pw, _, _) = counts(&wrap);
    let (pl, _, _) = counts(&wall);
    let total = dims_wrap.grass_cell_count;
    assert!(pw * 2 > total, "wrap grid must stay plains-majority");
    assert!(pl * 2 > total, "walled grid must stay plains-majority");
}
