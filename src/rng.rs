//! Deterministic RNG. xoshiro256++ seeded once per world.
//! v6 §J: string seeds are hashed with xxHash64.

use rand_xoshiro::rand_core::{RngCore, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::{Deserialize, Serialize};
use std::hash::Hasher;
use twox_hash::XxHash64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimRng(pub Xoshiro256PlusPlus);

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
}
