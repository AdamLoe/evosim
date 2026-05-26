//! Snapshot hash for the §16 acceptance test. xxHash64 over a canonical
//! ordering of world state per v6 §M. Stable across runs given identical
//! state — regression catch on the golden value.
//!
//! Hash input order (v6 §M, extended by S7+S8 audit; updated P1b+P1f):
//! 1. tick (u32)
//! 2. creature SoA — per-creature: id, position, velocity, energy, age,
//!    genome floats in struct-declaration order, NN weight slice,
//!    nn_mutation_rate, digestion_cooldown, cumulative_upkeep, species_id,
//!    parent_species_id, last_action, action_this_tick, max_size_reached,
//!    distance_travelled, birth_tick
//! 3. grass map — per-cell density in linear cell-index order (replaces sun map)
//! 4. carrion list — per-corpse: x, y, pool, age, id
//! 5. species list — per-species: id, anchor-genome sub-hash, parent_id,
//!    name, born_tick, died_tick, child_count, depth, anchor_brain_weights
//!    + top-level registry next_id
//! 6. RNG state — 4 u64s of xoshiro256++ internal state, native-endian
//!    (LE on both real targets: x86_64-linux, wasm32-unknown) via write_u64.
//!    Replaces serde_json byte stream (v1.0 approach; see DECISIONS audit v1.1).

use crate::constants::NN_WEIGHT_COUNT;
use crate::genome::Genome;
use crate::world::World;
use std::hash::Hasher;
use twox_hash::XxHash64;

