//! v2.0.3 Stream 2a — GrassPyramid smoke test.
//!
//! Minor validation (NOT the Stage-3 full battery). Builds a small pyramid for
//! a known grid, fills density with a known pattern, calls refresh, and checks:
//!   (a) level dims halve correctly (ceil-divide) down to the top;
//!   (b) an L1 cell equals the box-filter (mean) of its 2×2 L0 parents (within
//!       u8 rounding tolerance ≤1);
//!   (c) viewport_window copies the right clamped bytes;
//!   (d) no panic anywhere in the sequence.
//!
//! Lives in its OWN file (merge-hazard rule). Wired as a child module of `grass`
//! via `#[path = ...]` so it can reach the crate-internal fields/fns.

use super::super::GrassPyramid;
use std::sync::atomic::{AtomicU64, AtomicU8};

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build a density slice with a uniform fill value.
fn uniform_density(size: usize, byte: u8) -> Vec<AtomicU8> {
    (0..size).map(|_| AtomicU8::new(byte)).collect()
}

/// Build a density slice where cell (ix, iy) = `(iy * w + ix) as u8` (wraps
/// on u8, which is fine — we just need a non-trivial spatially-varying pattern).
fn indexed_density(w: usize, h: usize) -> Vec<AtomicU8> {
    (0..w * h).map(|i| AtomicU8::new(i as u8)).collect()
}

/// Empty tile_active bitset (pyramid refresh ignores it for now).
fn empty_tile_active(n_words: usize) -> Vec<AtomicU64> {
    (0..n_words).map(|_| AtomicU64::new(0)).collect()
}

// ── Test (a): level dims halve with div_ceil ─────────────────────────────────

/// For a 7×5 grid the level dims chain must be:
///   L0 = (7, 5)
///   L1 = (4, 3)   [7.div_ceil(2)=4, 5.div_ceil(2)=3]
///   L2 = (2, 2)   [4.div_ceil(2)=2, 3.div_ceil(2)=2]
///   L3 = (1, 1)   [2.div_ceil(2)=1, 2.div_ceil(2)=1]
///   stop at (1,1)
#[test]
fn level_dims_halve_with_ceil_small_odd() {
    let p = GrassPyramid::new_rect(7, 5);
    assert_eq!(
        p.level_dims,
        vec![(7, 5), (4, 3), (2, 2), (1, 1)],
        "7×5 pyramid dims mismatch: {:?}",
        p.level_dims
    );
    assert_eq!(p.levels.len(), 3, "should have 3 owned levels (L1/L2/L3)");
}

/// For a 1920×1920 (default grass_dim) grid: repeated div_ceil(2) halving
/// must reach (1,1) in exactly 11 halvings → 12 levels total (L0..L11).
#[test]
fn level_dims_default_grass_dim() {
    let p = GrassPyramid::new(1920);
    // 1920 → 960 → 480 → 240 → 120 → 60 → 30 → 15 → 8 → 4 → 2 → 1
    // That's 11 halvings → 12 levels (indices 0..=11).
    let expected_dims: Vec<usize> = {
        let mut v = vec![1920usize];
        loop {
            let last = *v.last().unwrap();
            if last == 1 {
                break;
            }
            v.push(last.div_ceil(2));
        }
        v
    };
    let actual: Vec<usize> = p.level_dims.iter().map(|&(w, _)| w).collect();
    assert_eq!(
        actual, expected_dims,
        "level widths mismatch at default grass_dim=1920"
    );
    assert_eq!(p.num_levels(), expected_dims.len());
    // The last level must be (1,1) since 1920 is a power-of-two-times-something
    // that eventually reaches 1 exactly.
    assert_eq!(
        *p.level_dims.last().unwrap(),
        (1, 1),
        "top level must be (1,1)"
    );
}

/// For a 4×4 grid, all dims must be exact powers of 2 halvings: 4→2→1.
#[test]
fn level_dims_power_of_two() {
    let p = GrassPyramid::new(4);
    assert_eq!(p.level_dims, vec![(4, 4), (2, 2), (1, 1)]);
}

// ── Test (b): L1 cell = box-filter mean of its 2×2 L0 parents ───────────────

/// Fill a 4×4 grid with uniform 200; after refresh every L1 cell must be 200
/// (all parents equal → mean = 200 exactly).
#[test]
fn l1_uniform_is_mean() {
    let w = 4usize;
    let h = 4usize;
    let density = uniform_density(w * h, 200);
    let tile_active = empty_tile_active(1);
    let mut p = GrassPyramid::new_rect(w, h);
    p.refresh(&density, &tile_active, 1);

    let (lw, lh) = p.level_dims[1];
    assert_eq!((lw, lh), (2, 2));
    for oy in 0..lh {
        for ox in 0..lw {
            let got = p.levels[0][oy * lw + ox];
            assert_eq!(
                got, 200,
                "L1 cell ({ox},{oy}) = {got}, expected 200 (uniform fill)"
            );
        }
    }
}

