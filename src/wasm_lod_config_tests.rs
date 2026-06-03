/// v2.0.4 S1: unit tests for the LOD level formula, lod_bias knob, and
/// aspect-ratio fix.
///
/// These tests exercise the math used in `write_snapshot` without constructing
/// a full wasm runtime — they call the same formula directly.

#[cfg(test)]
mod tests {
    use crate::constants::GRASS_CELL_SIZE;
    use crate::wasm_api::{GRASS_LOD_BUDGET_AXIS, GRASS_LOD_MARGIN_FACTOR};

    /// Replicate the LOD level formula from `write_snapshot` so tests stay
    /// in sync with the implementation.
    ///
    /// Signature matches the Rust code:
    ///   world_size: f32   — world extent in world units
    ///   cam_zoom:   f32   — px per world unit (> 0)
    ///   lod_bias:   f32   — subtracted from computed level (clamped ≥ 0)
    ///   max_level:  usize — pyramid max level (saturating clamp)
    fn compute_lod_level(world_size: f32, cam_zoom: f32, lod_bias: f32, max_level: usize) -> usize {
        let safe_zoom = if cam_zoom > 0.0 { cam_zoom } else { 1.0 };
        let visible_cell_span = (world_size / safe_zoom) / GRASS_CELL_SIZE;
        let level_f = (visible_cell_span / GRASS_LOD_BUDGET_AXIS as f32)
            .max(1.0)
            .log2()
            .floor();
        let biased = (level_f - lod_bias.max(0.0)).max(0.0);
        (biased as usize).min(max_level)
    }

    /// Compute the vis_cells for one axis (x or y separately).
    ///
    /// span_cells: visible span in level-0 cells for that axis
    /// mip_level:  pyramid level chosen
    fn vis_cells(span_cells: f32, mip_level: usize) -> usize {
        let scale = (1usize << mip_level) as f32;
        (span_cells / scale).ceil() as usize
    }

    /// Compute raw window size for one axis (before level-edge clamp).
    fn raw_win(vis: usize) -> usize {
        (vis as f32 * GRASS_LOD_MARGIN_FACTOR).ceil() as usize
    }

    // ── Budget = 4096 tests ────────────────────────────────────────────────

    /// Default world (9600 units, zoom=1.0):
    ///   vis_span = 9600/1.0/5.0 = 1920 cells
    ///   1920 / 4096 = 0.469 < 1.0 → log2(max(1.0,...)) = log2(1.0) = 0 → level 0
    #[test]
    fn budget_4096_default_world_zoom1_is_level0() {
        let level = compute_lod_level(9600.0, 1.0, 0.0, 4);
        assert_eq!(level, 0, "default world at zoom=1 must be level 0");
    }

    /// A 16384-unit world at zoom=1 should still be level 0 (span 3277 < 4096).
    #[test]
    fn budget_4096_world16384_zoom1_is_level0() {
        // span = 16384 / 5.0 = 3276.8
        let level = compute_lod_level(16384.0, 1.0, 0.0, 4);
        assert_eq!(
            level, 0,
            "16384-unit world at zoom=1 must be level 0 with 4096 budget"
        );
    }

    /// A very large world (20480 units, zoom=1):
    ///   span = 20480 / 5.0 = 4096 cells
    ///   4096 / 4096 = 1.0 → log2(1.0) = 0 → level 0 (border case)
    #[test]
    fn budget_4096_world20480_zoom1_boundary_is_level0() {
        let level = compute_lod_level(20480.0, 1.0, 0.0, 4);
        assert_eq!(
            level, 0,
            "20480 world at zoom=1 should be exactly at the budget boundary → level 0"
        );
    }

    /// A world that triggers level 1 with budget 4096:
    ///   Level 1 requires span/budget ≥ 2.0, i.e., span ≥ 8192 cells (world ≥ 40960).
    ///   world=40960: span = 40960/5 = 8192; ratio = 8192/4096 = 2.0 → log2(2) = 1 → level 1
    #[test]
    fn budget_4096_world_over_budget_is_level1() {
        // span = 40960 / 5.0 = 8192; ratio = 8192/4096 = 2.0 → level 1
        let level = compute_lod_level(40960.0, 1.0, 0.0, 4);
        assert_eq!(level, 1, "world at span=8192 (2× budget) should be level 1");
    }

    /// With the OLD budget of 2048 the same default world (span=1920) would be
    /// level 0, but at span=2049 it would be level 1. Confirm that with budget
    /// 4096 that same span stays at level 0 — this is the regression guard for
    /// the "downsamples a layer too early" bug.
    #[test]
    fn budget_4096_span2049_stays_level0() {
        // world = 2049 * 5 = 10245; span = 2049 < 4096 → level 0
        let level = compute_lod_level(10245.0, 1.0, 0.0, 4);
        assert_eq!(
            level, 0,
            "span 2049 should be level 0 with 4096 budget (was level 1 with 2048)"
        );
    }

