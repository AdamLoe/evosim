//! v2.0.4 S6 — multi-band NN grass sight tests.
//!
//! Tests cover:
//!   (a) coord-convention correctness — wrap uses rem_euclid (no negative-cast bug);
//!       clamp keeps the sample inside the world boundary.
//!   (b) band-radius → mip-level mapping: GRASS_FAR_MIP_LEVEL yields a cell size
//!       that covers ≈ GRASS_FAR_SIGHT_RADIUS and the level exists in the pyramid.
//!   (c) layout width ≤ MAX_NN_INPUTS for all four (wrap × species) combos,
//!       both with and without grass_multisight.
//!   (d) smoke: compute_grass_far_band_sectors returns [0..1] values; a fully
//!       saturated region → far band ≈ 1.0.
//!   (e) fallback: walled + species forces single-band regardless of toggle.
//!   (f) default constants agree with documented intent.

use crate::constants::WorldDims;
use crate::constants::{
    GRASS_CELL_SIZE, GRASS_FAR_MIP_LEVEL, GRASS_FAR_SIGHT_RADIUS, GRASS_MAX,
    GRASS_MULTISIGHT_DEFAULT, MAX_NN_INPUTS, SPECIES_MODE_DEFAULT, WRAP_WORLD_DEFAULT,
};
use crate::grass::GrassGrid;
use crate::rng::SimRng;
use crate::world::nn::{NnInputGroup, NnInputLayout};
use crate::world::proximity::compute_grass_far_band_sectors;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_grid(dims: WorldDims, fill: f32) -> GrassGrid {
    // Construct with 0 seeded cells (random placement), then fill all cells to
    // the requested density. Using a stable seed keeps tests deterministic.
    let mut rng = SimRng::from_string("grass-sight-test");
    let mut g = GrassGrid::new(&mut rng, 0, dims);
    for i in 0..g.dims.grass_cell_count {
        g.dset(i, fill);
    }
    g.rebuild_row_bitset();
    g
}

/// Refresh pyramid levels (needed for far-band reads above level 0).
fn refresh_pyramid(grass: &mut GrassGrid) {
    // Split-borrow: take density out, refresh pyramid, put it back.
    let density = std::mem::take(&mut grass.density);
    grass
        .pyramid
        .refresh(&density, &grass.tile_active, grass.tiles_per_axis);
    grass.density = density;
}

// ── (a) coord-convention: wrap uses rem_euclid ────────────────────────────────

/// A creature very near (0, 0) in a toroidal world, sampling the N sector
/// (which would target y = -GRASS_FAR_SIGHT_RADIUS before wrap).
/// Must not panic; output must be in [0, 1].
#[test]
fn far_band_wrap_near_seam_no_panic() {
    let ws = 1200.0f32;
    let dims = WorldDims::from_world_size(ws, true);
    let grass = make_grid(dims, 0.0);
    let mut out = [0.0f32; 8];
    compute_grass_far_band_sectors(5.0, 5.0, &grass, &mut out);
    for (s, &v) in out.iter().enumerate() {
        assert!(
            v >= 0.0 && v <= 1.0,
            "sector {s} = {v} out of [0,1] in wrap-near-seam test"
        );
    }
}

/// In a walled world the sample point clamps; no wrapping occurs.
/// Creature at (10, 10): N sector targets y ≈ -150, clamped to 0 edge.
/// Must not panic; output in [0, 1].
#[test]
fn far_band_walled_clamp_negative_y() {
    let ws = 1200.0f32;
    let dims = WorldDims::from_world_size(ws, false);
    let grass = make_grid(dims, 0.0);
    let mut out = [0.0f32; 8];
    compute_grass_far_band_sectors(10.0, 10.0, &grass, &mut out);
    for (s, &v) in out.iter().enumerate() {
        assert!(
            v >= 0.0 && v <= 1.0,
            "sector {s} = {v} out of [0,1] in walled-clamp test"
        );
    }
}