/// Fill a 4×4 grid with a spatially-varying pattern and manually verify that
/// L1 cell (0,0) equals the rounded mean of L0 parents (0,0),(1,0),(0,1),(1,1).
///
/// The box-filter formula is `(sum + 2) >> 2` (round-before-truncate when count=4).
#[test]
fn l1_cell_equals_box_filter_mean() {
    let w = 4usize;
    let h = 4usize;
    let density: Vec<AtomicU8> = vec![
        // row 0: 100, 200, 50, 150
        AtomicU8::new(100),
        AtomicU8::new(200),
        AtomicU8::new(50),
        AtomicU8::new(150),
        // row 1: 80, 120, 40, 60
        AtomicU8::new(80),
        AtomicU8::new(120),
        AtomicU8::new(40),
        AtomicU8::new(60),
        // row 2: 0, 0, 255, 255
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(255),
        AtomicU8::new(255),
        // row 3: 0, 0, 255, 255
        AtomicU8::new(0),
        AtomicU8::new(0),
        AtomicU8::new(255),
        AtomicU8::new(255),
    ];

    let tile_active = empty_tile_active(1);
    let mut p = GrassPyramid::new_rect(w, h);
    p.refresh(&density, &tile_active, 1);

    // L1 cell (0,0): parents are (0,0)=100, (1,0)=200, (0,1)=80, (1,1)=120.
    // sum = 500, (500 + 2) >> 2 = 502 / 4 = 125 (integer division).
    let expected_00 = (100u32 + 200 + 80 + 120 + 2) / 4; // = 125
    let got_00 = p.levels[0][0];
    assert_eq!(
        got_00, expected_00 as u8,
        "L1(0,0) expected {expected_00}, got {got_00}"
    );

    // L1 cell (1,0): parents are (2,0)=50, (3,0)=150, (2,1)=40, (3,1)=60.
    // sum = 300, (300 + 2) >> 2 = 302 / 4 = 75.
    let expected_10 = (50u32 + 150 + 40 + 60 + 2) / 4; // = 75
    let got_10 = p.levels[0][1];
    assert_eq!(
        got_10, expected_10 as u8,
        "L1(1,0) expected {expected_10}, got {got_10}"
    );

    // L1 cell (0,1): parents are (0,2)=0, (1,2)=0, (0,3)=0, (1,3)=0.
    // sum = 0, mean = 0.
    let got_01 = p.levels[0][2]; // L1 is 2×2; cell (0,1) = index 2
    assert_eq!(got_01, 0, "L1(0,1) expected 0, got {got_01}");

    // L1 cell (1,1): parents are (2,2)=255, (3,2)=255, (2,3)=255, (3,3)=255.
    // sum = 1020, (1020 + 2) >> 2 = 255 (exact, no rounding issue).
    let got_11 = p.levels[0][3];
    assert_eq!(got_11, 255, "L1(1,1) expected 255, got {got_11}");
}

/// Ragged (odd-width) grid: 3×3. The bottom-right L1 corner has only 1 parent.
/// Verify edge-clamped averaging: no padding with zeros (that would darken borders).
#[test]
fn l1_edge_clamp_no_dark_border() {
    // 3×3 uniform fill of 100.
    let density = uniform_density(3 * 3, 100);
    let tile_active = empty_tile_active(1);
    let mut p = GrassPyramid::new_rect(3, 3);
    // Level dims: (3,3) → (2,2) → (1,1)
    assert_eq!(p.level_dims[1], (2, 2));
    p.refresh(&density, &tile_active, 1);

    // L1 cell (0,0): parents (0,0),(1,0),(0,1),(1,1) all 100 → mean = 100.
    assert_eq!(p.levels[0][0], 100, "L1(0,0)");
    // L1 cell (1,0): parents (2,0),(2,1) only (x=3 is OOB) → mean of 2 = 100.
    assert_eq!(
        p.levels[0][1], 100,
        "L1(1,0) — ragged right column, still 100"
    );
    // L1 cell (0,1): parents (0,2),(1,2) only (y=3 is OOB) → mean of 2 = 100.
    assert_eq!(
        p.levels[0][2], 100,
        "L1(0,1) — ragged bottom row, still 100"
    );
    // L1 cell (1,1): parent (2,2) only → 100.
    assert_eq!(
        p.levels[0][3], 100,
        "L1(1,1) — single-parent corner, still 100"
    );
}

// ── Test (c): viewport_window copies the right clamped bytes ─────────────────

/// Exact-bounds window: request the full L1 of a 4×4 grid.
#[test]
fn viewport_window_full_level() {
    let w = 4usize;
    let h = 4usize;
    // Uniform 200 → L1 is 2×2 of all 200.
    let density = uniform_density(w * h, 200);
    let tile_active = empty_tile_active(1);
    let mut p = GrassPyramid::new_rect(w, h);
    p.refresh(&density, &tile_active, 1);

    let mut dst = vec![0u8; 4];
    p.viewport_window(&density, 1, false, 0, 0, 2, 2, &mut dst);
    assert_eq!(dst, vec![200, 200, 200, 200], "full L1 window");
}

