//! v2.0.6 S9: output-equivalence tests for BiomePyramid.
//!
//! The precomputed biome pyramid MUST produce byte-identical snapshot biome
//! output compared to the original per-tick nested-loop recompute, for all
//! mip levels and representative seeds / LOD / zoom configurations.
//!
//! These tests are the correctness gate for the biome-precompute optimization.
//! They live in their own file (merge-hazard rule: never append to a shared
//! `mod tests`).

use crate::constants::WorldDims;
use crate::wasm_api::{snapshot_window_copy_origin, BiomePyramid};
use crate::world::biome::generate_biome_grid;

/// Reference implementation of the old per-tick biome window loop (verbatim
/// copy from the pre-optimization `write_snapshot`). Used ONLY by these tests
/// to verify the precomputed pyramid is byte-identical.
#[allow(clippy::too_many_arguments)]
fn biome_window_reference(
    biome_grid: &[u8],
    grass_dim_l0: usize,
    mip_level: usize,
    win_origin_x: usize,
    win_origin_y: usize,
    win_w: usize,
    win_h: usize,
    out: &mut Vec<u8>,
) {
    let block = 1usize << mip_level;
    out.resize(win_w * win_h, 0);
    for out_y in 0..win_h {
        let lk_row = win_origin_y + out_y;
        let l0_row_base = (lk_row * block).min(grass_dim_l0.saturating_sub(1));
        for out_x in 0..win_w {
            let lk_col = win_origin_x + out_x;
            let l0_col_base = (lk_col * block).min(grass_dim_l0.saturating_sub(1));
            let mut counts = [0u32; 4];
            for dy in 0..block {
                let row = (l0_row_base + dy).min(grass_dim_l0 - 1);
                for dx in 0..block {
                    let col = (l0_col_base + dx).min(grass_dim_l0 - 1);
                    let tag = biome_grid[row * grass_dim_l0 + col] as usize;
                    counts[tag.min(counts.len() - 1)] += 1;
                }
            }
            let mut best_tag = 0u8;
            let mut best_count = 0u32;
            for (tag, &cnt) in counts.iter().enumerate() {
                if cnt > best_count {
                    best_count = cnt;
                    best_tag = tag as u8;
                }
            }
            out[out_y * win_w + out_x] = best_tag;
        }
    }
}

/// Minimal BiomePyramid reconstruction for direct testing (mirrors the
/// internal struct in wasm_api.rs — we test the observable output, not the
/// struct internals, so we replicate the build logic here).
fn build_biome_pyramid(biome_grid: &[u8], level_dims: &[(usize, usize)]) -> Vec<Vec<u8>> {
    let mut levels: Vec<Vec<u8>> = Vec::with_capacity(level_dims.len());
    levels.push(biome_grid.to_vec());
    let (l0_w, l0_h) = level_dims[0];
    for k in 1..level_dims.len() {
        let (lk_w, lk_h) = level_dims[k];
        let block = 1usize << k;
        let mut lk = vec![0u8; lk_w * lk_h];
        for oy in 0..lk_h {
            let l0_row_base = (oy * block).min(l0_h.saturating_sub(1));
            for ox in 0..lk_w {
                let l0_col_base = (ox * block).min(l0_w.saturating_sub(1));
                let mut counts = [0u32; 4];
                for dy in 0..block {
                    let row = (l0_row_base + dy).min(l0_h - 1);
                    for dx in 0..block {
                        let col = (l0_col_base + dx).min(l0_w - 1);
                        let tag = biome_grid[row * l0_w + col] as usize;
                        counts[tag.min(counts.len() - 1)] += 1;
                    }
                }
                let mut best_tag = 0u8;
                let mut best_count = 0u32;
                for (tag, &cnt) in counts.iter().enumerate() {
                    if cnt > best_count {
                        best_count = cnt;
                        best_tag = tag as u8;
                    }
                }
                lk[oy * lk_w + ox] = best_tag;
            }
        }
        levels.push(lk);
    }
    levels
}

