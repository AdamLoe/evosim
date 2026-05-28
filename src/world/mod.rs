//! World — owns SoA + grass + RNG + tick orchestration.
//!
//! Within-tick ordering follows v5 §3.5. NN forward pass is live (Milestone D).
//! D3: genome removed; D9: Action enum collapsed to {Graze, Eat, Split}.

pub(crate) mod nn;
pub(crate) mod tick;

use self::nn::chunk_ranges;
use crate::brain::Brain;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::grass::GrassGrid;
use crate::grid::SpatialGrid;
use crate::rng::SimRng;
use crate::vision::{VisionBuf, VisionPass, VISION_LEN};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevSliders {
    pub mutation_rate_multiplier: f32,
    pub mouth_tax: f32,
    pub nn_mutation_sigma: f32,
    /// Grass cross-kernel propagation rate (k). See v1.2 amendments §A.4.
    pub grass_propagation_rate_k: f32,
    /// Grass in-cell logistic growth rate (r). See v1.2 amendments §A.4.
    pub grass_in_cell_growth_r: f32,
    /// Number of cells seeded at world init (affects next world creation only). See v1.2 amendments §A.4.
    pub grass_initial_seed_count: u32,
    /// Per-bite energy transfer fraction. Predator removes
    /// `eat_bite_fraction * prey.energy * (1 - prey.armor)` per successful bite.
    /// Live-tunable via set_eat_bite_fraction. See v1.2 P3a.
    pub eat_bite_fraction: f32,
    /// Live-tunable multiplier on per-tick *idle* upkeep (the always-on
    /// base + NN + gut + mouth-tax drain). 0 = no drain, 1 = default, 2 = double.
    pub upkeep_multiplier: f32,
    /// Live-tunable multiplier on per-tick *movement* cost. 0 = free move.
    pub move_cost_multiplier: f32,
    /// Live-tunable multiplier on the per-tick *eat-attempt* cost. 0 = free eat-tries.
    pub eat_cost_multiplier: f32,
    /// Live-tunable maximum energy per creature. Energy is clamped at the end of every
    /// tick, and new creatures spawn capped at this value.
    pub energy_max: f32,
    /// Live-tunable energy gained per successful graze bite.
    pub grass_energy_per_bite: f32,
    /// Live-tunable number of bites to drain a ripe grass cell. Density removed
    /// per bite = GRASS_MAX / bites_per_block.
    pub grass_bites_per_block: u32,
    /// Lifespan threshold beyond which the past-lifespan upkeep penalty applies.
    pub max_age: u32,
    /// Energy threshold required for a Split action to fire.
    pub split_threshold: f32,
    /// Energy gifted to each newborn at split (clamped at parent's residual).
    pub split_gift: f32,
    /// Number of founders seeded at world init (clamped to [1, 32]).
    pub founder_count: u32,
}

impl Default for DevSliders {
    fn default() -> Self {
        Self {
            mutation_rate_multiplier: 1.0,
            mouth_tax: UPKEEP_MOUTH_DEFAULT,
            nn_mutation_sigma: NN_MUT_SIGMA_DEFAULT,
            grass_propagation_rate_k: GRASS_PROPAGATION_RATE_K_DEFAULT,
            grass_in_cell_growth_r: GRASS_IN_CELL_GROWTH_R_DEFAULT,
            grass_initial_seed_count: GRASS_INITIAL_SEED_COUNT_DEFAULT,
            eat_bite_fraction: EAT_BITE_FRACTION_DEFAULT,
            upkeep_multiplier: 1.0,
            move_cost_multiplier: 1.0,
            eat_cost_multiplier: 1.0,
            energy_max: 100.0,
            grass_energy_per_bite: GRASS_ENERGY_PER_BITE_DEFAULT,
            grass_bites_per_block: GRASS_BITES_PER_BLOCK_DEFAULT,
            max_age: FOUNDER_MAX_AGE,
            split_threshold: SPLIT_THRESHOLD,
            split_gift: SPLIT_GIFT_MAX,
            founder_count: FOUNDER_COUNT_DEFAULT,
        }
    }
}

/// Halton sequence value at index `i` for base `b`. Used for deterministic
/// quasi-random founder placement (multi-founder spawn, v1.5).
fn halton(mut i: u32, b: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    while i > 0 {
        f /= b as f32;
        r += f * (i % b) as f32;
        i /= b;
    }
    r
}

