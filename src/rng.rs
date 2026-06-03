//! Deterministic RNG. xoshiro256++ seeded once per world.
//! v6 §J: string seeds are hashed with xxHash64.

use rand_xoshiro::rand_core::{RngCore, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::{Deserialize, Serialize};
use std::hash::Hasher;
use twox_hash::XxHash64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimRng(pub Xoshiro256PlusPlus);

/// v2.0.2 Stream 1b: stateless, position-addressable hash RNG for the grass
/// scatter kernel. **This is deliberately NOT `SimRng`** — the scatter rolls run
/// inside a rayon `par_iter` over active tiles, so they must not draw from the
/// shared sequential `SimRng` (that would need a lock and make the draw order
/// thread-dependent, desyncing the *creature* stream and breaking global
/// determinism). Instead a cell's draws are a pure hash of
/// `(world_seed, cell_or_tile_id, tick, salt)` → order-independent, lock-free,
/// and it never touches `SimRng`.
///
/// `salt` distinguishes independent draws for the same cell+tick (e.g. the decay
/// roll vs the spread roll vs the spread-target pick) so they are uncorrelated.
///
/// The mixer is SplitMix64's finalizer applied to a folded key. It is fast
/// (a handful of xor/mul/shift) and has good avalanche — sufficient for visual
/// "living noise"; cryptographic quality is not required. Determinism scope: the
/// *draw* for a fixed key is reproducible (so grass never perturbs `SimRng`); the
/// *field* is not reproducible across thread counts (lossy relaxed writes), and
/// is not required to be (see the v2.0.2 plan).
#[inline]
pub fn grass_hash_u64(world_seed: u32, id: u64, tick: u32, salt: u32) -> u64 {
    // Fold the four key parts into one 64-bit word with odd-constant multiplies
    // (each constant is a large odd number → bijective mod 2^64, so distinct keys
    // stay distinct before the avalanche finalizer).
    let mut z = id
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((world_seed as u64).wrapping_mul(0xD1B5_4A32_D192_ED03))
        .wrapping_add((tick as u64).wrapping_mul(0xCA5A_826F_5C26_2A4D))
        .wrapping_add((salt as u64).wrapping_mul(0x2545_F491_4F6C_DD1D));
    // SplitMix64 finalizer (good avalanche).
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// v2.0.2 Stream 1b: a hashed `u64` mapped to `[0, 1)` (top 24 bits, matching
/// [`SimRng::unit`]'s precision). Pure function of the inputs.
#[inline]
pub fn grass_unit(world_seed: u32, id: u64, tick: u32, salt: u32) -> f32 {
    let bits = (grass_hash_u64(world_seed, id, tick, salt) >> 40) as u32;
    (bits as f32) / ((1u32 << 24) as f32)
}

impl SimRng {
    pub fn from_u64(seed: u64) -> Self {
        Self(Xoshiro256PlusPlus::seed_from_u64(seed))
    }

    pub fn from_string(s: &str) -> Self {
        let mut h = XxHash64::with_seed(0);
        h.write(s.as_bytes());
        Self::from_u64(h.finish())
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn unit(&mut self) -> f32 {
        // top 24 bits, scaled to [0,1)
        let bits = (self.0.next_u64() >> 40) as u32;
        (bits as f32) / ((1u32 << 24) as f32)
    }

    /// Uniform in [-1, 1).
    #[inline]
    pub fn symm(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// Uniform in [lo, hi).
    #[inline]
    pub fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// Standard-normal via polar Box–Muller. Returns one sample; the second
    /// is discarded (callers wanting both samples can use [`normal_pair`]).
    #[inline]
    pub fn normal(&mut self) -> f32 {
        self.normal_pair().0
    }

    pub fn normal_pair(&mut self) -> (f32, f32) {
        loop {
            let u = self.symm();
            let v = self.symm();
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let f = (-2.0 * s.ln() / s).sqrt();
                return (u * f, v * f);
            }
        }
    }

    /// Geometric-skip sample: number of "no-mutate" trials before the next
    /// mutation index. Equivalent to `Geom(p)`. v6 §E mutation sampling.
    /// Returns `usize::MAX` if `p <= 0`. For `p >= 1` always returns 0.
    pub fn geom_skip(&mut self, p: f32) -> usize {
        if p <= 0.0 {
            return usize::MAX;
        }
        if p >= 1.0 {
            return 0;
        }
        let u = self.unit().max(f32::MIN_POSITIVE);
        ((u.ln() / (1.0 - p).ln()).floor()) as usize
    }

    /// Pick one of `n` uniformly.
    #[inline]
    pub fn index(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        (((self.next_u64() as u128) * (n as u128)) >> 64) as usize
    }

    /// Return the four `u64`s of the xoshiro256++ internal state, in storage
    /// order (`s[0]..s[3]`). Test-only (not shipped to wasm production bundle).
    ///
    /// The field `Xoshiro256PlusPlus::s` is private upstream; this accessor
    /// extracts it via `serde_json::to_value` as a one-shot field extractor,
    /// then parses the four `u64` values.
    ///
    /// Stable: hinges on `rand_xoshiro 0.6`'s `Serialize` derive emitting
    /// the four state words under the field key `"s"`. The version is pinned
    /// to `0.6` in `Cargo.toml`. If `rand_xoshiro` ever bumps and renames the
    /// field, the `rng_state_round_trip_matches_serde` unit test will catch it.
    #[cfg(test)]
    pub(crate) fn state(&self) -> [u64; 4] {
        let v =
            serde_json::to_value(&self.0).expect("Xoshiro256PlusPlus serialization is infallible");
        let arr = v["s"]
            .as_array()
            .expect("rand_xoshiro Serialize derive must emit field 's' as array");
        debug_assert_eq!(arr.len(), 4, "xoshiro256++ has exactly 4 state words");
        [
            arr[0].as_u64().expect("state word 0 must be u64"),
            arr[1].as_u64().expect("state word 1 must be u64"),
            arr[2].as_u64().expect("state word 2 must be u64"),
            arr[3].as_u64().expect("state word 3 must be u64"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = SimRng::from_string("evosim");
        let mut b = SimRng::from_string("evosim");
        for _ in 0..1024 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn unit_in_range() {
        let mut r = SimRng::from_u64(42);
        for _ in 0..100_000 {
            let x = r.unit();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn normal_finite() {
        let mut r = SimRng::from_u64(7);
        let mut sum = 0.0_f32;
        let mut sum_sq = 0.0_f32;
        let n = 100_000;
        for _ in 0..n {
            let x = r.normal();
            assert!(x.is_finite());
            sum += x;
            sum_sq += x * x;
        }
        let mean = sum / n as f32;
        let var = sum_sq / n as f32 - mean * mean;
        // generous bounds; just confirming sane distribution
        assert!(mean.abs() < 0.05, "mean = {mean}");
        assert!((var - 1.0).abs() < 0.05, "var = {var}");
    }

    #[test]
    fn geom_skip_edges() {
        let mut r = SimRng::from_u64(1);
        assert_eq!(r.geom_skip(0.0), usize::MAX);
        assert_eq!(r.geom_skip(1.0), 0);
    }

    /// S8: state() returns four populated u64s (xoshiro256++ never has all-zero
    /// state post-construction; the seed_from_u64 path uses a splitmix step).
    #[test]
    fn rng_state_is_4_u64() {
        let r = SimRng::from_u64(7);
        let s = r.state();
        assert_eq!(s.len(), 4, "state must have exactly 4 words");
        assert_ne!(
            s, [0u64; 4],
            "xoshiro256++ state must not be all-zero after seed"
        );
        // Calling state() twice without advancing must return identical arrays.
        let s2 = r.state();
        assert_eq!(s, s2, "state() must be idempotent (no RNG advance)");
    }

    /// S8: advancing the RNG via next_u64 must change the state.
    #[test]
    fn rng_state_changes_after_next_u64() {
        let mut r = SimRng::from_u64(42);
        let before = r.state();
        r.next_u64();
        let after = r.state();
        assert_ne!(before, after, "state must change after advancing the RNG");
    }

    /// S8: state() round-trips against serde — the extractor returns the same
    /// four words that the Serialize derive would emit under "s". Protects
    /// against rand_xoshiro silently renaming the field.
    #[test]
    fn rng_state_round_trip_matches_serde() {
        let r = SimRng::from_u64(99);
        let state = r.state();
        // Parse via a separate serde_json round-trip (same code path as accessor,
        // but done explicitly so we verify the extractor logic is consistent).
        let v = serde_json::to_value(&r.0).expect("serialize");
        let arr = v["s"].as_array().expect("field 's' must exist");
        let serde_words: [u64; 4] = [
            arr[0].as_u64().unwrap(),
            arr[1].as_u64().unwrap(),
            arr[2].as_u64().unwrap(),
            arr[3].as_u64().unwrap(),
        ];
        assert_eq!(
            state, serde_words,
            "state() must match serde-extracted words"
        );
    }
}
