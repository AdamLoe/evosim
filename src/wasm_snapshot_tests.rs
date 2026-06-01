//! v2.0 Wave 2a: snapshot-repack layout tests. Asserts the new render-only lane
//! offsets (x, y, radius, color_u32, id_lo, id_hi, packed_u32, pad), that the
//! creature region byte size + stride are UNCHANGED, and that the `color_u32` /
//! `packed_u32` bit packings round-trip. Attached via `#[path]` in `wasm_api.rs`.

use super::*;
use crate::creature::{FlashTag, Genome, FLASH_TICKS};

/// The creature region stride + total bytes are unchanged by the repack (only
/// the *meaning* of the lanes shifted).
#[test]
fn creature_region_size_unchanged() {
    assert_eq!(SNAPSHOT_CREATURE_STRIDE, 32, "stride must stay 32 B / 8 lanes");
    assert_eq!(
        SNAPSHOT_CREATURE_BYTES,
        MAX_POP_FOR_SIM * 32,
        "creature region size must stay MAX_POP_FOR_SIM × 32"
    );
    assert_eq!(SNAPSHOT_HEADER_BYTES, 32, "header bytes unchanged");
}

/// New lane offsets: x@0, y@4, radius@8, color_u32@12, id_lo@16, id_hi@20,
/// packed_u32@24, pad@28. We read them out of a freshly written native snapshot.
#[test]
fn snapshot_lane_offsets_match_repack() {
    let mut handle =
        WorldHandle::new_with_founder_count("wave2a-lanes", 0, 100.0, 4, false, "", 1200.0, false, 1)
            .unwrap();
    let mut creatures = Vec::new();
    let mut grass = Vec::new();
    let mut stats = Vec::new();
    let pop = handle.write_snapshot_to_native(&mut creatures, &mut grass, &mut stats);
    assert_eq!(pop, 4);
    assert_eq!(creatures.len(), 4 * 32);

    for i in 0..pop {
        let base = i * 32;
        // x@0, y@4.
        let x = f32::from_le_bytes(creatures[base..base + 4].try_into().unwrap());
        let y = f32::from_le_bytes(creatures[base + 4..base + 8].try_into().unwrap());
        assert_eq!(x, handle.inner.creatures.x[i], "x lane @0");
        assert_eq!(y, handle.inner.creatures.y[i], "y lane @4");
        // radius@8 = body_size-derived.
        let radius = f32::from_le_bytes(creatures[base + 8..base + 12].try_into().unwrap());
        let expected_r = CREATURE_SIZE
            * BODY_RADIUS_PER_SIZE
            * handle.inner.creatures.genome[i].body_size_factor();
        assert!((radius - expected_r).abs() < 1e-5, "radius lane @8");
        // color_u32@12 — alpha byte is 255.
        let color = u32::from_le_bytes(creatures[base + 12..base + 16].try_into().unwrap());
        assert_eq!((color >> 24) & 0xFF, 0xFF, "color_u32 @12 alpha");
        // id halves @16/@20.
        let id_lo = u32::from_le_bytes(creatures[base + 16..base + 20].try_into().unwrap());
        let id_hi = u32::from_le_bytes(creatures[base + 20..base + 24].try_into().unwrap());
        let id = ((id_hi as u64) << 32) | id_lo as u64;
        assert_eq!(id, handle.inner.creatures.id[i], "id lanes @16/@20");
        // packed_u32@24.
        let packed = u32::from_le_bytes(creatures[base + 24..base + 28].try_into().unwrap());
        assert_eq!(packed & 0x7, handle.inner.creatures.flash_tag[i] as u32);
        assert_eq!((packed >> 3) & 0xF, handle.inner.creatures.flash_ticks[i] as u32);
        assert_eq!((packed >> 7) & 0xFFFF, 0, "species_id reserved");
        // pad@28 is zero.
        let pad = u32::from_le_bytes(creatures[base + 28..base + 32].try_into().unwrap());
        assert_eq!(pad, 0, "pad lane @28 must be zero");
    }
}

/// `pack_render_u32` round-trips flash_tag + flash_ticks + species_id within
/// their bit widths.
#[test]
fn pack_render_u32_roundtrips() {
    for &tag in &[
        FlashTag::None,
        FlashTag::Born,
        FlashTag::Grazed,
        FlashTag::Attacked,
        FlashTag::CreatedChild,
        FlashTag::Killed,
    ] {
        for ticks in 0..=FLASH_TICKS {
            // species_id 0 in single-pool, but exercise the reserved bits too.
            for &sp in &[0u16, 1, 1234, 0xFFFF] {
                let packed = pack_render_u32(tag as u8, ticks, sp);
                assert_eq!(packed & 0x7, tag as u32, "flash_tag bits");
                assert_eq!((packed >> 3) & 0xF, ticks as u32, "flash_ticks bits");
                assert_eq!((packed >> 7) & 0xFFFF, sp as u32, "species_id bits");
            }
        }
    }
}

/// `genome_color_u32`: a pure grazer (diet=0) skews green; a pure predator
/// (diet=1) skews red. Alpha is always 255.
#[test]
fn genome_color_grazer_green_predator_red() {
    let grazer = Genome {
        diet: 0.0,
        body_size: 1.0,
        max_speed: 1.0,
        ..Genome::median()
    };
    let predator = Genome {
        diet: 1.0,
        body_size: 1.0,
        max_speed: 1.0,
        ..Genome::median()
    };
    let gc = genome_color_u32(&grazer);
    let pc = genome_color_u32(&predator);
    let (gr, gg) = (gc & 0xFF, (gc >> 8) & 0xFF);
    let (pr, pg) = (pc & 0xFF, (pc >> 8) & 0xFF);
    assert!(gg > gr, "grazer must be greener than red: g={gg} r={gr}");
    assert!(pr > pg, "predator must be redder than green: r={pr} g={pg}");
    assert_eq!((gc >> 24) & 0xFF, 0xFF, "alpha 255");
    assert_eq!((pc >> 24) & 0xFF, 0xFF, "alpha 255");
}
