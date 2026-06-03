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
        let cap = w.grass.capacity[idx];
        // Stage 3: byte-exact assertion. full_grass_on_init fills each cell to
        // encode(cap), so dget_u8(idx) must equal the cap byte exactly.
        let cap_byte = {
            let q = (cap / GRASS_MAX).clamp(0.0, 1.0) * 255.0;
            (q + 0.5) as u8
        };
        let d_byte = w.grass.dget_u8(idx);
        match crate::world::biome::biome_from_u8(w.biome_grid[idx]) {
            // encode(GRASS_CAPACITY_PLAINS * GRASS_MAX = 1.0) = 255
            Biome::Plains => {
                assert!((cap - GRASS_CAPACITY_PLAINS * GRASS_MAX).abs() < 1e-6);
                assert_eq!(
                    d_byte, cap_byte,
                    "plains cell {idx} must fill to cap byte {cap_byte}"
                );
                saw_plains = true;
            }
            // encode(GRASS_CAPACITY_DESERT * GRASS_MAX = 0.30) = 77
            Biome::Desert => {
                assert!((cap - GRASS_CAPACITY_DESERT * GRASS_MAX).abs() < 1e-6);
                assert_eq!(
                    d_byte, cap_byte,
                    "desert cell {idx} must fill to cap byte {cap_byte}"
                );
                saw_desert = true;
            }
            // encode(GRASS_CAPACITY_WATER * GRASS_MAX = 0.04) = 10
            Biome::Water => {
                assert!((cap - GRASS_CAPACITY_WATER * GRASS_MAX).abs() < 1e-6);
                assert_eq!(
                    d_byte, cap_byte,
                    "water cell {idx} must fill to cap byte {cap_byte}"
                );
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
        let cap = w.grass.capacity[idx];
        let cap_byte = {
            let q = (cap / GRASS_MAX).clamp(0.0, 1.0) * 255.0;
            (q + 0.5) as u8
        };
        let d_byte = w.grass.dget_u8(idx);
        // Stage 3: byte-exact assertions.
        // Seeded cells are set to encode(cap), so they must store exactly cap_byte.
        // Unseeded cells are 0. No byte should EXCEED cap_byte.
        assert!(
            d_byte <= cap_byte,
            "seeded density byte {d_byte} exceeds biome cap byte {cap_byte} at cell {idx}"
        );
        // Any non-empty cell was seeded to exactly its cap byte.
        if d_byte > 0 {
            assert_eq!(
                d_byte, cap_byte,
                "seeded cell {idx} must store exactly cap byte {cap_byte}"
            );
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
    g.dset(water_idx, 0.01);
    g.dset(desert_idx, 0.01);
    g.dset(plains_idx, 0.01);
    for _ in 0..2000 {
        g.step(0.5, 0.0);
    }
    // Note: f32 values are no longer used; assertions operate on bytes directly.
    let _water = g.dget(water_idx);
    let _desert = g.dget(desert_idx);
    let _plains = g.dget(plains_idx);
    // Stage 3: byte-exact assertions on the stable fixed points of the quantized
    // logistic map (2000 steps, r=0.5, k=0.0):
    //   Water  (cap=0.04): byte 10  — decode(10)=0.03922 < cap; step stays 10.
    //   Desert (cap=0.30): byte 76  — encode(0.30)=77 but decode(77)=0.30196>cap, so
    //                                  logistic drives d back down; decode(76)=0.29804
    //                                  is the stable fixed point (step 76→76).
    //   Plains (cap=1.0):  byte 254 — decode(254)=0.99608; step→0.99803, rounds to 254.
    //                                  byte 255 is theoretically stable but unreachable
    //                                  from below: the step from 254 never crosses 255.
    assert_eq!(
        g.dget_u8(water_idx),
        10u8,
        "water cell must saturate at byte 10; got byte {}",
        g.dget_u8(water_idx)
    );
    assert_eq!(
        g.dget_u8(desert_idx),
        76u8,
        "desert cell must saturate at byte 76 (stable fixed point at cap 0.30); got byte {}",
        g.dget_u8(desert_idx)
    );
    assert_eq!(
        g.dget_u8(plains_idx),
        254u8,
        "plains cell must saturate at byte 254 (stable fixed point at cap 1.0, r=0.5); got byte {}",
        g.dget_u8(plains_idx)
    );
    // No cell ever exceeds its capacity byte.
    for c in 0..g.capacity.len() {
        let d_byte = g.dget_u8(c);
        // cap_byte = encode(capacity[c]); stored density must not exceed it.
        let cap_byte = {
            let q = (g.capacity[c] / GRASS_MAX).clamp(0.0, 1.0) * 255.0;
            (q + 0.5) as u8
        };
        assert!(
            d_byte <= cap_byte,
            "cell {c}: density byte {d_byte} exceeded capacity byte {cap_byte}"
        );
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
        a.density_u8_snapshot(),
        b.density_u8_snapshot(),
        "seed must produce identical init density"
    );
    assert_eq!(a.capacity, b.capacity, "capacity must be identical");

    for _ in 0..50 {
        a.step(0.05, 0.01);
        b.step(0.05, 0.01);
    }
    assert_eq!(
        a.density_u8_snapshot(),
        b.density_u8_snapshot(),
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
        legacy.density_u8_snapshot(),
        biome.density_u8_snapshot(),
        "all-plains biome init must equal legacy init"
    );
    for _ in 0..100 {
        legacy.step(0.05, 0.02);
        biome.step(0.05, 0.02);
    }
    assert_eq!(
        legacy.density_u8_snapshot(),
        biome.density_u8_snapshot(),
        "all-plains propagation must match legacy GRASS_MAX-cap behavior"
    );
}
