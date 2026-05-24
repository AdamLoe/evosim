//! Body genome and per-trait mutation rates.
//!
//! Milestone B: only the founder defaults are used; mutation is stubbed
//! (no-op copy) until Milestone C lands. The struct layout is final.

use crate::constants::*;
use crate::rng::SimRng;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraitMutationRates {
    pub size: f32,
    pub max_age: f32,
    pub photosynth_efficiency: f32,
    pub eat_efficiency: f32,
    pub scavenge_efficiency: f32,
    pub move_speed: f32,
    pub eye_count: f32,
    pub eye_offsets: f32, // applied per-slot
    pub vision_range: f32,
    pub armor: f32,
    pub bite_reach: f32,
    pub pigment_r: f32,
    pub pigment_g: f32,
    pub pigment_b: f32,
}

impl TraitMutationRates {
    pub const fn uniform(rate: f32) -> Self {
        Self {
            size: rate,
            max_age: rate,
            photosynth_efficiency: rate,
            eat_efficiency: rate,
            scavenge_efficiency: rate,
            move_speed: rate,
            eye_count: rate,
            eye_offsets: rate,
            vision_range: rate,
            armor: rate,
            bite_reach: rate,
            pigment_r: rate,
            pigment_g: rate,
            pigment_b: rate,
        }
    }

