//! v2.0.6 S3: unit tests for seeded grass clump generation.
//!
//! Asserts that:
//! 1. Clump seeding is deterministic/reproducible from the same `world_seed`.
//! 2. Different seeds produce different layouts.
//! 3. The occupied-cell count is within the expected budget range.
//! 4. Clump count = 0 produces an empty grid (fallback to uniform-scatter handled
//!    by the caller; this verifies the no-op guard).
//!
//! Attached via `#[path]` in `lib.rs` (see below) per the project convention
//! (new tests go in their OWN file — never appended to a shared `mod tests`).

use crate::constants::{WorldDims, GRASS_CLUMP_COUNT_DEFAULT, GRASS_CLUMP_SIZE_DEFAULT, GRASS_MAX};
use crate::grass::GrassGrid;
use crate::rng::SimRng;

/// Compute the number of occupied (non-zero density) cells in a grid.
fn occupied_count(grid: &GrassGrid) -> usize {
    grid.density_u8_snapshot()
        .iter()
        .filter(|&&b| b > 0)
        .count()
}

/// A small world dims useful for fast unit tests.
fn small_dims() -> WorldDims {
    // 200u world / 5u cell = 40 cells/axis → 1600 cells total.
    WorldDims::from_world_size(200.0, true)
}

/// A default-scale world dims (9600u, 5u cell → 1920² = 3,686,400 cells).
fn default_dims() -> WorldDims {
    WorldDims::from_world_size(9600.0, true)
}

/// Seed a fresh GrassGrid with clumps using the given parameters.
fn make_grid_with_clumps(
    dims: WorldDims,
    world_seed: u32,
    clump_count: u32,
    clump_size: u32,
) -> GrassGrid {
    let mut rng = SimRng::from_string("test-seed");
    let mut grid = GrassGrid::new_with_capacity(&mut rng, 0, dims, None);
    grid.seed_clumps(world_seed, clump_count, clump_size);
    grid
}

// ─── Reproducibility ─────────────────────────────────────────────────────────

/// Two constructions with the SAME world_seed must produce byte-identical density.
#[test]
fn clumps_are_reproducible_same_seed() {
    let dims = small_dims();
    let g1 = make_grid_with_clumps(dims, 42, 5, 4);
    let g2 = make_grid_with_clumps(dims, 42, 5, 4);
    assert_eq!(
        g1.density_u8_snapshot(),
        g2.density_u8_snapshot(),
        "same world_seed must produce identical clump layout"
    );
}

/// Two constructions with DIFFERENT world_seeds must differ.
/// (Probabilistic — astronomically unlikely to collide for any non-trivial grid.)
#[test]
fn clumps_differ_on_different_seeds() {
    let dims = small_dims();
    let g1 = make_grid_with_clumps(dims, 1, 5, 4);
    let g2 = make_grid_with_clumps(dims, 2, 5, 4);
    assert_ne!(
        g1.density_u8_snapshot(),
        g2.density_u8_snapshot(),
        "different world_seeds must produce different clump layouts"
    );
}

// ─── Budget ───────────────────────────────────────────────────────────────────

/// Default defaults (40 clumps, radius 8) on a 1920² grid must produce an
/// occupied-cell count between 1,000 and 15,000 (generous range to tolerate
/// clump overlap at edges), and roughly matching the old 8000-cell budget.
#[test]
fn default_clumps_budget_near_8000() {
    let dims = default_dims();
    let g = make_grid_with_clumps(
        dims,
        12345,
        GRASS_CLUMP_COUNT_DEFAULT,
        GRASS_CLUMP_SIZE_DEFAULT,
    );
    let occupied = occupied_count(&g);
    assert!(
        (1_000..=15_000).contains(&occupied),
        "expected occupied cell count ~8000 (within [1000, 15000]); got {occupied}"
    );
}

/// With clump_count = 0 the grid stays empty (no-op guard).
#[test]
fn zero_clump_count_leaves_grid_empty() {
    let dims = small_dims();
    let g = make_grid_with_clumps(dims, 99, 0, 8);
    assert_eq!(
        occupied_count(&g),
        0,
        "clump_count=0 must not seed any cells"
    );
}

/// With clump_size = 0 the grid stays empty (no-op guard).
#[test]
fn zero_clump_size_leaves_grid_empty() {
    let dims = small_dims();
    let g = make_grid_with_clumps(dims, 99, 10, 0);
    assert_eq!(
        occupied_count(&g),
        0,
        "clump_size=0 must not seed any cells"
    );
}

// ─── Seeded cells at capacity ─────────────────────────────────────────────────

/// Every occupied cell must be seeded to `GRASS_MAX` (capacity = GRASS_MAX for
/// a plain all-Plains grid constructed without biome).
#[test]
fn clump_cells_seeded_to_capacity() {
    let dims = small_dims();
    let g = make_grid_with_clumps(dims, 7, 3, 3);
    for (i, &b) in g.density_u8_snapshot().iter().enumerate() {
        if b > 0 {
            let decoded = b as f32 / 255.0 * GRASS_MAX;
            assert!(
                (decoded - GRASS_MAX).abs() < 0.01,
                "cell {i}: expected GRASS_MAX ({GRASS_MAX}), got {decoded}"
            );
        }
    }
}

// ─── Walled world — clumps don't panic at borders ────────────────────────────

/// Walled (non-toroidal) world: clumps near the edges must clamp correctly,
/// not panic or produce out-of-bounds writes.
#[test]
fn clumps_walled_world_no_panic() {
    let dims = WorldDims::from_world_size(200.0, false); // walled
                                                         // Use a seed that places clumps near the edges.
    let g = make_grid_with_clumps(dims, 0xDEAD_BEEF, 20, 8);
    // Just verify it ran without panic and produced a non-trivial result.
    assert!(occupied_count(&g) > 0);
}
