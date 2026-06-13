//! Deterministic science-mode fixtures.

use super::*;

fn hash_bytes(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01B3);
    }
    h
}

fn hash_u32(h: u64, v: u32) -> u64 {
    hash_bytes(h, &v.to_le_bytes())
}

fn hash_u64(h: u64, v: u64) -> u64 {
    hash_bytes(h, &v.to_le_bytes())
}

fn hash_f32(h: u64, v: f32) -> u64 {
    hash_u32(h, v.to_bits())
}

fn world_state_hash(w: &World) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    h = hash_u32(h, w.tick);
    h = hash_u32(h, w.population());
    h = hash_bytes(
        h,
        &[
            w.world_ended as u8,
            w.sliders.deterministic_science_mode as u8,
        ],
    );
    h = hash_bytes(h, &w.grass.density_u8_snapshot());
    for i in 0..w.creatures.len() {
        h = hash_u64(h, w.creatures.id[i]);
        h = hash_f32(h, w.creatures.x[i]);
        h = hash_f32(h, w.creatures.y[i]);
        h = hash_f32(h, w.creatures.vx[i]);
        h = hash_f32(h, w.creatures.vy[i]);
        h = hash_f32(h, w.creatures.energy[i]);
        h = hash_u32(h, w.creatures.age[i]);
        h = hash_u32(h, w.creatures.birth_tick[i]);
        h = hash_u32(h, w.creatures.species_id[i] as u32);
    }
    h
}

fn science_fixture_hash() -> u64 {
    let sliders = DevSliders {
        deterministic_science_mode: true,
        world_size: 640.0,
        wrap_world: true,
        world_seed: 0x51C1_ECE5,
        grass_cell_size: 10.0,
        grass_initial_seed_count: 0,
        grass_clump_count: 6,
        grass_clump_size: 4,
        founder_count: 24,
        max_population: 256,
        energy_max: 120.0,
        ..Default::default()
    };
    let mut world = World::new_with_sliders("science-fixture", sliders);
    for _ in 0..80 {
        world.tick_once();
    }
    world_state_hash(&world)
}

#[test]
fn deterministic_science_mode_is_default_off() {
    let sliders = DevSliders::default();
    assert!(!sliders.deterministic_science_mode);
    let world = World::new_with_sliders("science-default-off", sliders);
    assert!(!world.grass.deterministic_science_mode);
}

#[test]
fn science_mode_fixture_hash_is_exact_and_stable() {
    const EXPECTED: u64 = 0xda58_6f7d_1ea0_1dbc;
    let a = science_fixture_hash();
    let b = science_fixture_hash();
    assert_eq!(a, b, "same seed/config must produce exact repeated hash");
    assert_eq!(
        a, EXPECTED,
        "science fixture hash must match across default and --features threads"
    );
}