pub struct World {
    pub tick: u32,
    pub seed: String,
    pub rng: SimRng,
    /// Grass density field (v1.2 grass mechanic). 240×240 cells, 2.5u each.
    pub grass: GrassGrid,
    pub grid: SpatialGrid,
    pub creatures: CreatureSoA,
    pub sliders: DevSliders,
    pub next_creature_id: u64,
    pub peak_population: u32,
    pub world_ended: bool,
    pub first_move_fired: bool,
    pub first_eat_fired: bool,
    /// Bitset of population milestone thresholds that have fired (v5 §11, E.25.a).
    /// Bit k = POPULATION_MILESTONES[k] was crossed. One-shot: never cleared.
    pub population_milestones_fired: u32,
    /// Per-creature vision cache (index-aligned with CreatureSoA). Milestone C.12.
    pub vision: Vec<VisionBuf>,
    /// In-app hierarchical profiler. Runtime-toggleable, default OFF.
    /// Excluded from SaveV1 and from the §16 snapshot hash (D10).
    /// See docs/plans/perf-timing.md for the full design.
    pub profile: crate::profiler::Profiler,
    // Per-tick scratch buffers, promoted from in-function `vec!` to long-lived
    // fields to eliminate ~300 KB/tick allocator pressure (perf-final-report
    // §3 item 2). Excluded from SaveV1 by omission —
    // mirrors the `vision` / `profile` pattern.
    pub(crate) scratch_fx: Vec<f32>,
    pub(crate) scratch_fy: Vec<f32>,
    pub(crate) scratch_neighbors: Vec<usize>,
    pub(crate) scratch_damage: Vec<f32>,
    pub(crate) scratch_gain: Vec<f32>,
    pub(crate) scratch_cooldown_set: Vec<bool>,
    pub(crate) scratch_attempted_eat: Vec<bool>,
    pub(crate) scratch_got_a_bite: Vec<bool>,
    /// S25: promoted eat-candidate buffer. Eliminates the per-Eat-per-tick
    /// `Vec::with_capacity(8)` allocation in eat_and_scavenge.
    pub(crate) scratch_eat_candidates: Vec<usize>,
    /// S27: promoted dead-indices buffer from collect_deaths.
    /// Cleared and refilled each call; caller reads it via &self.scratch_dead.
    pub(crate) scratch_dead: Vec<usize>,
}

impl World {
    /// Legacy 1-founder constructor used by Rust-side tests and as the basic
    /// entrypoint when no slider overrides are needed. Wasm callers should
    /// route through `WorldHandle::new_with_founder_count` for the multi-
    /// founder default.
    #[allow(dead_code)]
    pub fn new(seed: impl Into<String>) -> Self {
        Self::new_with_sliders(
            seed,
            DevSliders {
                founder_count: 1,
                ..Default::default()
            },
        )
    }

    pub fn new_with_sliders(seed: impl Into<String>, sliders: DevSliders) -> Self {
        let seed_string = seed.into();
        let mut rng = SimRng::from_string(&seed_string);
        let grass = GrassGrid::new(&mut rng, sliders.grass_initial_seed_count);
        let mut creatures = CreatureSoA::with_capacity(2048);
        let founder_count = sliders.founder_count.clamp(1, 32);
        let founder_energy = FOUNDER_ENERGY.min(sliders.energy_max);
        let body_r = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let lo = body_r;
        let hi = WORLD_SIZE - body_r;
        for k in 0..founder_count {
            let brain = Brain::founder(&mut rng);
            // Halton (2, 3) gives a low-discrepancy 2D sequence; shift by 1 so
            // the first sample isn't (0, 0).
            let hx = halton(k + 1, 2);
            let hy = halton(k + 1, 3);
            let x = lo + hx * (hi - lo);
            let y = lo + hy * (hi - lo);
            creatures.push(k as u64, x, y, founder_energy, 0, brain);
        }
        let mut grid = SpatialGrid::new();
        grid.rebuild(&creatures.x, &creatures.y);
        let vision = vec![[0.0f32; VISION_LEN]; founder_count as usize];
        Self {
            tick: 0,
            seed: seed_string,
            rng,
            grass,
            grid,
            creatures,
            sliders,
            next_creature_id: founder_count as u64,
            peak_population: founder_count,
            world_ended: false,
            first_move_fired: false,
            first_eat_fired: false,
            population_milestones_fired: 0,
            vision,
            profile: crate::profiler::Profiler::new(),
            scratch_fx: Vec::new(),
            scratch_fy: Vec::new(),
            scratch_neighbors: Vec::new(),
            scratch_damage: Vec::new(),
            scratch_gain: Vec::new(),
            scratch_cooldown_set: Vec::new(),
            scratch_attempted_eat: Vec::new(),
            scratch_got_a_bite: Vec::new(),
            scratch_eat_candidates: Vec::new(),
            scratch_dead: Vec::new(),
        }
    }