/// Compute the xxHash64 snapshot hash of the given world state.
pub fn snapshot_hash(w: &World) -> u64 {
    let mut h = XxHash64::with_seed(0);

    // (1) tick
    h.write_u32(w.tick);

    // (2) creature SoA — per-creature, in current SoA index order
    let n = w.creatures.len();
    h.write_u32(n as u32);
    for i in 0..n {
        h.write_u64(w.creatures.id[i]);
        write_f32(&mut h, w.creatures.x[i]);
        write_f32(&mut h, w.creatures.y[i]);
        write_f32(&mut h, w.creatures.vx[i]);
        write_f32(&mut h, w.creatures.vy[i]);
        write_f32(&mut h, w.creatures.energy[i]);
        h.write_u32(w.creatures.age[i]);
        hash_genome(&mut h, &w.creatures.genomes[i]);
        // NN weight slice
        for &wf in w.creatures.brains[i].weights.iter() {
            write_f32(&mut h, wf);
        }
        write_f32(&mut h, w.creatures.brains[i].nn_mutation_rate);
        // S7: 9 additional SoA fields appended after nn_mutation_rate (#1–#9)
        h.write_u32(w.creatures.digestion_cooldown[i]); // #1
        write_f32(&mut h, w.creatures.cumulative_upkeep[i]); // #2
        h.write_u32(w.creatures.species_id[i]); // #3
        h.write_u32(w.creatures.parent_species_id[i]); // #4
        h.write_u8(w.creatures.last_action[i] as u8); // #5
        h.write_u8(w.creatures.action_this_tick[i] as u8); // #6
        write_f32(&mut h, w.creatures.max_size_reached[i]); // #7
        write_f32(&mut h, w.creatures.distance_travelled[i]); // #8
        h.write_u32(w.creatures.birth_tick[i]); // #9
    }

    // (3) grass map — per-cell density in linear cell-index order
    let mg = w.grass.density.len();
    h.write_u32(mg as u32);
    for k in 0..mg {
        write_f32(&mut h, w.grass.density[k]);
    }

    // (4) carrion list — positional fingerprint + S7 id
    let nc = w.carrion.len();
    h.write_u32(nc as u32);
    for cc in &w.carrion {
        write_f32(&mut h, cc.x);
        write_f32(&mut h, cc.y);
        write_f32(&mut h, cc.pool);
        h.write_u32(cc.age);
        h.write_u64(cc.id);
    }

    // (5) species list — id + anchor genome sub-hash + S7 fields (#12–#17) + next_id (#18)
    let ns = w.species.list.len();
    h.write_u32(ns as u32);
    for sp in &w.species.list {
        h.write_u32(sp.id);
        // Sub-hash the anchor genome to keep the per-species loop body compact.
        let mut ah = XxHash64::with_seed(0);
        hash_genome(&mut ah, &sp.anchor_genome);
        h.write_u64(ah.finish());
        // S7: species fields appended after anchor sub-hash (#12–#17)
        // #12: parent_id — Option<u32> encoded as (presence u8, value u32)
        match sp.parent_id {
            None => {
                h.write_u8(0);
                h.write_u32(0);
            }
            Some(v) => {
                h.write_u8(1);
                h.write_u32(v);
            }
        }
        // #13: name — length-prefixed UTF-8 bytes (prevents boundary collision)
        h.write_u32(sp.name.len() as u32);
        h.write(sp.name.as_bytes());
        // #13.5 / 14 ordering per plan: born_tick → died_tick → child_count → depth → anchor_brain_weights
        h.write_u32(sp.born_tick); // #13.5
                                   // #14: died_tick — Option<u32> same encoding as parent_id
        match sp.died_tick {
            None => {
                h.write_u8(0);
                h.write_u32(0);
            }
            Some(v) => {
                h.write_u8(1);
                h.write_u32(v);
            }
        }
        h.write_u32(sp.child_count); // #15
        h.write_u32(sp.depth); // #16
                               // #17: anchor_brain_weights — length is NN_WEIGHT_COUNT (invariant)
        debug_assert_eq!(
            sp.anchor_brain_weights.len(),
            NN_WEIGHT_COUNT,
            "anchor_brain_weights length must equal NN_WEIGHT_COUNT"
        );
        for &wf in &sp.anchor_brain_weights {
            write_f32(&mut h, wf); // #17
        }
    }
    // #18: SpeciesRegistry.next_id — appended after the species loop
    h.write_u32(w.species.next_id);

    // (6) RNG state — four u64s of xoshiro256++ internal state, in storage order.
    // Written via Hasher::write_u64 (native-endian → LE on both real targets).
    // Replaces serde_json byte stream: hash is now independent of serde_json
    // formatting policy (see DECISIONS audit v1.1 and audit-s8-rng-hash-direct.md).
    let state = w.rng.state();
    h.write_u64(state[0]);
    h.write_u64(state[1]);
    h.write_u64(state[2]);
    h.write_u64(state[3]);

    h.finish()
}

/// Hash all genome fields in struct-declaration order (v6 §M "struct order").
/// Shared between the creature loop and the species anchor sub-hash.
#[inline]
fn hash_genome(h: &mut XxHash64, g: &Genome) {
    write_f32(h, g.size);
    h.write_u32(g.max_age);
    write_f32(h, g.graze_efficiency);
    write_f32(h, g.eat_efficiency);
    write_f32(h, g.scavenge_efficiency);
    write_f32(h, g.move_speed);
    h.write_u8(g.eye_count);
    for k in 0..g.eye_offsets.len() {
        write_f32(h, g.eye_offsets[k]);
    }
    h.write_u8(g.nose_count);
    write_f32(h, g.vision_range);
    write_f32(h, g.armor);
    write_f32(h, g.bite_reach);
    write_f32(h, g.pigment_r);
    write_f32(h, g.pigment_g);
    write_f32(h, g.pigment_b);
    // TraitMutationRates — in struct-declaration order.
    let mr = &g.mutation_rates;
    write_f32(h, mr.size);
    write_f32(h, mr.max_age);
    write_f32(h, mr.graze_efficiency);
    write_f32(h, mr.eat_efficiency);
    write_f32(h, mr.scavenge_efficiency);
    write_f32(h, mr.move_speed);
    write_f32(h, mr.eye_count);
    write_f32(h, mr.eye_offsets);
    write_f32(h, mr.nose_count);
    write_f32(h, mr.vision_range);
    write_f32(h, mr.armor);
    write_f32(h, mr.bite_reach);
    write_f32(h, mr.pigment_r);
    write_f32(h, mr.pigment_g);
    write_f32(h, mr.pigment_b);
}

