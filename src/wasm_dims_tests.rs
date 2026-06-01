//! v2.0 Wave 1a — computed snapshot/biome dims tests.
//!
//! Pins the "computed-dims-equality" safety model: the snapshot grass region
//! (u8) and the dedicated biomeSab are both `grass_cell_count` BYTES, derived at
//! boot from the runtime `world_size`. Lives in its OWN file/module (wired from
//! `wasm_api.rs` via `#[path]`) to avoid the shared-`mod tests` merge hazard.

use crate::constants::{WorldDims, WORLD_SIZE_DEFAULT};
use super::WorldHandle;

/// world_size=9600 → grass_dim=1920 → grass_cell_count=3_686_400.
#[test]
fn default_dims_9600() {
    let d = WorldDims::from_world_size(WORLD_SIZE_DEFAULT, true);
    assert_eq!(d.grass_dim, 1920);
    assert_eq!(d.grass_cell_count, 3_686_400);
}

/// world_size=1200 → grass_dim=240.
#[test]
fn dims_1200() {
    let d = WorldDims::from_world_size(1200.0, false);
    assert_eq!(d.grass_dim, 240);
    assert_eq!(d.grass_cell_count, 240 * 240);
}

/// The snapshot grass region (u8) + the biomeSab byte sizes both derive from
/// `grass_cell_count` and agree (computed-dims-equality model). Exercised
/// against a real boot at a small walled world so the buffers stay test-cheap.
#[test]
fn snapshot_and_biome_sizes_derive_from_grass_cell_count() {
    // Small 1200u walled world → 240² = 57_600 grass cells.
    let h = WorldHandle::new_with_founder_count(
        "dims-seam", 0, 100.0, 1, false, "", 1200.0, false, 1,
    )
    .unwrap();
    let cells = 240usize * 240;
    assert_eq!(h.grass_dim(), 240);
    // u8 grass: one byte per cell.
    assert_eq!(h.snapshot_grass_bytes() as usize, cells);
    // biomeSab: one u8 per cell.
    assert_eq!(h.biome_buf_byte_len() as usize, cells);
    // The two derived sizes must be equal (same source dim).
    assert_eq!(h.snapshot_grass_bytes(), h.biome_buf_byte_len());

    // Slot/buf totals follow from header + creatures + u8 grass.
    let header = h.snapshot_header_bytes() as usize;
    let creatures = h.snapshot_creature_bytes() as usize;
    let slot = h.snapshot_slot_bytes() as usize;
    assert_eq!(slot, header + creatures + cells);
    assert_eq!(h.snapshot_buf_byte_len() as usize, 2 * slot);
}

/// The default (9600u) boot derives the documented 1920² grass dim end-to-end,
/// and the snapshot grass region is exactly `grass_cell_count` u8 bytes.
#[test]
fn default_boot_reports_1920_grass_dim() {
    let h = WorldHandle::new("dims-default");
    assert_eq!(h.grass_dim(), 1920);
    assert_eq!(h.snapshot_grass_bytes() as usize, 1920 * 1920);
    assert_eq!(h.biome_buf_byte_len() as usize, 1920 * 1920);
}