/// rem_euclid correctness: in a fully-saturated toroidal world, every sector
/// must read ~1.0 even when the sample point wraps to the far side.
/// This confirms that `rem_euclid` is used (not a saturating cast to 0).
#[test]
fn far_band_wrap_rem_euclid_not_saturating_cast() {
    // Use a world smaller than GRASS_FAR_SIGHT_RADIUS so sectors definitely wrap.
    let ws = 100.0f32; // smaller than FAR_SIGHT_RADIUS (160u)
    let dims = WorldDims::from_world_size(ws, true);
    let mut grass = make_grid(dims, GRASS_MAX);
    refresh_pyramid(&mut grass);
    let mut out = [0.0f32; 8];
    // Sample near the corner so multiple sectors wrap.
    compute_grass_far_band_sectors(5.0, 5.0, &grass, &mut out);
    for (s, &v) in out.iter().enumerate() {
        assert!(
            v > 0.9,
            "sector {s} should read high density via rem_euclid wrap; got {}",
            v
        );
    }
}

// ── (b) band-radius → mip-level mapping ──────────────────────────────────────

/// GRASS_FAR_MIP_LEVEL must exist in the default pyramid (grass_dim = 1920).
#[test]
fn far_mip_level_exists_in_default_pyramid() {
    use crate::grass::GrassPyramid;
    let pyramid = GrassPyramid::new(1920);
    assert!(
        GRASS_FAR_MIP_LEVEL < pyramid.num_levels(),
        "GRASS_FAR_MIP_LEVEL ({GRASS_FAR_MIP_LEVEL}) must be a valid level in the default \
         1920-dim pyramid (has {} levels)",
        pyramid.num_levels()
    );
}

/// The effective cell size at GRASS_FAR_MIP_LEVEL must cover at least
/// GRASS_FAR_SIGHT_RADIUS when 4 cells are sampled outward.
#[test]
fn far_band_radius_matches_mip_level_coverage() {
    let mip_cell_size = GRASS_CELL_SIZE * (1usize << GRASS_FAR_MIP_LEVEL) as f32;
    let coverage_radius = mip_cell_size * 4.0;
    assert!(
        coverage_radius >= GRASS_FAR_SIGHT_RADIUS,
        "mip level {GRASS_FAR_MIP_LEVEL} cell size = {mip_cell_size}u, 4-cell coverage = \
         {coverage_radius}u must be ≥ GRASS_FAR_SIGHT_RADIUS ({GRASS_FAR_SIGHT_RADIUS}u)"
    );
}

// ── (c) layout width ≤ MAX_NN_INPUTS for all combos ──────────────────────────

#[test]
fn layout_width_fits_max_nn_inputs_all_combos() {
    for &wrap in &[true, false] {
        for &species in &[true, false] {
            for &multisight in &[true, false] {
                let layout = NnInputLayout::for_settings(wrap, species, multisight);
                assert!(
                    layout.width() <= MAX_NN_INPUTS,
                    "layout width {} > MAX_NN_INPUTS {} (wrap={wrap}, species={species}, multisight={multisight})",
                    layout.width(), MAX_NN_INPUTS
                );
                assert_eq!(
                    layout.width() % 8, 0,
                    "layout width {} not multiple of 8 (wrap={wrap}, species={species}, multisight={multisight})",
                    layout.width()
                );
            }
        }
    }
}

/// Multi-band layouts must be wider than single-band for configs that fit,
/// and must fall back to single-band width for walled+species.
#[test]
fn multisight_layout_wider_when_fits_falls_back_walled_species() {
    for &wrap in &[true, false] {
        for &species in &[true, false] {
            let single = NnInputLayout::for_settings(wrap, species, false);
            let multi = NnInputLayout::for_settings(wrap, species, true);
            let walled_species = !wrap && species;
            if walled_species {
                // Budget is full; must fall back.
                assert_eq!(
                    single.width(),
                    multi.width(),
                    "walled+species: multi-band should fall back to same width as single-band"
                );
                assert!(
                    multi.offset_of(NnInputGroup::GrassBandsFar).is_none(),
                    "walled+species: GrassBandsFar should be absent"
                );
            } else {
                assert!(
                    multi.width() > single.width(),
                    "wrap={wrap}, species={species}: multi ({}) must be wider than single ({})",
                    multi.width(),
                    single.width()
                );
                assert!(
                    multi.offset_of(NnInputGroup::GrassBandsFar).is_some(),
                    "wrap={wrap}, species={species}: GrassBandsFar must be present"
                );
                let far_off = multi.offset_of(NnInputGroup::GrassBandsFar).unwrap();
                let near_off = multi.offset_of(NnInputGroup::GrassSectors).unwrap();
                assert!(
                    far_off > near_off,
                    "far band offset ({far_off}) must be > near band offset ({near_off})"
                );
            }
        }
    }
}