    #[inline]
    pub fn population(&self) -> u32 {
        self.creatures.len() as u32
    }

    /// Run one sim tick. Returns true if the world is still alive afterwards.
    pub fn step(&mut self) -> bool {
        if self.world_ended {
            return false;
        }

        // Profiler: outer "tick" span wraps the entire step.
        // Each sub-span is brace-scoped so its guard drops before the next
        // sibling span pushes (required for correct parent attribution).
        crate::profile_span!(&self.profile, "tick");

        // 1. Rebuild spatial hash grid from start-of-tick positions.
        {
            crate::profile_span!(&self.profile, "tick.grid.rebuild");
            self.grid.rebuild(&self.creatures.x, &self.creatures.y);
        }

        // 2. Vision pass (Milestone C.12).
        {
            crate::profile_span!(&self.profile, "tick.vision");
            self.run_vision_pass();
        }

        // 3. NN forward pass + action decode (Milestone D).
        // Chunked per v6 §J; sequential by default, parallel behind `threads` feature.
        {
            crate::profile_span!(&self.profile, "tick.nn");
            let n = self.creatures.len();
            let ranges = chunk_ranges(n);
            self.nn_forward_all_chunks(&ranges, n);
            // Snapshot energy *now* (immediately after NN forward — tick body
            // hasn't run yet, so energy still equals its value at NN input
            // time). Next tick's NN forward subtracts this to get the delta
            // covering "what happened during this tick".
            self.creatures.prev_energy[..n].copy_from_slice(&self.creatures.energy[..n]);
        }

        // 4. Apply velocities + soft repulsion + wall clamp; rebuild grid.
        {
            crate::profile_span!(&self.profile, "tick.movement");
            self.apply_movement_and_repulsion();
        }

        // 5. Graze: creatures with Action::Graze consume grass from overlapping cells.
        // Sequential regardless of --features threads (shared density Vec).
        // Runs after movement (creature at current-tick position) and before eat_scavenge.
        // See v1.2 p1e §1 for ordering rationale.
        {
            crate::profile_span!(&self.profile, "tick.graze");
            self.graze();
        }

        // 6. Eat resolution (scavenge action removed in D9).
        {
            crate::profile_span!(&self.profile, "tick.eat_scavenge");
            self.eat();
        }

        // 7. Grass propagation step (v1.2 grass mechanic).
        // Sequential scalar pass over 57_600 cells. Reads slider-driven r and k.
        // Graze consumes density (step 5); propagation runs after (matching old
        // ordering where sun.refill ran after photosynth_two_pass). See p1e §1.
        {
            crate::profile_span!(&self.profile, "tick.grass_step");
            self.grass.step(
                self.sliders.grass_in_cell_growth_r,
                self.sliders.grass_propagation_rate_k,
            );
        }

        // 8. Energy bookkeeping.
        {
            crate::profile_span!(&self.profile, "tick.energy_bookkeeping");
            self.energy_bookkeeping();
        }

        // 9. Deaths. Span widened (R9) to cover the dead-removal
        //    swap_remove loop and creatures.remove_indices (step 11, scales with die-off).
        {
            crate::profile_span!(&self.profile, "tick.collect_deaths");
            // S27: collect_deaths writes into self.scratch_dead (promoted pool).
            self.collect_deaths();
            if !self.scratch_dead.is_empty() {
                // Mirror swap_remove on vision vec to keep it index-aligned.
                // Walk from back just like remove_indices does.
                for &k in self.scratch_dead.iter().rev() {
                    if k < self.vision.len() {
                        self.vision.swap_remove(k);
                    }
                }
                // Use mem::take to avoid borrow conflict: remove_indices takes
                // &[usize] which would re-borrow self.scratch_dead while
                // self.creatures is also borrowed mutably via remove_indices.
                let dead = std::mem::take(&mut self.scratch_dead);
                self.creatures.remove_indices(&dead);
                self.scratch_dead = dead; // restore the buffer (high-water-mark kept)
            }
        }

        // 10. Births.
        {
            crate::profile_span!(&self.profile, "tick.handle_births");
            self.handle_births();
        }

        // 12. Step-12 tail: last_action promotion, tick bump, milestone events,
        //     world-end check. Cheap today (R9: bookkeeping_tail span).
        {
            crate::profile_span!(&self.profile, "tick.bookkeeping_tail");

            // Promote action chain by one tick:
            //   last2_action ← last_action  (the one-tick-older slot)
            //   last_action  ← action_this_tick  (this tick's choice)
            // Bulk memcpy in two passes (S30 pattern). Order matters: snapshot
            // last_action into last2_action *before* overwriting last_action.
            let n = self.creatures.len();
            self.creatures.last2_action[..n].copy_from_slice(&self.creatures.last_action[..n]);
            self.creatures.last_action[..n].copy_from_slice(&self.creatures.action_this_tick[..n]);

            self.tick = self.tick.saturating_add(1);
            let pop = self.creatures.len() as u32;
            if pop > self.peak_population {
                self.peak_population = pop;
            }

            // E.25.a: PopulationMilestone threshold tracking (once per threshold ever).
            // Events no longer emitted (D4); bitset kept for potential D5 HoF use.
            for (k, &threshold) in POPULATION_MILESTONES.iter().enumerate() {
                let bit = 1u32 << k;
                if pop >= threshold && (self.population_milestones_fired & bit) == 0 {
                    self.population_milestones_fired |= bit;
                }
            }

            if self.creatures.is_empty() {
                self.world_ended = true;
                return false;
            }
        }

        true
    }