fn copy_window_from_level(
    levels: &[Vec<u8>],
    level_dims: &[(usize, usize)],
    k: usize,
    origin_x: usize,
    origin_y: usize,
    win_w: usize,
    win_h: usize,
) -> Vec<u8> {
    let (lk_w, _) = level_dims[k];
    let src = &levels[k];
    let mut dst = vec![0u8; win_w * win_h];
    for row in 0..win_h {
        let src_off = (origin_y + row) * lk_w + origin_x;
        let dst_off = row * win_w;
        dst[dst_off..dst_off + win_w].copy_from_slice(&src[src_off..src_off + win_w]);
    }
    dst
}

/// Compute GrassPyramid-style level_dims for a square grid of side `grass_dim`.
/// Mirrors `GrassPyramid::compute_level_dims` without importing the private fn.
fn compute_level_dims(grass_dim: usize) -> Vec<(usize, usize)> {
    const GRASS_PYRAMID_MAX_LEVELS: usize = 12;
    let mut dims = Vec::with_capacity(GRASS_PYRAMID_MAX_LEVELS);
    dims.push((grass_dim, grass_dim));
    loop {
        if dims.len() >= GRASS_PYRAMID_MAX_LEVELS {
            break;
        }
        let (pw, ph) = *dims.last().unwrap();
        if pw == 1 && ph == 1 {
            break;
        }
        let nw = pw.div_ceil(2);
        let nh = ph.div_ceil(2);
        dims.push((nw, nh));
    }
    dims
}

// ── Test cases ────────────────────────────────────────────────────────────────

/// T1: Full L0 window (mip_level=0, full-field) for multiple seeds.
/// This is the hot path at default zoom. Must be byte-identical.
#[test]
fn biome_pyramid_l0_full_window_byte_identical() {
    // Use a small grid (64×64) so the test is fast but non-trivial.
    let world_size = 320.0_f32; // 320/5.0 = 64 cells per axis
    let dims = WorldDims::from_world_size(world_size, true);
    let grass_dim = dims.grass_dim;

    for seed in [1u32, 42, 99999, 0xDEADBEEF] {
        let biome_grid = generate_biome_grid(seed, &dims);
        let level_dims = compute_level_dims(grass_dim);
        let pyramid_levels = build_biome_pyramid(&biome_grid, &level_dims);

        // L0: full window.
        let mip = 0;
        let win_w = grass_dim;
        let win_h = grass_dim;

        let mut reference = Vec::new();
        biome_window_reference(
            &biome_grid,
            grass_dim,
            mip,
            0,
            0,
            win_w,
            win_h,
            &mut reference,
        );
        let precomputed =
            copy_window_from_level(&pyramid_levels, &level_dims, mip, 0, 0, win_w, win_h);

        assert_eq!(
            reference, precomputed,
            "L0 full window not byte-identical for seed={seed}"
        );
    }
}

/// T2: L0 subwindow (panned camera) — origin non-zero.
#[test]
fn biome_pyramid_l0_subwindow_byte_identical() {
    let world_size = 320.0_f32;
    let dims = WorldDims::from_world_size(world_size, true);
    let grass_dim = dims.grass_dim; // 64

    let biome_grid = generate_biome_grid(42, &dims);
    let level_dims = compute_level_dims(grass_dim);
    let pyramid_levels = build_biome_pyramid(&biome_grid, &level_dims);

    // Subwindow: origin=(10, 5), size=40×30.
    let mip = 0;
    let win_w = 40;
    let win_h = 30;
    let ox = 10;
    let oy = 5;

    let mut reference = Vec::new();
    biome_window_reference(
        &biome_grid,
        grass_dim,
        mip,
        ox,
        oy,
        win_w,
        win_h,
        &mut reference,
    );
    let precomputed =
        copy_window_from_level(&pyramid_levels, &level_dims, mip, ox, oy, win_w, win_h);

    assert_eq!(reference, precomputed, "L0 subwindow not byte-identical");
}

/// T3: L1 full window (mip_level=1, block=2×2 mode downsample).
/// Key correctness check: mode accumulation at L1 must match reference.
#[test]
fn biome_pyramid_l1_full_window_byte_identical() {
    let world_size = 320.0_f32;
    let dims = WorldDims::from_world_size(world_size, true);
    let grass_dim = dims.grass_dim; // 64

    for seed in [1u32, 42, 12345, 0xCAFEBABE] {
        let biome_grid = generate_biome_grid(seed, &dims);
        let level_dims = compute_level_dims(grass_dim);
        let pyramid_levels = build_biome_pyramid(&biome_grid, &level_dims);

        let mip = 1;
        let (lk_w, lk_h) = level_dims[mip];

        let mut reference = Vec::new();
        biome_window_reference(
            &biome_grid,
            grass_dim,
            mip,
            0,
            0,
            lk_w,
            lk_h,
            &mut reference,
        );
        let precomputed =
            copy_window_from_level(&pyramid_levels, &level_dims, mip, 0, 0, lk_w, lk_h);

        assert_eq!(
            reference, precomputed,
            "L1 full window not byte-identical for seed={seed}"
        );
    }
}

