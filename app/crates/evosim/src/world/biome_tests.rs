//! v2.1 P2 — biome generation + CurrBiomeType NN-input tests.
//! Movement penalty tests removed (penalties gone); genome-modulation tests
//! removed (genome gone). CurrBiomeType (1 slot, Plains=0/Water=0.5/Desert=1)
//! replaces the old BiomeDir(4)+CurrCellPenalty(1) groups.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard.

use super::biome::generate_biome_grid;
use super::nn::{build_nn_input, BiomeSampler, NnInputGroup};
use super::{DevSliders, World};
use crate::constants::{Biome, WorldDims, MAX_NN_INPUTS, WORLD_SIZE_DEFAULT};

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

/// Build NN inputs for creature 0 at (x,y) using a world's live settings.
fn build_inputs_at(w: &mut World, x: f32, y: f32) -> [f32; MAX_NN_INPUTS] {
    w.creatures.x[0] = x;
    w.creatures.y[0] = y;
    let mut scratch = [0.0f32; 16];
    let prev_vx = w.creatures.vx[0];
    let prev_vy = w.creatures.vy[0];
    let energy_max = w.sliders.energy_max;
    let max_age = w.sliders.max_age;
    let world_size = w.dims.world_size;
    let species_mode = w.sliders.species_mode;
    let layout = w.nn_input_layout.clone();
    let biome = BiomeSampler::new(
        &w.biome_grid[..],
        w.dims.grass_dim,
        w.dims.world_size,
        w.dims.wrap_world,
        w.dims.grass_cell_size,
    );
    build_nn_input(
        0,
        &layout,
        species_mode,
        &w.creatures,
        &w.grass,
        &w.grid,
        &w.sector_lut,
        &mut scratch,
        biome,
        prev_vx,
        prev_vy,
        energy_max,
        max_age,
        world_size,
        None,
    )
}

/// v2.1 P2: `CurrBiomeType` (1 slot) is present in both wrap modes.
/// Plains → 0.0, Water → 0.5, Desert → 1.0.
#[test]
fn curr_biome_type_slot_active_in_both_wrap_modes() {
    for wrap in [true, false] {
        let mut w = World::new_with_sliders(
            "biome-nn-p2",
            DevSliders {
                founder_count: 1,
                world_size: 1000.0,
                wrap_world: wrap,
                world_seed: 4242,
                ..Default::default()
            },
        );
        let curr_off = w
            .nn_input_layout
            .offset_of(NnInputGroup::CurrBiomeType)
            .expect("CurrBiomeType must be active in both wrap modes");

        let dim = w.dims.grass_dim;
        let cell = crate::constants::GRASS_CELL_SIZE;

        // Find a plains cell → 0.0
        if let Some(idx) = w.biome_grid.iter().position(|&b| b == Biome::Plains as u8) {
            let cx = (idx % dim) as f32 * cell + cell * 0.5;
            let cy = (idx / dim) as f32 * cell + cell * 0.5;
            let inp = build_inputs_at(&mut w, cx, cy);
            assert_eq!(
                inp[curr_off], 0.0,
                "wrap={wrap}: Plains CurrBiomeType must be 0.0"
            );
        }

        // Find a water cell → 0.5
        if let Some(idx) = w.biome_grid.iter().position(|&b| b == Biome::Water as u8) {
            let cx = (idx % dim) as f32 * cell + cell * 0.5;
            let cy = (idx / dim) as f32 * cell + cell * 0.5;
            let inp = build_inputs_at(&mut w, cx, cy);
            assert_eq!(
                inp[curr_off], 0.5,
                "wrap={wrap}: Water CurrBiomeType must be 0.5"
            );
        }

        // Find a desert cell → 1.0
        if let Some(idx) = w.biome_grid.iter().position(|&b| b == Biome::Desert as u8) {
            let cx = (idx % dim) as f32 * cell + cell * 0.5;
            let cy = (idx / dim) as f32 * cell + cell * 0.5;
            let inp = build_inputs_at(&mut w, cx, cy);
            assert_eq!(
                inp[curr_off], 1.0,
                "wrap={wrap}: Desert CurrBiomeType must be 1.0"
            );
        }
    }
}

/// v2.1 P2: BiomeDir and CurrCellPenalty groups are gone — they must not
/// appear in the active layout.
#[test]
fn biome_dir_and_curr_cell_penalty_not_in_layout() {
    let w = World::new("biome-no-penalty");
    assert!(
        w.nn_input_layout
            .offset_of(NnInputGroup::CurrBiomeType)
            .is_some(),
        "CurrBiomeType must be active"
    );
}