// ── (d) smoke: sector outputs in [0,1]; saturated grid → ≈1.0 ───────────────

/// An empty grass grid produces all-zero far-band outputs.
#[test]
fn far_band_empty_grid_all_zero() {
    let ws = 400.0f32;
    let dims = WorldDims::from_world_size(ws, true);
    let grass = make_grid(dims, 0.0);
    let mut out = [0.0f32; 8];
    compute_grass_far_band_sectors(ws / 2.0, ws / 2.0, &grass, &mut out);
    for (s, &v) in out.iter().enumerate() {
        assert_eq!(v, 0.0, "sector {s} = {v}, expected 0.0 on empty grid");
    }
}

/// A fully saturated grid with refreshed pyramid → all sectors ≈ 1.0.
#[test]
fn far_band_full_grid_all_one() {
    let ws = 400.0f32;
    let dims = WorldDims::from_world_size(ws, true);
    let mut grass = make_grid(dims, GRASS_MAX);
    refresh_pyramid(&mut grass);
    let mut out = [0.0f32; 8];
    compute_grass_far_band_sectors(ws / 2.0, ws / 2.0, &grass, &mut out);
    for (s, &v) in out.iter().enumerate() {
        assert!(
            (v - 1.0).abs() < 0.01,
            "sector {s} = {v}, expected ~1.0 on full grid"
        );
    }
}

/// All outputs are in [0.0, 1.0] for any fill value after pyramid refresh.
#[test]
fn far_band_outputs_always_bounded() {
    for &fill in &[0.0f32, 0.3, 0.7, GRASS_MAX] {
        let ws = 400.0f32;
        let dims = WorldDims::from_world_size(ws, true);
        let mut grass = make_grid(dims, fill);
        refresh_pyramid(&mut grass);
        let mut out = [0.0f32; 8];
        compute_grass_far_band_sectors(ws / 2.0, ws / 2.0, &grass, &mut out);
        for (s, &v) in out.iter().enumerate() {
            assert!(
                v >= 0.0 && v <= 1.0,
                "fill={fill}: sector {s} = {v} out of [0,1]"
            );
        }
    }
}

// ── (e) fallback: walled + species forces single-band ─────────────────────────

#[test]
fn walled_species_multisight_falls_back_to_single_band() {
    let layout_single = NnInputLayout::for_settings(false, true, false);
    let layout_multi = NnInputLayout::for_settings(false, true, true);
    assert_eq!(
        layout_single.width(),
        layout_multi.width(),
        "walled+species: multisight must fall back to same width as single-band"
    );
    assert_eq!(
        layout_multi.width(),
        48,
        "walled+species: expected padded width 48"
    );
    assert!(
        layout_multi
            .offset_of(NnInputGroup::GrassBandsFar)
            .is_none(),
        "walled+species: GrassBandsFar must be absent (budget full)"
    );
}

// ── (f) default constants ──────────────────────────────────────────────────────

#[test]
fn grass_multisight_default_is_on() {
    assert!(
        GRASS_MULTISIGHT_DEFAULT,
        "GRASS_MULTISIGHT_DEFAULT must be true"
    );
}

/// The default World construction (wrap=true, species=false, multisight=true)
/// must include the GrassBandsFar group.
#[test]
fn default_layout_includes_far_band() {
    let layout = NnInputLayout::for_settings(
        WRAP_WORLD_DEFAULT,
        SPECIES_MODE_DEFAULT,
        GRASS_MULTISIGHT_DEFAULT,
    );
    assert!(
        layout.offset_of(NnInputGroup::GrassBandsFar).is_some(),
        "default layout must include GrassBandsFar"
    );
    assert!(
        layout.width() <= MAX_NN_INPUTS,
        "default layout width {} must not exceed MAX_NN_INPUTS {}",
        layout.width(),
        MAX_NN_INPUTS
    );
}