/// T4: L2 full window (mip_level=2, block=4×4).
#[test]
fn biome_pyramid_l2_full_window_byte_identical() {
    let world_size = 320.0_f32;
    let dims = WorldDims::from_world_size(world_size, true);
    let grass_dim = dims.grass_dim; // 64

    let biome_grid = generate_biome_grid(7777, &dims);
    let level_dims = compute_level_dims(grass_dim);
    let pyramid_levels = build_biome_pyramid(&biome_grid, &level_dims);

    let mip = 2;
    let (lk_w, lk_h) = level_dims[mip];

    let mut reference = Vec::new();
    biome_window_reference(
        &biome_grid,
        grass_dim,
        mip,
        0,
        0,
        lk_w,
        lk_h,
        &mut reference,
    );
    let precomputed = copy_window_from_level(&pyramid_levels, &level_dims, mip, 0, 0, lk_w, lk_h);

    assert_eq!(reference, precomputed, "L2 full window not byte-identical");
}

/// T5: L3 subwindow (mip_level=3, block=8×8, panned camera).
#[test]
fn biome_pyramid_l3_subwindow_byte_identical() {
    let world_size = 640.0_f32; // 128 cells per axis
    let dims = WorldDims::from_world_size(world_size, true);
    let grass_dim = dims.grass_dim; // 128

    let biome_grid = generate_biome_grid(1234, &dims);
    let level_dims = compute_level_dims(grass_dim);
    let pyramid_levels = build_biome_pyramid(&biome_grid, &level_dims);

    let mip = 3;
    let (lk_w, lk_h) = level_dims[mip]; // 16×16
                                        // Subwindow: origin=(2,1), size=8×6 (within bounds).
    let ox = 2;
    let oy = 1;
    let win_w = 8.min(lk_w - ox);
    let win_h = 6.min(lk_h - oy);

    let mut reference = Vec::new();
    biome_window_reference(
        &biome_grid,
        grass_dim,
        mip,
        ox,
        oy,
        win_w,
        win_h,
        &mut reference,
    );
    let precomputed =
        copy_window_from_level(&pyramid_levels, &level_dims, mip, ox, oy, win_w, win_h);

    assert_eq!(reference, precomputed, "L3 subwindow not byte-identical");
}

/// T6: Default scale (grass_dim=1920, world_size=9600) L0 full window spot check.
/// Uses a 1920×1920 grid. Checks a sample of pixels, not all 3.7M, to keep
/// test time reasonable. Verifies the "hot path" grid size is correct.
#[test]
fn biome_pyramid_default_scale_l0_spot_check() {
    let world_size = 9600.0_f32;
    let dims = WorldDims::from_world_size(world_size, true);
    let grass_dim = dims.grass_dim; // 1920

    let biome_grid = generate_biome_grid(12345, &dims);
    let level_dims = compute_level_dims(grass_dim);
    let pyramid_levels = build_biome_pyramid(&biome_grid, &level_dims);

    // Spot-check: verify L0 pyramid level is byte-identical to the biome_grid.
    assert_eq!(
        pyramid_levels[0], biome_grid,
        "L0 level must be byte-identical to biome_grid"
    );
    assert_eq!(
        pyramid_levels[0].len(),
        grass_dim * grass_dim,
        "L0 level size must equal grass_dim²"
    );

    // Spot-check a subwindow (viewport window: origin=0,0, size=1920×1620).
    let win_w = 1920;
    let win_h = 1620.min(grass_dim);
    // Sample 1000 evenly-spaced pixels from the reference output and compare.
    let mut reference = Vec::new();
    biome_window_reference(
        &biome_grid,
        grass_dim,
        0,
        0,
        0,
        win_w,
        win_h,
        &mut reference,
    );
    let precomputed = copy_window_from_level(&pyramid_levels, &level_dims, 0, 0, 0, win_w, win_h);
    assert_eq!(
        reference.len(),
        precomputed.len(),
        "reference and precomputed output lengths must match"
    );
    // Full compare (both are just a copy of the biome_grid subregion at L0).
    assert_eq!(
        reference, precomputed,
        "default-scale L0 viewport window not byte-identical"
    );
}