    pub fn default_founder() -> Self {
        Self::uniform(BODY_MUT_RATE_DEFAULT)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Genome {
    pub size: f32,
    pub max_age: u32,
    pub photosynth_efficiency: f32,
    pub eat_efficiency: f32,
    pub scavenge_efficiency: f32,
    pub move_speed: f32,
    pub eye_count: u8,
    pub eye_offsets: [f32; EYE_SLOTS],
    pub vision_range: f32,
    pub armor: f32,
    pub bite_reach: f32,
    pub pigment_r: f32,
    pub pigment_g: f32,
    pub pigment_b: f32,
    pub mutation_rates: TraitMutationRates,
}

impl Genome {
    pub fn founder() -> Self {
        Self {
            size: FOUNDER_SIZE,
            max_age: FOUNDER_MAX_AGE,
            photosynth_efficiency: FOUNDER_PHOTO_EFF,
            eat_efficiency: 0.0,
            scavenge_efficiency: 0.0,
            move_speed: 0.0,
            eye_count: 0,
            eye_offsets: [0.0; EYE_SLOTS],
            vision_range: 0.0,
            armor: 0.0,
            bite_reach: FOUNDER_BITE_REACH,
            pigment_r: FOUNDER_PIGMENT_R,
            pigment_g: FOUNDER_PIGMENT_G,
            pigment_b: FOUNDER_PIGMENT_B,
            mutation_rates: TraitMutationRates::default_founder(),
        }
    }

    /// Per v5 §6 + v6 §F. Iterates traits in struct-declaration order with
    /// a single RNG (deterministic).
    ///
    /// `rate_multiplier` is the dev-panel `mutation_rate_multiplier` slider.
    pub fn mutate_in_place(&mut self, rng: &mut SimRng, rate_multiplier: f32) {
        // Snapshot per-trait rates and sigmas up front so the per-field
        // &mut borrows below don't tangle with reads of `self`.
        let r = self.mutation_rates.clone();
        let sigma_size = 0.1 * self.size;
        let sigma_max_age = 0.1 * self.max_age as f32;
        let sigma_photo = 0.1 * self.photosynth_efficiency.max(0.1);
        let sigma_eat = 0.1 * self.eat_efficiency.max(0.1);
        let sigma_scav = 0.1 * self.scavenge_efficiency.max(0.1);
        let sigma_move = 0.1 * self.move_speed.max(0.1);
        let sigma_vision = 0.1 * self.vision_range.max(1.0);
        let sigma_armor = 0.1 * self.armor.max(0.05);
        let sigma_bite = 0.1 * self.bite_reach;
        let sigma_pr = 0.1 * self.pigment_r.max(0.05);
        let sigma_pg = 0.1 * self.pigment_g.max(0.05);
        let sigma_pb = 0.1 * self.pigment_b.max(0.05);

        mutate_f32(
            &mut self.size,
            r.size * rate_multiplier,
            sigma_size,
            SIZE_MIN,
            SIZE_MAX,
            rng,
        );
        mutate_u32(
            &mut self.max_age,
            r.max_age * rate_multiplier,
            sigma_max_age,
            MAX_AGE_MIN,
            MAX_AGE_MAX,
            rng,
        );
        mutate_f32(
            &mut self.photosynth_efficiency,
            r.photosynth_efficiency * rate_multiplier,
            sigma_photo,
            PHOTO_EFF_MIN,
            PHOTO_EFF_MAX,
            rng,
        );
        mutate_f32(
            &mut self.eat_efficiency,
            r.eat_efficiency * rate_multiplier,
            sigma_eat,
            EAT_EFF_MIN,
            EAT_EFF_MAX,
            rng,
        );
        mutate_f32(
            &mut self.scavenge_efficiency,
            r.scavenge_efficiency * rate_multiplier,
            sigma_scav,
            SCAVENGE_EFF_MIN,
            SCAVENGE_EFF_MAX,
            rng,
        );
        mutate_f32(
            &mut self.move_speed,
            r.move_speed * rate_multiplier,
            sigma_move,
            MOVE_SPEED_MIN,
            MOVE_SPEED_MAX,
            rng,
        );

        // eye_count: pick uniformly from index-adjacent valid values.
        if rng.unit() < r.eye_count * rate_multiplier {
            let cur_idx = EYE_VALID
                .iter()
                .position(|&v| v == self.eye_count)
                .unwrap_or(0);
            let mut candidates: heapless::Vec<u8, 2> = heapless::Vec::new();
            if cur_idx > 0 {
                let _ = candidates.push(EYE_VALID[cur_idx - 1]);
            }
            if cur_idx + 1 < EYE_VALID.len() {
                let _ = candidates.push(EYE_VALID[cur_idx + 1]);
            }
            if !candidates.is_empty() {
                self.eye_count = candidates[rng.index(candidates.len())];
            }
        }

        // eye_offsets: only mutate slots i < current_eye_count, circular σ=0.1 rad
        let n_active = self.eye_count as usize;
        for i in 0..n_active {
            if rng.unit() < r.eye_offsets * rate_multiplier {
                let delta = rng.normal() * 0.1;
                let mut next = self.eye_offsets[i] + delta;
                let two_pi = std::f32::consts::TAU;
                next = (next % two_pi + two_pi) % two_pi;
                self.eye_offsets[i] = next;
            }
        }

        mutate_f32(
            &mut self.vision_range,
            r.vision_range * rate_multiplier,
            sigma_vision,
            VISION_RANGE_MIN,
            VISION_RANGE_MAX,
            rng,
        );
        mutate_f32(
            &mut self.armor,
            r.armor * rate_multiplier,
            sigma_armor,
            ARMOR_MIN,
            ARMOR_MAX,
            rng,
        );
        mutate_f32(
            &mut self.bite_reach,
            r.bite_reach * rate_multiplier,
            sigma_bite,
            BITE_REACH_MIN,
            BITE_REACH_MAX,
            rng,
        );
        mutate_f32(
            &mut self.pigment_r,
            r.pigment_r * rate_multiplier,
            sigma_pr,
            0.0,
            1.0,
            rng,
        );
        mutate_f32(
            &mut self.pigment_g,
            r.pigment_g * rate_multiplier,
            sigma_pg,
            0.0,
            1.0,
            rng,
        );
        mutate_f32(
            &mut self.pigment_b,
            r.pigment_b * rate_multiplier,
            sigma_pb,
            0.0,
            1.0,
            rng,
        );

        // Mutation rates themselves drift (per v5 §5 NN + §6 body): 0.5% per
        // birth, jitter ±20%. We apply the same drift to every per-trait
        // rate and the eye-offset rate.
        drift_rate(&mut self.mutation_rates.size, rng);
        drift_rate(&mut self.mutation_rates.max_age, rng);
        drift_rate(&mut self.mutation_rates.photosynth_efficiency, rng);
        drift_rate(&mut self.mutation_rates.eat_efficiency, rng);
        drift_rate(&mut self.mutation_rates.scavenge_efficiency, rng);
        drift_rate(&mut self.mutation_rates.move_speed, rng);
        drift_rate(&mut self.mutation_rates.eye_count, rng);
        drift_rate(&mut self.mutation_rates.eye_offsets, rng);
        drift_rate(&mut self.mutation_rates.vision_range, rng);
        drift_rate(&mut self.mutation_rates.armor, rng);
        drift_rate(&mut self.mutation_rates.bite_reach, rng);
        drift_rate(&mut self.mutation_rates.pigment_r, rng);
        drift_rate(&mut self.mutation_rates.pigment_g, rng);
        drift_rate(&mut self.mutation_rates.pigment_b, rng);
    }
}

fn mutate_f32(value: &mut f32, p: f32, sigma: f32, lo: f32, hi: f32, rng: &mut SimRng) {
    if rng.unit() < p {
        let delta = rng.normal() * sigma;
        *value = (*value + delta).clamp(lo, hi);
    }
}

fn mutate_u32(value: &mut u32, p: f32, sigma: f32, lo: u32, hi: u32, rng: &mut SimRng) {
    if rng.unit() < p {
        let delta = rng.normal() * sigma;
        let next = (*value as f32 + delta).round();
        *value = (next.clamp(lo as f32, hi as f32)) as u32;
    }
}

fn drift_rate(rate: &mut f32, rng: &mut SimRng) {
    if rng.unit() < MUT_RATE_OF_RATES {
        let jitter = 1.0 + rng.symm() * MUT_RATE_JITTER;
        *rate = (*rate * jitter).clamp(0.0, 1.0);
    }
}

// Tiny stack-only Vec replacement so we don't pull `heapless` if we don't need to.
// Use std Vec — heapless was overkill here.
mod heapless {
    pub struct Vec<T, const N: usize> {
        buf: [Option<T>; N],
        len: usize,
    }

    impl<T, const N: usize> Vec<T, N> {
        pub fn new() -> Self
        where
            T: Copy,
        {
            Self {
                buf: [None; N],
                len: 0,
            }
        }
        pub fn push(&mut self, v: T) -> Result<(), T> {
            if self.len < N {
                self.buf[self.len] = Some(v);
                self.len += 1;
                Ok(())
            } else {
                Err(v)
            }
        }
        pub fn is_empty(&self) -> bool {
            self.len == 0
        }
        pub fn len(&self) -> usize {
            self.len
        }
    }

    impl<T: Copy, const N: usize> std::ops::Index<usize> for Vec<T, N> {
        type Output = T;
        fn index(&self, i: usize) -> &T {
            self.buf[i].as_ref().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn founder_is_within_bounds() {
        let g = Genome::founder();
        assert!(g.size >= SIZE_MIN && g.size <= SIZE_MAX);
        assert!(g.bite_reach >= BITE_REACH_MIN && g.bite_reach <= BITE_REACH_MAX);
    }

    #[test]
    fn mutation_with_zero_rate_is_noop() {
        let mut g = Genome::founder();
        g.mutation_rates = TraitMutationRates::uniform(0.0);
        let before = g.clone();
        let mut rng = SimRng::from_u64(1);
        for _ in 0..100 {
            g.mutate_in_place(&mut rng, 1.0);
        }
        assert_eq!(g.size, before.size);
        assert_eq!(g.eye_count, before.eye_count);
    }

    #[test]
    fn mutation_with_full_rate_changes_traits() {
        let mut g = Genome::founder();
        g.mutation_rates = TraitMutationRates::uniform(1.0);
        let before = g.clone();
        let mut rng = SimRng::from_u64(2);
        g.mutate_in_place(&mut rng, 1.0);
        // size definitely drifted
        assert!((g.size - before.size).abs() > 0.0);
        // bounds still hold
        assert!(g.size >= SIZE_MIN && g.size <= SIZE_MAX);
        assert!(g.photosynth_efficiency >= PHOTO_EFF_MIN);
    }

    #[test]
    fn eye_count_only_mutates_to_adjacent() {
        let mut g = Genome::founder();
        g.eye_count = 6;
        g.mutation_rates = TraitMutationRates::uniform(0.0);
        g.mutation_rates.eye_count = 1.0;
        let mut rng = SimRng::from_u64(3);
        let mut saw_4 = false;
        let mut saw_8 = false;
        for _ in 0..1000 {
            let mut h = g.clone();
            h.mutate_in_place(&mut rng, 1.0);
            assert!(matches!(h.eye_count, 4 | 8 | 6));
            if h.eye_count == 4 {
                saw_4 = true;
            }
            if h.eye_count == 8 {
                saw_8 = true;
            }
        }
        assert!(saw_4 && saw_8);
    }
}
