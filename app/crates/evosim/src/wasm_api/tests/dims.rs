//! v2.0 Wave 1a — computed snapshot/window dims tests.
//!
//! Pins the runtime slot geometry: the snapshot grass region (u8) and the
//! per-slot biome window region are both derived at boot from the runtime
//! `world_size`. Lives in its OWN file/module (wired from `wasm_api.rs` via
//! `#[path]`) to avoid the shared-`mod tests` merge hazard.

use super::super::WorldHandle;
use crate::constants::{WorldDims, WORLD_SIZE_DEFAULT};

/// world_size=9600 → fallback grass_dim=1920 → grass_cell_count=3_686_400.
#[test]
fn default_dims_9600() {
    let d = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
    assert_eq!(d.grass_dim, 1920);
    assert_eq!(d.grass_cell_count, 3_686_400);
}

/// world_size=1200 → fallback grass_dim=240.
#[test]
fn dims_1200() {
    let d = WorldDims::from_world_size(1200.0, false);
    assert_eq!(d.grass_dim, 240);
    assert_eq!(d.grass_cell_count, 240 * 240);
}

/// The snapshot grass region (u8) + the per-slot biome window region both
/// derive from the live grass-window allocation. Exercised against a real boot
/// at a small walled world so the buffers stay test-cheap.
/// v2.0.3 Stream 2d: slot_bytes now includes the biome window region (same size
/// as the grass region), so slot = header + creatures + grass + biome_win.
#[test]
fn snapshot_and_biome_window_sizes_follow_live_slot_layout() {
    // Small 1200u walled world → 240² = 57_600 grass cells.
    let h = WorldHandle::new_with_founder_count(
        "dims-seam",
        0,
        100.0,
        1,
        false,
        "",
        1200.0,
        false,
        1,
        false,
        1.0,
        10,
        10,
        3.0,
        // v2.0.5 S4: new grass_cell_size arg — default 5.0.
        5.0,
        // v2.0.4 S6: grass_multisight default ON.
        true,
        // v2.0.6 S3: clump_count=0 → old uniform-scatter.
        0,
        0,
        // Founder action-output boosts — neutral (1.0).
        1.0,
        1.0,
    )
    .unwrap();
    let cells = 240usize * 240;
    assert_eq!(h.grass_dim(), 240);
    // u8 grass: one byte per cell.
    assert_eq!(h.snapshot_grass_bytes() as usize, cells);
    // v2.0.3 Stream 2d: biome window allocation = same live window budget as grass.
    assert_eq!(h.snapshot_biome_win_bytes() as usize, cells);
    assert_eq!(h.snapshot_grass_bytes(), h.snapshot_biome_win_bytes());

    // Slot/buf totals follow from header + creatures + u8 grass + biome_win.
    // (Stream 2d: biome_win appended after grass — same allocation size.)
    let header = h.snapshot_header_bytes() as usize;
    let creatures = h.snapshot_creature_bytes() as usize;
    let biome_win = h.snapshot_biome_win_bytes() as usize;
    let slot = h.snapshot_slot_bytes() as usize;
    assert_eq!(slot, header + creatures + cells + biome_win);
    assert_eq!(h.snapshot_buf_byte_len() as usize, 2 * slot);
}

/// The default (9600u) boot derives the documented 480² grass dim end-to-end,
/// and the grass/biome-window slot regions are exactly `grass_cell_count` bytes.
#[test]
fn default_boot_reports_480_grass_dim() {
    let h = WorldHandle::new("dims-default");
    assert_eq!(h.grass_dim(), 480);
    assert_eq!(h.snapshot_grass_bytes() as usize, 480 * 480);
    assert_eq!(h.snapshot_biome_win_bytes() as usize, 480 * 480);
}
