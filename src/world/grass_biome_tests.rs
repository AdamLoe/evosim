//! v2.0 Wave 1 — biome-driven grass carrying-capacity tests.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard. Verifies that per-biome carrying
//! capacity (Plains ×1.0, Desert ×0.30, Water ×0.04) bounds BOTH the initial
//! seeding and the logistic regrowth cap, while keeping grass propagation
//! deterministic and the plains path unchanged from today's behavior.

use super::{DevSliders, World};
use crate::constants::{
    Biome, WorldDims, GRASS_CAPACITY_DESERT, GRASS_CAPACITY_PLAINS, GRASS_CAPACITY_WATER, GRASS_MAX,
};
use crate::grass::GrassGrid;
use crate::rng::SimRng;

/// A walled test world deterministically seeded so we exercise all three biomes.
fn test_world() -> World {
    World::new_with_sliders(
        "grass-biome-cap",
        DevSliders {
            founder_count: 1,
            world_size: 1000.0,
            wrap_world: false,
            world_seed: 4242,
            // Saturate every cell on init so we can read each cell's capacity
            // directly off the density field.
            full_grass_on_init: true,
            ..Default::default()
        },
    )
}

/// `full_grass_on_init` fills every cell to ITS biome capacity, not a flat
/// GRASS_MAX: plains cells sit at GRASS_MAX, desert at 0.30, water at 0.04.
#[test]
fn full_grass_on_init_respects_biome_capacity() {
    let w = test_world();
    let mut saw_plains = false;
    let mut saw_water = false;
    let mut saw_desert = false;
    for idx in 0..w.biome_grid.len() {
        let dens = w.grass.density[idx];
        let cap = w.grass.capacity[idx];
        match crate::world::biome::biome_from_u8(w.biome_grid[idx]) {
            Biome::Plains => {
                assert!((cap - GRASS_CAPACITY_PLAINS * GRASS_MAX).abs() < 1e-6);
                assert!((dens - cap).abs() < 1e-6, "plains cell must fill to cap");
                saw_plains = true;
            }
            Biome::Desert => {
                assert!((cap - GRASS_CAPACITY_DESERT * GRASS_MAX).abs() < 1e-6);
                assert!((dens - cap).abs() < 1e-6, "desert cell must fill to cap");
                saw_desert = true;
            }
            Biome::Water => {
                assert!((cap - GRASS_CAPACITY_WATER * GRASS_MAX).abs() < 1e-6);
                assert!((dens - cap).abs() < 1e-6, "water cell must fill to cap");
                saw_water = true;
            }
        }
    }
    assert!(
        saw_plains && saw_water && saw_desert,
        "need all three biomes"
    );
}

/// A seeded cell is filled to its OWN capacity at init (water near 0.04, desert
/// near 0.30) — never above. Cross-check against the biome grid for every
/// non-empty cell after a normal (sparse) seed.
#[test]
fn initial_seed_capped_by_biome_capacity() {
    let w = World::new_with_sliders(
        "grass-biome-seed",
        DevSliders {
            founder_count: 1,
            world_size: 1000.0,
            wrap_world: false,
            world_seed: 4242,
            grass_initial_seed_count: 20_000,
            ..Default::default()
        },
    );
    for idx in 0..w.biome_grid.len() {
        let dens = w.grass.density[idx];
        let cap = w.grass.capacity[idx];
        assert!(
            dens <= cap + 1e-6,
            "seeded density {dens} exceeds biome cap {cap} at cell {idx}"
        );
        // Any non-empty cell was seeded to exactly its cap.
        if dens > 0.0 {
            assert!((dens - cap).abs() < 1e-6, "seeded cell must equal its cap");
        }
    }
}