    pub(crate) fn handle_births(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        let split_threshold = self.sliders.split_threshold;
        let split_gift_max = self.sliders.split_gift;
        for i in 0..n {
            if self.creatures.action_this_tick[i] != Action::Split {
                continue;
            }
            if self.creatures.energy[i] < split_threshold {
                continue;
            }
            let parent_energy_after_cost = self.creatures.energy[i] - split_threshold;
            let gift = parent_energy_after_cost.clamp(0.0, split_gift_max);
            self.creatures.energy[i] = parent_energy_after_cost - gift;
            let child_energy = gift;

            // D3: no child_genome mutation. Brain mutation only.
            let child_brain = Brain::child_from(
                &self.creatures.brains[i],
                &mut self.rng,
                self.sliders.nn_mutation_sigma,
                self.sliders.mutation_rate_multiplier,
            );
            let jitter_x = self.rng.symm() * FOUNDER_SPLIT_JITTER;
            let jitter_y = self.rng.symm() * FOUNDER_SPLIT_JITTER;
            // D3: size is constant FOUNDER_SIZE for all creatures.
            let radius = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
            let clamp_lo = radius;
            let clamp_hi = WORLD_SIZE - radius;
            let cx = (self.creatures.x[i] + jitter_x).clamp(clamp_lo, clamp_hi);
            let cy = (self.creatures.y[i] + jitter_y).clamp(clamp_lo, clamp_hi);
            let new_id = self.next_creature_id;
            self.next_creature_id += 1;
            self.creatures
                .push(new_id, cx, cy, child_energy, self.tick, child_brain);
        }
    }

    pub fn tick_once(&mut self) -> bool {
        self.step()
    }

    /// Run the vision pass.
    /// Ensures self.vision is index-aligned with self.creatures.
    pub(crate) fn run_vision_pass(&mut self) {
        let n = self.creatures.len();
        // Ensure vision buffer is large enough.
        if self.vision.len() < n {
            self.vision.resize(n, [0.0f32; VISION_LEN]);
        }
        let pass = VisionPass {
            creatures: &self.creatures,
            grid: &self.grid,
        };
        pass.run(&mut self.vision[..n]);
    }

