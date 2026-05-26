//! Snapshot hash for the §16 acceptance test. xxHash64 over a canonical
//! ordering of world state per v6 §M. Stable across runs given identical
//! state — regression catch on the golden value.
//!
//! D3: hash_genome removed. Per-creature genome hash replaced with nothing
//! (genome deleted). Species anchor_genome sub-hash replaced with empty write.
//!
//! Hash input order (D3-updated):
//! 1. tick (u32)
//! 2. creature SoA — per-creature: id, position, velocity, energy, age,
//!    NN weight slice, nn_mutation_rate, digestion_cooldown, cumulative_upkeep,
//!    species_id, parent_species_id, last_action, action_this_tick,
//!    distance_travelled, birth_tick, move_bias_x, move_bias_y, move_bias_reroll_at
//! 3. grass map — per-cell density in linear cell-index order
//! 4. carrion list — per-corpse: x, y, pool, age, id
//! 5. species list — per-species: id, parent_id, name, born_tick, died_tick,
//!    child_count, depth, anchor_brain_weights + next_id
//! 6. RNG state — 4 u64s of xoshiro256++ internal state

use crate::constants::NN_WEIGHT_COUNT;
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
        // D3: no genome fields to hash
        // NN weight slice
        for &wf in w.creatures.brains[i].weights.iter() {
            write_f32(&mut h, wf);
        }
        write_f32(&mut h, w.creatures.brains[i].nn_mutation_rate);
        h.write_u32(w.creatures.digestion_cooldown[i]);
        write_f32(&mut h, w.creatures.cumulative_upkeep[i]);
        h.write_u32(w.creatures.species_id[i]);
        h.write_u32(w.creatures.parent_species_id[i]);
        h.write_u8(w.creatures.last_action[i] as u8);
        h.write_u8(w.creatures.action_this_tick[i] as u8);
        // D3: max_size_reached removed (was constant FOUNDER_SIZE)
        write_f32(&mut h, w.creatures.distance_travelled[i]);
        h.write_u32(w.creatures.birth_tick[i]);
        write_f32(&mut h, w.creatures.move_bias_x[i]);
        write_f32(&mut h, w.creatures.move_bias_y[i]);
        h.write_u32(w.creatures.move_bias_reroll_at[i]);
    }

    // (3) grass map — per-cell density in linear cell-index order
    let mg = w.grass.density.len();
    h.write_u32(mg as u32);
    for k in 0..mg {
        write_f32(&mut h, w.grass.density[k]);
    }

    // (4) carrion list — positional fingerprint
    let nc = w.carrion.len();
    h.write_u32(nc as u32);
    for cc in &w.carrion {
        write_f32(&mut h, cc.x);
        write_f32(&mut h, cc.y);
        write_f32(&mut h, cc.pool);
        h.write_u32(cc.age);
        h.write_u64(cc.id);
    }

    // (5) species list — D3: no anchor_genome sub-hash
    let ns = w.species.list.len();
    h.write_u32(ns as u32);
    for sp in &w.species.list {
        h.write_u32(sp.id);
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
        h.write_u32(sp.name.len() as u32);
        h.write(sp.name.as_bytes());
        h.write_u32(sp.born_tick);
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
        h.write_u32(sp.child_count);
        h.write_u32(sp.depth);
        debug_assert_eq!(
            sp.anchor_brain_weights.len(),
            NN_WEIGHT_COUNT,
            "anchor_brain_weights length must equal NN_WEIGHT_COUNT"
        );
        for &wf in &sp.anchor_brain_weights {
            write_f32(&mut h, wf);
        }
    }
    h.write_u32(w.species.next_id);

    // (6) RNG state — four u64s of xoshiro256++ internal state
    let state = w.rng.state();
    h.write_u64(state[0]);
    h.write_u64(state[1]);
    h.write_u64(state[2]);
    h.write_u64(state[3]);

    h.finish()
}

/// Hash a f32 with NaN canonicalization (S7 / C4).
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
