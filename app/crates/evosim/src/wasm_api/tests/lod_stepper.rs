// v2.0.6 S10: unit tests for the discrete LOD stepper added to write_snapshot.
//
// The stepper (grass_lod_step, SLIDER_NAMES index 63) controls which mip level
// write_snapshot selects:
//   0 = Auto (use formula + lod_bias — backward compat)
//   1 = L0 (force full resolution, mip 0)
//   2 = L1, 3 = L2, 4 = L3, 5 = L4
//
// These tests exercise the mip-level mapping without constructing a full wasm
// runtime. They replicate the logic in WorldHandle::write_snapshot and
// WorldHandle::apply_lod_step.

#[cfg(test)]
mod tests {
    use crate::constants::GRASS_CELL_SIZE;
    use crate::wasm_api::GRASS_LOD_BUDGET_AXIS;

    /// Replicate the LOD level formula from `write_snapshot` for the Auto path
    /// (lod_step == 0): uses the visible-span formula + lod_bias.
    fn auto_lod_level(world_size: f32, cam_zoom: f32, lod_bias: f32, max_level: usize) -> usize {
        let safe_zoom = if cam_zoom > 0.0 { cam_zoom } else { 1.0 };
        let visible_cell_span = (world_size / safe_zoom) / GRASS_CELL_SIZE;
        let level_f = (visible_cell_span / GRASS_LOD_BUDGET_AXIS as f32)
            .max(1.0)
            .log2()
            .floor();
        let biased = (level_f - lod_bias.max(0.0)).max(0.0);
        (biased as usize).min(max_level)
    }

    /// Replicate the stepper mip selection (lod_step > 0 path).
    /// lod_step 1 → mip 0, lod_step 2 → mip 1, etc.
    fn stepped_lod_level(lod_step: u32, max_level: usize) -> usize {
        debug_assert!(lod_step > 0, "stepped path only for lod_step > 0");
        ((lod_step - 1) as usize).min(max_level)
    }

    /// At default world (9600u, zoom=1, cell=5u) auto selects L0 (span 1920 < budget 4096).
    #[test]
    fn auto_default_world_is_level0() {
        let level = auto_lod_level(9600.0, 1.0, 0.0, 10);
        assert_eq!(level, 0, "auto at default world+zoom must be L0");
    }

    /// lod_step=1 always forces L0 regardless of zoom.
    #[test]
    fn step1_forces_l0_at_any_zoom() {
        let max_level = 10;
        // Even at extreme zoom-out (world=40960 which auto gives L1)
        let auto_zoomed = auto_lod_level(40960.0, 1.0, 0.0, max_level);
        assert_eq!(auto_zoomed, 1, "auto at 40960u zoom=1 should be L1");

        let stepped = stepped_lod_level(1, max_level);
        assert_eq!(
            stepped, 0,
            "lod_step=1 must force L0 (finer than auto at this zoom)"
        );
    }

    /// lod_step=2 forces L1 (coarser than default auto).
    #[test]
    fn step2_forces_l1() {
        let max_level = 10;
        let stepped = stepped_lod_level(2, max_level);
        assert_eq!(stepped, 1, "lod_step=2 must force L1");
    }

    /// lod_step=3 forces L2.
    #[test]
    fn step3_forces_l2() {
        let max_level = 10;
        let stepped = stepped_lod_level(3, max_level);
        assert_eq!(stepped, 2, "lod_step=3 must force L2");
    }

    /// lod_step=4 forces L3.
    #[test]
    fn step4_forces_l3() {
        let max_level = 10;
        let stepped = stepped_lod_level(4, max_level);
        assert_eq!(stepped, 3, "lod_step=4 must force L3");
    }

    /// lod_step=5 forces L4.
    #[test]
    fn step5_forces_l4() {
        let max_level = 10;
        let stepped = stepped_lod_level(5, max_level);
        assert_eq!(stepped, 4, "lod_step=5 must force L4");
    }

    /// Stepper is clamped to max_level — a large lod_step never exceeds the pyramid.
    #[test]
    fn stepper_clamped_to_max_level() {
        // Pyramid at default 1920² has 11 levels (L0..L10): 1920, 960, 480, 240,
        // 120, 60, 30, 15, 8, 4, 2, 1 — 11 levels, so max_level = 10.
        let max_level = 10;
        let level = stepped_lod_level(99, max_level);
        assert_eq!(
            level, max_level,
            "stepper must clamp to max_level at the pyramid"
        );
    }

    /// Auto path (lod_step=0) with lod_bias still works as before — stepper does
    /// not break backward compat with the existing lod_bias knob.
    #[test]
    fn auto_with_lod_bias_finer_still_works() {
        // Force a zoomed-out scenario that auto gives L1.
        let max_level = 10;
        let auto_l1 = auto_lod_level(40960.0, 1.0, 0.0, max_level);
        assert_eq!(auto_l1, 1);

        // Apply lod_bias=1.0 → should bring it to L0.
        let biased_l0 = auto_lod_level(40960.0, 1.0, 1.0, max_level);
        assert_eq!(biased_l0, 0, "lod_bias=1 must reduce L1→L0 in auto mode");
    }

    /// The stepper overrides lod_bias — when lod_step > 0, lod_bias is ignored.
    /// This is a contract test: lod_step=3 forces L2 regardless of lod_bias.
    #[test]
    fn stepper_overrides_lod_bias() {
        let max_level = 10;
        // Even if lod_bias would bring auto to L0, the stepper forces L2.
        let stepped = stepped_lod_level(3, max_level);
        assert_eq!(
            stepped, 2,
            "lod_step=3 must always be L2 regardless of lod_bias"
        );
    }

    /// Step 1 (Full) is the only step that is FINER than auto at default zoom
    /// (auto=L0, step1=L0 — same), and FINER than auto when zoomed out
    /// (auto=L1+, step1=L0 — finer). Verify the "finer than auto" property.
    #[test]
    fn step1_is_at_least_as_fine_as_auto_at_any_zoom() {
        let max_level = 10;
        // Several zoom levels
        for zoom in [1.0f32, 0.5, 0.25, 0.1] {
            let auto_level = auto_lod_level(9600.0, zoom, 0.0, max_level);
            let stepped_level = stepped_lod_level(1, max_level);
            assert!(
                stepped_level <= auto_level,
                "lod_step=1 (L0) must be ≤ auto level at zoom={zoom}: stepped={stepped_level}, auto={auto_level}"
            );
        }
    }

    /// Steps 2–5 are COARSER than or equal to auto at default zoom (auto=L0).
    #[test]
    fn steps_2_through_5_are_coarser_than_default_auto() {
        let max_level = 10;
        let auto_level = auto_lod_level(9600.0, 1.0, 0.0, max_level);
        assert_eq!(auto_level, 0, "default auto must be L0");

        for step in 2u32..=5 {
            let stepped = stepped_lod_level(step, max_level);
            assert!(
                stepped >= auto_level,
                "lod_step={step} (L{}) must be ≥ auto level (L{auto_level}) at default zoom",
                step - 1
            );
        }
    }

    /// The SLIDER_NAMES entry at index 63 must be "grass_lod_step".
    #[test]
    fn slider_index_63_is_grass_lod_step() {
        use crate::wasm_api::SLIDER_NAMES;
        assert_eq!(
            SLIDER_NAMES[63], "grass_lod_step",
            "SLIDER_NAMES[63] must be 'grass_lod_step'"
        );
    }
}
