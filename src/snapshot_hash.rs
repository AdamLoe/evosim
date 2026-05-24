//! Snapshot hash for the §16 acceptance test. xxHash64 over a canonical
//! ordering of world state per v6 §M. Stable across runs given identical
//! state — regression catch on the golden value.
//!
//! Hash input order (v6 §M):
//! 1. tick (u32)
//! 2. creature SoA — per-creature: id, position, velocity, energy, age,
//!    genome floats in struct-declaration order, NN weight slice
//! 3. sun map — per-cell: current, capacity in row-major order
//! 4. carrion list — per-corpse: x, y, pool, age (no stable id in v1;
//!    positional fingerprint is sufficient — see F.29 DECISIONS)
//! 5. species list — per-species: id + anchor-genome sub-hash
//! 6. RNG state — serde_json bytes of SimRng (same serialization as F.26)

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
    }

    // (3) sun map — per-cell, row-major
    let m = w.sun.capacity.len();
    h.write_u32(m as u32);
    for k in 0..m {
        write_f32(&mut h, w.sun.current[k]);
        write_f32(&mut h, w.sun.capacity[k]);
    }

    // (4) carrion list — positional fingerprint (no stable id in v1)
    let nc = w.carrion.len();
    h.write_u32(nc as u32);
    for cc in &w.carrion {
        write_f32(&mut h, cc.x);
        write_f32(&mut h, cc.y);
        write_f32(&mut h, cc.pool);
        h.write_u32(cc.age);
    }

    // (5) species list — id + anchor genome sub-hash
    let ns = w.species.list.len();
    h.write_u32(ns as u32);
    for sp in &w.species.list {
        h.write_u32(sp.id);
        // Sub-hash the anchor genome to keep the per-species loop body compact.
        let mut ah = XxHash64::with_seed(0);
        hash_genome(&mut ah, &sp.anchor_genome);
        h.write_u64(ah.finish());
    }

    // (6) RNG state — serde_json bytes of SimRng (uses the serde1 feature from F.26).
    let rng_bytes = serde_json::to_vec(&w.rng).expect("SimRng serialization is infallible");
    h.write(&rng_bytes);

    h.finish()
}

/// Hash all genome fields in struct-declaration order (v6 §M "struct order").
/// Shared between the creature loop and the species anchor sub-hash.
fn hash_genome(h: &mut XxHash64, g: &Genome) {
    write_f32(h, g.size);
    h.write_u32(g.max_age);
    write_f32(h, g.photosynth_efficiency);
    write_f32(h, g.eat_efficiency);
    write_f32(h, g.scavenge_efficiency);
    write_f32(h, g.move_speed);
    h.write_u8(g.eye_count);
    for k in 0..g.eye_offsets.len() {
        write_f32(h, g.eye_offsets[k]);
    }
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
    write_f32(h, mr.photosynth_efficiency);
    write_f32(h, mr.eat_efficiency);
    write_f32(h, mr.scavenge_efficiency);
    write_f32(h, mr.move_speed);
    write_f32(h, mr.eye_count);
    write_f32(h, mr.eye_offsets);
    write_f32(h, mr.vision_range);
    write_f32(h, mr.armor);
    write_f32(h, mr.bite_reach);
    write_f32(h, mr.pigment_r);
    write_f32(h, mr.pigment_g);
    write_f32(h, mr.pigment_b);
}

#[inline]
fn write_f32(h: &mut XxHash64, v: f32) {
    h.write_u32(v.to_bits());
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
}
