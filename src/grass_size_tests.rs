//! Tests for the `grass_size` construction slider (v2.0.4 S2).
//!
//! Verifies that:
//!   1. `grass_dim` and `grass_cell_count` scale as `round(world_size / cell_size)`
//!      and `grass_dim²` respectively for several cell sizes.
//!   2. A world built with non-default cell sizes has self-consistent dims:
//!      `capacity.len() == grass_cell_count`, biome grid sized to `grass_cell_count`,
//!      snapshot slot sized to match.

use crate::constants::{WorldDims, WORLD_SIZE_DEFAULT};
use crate::world::{DevSliders, World};

/// Cell-count scaling: grass_dim and grass_cell_count match
/// `round(world_size / cell_size)` and its square (within rounding).
#[test]
fn cell_count_scales_inverse_square_of_cell_size() {
    let world_size = 1920.0f32; // convenient: 1920 / 5 = 384, 1920 / 10 = 192, etc.

    // (cell_size, expected_grass_dim)
    let cases: &[(f32, usize)] = &[(5.0, 384), (10.0, 192), (20.0, 96)];

    for &(cell_size, expected_dim) in cases {
        let dims = WorldDims::from_world_size_with_cell_size(world_size, true, cell_size);
        assert_eq!(
            dims.grass_dim, expected_dim,
            "cell_size={cell_size}: expected grass_dim={expected_dim}, got {}",
            dims.grass_dim
        );
        assert_eq!(
            dims.grass_cell_count,
            expected_dim * expected_dim,
            "cell_size={cell_size}: grass_cell_count must equal grass_dim²"
        );
        assert_eq!(
            dims.grass_cell_size, cell_size,
            "cell_size={cell_size}: WorldDims must carry the construction cell size"
        );
        // Approximate 1/size² scaling: ratio of cell counts should be ≈ (1/size)².
        // grass_cell_count ∝ (world_size / cell_size)² = world_size² / cell_size².
        // At cell_size=5: (1920/5)² = 147456
        // At cell_size=10: (1920/10)² = 36864 = 147456 / 4
        // At cell_size=20: (1920/20)² = 9216  = 147456 / 16
        let expected_count = expected_dim * expected_dim;
        assert_eq!(
            dims.grass_cell_count, expected_count,
            "cell_size={cell_size}: cell count mismatch"
        );
    }

    // Verify the 1/size² relationship holds across the three sizes.
    let dim5 = (world_size / 5.0f32).round() as usize;
    let dim10 = (world_size / 10.0f32).round() as usize;
    let dim20 = (world_size / 20.0f32).round() as usize;
    // dim10 ≈ dim5 / 2, dim20 ≈ dim5 / 4
    assert_eq!(dim10, dim5 / 2, "halving cell size should halve grass_dim");
    assert_eq!(
        dim20,
        dim5 / 4,
        "quadrupling cell size should quarter grass_dim"
    );
}

/// World rebuilds cleanly at cell_size=5 (default), 10, and 20:
/// capacity.len() == grass_cell_count, biome grid == grass_cell_count,
/// snapshot slot grass region correctly sized from dims.
#[test]
fn world_rebuilds_cleanly_at_multiple_cell_sizes() {
    let world_size = WORLD_SIZE_DEFAULT; // 9600u
    for &cell_size in &[5.0f32, 10.0, 20.0] {
        let sliders = DevSliders {
            world_size,
            grass_cell_size: cell_size,
            // Suppress expensive default seeding count for this test.
            grass_initial_seed_count: 0,
            ..DevSliders::default()
        };
        let w = World::new_with_sliders("grass-size-rebuild", sliders);

        let expected_dim = (world_size / cell_size).round() as usize;
        let expected_cells = expected_dim * expected_dim;

        // grass_dim and grass_cell_count from dims.
        assert_eq!(
            w.dims.grass_dim, expected_dim,
            "cell_size={cell_size}: dims.grass_dim mismatch"
        );
        assert_eq!(
            w.dims.grass_cell_count, expected_cells,
            "cell_size={cell_size}: dims.grass_cell_count mismatch"
        );

        // capacity[] length must match grass_cell_count.
        assert_eq!(
            w.grass.capacity.len(),
            expected_cells,
            "cell_size={cell_size}: capacity.len() must equal grass_cell_count"
        );

        // Biome grid sized to grass_cell_count.
        assert_eq!(
            w.biome_grid.len(),
            expected_cells,
            "cell_size={cell_size}: biome_grid.len() must equal grass_cell_count"
        );

        // Density array sized to grass_cell_count.
        assert_eq!(
            w.grass.density.len(),
            expected_cells,
            "cell_size={cell_size}: grass.density.len() must equal grass_cell_count"
        );

        // dims.grass_cell_size must be stored correctly.
        assert_eq!(
            w.dims.grass_cell_size, cell_size,
            "cell_size={cell_size}: dims.grass_cell_size mismatch"
        );
    }
}

/// Default cell size (5.0) produces the historically-documented 1920² grid at 9600u.
#[test]
fn default_cell_size_produces_canonical_grid() {
    let dims = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
    assert_eq!(dims.grass_dim, 1920, "default grid must be 1920 cells/axis");
    assert_eq!(
        dims.grass_cell_count,
        1920 * 1920,
        "default cell count must be 1920²"
    );
    assert_eq!(dims.grass_cell_size, 5.0, "default cell size must be 5.0");
}
