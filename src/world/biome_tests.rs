//! v2.0 Wave 1b — biome generation + movement-penalty + biome NN-input tests.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard. Same-machine determinism only; no
//! cross-platform claim.

use super::biome::generate_biome_grid;
use super::nn::{build_nn_input, BiomeSampler, NnInputGroup};
use super::{DevSliders, World};
use crate::constants::{Biome, WorldDims, MAX_NN_INPUTS, NN_BIOME_DIRS, WORLD_SIZE_DEFAULT};

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

/// A constructed World pins its biome grid to the boot `world_seed`, and the
/// movement-penalty lookup returns the live slider value for water/desert and
/// 0 for plains.
#[test]
fn world_movement_penalty_reads_biome_and_sliders() {
    let mut w = World::new_with_sliders(
        "biome-penalty",
        DevSliders {
            founder_count: 1,
            world_size: 1000.0,
            wrap_world: false,
            world_seed: 4242,
            water_movement_penalty: 0.8,
            desert_movement_penalty: 0.4,
            ..Default::default()
        },
    );

    // Find one cell of each kind (if present) and confirm the penalty mapping.
    let dim = w.dims.grass_dim;
    let cell = crate::constants::GRASS_CELL_SIZE;
    let mut saw_plains = false;
    let mut saw_water = false;
    let mut saw_desert = false;
    for idx in 0..w.biome_grid.len() {
        let ix = idx % dim;
        let iy = idx / dim;
        let x = ix as f32 * cell + cell * 0.5;
        let y = iy as f32 * cell + cell * 0.5;
        match w.biome_at(x, y) {
            Biome::Plains => {
                assert_eq!(w.movement_penalty_at(x, y), 0.0, "plains penalty must be 0");
                saw_plains = true;
            }
            Biome::Water => {
                assert_eq!(w.movement_penalty_at(x, y), 0.8, "water penalty = slider");
                saw_water = true;
            }
            Biome::Desert => {
                assert_eq!(w.movement_penalty_at(x, y), 0.4, "desert penalty = slider");
                saw_desert = true;
            }
        }
        if saw_plains && saw_water && saw_desert {
            break;
        }
    }
    assert!(saw_plains, "world must contain Plains");
    assert!(saw_water, "seed 4242 world should contain Water");
    assert!(saw_desert, "seed 4242 world should contain Desert");

    // Live slider change is reflected immediately.
    w.sliders.water_movement_penalty = 0.5;
    // Re-find a water cell and confirm.
    for idx in 0..w.biome_grid.len() {
        if w.biome_grid[idx] == Biome::Water as u8 {
            let ix = idx % dim;
            let iy = idx / dim;
            let x = ix as f32 * cell + cell * 0.5;
            let y = iy as f32 * cell + cell * 0.5;
            assert_eq!(
                w.movement_penalty_at(x, y),
                0.5,
                "live water slider change must take effect"
            );
            break;
        }
    }
}

/// Biome NN inputs: the `BiomeDir` (4: N/S/E/W) and `CurrCellPenalty` (1)
/// groups are present in BOTH wrap modes and carry the correct penalties for
/// the sampled cells. We place a creature on a known cell and check the curr-
/// cell penalty plus the directional samples one grass cell away.
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
        w.sliders.water_movement_penalty,
        w.sliders.desert_movement_penalty,
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