/// Window larger than the level: all out-of-bounds coords edge-clamp to the
/// last valid cell value (not zeros — avoids black border on overscan).
#[test]
fn viewport_window_overscan_clamped() {
    let w = 2usize;
    let h = 2usize;
    // L0 = 2×2 with values [10, 20, 30, 40].
    let density: Vec<AtomicU8> = vec![
        AtomicU8::new(10),
        AtomicU8::new(20),
        AtomicU8::new(30),
        AtomicU8::new(40),
    ];
    let _tile_active = empty_tile_active(1);
    let p = GrassPyramid::new_rect(w, h); // no refresh needed; reading L0

    // Request a 3×3 window starting at (0,0) from L0.
    let mut dst = vec![0u8; 9];
    p.viewport_window(&density, 0, false, 0, 0, 3, 3, &mut dst);
    // Expected (edge-clamp): col 2 repeats col 1; row 2 repeats row 1.
    //   [10, 20, 20]
    //   [30, 40, 40]
    //   [30, 40, 40]
    let expected = vec![10, 20, 20, 30, 40, 40, 30, 40, 40];
    assert_eq!(dst, expected, "overscan window: {:?}", dst);
}

/// Partial interior window: request a 1×1 sub-window at an interior cell.
#[test]
fn viewport_window_partial_interior() {
    let w = 4usize;
    let h = 4usize;
    let density = indexed_density(w, h);
    let tile_active = empty_tile_active(1);
    let mut p = GrassPyramid::new_rect(w, h);
    p.refresh(&density, &tile_active, 1);

    // Read L0 cell at (2, 1) directly via viewport_window.
    let mut dst = vec![0u8; 1];
    p.viewport_window(&density, 0, false, 2, 1, 1, 1, &mut dst);
    // Cell (2, 1) = index 1*4+2 = 6, value = 6 as u8.
    assert_eq!(dst[0], 6, "L0 cell (2,1)");
}

#[test]
fn viewport_window_wraps_both_seams() {
    let w = 4usize;
    let h = 3usize;
    let density = indexed_density(w, h);
    let p = GrassPyramid::new_rect(w, h);

    let mut dst = vec![0u8; 6];
    p.viewport_window(&density, 0, true, 3, 2, 3, 2, &mut dst);

    assert_eq!(dst, vec![11, 8, 9, 3, 0, 1]);
}

// ── Test (d): sample_clamped API ─────────────────────────────────────────────

/// sample_clamped at L0 reads the density byte directly (no copy).
#[test]
fn sample_clamped_l0_reads_density() {
    let w = 4usize;
    let h = 4usize;
    let density: Vec<AtomicU8> = (0..16u8).map(AtomicU8::new).collect();
    let p = GrassPyramid::new_rect(w, h);
    for iy in 0..h {
        for ix in 0..w {
            let got = p.sample_clamped(&density, 0, ix, iy);
            let expected = (iy * w + ix) as u8;
            assert_eq!(got, expected, "sample_clamped L0 ({ix},{iy})");
        }
    }
}

/// sample_clamped out-of-bounds clamps to the last valid cell.
#[test]
fn sample_clamped_oob_clamps() {
    let w = 2usize;
    let h = 2usize;
    let density: Vec<AtomicU8> = vec![
        AtomicU8::new(1),
        AtomicU8::new(2),
        AtomicU8::new(3),
        AtomicU8::new(4),
    ];
    let p = GrassPyramid::new_rect(w, h);
    // OOB x → clamped to w-1=1; OOB y → clamped to h-1=1.
    let got = p.sample_clamped(&density, 0, 999, 999);
    assert_eq!(got, 4, "OOB clamp should return bottom-right cell = 4");
}

// ── Test (d extra): no panic on zero-density, multi-level refresh ────────────

/// Full multi-level refresh on a 5×5 grid with zero density must not panic.
#[test]
fn refresh_zero_density_no_panic() {
    let w = 5usize;
    let h = 5usize;
    let density = uniform_density(w * h, 0);
    let tile_active = empty_tile_active(1);
    let mut p = GrassPyramid::new_rect(w, h);
    p.refresh(&density, &tile_active, 1); // must not panic
    for (k, lvl) in p.levels.iter().enumerate() {
        for &b in lvl.iter() {
            assert_eq!(
                b,
                0,
                "L{} should be all-zero after zero-density refresh",
                k + 1
            );
        }
    }
}

/// Multi-level propagation: fill a 16×16 with 128; after refresh every level
/// must be 128 (averaging a uniform field is identity).
#[test]
fn refresh_uniform_propagates_through_all_levels() {
    let w = 16usize;
    let h = 16usize;
    let density = uniform_density(w * h, 128);
    let tile_active = empty_tile_active(1);
    let mut p = GrassPyramid::new_rect(w, h);
    // Dims: 16→8→4→2→1 (5 levels, 4 owned).
    assert_eq!(p.level_dims.len(), 5);
    p.refresh(&density, &tile_active, 1);

    for (k, lvl) in p.levels.iter().enumerate() {
        for (i, &b) in lvl.iter().enumerate() {
            assert_eq!(
                b,
                128,
                "L{} cell {} = {b}, expected 128 (uniform field)",
                k + 1,
                i
            );
        }
    }
}