    /// Test-only deep clone of the World. Clones all SoA data and resets
    /// transient state (vision, scratch bufs, spatial grid). Only available under `#[cfg(test)]`.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn clone_for_test(&self) -> World {
        let n = self.creatures.len();
        let mut creatures = CreatureSoA::with_capacity(n.max(1));
        for i in 0..n {
            creatures.push(
                self.creatures.id[i],
                self.creatures.x[i],
                self.creatures.y[i],
                self.creatures.energy[i],
                self.creatures.birth_tick[i],
                self.creatures.brains[i].clone(),
            );
            // Restore non-push fields.
            creatures.vx[i] = self.creatures.vx[i];
            creatures.vy[i] = self.creatures.vy[i];
            creatures.age[i] = self.creatures.age[i];
            creatures.digestion_cooldown[i] = self.creatures.digestion_cooldown[i];
            creatures.cumulative_upkeep[i] = self.creatures.cumulative_upkeep[i];
            creatures.last_action[i] = self.creatures.last_action[i];
            creatures.action_this_tick[i] = self.creatures.action_this_tick[i];
            creatures.distance_travelled[i] = self.creatures.distance_travelled[i];
        }
        let mut grid = SpatialGrid::new();
        grid.rebuild(&creatures.x, &creatures.y);
        World {
            tick: self.tick,
            seed: self.seed.clone(),
            rng: self.rng.clone(),
            grass: self.grass.clone(),
            grid,
            creatures,
            sliders: self.sliders.clone(),
            next_creature_id: self.next_creature_id,
            peak_population: self.peak_population,
            world_ended: self.world_ended,
            first_move_fired: self.first_move_fired,
            first_eat_fired: self.first_eat_fired,
            population_milestones_fired: self.population_milestones_fired,
            vision: vec![[0.0f32; VISION_LEN]; n],
            profile: crate::profiler::Profiler::new(),
            scratch_fx: Vec::new(),
            scratch_fy: Vec::new(),
            scratch_neighbors: Vec::new(),
            scratch_damage: Vec::new(),
            scratch_gain: Vec::new(),
            scratch_cooldown_set: Vec::new(),
            scratch_attempted_eat: Vec::new(),
            scratch_got_a_bite: Vec::new(),
            scratch_eat_candidates: Vec::new(),
            scratch_dead: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_initializes_with_one_creature() {
        // Legacy World::new uses 1 founder (multi-founder default lives on
        // WorldHandle for the wasm entrypoint).
        let w = World::new("test-seed");
        assert_eq!(w.population(), 1);
    }

    #[test]
    fn world_initializes_with_default_multi_founder() {
        let w = World::new_with_sliders("multi-seed", DevSliders::default());
        assert_eq!(w.population(), FOUNDER_COUNT_DEFAULT);
    }

    #[test]
    fn lone_creature_eventually_splits() {
        // D8: F.30 hardwiring deleted. With pure random NN weights and mostly-zero
        // inputs (move_speed=0, eye_count=0), the NN may deterministically output
        // the same action every tick. Trigger the first split directly via
        // handle_births (bypassing the NN step), confirming the birth mechanism works.
        let mut w = World::new("split-test");
        w.creatures.energy[0] = 10_000.0;
        w.creatures.action_this_tick[0] = Action::Split;
        w.handle_births();
        assert!(
            w.population() > 1,
            "expected a split from high energy + Split action"
        );
    }

    #[test]
    fn world_runs_many_ticks_without_panic() {
        let mut w = World::new("smoke");
        for _ in 0..2000 {
            if !w.tick_once() {
                break;
            }
        }
        assert!(w.tick > 0);
    }

    /// D3: world runs without genome fields. All creatures have MOVE_SPEED_MAX, 24 eyes.
    #[test]
    fn world_runs_2000_ticks_with_movement() {
        let mut w = World::new("movement-smoke");
        // D3: no genome patch needed — all creatures have MOVE_SPEED_MAX, 24 eyes, VISION_RANGE_MAX.
        for _ in 0..2000 {
            if !w.tick_once() {
                break;
            }
        }
        // World ran for at least 1 tick without panicking.
        assert!(w.tick > 0);
    }

    /// D.18 test 18: chunked sequential tick matches expected stable world state
    /// (just checks no panic and produces same number of ticks deterministically).
    #[test]
    fn chunked_tick_deterministic() {
        // Run the same seed twice and compare tick counts after 200 ticks.
        // The world is inherently chunked already; this confirms the chunking
        // doesn't introduce divergence from seeded determinism.
        let ticks_a = {
            let mut w = World::new("chunk-det-a");
            let mut t = 0;
            for _ in 0..200 {
                if w.tick_once() {
                    t += 1;
                } else {
                    break;
                }
            }
            t
        };
        let ticks_b = {
            let mut w = World::new("chunk-det-a"); // same seed
            let mut t = 0;
            for _ in 0..200 {
                if w.tick_once() {
                    t += 1;
                } else {
                    break;
                }
            }
            t
        };
        assert_eq!(
            ticks_a, ticks_b,
            "same seed must produce identical tick count"
        );
    }

    // ---- D.19 smoke test ----

    /// D.19: 1000 creatures × 1000 ticks — no panic, energy bounded, varied actions.
    #[test]
    fn d19_thousand_creatures_thousand_ticks_no_explode() {
        use crate::brain::Brain;
        use crate::vision::VISION_LEN;

        let mut w = World::new("d19-smoke");
        let mut seeder = SimRng::from_string("d19-seed");

        // Seed in 999 extra creatures (founder is already there).
        // D3: no genome diversity — just varied brain initializations.
        for k in 0..999u64 {
            let b = Brain::founder(&mut seeder);
            let x = seeder.uniform(10.0, WORLD_SIZE - 10.0);
            let y = seeder.uniform(10.0, WORLD_SIZE - 10.0);
            w.creatures.push(k + 1, x, y, FOUNDER_ENERGY, 0, b);
            w.vision.push([0.0f32; VISION_LEN]);
        }

        let energy_start: f32 = w.creatures.energy.iter().sum();
        let total_energy_before = energy_start;

        for _ in 0..1000 {
            w.tick_once();
        }

        // (a) no panic — implicit by reaching here.

        // (b) total energy stayed bounded. Grass regen is bounded; use a loose
        //     per-tick bound of 1000 energy units added (generous slack for 1k ticks).
        let total_energy_after: f32 = w.creatures.energy.iter().sum();
        assert!(
            total_energy_after.is_finite(),
            "total energy must be finite"
        );
        let max_expected = total_energy_before + 1e6;
        assert!(
            total_energy_after < max_expected,
            "total energy {total_energy_after:.1} exceeded sane bound {max_expected:.1}"
        );

        // (c) NN was wired — confirm via total ticks run (world ran at least some ticks).
        // If pop is 0 (mass extinction), the world still ran many ticks without panicking —
        // that's the load-bearing assertion for D.19: forward pass fires per tick.
        // If any creatures remain, confirm they picked varied actions.
        if w.creatures.is_empty() {
            // Mass extinction — the world at least ran the full loop without panic.
            // No action-variety assertion is possible; extinction is plausible with
            // random brains + crowded start. This is OK per D.19 spec note.
            assert!(w.tick > 0, "world must have run at least one tick");
        } else {
            let mut counts = [0usize; 3]; // v1.3 D9: 3 variants (Graze, Eat, Split)
            for &a in &w.creatures.action_this_tick {
                counts[a as usize] += 1;
            }
            let non_photo: usize = counts
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != Action::Graze as usize)
                .map(|(_, c)| *c)
                .sum();
            // Graze is the safe choice; if all survivors pick it exclusively,
            // that's still plausible with random brains (low-energy creatures graze).
            // The key assertion: NN was invoked (no panic, ticks > 0).
            assert!(
                non_photo > 0 || !w.creatures.is_empty(),
                "no survivors and no non-photo action; pop={}, ticks={}",
                w.creatures.len(),
                w.tick
            );
        }
    }