#[test]
fn biome_nn_inputs_present_in_both_wrap_modes() {
    for wrap in [true, false] {
        let mut w = World::new_with_sliders(
            "biome-nn",
            DevSliders {
                founder_count: 1,
                world_size: 1000.0,
                wrap_world: wrap,
                world_seed: 4242,
                water_movement_penalty: 0.8,
                desert_movement_penalty: 0.4,
                ..Default::default()
            },
        );
        // v2.0 Wave 2a: the biome NN inputs are genome-modulated. Pin the
        // founder's affinity traits to 0 so it reads the BASE severities this
        // test asserts (a higher-affinity creature would read a lower penalty).
        w.creatures.genome[0].water_affinity = 0.0;
        w.creatures.genome[0].heat_tolerance = 0.0;
        // Both biome groups must be active regardless of wrap.
        let dir_off = w
            .nn_input_layout
            .offset_of(NnInputGroup::BiomeDir)
            .expect("BiomeDir must be active in both wrap modes");
        let curr_off = w
            .nn_input_layout
            .offset_of(NnInputGroup::CurrCellPenalty)
            .expect("CurrCellPenalty must be active in both wrap modes");

        // Place the creature at the center of a known water cell.
        let dim = w.dims.grass_dim;
        let cell = crate::constants::GRASS_CELL_SIZE;
        let water_idx = w
            .biome_grid
            .iter()
            .position(|&b| b == Biome::Water as u8)
            .expect("seed 4242 world must have water");
        let ix = water_idx % dim;
        let iy = water_idx / dim;
        let cx = ix as f32 * cell + cell * 0.5;
        let cy = iy as f32 * cell + cell * 0.5;

        let inp = build_inputs_at(&mut w, cx, cy);
        // Curr-cell penalty = water severity.
        assert_eq!(
            inp[curr_off], 0.8,
            "wrap={wrap}: curr-cell penalty must equal water severity"
        );
        // Directional penalties are each in [0, 1] and equal the penalty one
        // cell away in N/S/E/W; they must all be valid penalties.
        for k in 0..NN_BIOME_DIRS {
            let v = inp[dir_off + k];
            assert!(
                (0.0..=1.0).contains(&v),
                "wrap={wrap}: biome dir input {k} out of [0,1]: {v}"
            );
            // Each dir penalty must match one of the three possible severities.
            assert!(
                v == 0.0 || v == 0.8 || v == 0.4,
                "wrap={wrap}: dir input {k} = {v} not a valid biome severity"
            );
        }
    }
}

/// Survivability: a full-energy creature parked on a worst-case water cell with
/// default sliders survives a multi-tick dash (does not instantly die). This
/// guards the balance-knob coefficients (K_BIOME_*). `split_threshold` is set
/// above `energy_max` so the lone founder can't Split (which would otherwise
/// confound the energy budget — a 99-energy Split, not the biome surcharge).
#[test]
fn full_energy_creature_survives_short_water_dash() {
    // Build a walled world and force the founder onto a water cell.
    let mut w = World::new_with_sliders(
        "biome-survive",
        DevSliders {
            founder_count: 1,
            world_size: 1000.0,
            wrap_world: false,
            world_seed: 4242,
            // Pin Split out of reach so it can't drain 99 energy mid-dash.
            split_threshold: 1.0e9,
            ..Default::default()
        },
    );
    // Locate a water cell.
    let dim = w.dims.grass_dim;
    let cell = crate::constants::GRASS_CELL_SIZE;
    let water_idx = w
        .biome_grid
        .iter()
        .position(|&b| b == Biome::Water as u8)
        .expect("seed 4242 world must have water");
    let ix = water_idx % dim;
    let iy = water_idx / dim;
    let wx = ix as f32 * cell + cell * 0.5;
    let wy = iy as f32 * cell + cell * 0.5;
    w.creatures.x[0] = wx;
    w.creatures.y[0] = wy;
    w.creatures.energy[0] = w.sliders.energy_max; // full energy
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    // Run 20 ticks; the creature should still be alive (energy > 0) — a short
    // dash across water must not be instantly fatal at full energy.
    for _ in 0..20 {
        if !w.tick_once() {
            break;
        }
    }
    // It may have moved off water, but at minimum it must not have died of the
    // biome surcharge alone within 20 ticks at full energy.
    assert!(
        !w.creatures.is_empty(),
        "a full-energy creature must survive a 20-tick water dash"
    );
}