/// Logistic regrowth saturates each cell near its biome capacity, not GRASS_MAX:
/// a water cell tops out near 0.04, a desert cell near 0.30, plains near 1.0.
#[test]
fn regrowth_saturates_at_biome_capacity() {
    let dims = WorldDims::from_world_size(500.0, false);
    let mut rng = SimRng::from_u64(7);
    // Build a biome grid with one cell of each kind: 0=plains, 1=water, 2=desert.
    let mut biome = vec![Biome::Plains as u8; dims.grass_cell_count];
    let water_idx = 0usize;
    let desert_idx = 1usize;
    let plains_idx = 2usize;
    biome[water_idx] = Biome::Water as u8;
    biome[desert_idx] = Biome::Desert as u8;
    // plains_idx stays Plains.

    let mut g = GrassGrid::new_with_capacity(&mut rng, 0, dims, Some(&biome));
    // Seed each target cell with a small density and grow with high logistic
    // rate, no propagation, so each cell evolves toward its own capacity.
    g.density[water_idx] = 0.01;
    g.density[desert_idx] = 0.01;
    g.density[plains_idx] = 0.01;
    for _ in 0..2000 {
        g.step(0.5, 0.0);
    }
    let water = g.density[water_idx];
    let desert = g.density[desert_idx];
    let plains = g.density[plains_idx];
    assert!(
        (water - GRASS_CAPACITY_WATER).abs() < 1e-3,
        "water cell must saturate near {GRASS_CAPACITY_WATER}; got {water}"
    );
    assert!(
        (desert - GRASS_CAPACITY_DESERT).abs() < 1e-3,
        "desert cell must saturate near {GRASS_CAPACITY_DESERT}; got {desert}"
    );
    assert!(
        (plains - GRASS_CAPACITY_PLAINS).abs() < 1e-3,
        "plains cell must saturate near {GRASS_CAPACITY_PLAINS}; got {plains}"
    );
    // No cell ever exceeds its capacity.
    for (&d, &c) in g.density.iter().zip(g.capacity.iter()) {
        assert!(d <= c + 1e-6, "density {d} exceeded capacity {c}");
    }
}

/// Determinism: biome-capacity grass propagation is bit-stable across two runs
/// from the same seed (capacity changes the value, never the determinism).
#[test]
fn biome_grass_propagation_is_deterministic() {
    let dims = WorldDims::from_world_size(600.0, true);
    let biome = super::biome::generate_biome_grid(0xABCD, &dims);

    let mut rng_a = SimRng::from_u64(99);
    let mut a = GrassGrid::new_with_capacity(&mut rng_a, 5_000, dims, Some(&biome));
    let mut rng_b = SimRng::from_u64(99);
    let mut b = GrassGrid::new_with_capacity(&mut rng_b, 5_000, dims, Some(&biome));
    assert_eq!(
        a.density, b.density,
        "seed must produce identical init density"
    );
    assert_eq!(a.capacity, b.capacity, "capacity must be identical");

    for _ in 0..50 {
        a.step(0.05, 0.01);
        b.step(0.05, 0.01);
    }
    assert_eq!(
        a.density, b.density,
        "biome grass propagation must stay deterministic"
    );
}

/// The plains-only path (no biome grid, or all-plains) reproduces today's
/// behavior exactly: a uniform GRASS_MAX cap and the legacy `new` ctor agree
/// with an all-plains biome grid bit-for-bit after propagation.
#[test]
fn all_plains_matches_legacy_behavior() {
    let dims = WorldDims::from_world_size(400.0, false);
    let plains = vec![Biome::Plains as u8; dims.grass_cell_count];

    let mut rng_legacy = SimRng::from_u64(123);
    let mut legacy = GrassGrid::new(&mut rng_legacy, 2_000, dims);
    let mut rng_biome = SimRng::from_u64(123);
    let mut biome = GrassGrid::new_with_capacity(&mut rng_biome, 2_000, dims, Some(&plains));

    assert_eq!(
        legacy.density, biome.density,
        "all-plains biome init must equal legacy init"
    );
    for _ in 0..100 {
        legacy.step(0.05, 0.02);
        biome.step(0.05, 0.02);
    }
    assert_eq!(
        legacy.density, biome.density,
        "all-plains propagation must match legacy GRASS_MAX-cap behavior"
    );
}