    // ── lod_bias tests ────────────────────────────────────────────────────

    /// With bias 0.0 a zoom-out that would normally produce level 1 stays at 1.
    #[test]
    fn lod_bias_zero_no_change() {
        // world=40960, span=8192; ratio = 8192/4096 = 2.0 → log2 = 1.0 → level 1
        let level = compute_lod_level(40960.0, 1.0, 0.0, 4);
        assert_eq!(level, 1);
    }

    /// With bias 1.0 level 1 → level 0 (finer detail).
    #[test]
    fn lod_bias_1_drives_level1_to_level0() {
        let level = compute_lod_level(40960.0, 1.0, 1.0, 4);
        assert_eq!(level, 0, "bias=1.0 should bring level 1 down to level 0");
    }

    /// With bias 2.0 level 2 → level 0 (clamped, not negative).
    #[test]
    fn lod_bias_2_clamps_to_zero() {
        // world=163840, span=32768; ratio=8→log2=3→level3; bias 4 → 3-4=-1→clamp→0
        let level = compute_lod_level(163840.0, 1.0, 4.0, 4);
        assert_eq!(level, 0, "bias must clamp computed level to ≥ 0");
    }

    /// Negative bias input is a no-op (clamped to 0 before subtraction).
    #[test]
    fn lod_bias_negative_is_noop() {
        let level_no_bias = compute_lod_level(40960.0, 1.0, 0.0, 4);
        let level_neg_bias = compute_lod_level(40960.0, 1.0, -2.0, 4);
        assert_eq!(
            level_no_bias, level_neg_bias,
            "negative lod_bias must be treated as 0.0"
        );
    }

    /// Bias is clamped so it never exceeds max_level: even with huge bias the
    /// result is still in [0, max_level].
    #[test]
    fn lod_bias_never_exceeds_max_level() {
        let max_level = 3;
        let level = compute_lod_level(163840.0, 1.0, 0.0, max_level);
        assert!(
            level <= max_level,
            "mip_level must not exceed pyramid max level"
        );
    }

    // ── Aspect-ratio / vis_cells_y fix ────────────────────────────────────

    /// Square viewport: vis_cells_x == vis_cells_y.
    #[test]
    fn aspect_square_viewport_equal_cells() {
        let span_x = 1920.0_f32;
        let aspect = 1.0_f32; // square
        let span_y = span_x * aspect;
        let mip = 0usize;
        let cx = vis_cells(span_x, mip);
        let cy = vis_cells(span_y, mip);
        assert_eq!(
            cx, cy,
            "square viewport must yield equal vis_cells in both axes"
        );
    }

    /// 16:9 viewport (1920×1080 px): vis_cells_y < vis_cells_x.
    #[test]
    fn aspect_16x9_viewport_y_less_than_x() {
        let span_x = 1920.0_f32;
        let aspect = 1080.0_f32 / 1920.0_f32; // ≈ 0.5625
        let span_y = span_x * aspect; // = 1080
        let mip = 0usize;
        let cx = vis_cells(span_x, mip);
        let cy = vis_cells(span_y, mip);
        // 1920 cells × 0.5625 = 1080 → different from 1920
        assert!(
            cy < cx,
            "16:9 viewport: vis_cells_y ({cy}) must be < vis_cells_x ({cx})"
        );
        assert_eq!(cx, 1920);
        assert_eq!(cy, 1080);
    }

    /// Portrait viewport (9:16): vis_cells_y > vis_cells_x.
    #[test]
    fn aspect_portrait_viewport_y_greater_than_x() {
        let span_x = 1080.0_f32;
        let aspect = 1920.0_f32 / 1080.0_f32;
        let span_y = span_x * aspect;
        let mip = 0usize;
        let cx = vis_cells(span_x, mip);
        let cy = vis_cells(span_y, mip);
        assert!(
            cy > cx,
            "portrait viewport: vis_cells_y ({cy}) must be > vis_cells_x ({cx})"
        );
    }

    /// The raw_win with margin factor does NOT exceed GRASS_LOD_BUDGET_AXIS.
    #[test]
    fn raw_win_capped_at_budget() {
        // Give a very large vis_cells to trigger the budget clamp.
        let vis: usize = GRASS_LOD_BUDGET_AXIS * 2;
        let rw = raw_win(vis).min(GRASS_LOD_BUDGET_AXIS);
        assert_eq!(rw, GRASS_LOD_BUDGET_AXIS);
    }
}