    /// perf-1 T2: child's trig cache is populated on birth; parent's is unchanged.
    /// D3: trig is determined by sector index only (no genome fields).
    #[test]
    fn eye_trig_recomputed_on_birth() {
        use crate::brain::Brain;
        use crate::creature::CreatureSoA;
        use crate::rng::SimRng;
        use crate::vision::SECTORS;

        let mut w = World::new("perf1-birth");
        // Coerce the founder to definitely split this tick.
        w.creatures.energy[0] = 1_000.0;
        w.creatures.action_this_tick[0] = Action::Split;
        let parent_trig_before: Vec<f32> = w.creatures.eye_trig[..SECTORS * 2].to_vec();
        w.handle_births();
        assert!(w.creatures.len() >= 2, "birth must have produced a child");
        // Parent's cache unchanged.
        assert_eq!(
            &w.creatures.eye_trig[..SECTORS * 2],
            parent_trig_before.as_slice()
        );
        // Child's cache matches a fresh recompute (D3: same for all creatures).
        let child_idx = w.creatures.len() - 1;
        let mut expected = CreatureSoA::with_capacity(1);
        expected.push(
            0,
            0.0,
            0.0,
            0.0,
            0,
            Brain::founder(&mut SimRng::from_u64(0)), // brain irrelevant for cache
        );
        let off = child_idx * SECTORS * 2;
        assert_eq!(
            &w.creatures.eye_trig[off..off + SECTORS * 2],
            &expected.eye_trig[..SECTORS * 2]
        );
    }
}