/// T7: Tie-breaking correctness — equal counts → lowest tag wins (Plains=0 over Water=1).
/// Exercises the mode tie-break path independently.
#[test]
fn biome_pyramid_tiebreak_lowest_tag_wins() {
    // Manually construct a 4×4 biome grid with known values to test tie-breaking.
    // 2×2 block at L1 for cell (0,0): contains 2 Plains (0) and 2 Water (1) → tie → Plains wins.
    // 2×2 block at L1 for cell (1,0): contains 2 Water (1) and 2 Desert (2) → tie → Water wins.
    let grass_dim = 4;
    // Row-major 4×4:
    //   (0,0)=0 (1,0)=1 (2,0)=1 (3,0)=2
    //   (0,1)=0 (1,1)=1 (2,1)=1 (3,1)=2
    //   (0,2)=0 (1,2)=0 (2,2)=2 (3,2)=2
    //   (0,3)=0 (1,3)=0 (2,3)=2 (3,3)=2
    // At L1, cell (0,0) covers rows 0..2, cols 0..2 of L0:
    //   values = [0,1, 0,1] → counts[0]=2, counts[1]=2 → tie → tag 0 (Plains).
    // At L1, cell (1,0) covers rows 0..2, cols 2..4 of L0:
    //   values = [1,2, 1,2] → counts[1]=2, counts[2]=2 → tie → tag 1 (Water).
    // At L1, cell (0,1) covers rows 2..4, cols 0..2 of L0:
    //   values = [0,0, 0,0] → counts[0]=4 → Plains.
    // At L1, cell (1,1) covers rows 2..4, cols 2..4 of L0:
    //   values = [2,2, 2,2] → counts[2]=4 → Desert.
    let biome_grid: Vec<u8> = vec![0, 1, 1, 2, 0, 1, 1, 2, 0, 0, 2, 2, 0, 0, 2, 2];
    let level_dims = compute_level_dims(grass_dim);
    let pyramid_levels = build_biome_pyramid(&biome_grid, &level_dims);

    // L1 should be a 2×2 grid:
    assert_eq!(level_dims[1], (2, 2));
    let l1 = &pyramid_levels[1];
    // (0,0)=0 (Plains, tie-break), (1,0)=1 (Water, tie-break),
    // (0,1)=0 (Plains, majority), (1,1)=2 (Desert, majority).
    assert_eq!(l1[0], 0, "cell (0,0) must be Plains (tie-break)");
    assert_eq!(l1[1], 1, "cell (1,0) must be Water (tie-break)");
    assert_eq!(l1[2], 0, "cell (0,1) must be Plains (majority)");
    assert_eq!(l1[3], 2, "cell (1,1) must be Desert (majority)");

    // Also verify reference matches.
    let mut reference = Vec::new();
    biome_window_reference(&biome_grid, grass_dim, 1, 0, 0, 2, 2, &mut reference);
    assert_eq!(
        reference, *l1,
        "reference must match precomputed for tie-break test"
    );
}

#[test]
fn biome_pyramid_copy_window_wraps_both_seams() {
    let level_dims = vec![(4, 3)];
    let pyramid = BiomePyramid::build(&(0u8..12).collect::<Vec<_>>(), &level_dims);
    let mut dst = vec![0u8; 6];

    pyramid.copy_window(0, true, 3, 2, 3, 2, &mut dst);

    assert_eq!(dst, vec![11, 8, 9, 3, 0, 1]);
}

#[test]
fn snapshot_window_copy_origin_wraps_but_walled_origin_clamps() {
    assert_eq!(snapshot_window_copy_origin(-2, 8, 3, true), 6);
    assert_eq!(snapshot_window_copy_origin(9, 8, 3, true), 1);
    assert_eq!(snapshot_window_copy_origin(-2, 8, 3, false), 0);
    assert_eq!(snapshot_window_copy_origin(9, 8, 3, false), 5);
}