/// Hash a f32 with NaN canonicalization (S7 / C4).
///
/// All NaN bit-patterns hash to the canonical quiet-NaN `0x7fc0_0000`.
/// Finite values (including ±0.0, ±Inf, denormals) pass through unchanged.
#[inline]
fn write_f32(h: &mut XxHash64, v: f32) {
    let bits = if v.is_nan() {
        0x7fc0_0000_u32
    } else {
        v.to_bits()
    };
    h.write_u32(bits);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    /// Hash determinism: same world produces the same hash twice in a row.
    #[test]
    fn snapshot_hash_is_deterministic() {
        let mut w = World::new("hash-determinism");
        for _ in 0..200 {
            w.tick_once();
        }
        let h1 = snapshot_hash(&w);
        let h2 = snapshot_hash(&w);
        assert_eq!(
            h1, h2,
            "hash must be identical across two calls on the same state"
        );
    }

    /// Two worlds from the same seed produce the same hash at the same tick.
    #[test]
    fn snapshot_hash_same_seed_same_hash() {
        let mut w1 = World::new("hash-same-seed");
        let mut w2 = World::new("hash-same-seed");
        for _ in 0..100 {
            w1.tick_once();
            w2.tick_once();
        }
        assert_eq!(
            snapshot_hash(&w1),
            snapshot_hash(&w2),
            "same seed and tick must produce same hash"
        );
    }

    /// S8: advancing the RNG via tick_once changes the snapshot hash.
    #[test]
    fn rng_hash_changes_after_step() {
        let mut w = World::new("rng-hash-changes");
        let h_before = snapshot_hash(&w);
        w.tick_once();
        let h_after = snapshot_hash(&w);
        assert_ne!(
            h_before, h_after,
            "snapshot hash must reflect RNG state after a step"
        );
    }

    /// S7: write_f32 NaN canonicalization — all NaN patterns hash the same.
    #[test]
    fn write_f32_nan_canonical() {
        let mut h1 = XxHash64::with_seed(0);
        let mut h2 = XxHash64::with_seed(0);
        write_f32(&mut h1, f32::NAN);
        write_f32(&mut h2, f32::from_bits(0x7fff_ffff)); // a different NaN payload
        assert_eq!(
            h1.finish(),
            h2.finish(),
            "all NaN patterns must hash identically (canonical 0x7fc0_0000)"
        );
    }

    /// P2c: nose_count byte appears in snapshot hash — mutating it changes the hash.
    #[test]
    fn nose_count_appears_in_hash() {
        use crate::world::World;
        let mut w1 = World::new("nose-hash-base");
        let mut w2 = World::new("nose-hash-base");
        // Advance both worlds identically to the same state.
        for _ in 0..50 {
            w1.tick_once();
            w2.tick_once();
        }
        // Mutate nose_count on creature 0 in w2 only.
        if !w2.creatures.is_empty() {
            let prev = w2.creatures.genomes[0].nose_count;
            w2.creatures.genomes[0].nose_count = if prev == 5 { 4 } else { prev + 1 };
            w2.creatures.resync_hot_mirrors_at(0);
        }
        assert_ne!(
            snapshot_hash(&w1),
            snapshot_hash(&w2),
            "changing nose_count must alter the snapshot hash"
        );
    }

    /// S7: write_f32 does not canonicalize finite values or ±Inf.
    #[test]
    fn write_f32_finite_values_pass_through() {
        let mut h_pi = XxHash64::with_seed(0);
        let mut h_e = XxHash64::with_seed(0);
        write_f32(&mut h_pi, std::f32::consts::PI);
        write_f32(&mut h_e, std::f32::consts::E);
        assert_ne!(
            h_pi.finish(),
            h_e.finish(),
            "distinct finite values must hash differently"
        );
    }
}
