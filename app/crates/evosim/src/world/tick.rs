//! Per-tick step bodies that mutate `World`. All as `impl World` blocks; private to the `crate::world` parent.
//!
//! v2.0 Wave 2a: per-creature body genome reintroduced. Body trait reads are
//! genome-rescaled at each use site (radius, speed cap, move cost, idle upkeep,
//! graze yield, attack damage). The `Eat` action was renamed to `Attack`. The
//! biome movement penalty is genome-modulated per creature (water_affinity /
//! heat_tolerance reduce the water / desert severity).

use super::World;
use crate::constants::*;
use crate::creature::{Action, FlashTag};
#[cfg(feature = "threads")]
use rayon::prelude::*;

/// Per-predator outcome from the parallel scan phase of `attack()`. Applied
/// sequentially after the parallel block since `Hit` writes touch both the
/// attacker and victim scratch lanes — a race risk under rayon.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum AttackPick {
    /// Creature didn't choose `Action::Attack` (or n == 0).
    #[default]
    Skip,
    /// Creature chose Attack but was in digestion cooldown, or had no in-reach
    /// target. Records the "attempt" for downstream bookkeeping without applying
    /// any bite.
    Miss,
    /// Attack landed on victim `j`; `transfer` is the energy delta removed from
    /// the victim (already scaled by the attacker's diet+body_size).
    Hit { j: u32, transfer: f32 },
}

impl World {
    /// Multi-cell graze: for every creature that chose `Action::Graze` this tick,
    /// consume grass density from every `GrassGrid` cell whose box overlaps the
    /// creature's body circle.
    ///
    /// Per-cell delta is discrete: a cell with density ≥ one chunk yields
    /// exactly one bite of `grass_energy_per_bite` energy; under-threshold
    /// cells yield zero. Energy is independent of remaining density.
    ///
    /// Runs SEQUENTIALLY regardless of `--features threads` (two creatures may overlap
    /// the same grass cell; parallel writes would race on `density[cell]`).
    pub(crate) fn graze(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }

        let density_chunk = GRASS_MAX / self.sliders.grass_bites_per_block.max(1) as f32;
        let energy_per_bite = self.sliders.grass_energy_per_bite;

        for i in 0..n {
            if self.creatures.action_this_tick[i] != Action::Graze {
                continue;
            }
            let xi = self.creatures.x[i];
            let yi = self.creatures.y[i];
            // v2.0 Wave 2a: body radius is genome-scaled.
            let ri =
                CREATURE_SIZE * BODY_RADIUS_PER_SIZE * self.creatures.genome[i].body_size_factor();
            // v2.0 Wave 2a: graze yield favors grazers — a pure predator
            // (diet=1) grazes at half yield; a pure grazer (diet=0) at full.
            let diet = self.creatures.genome[i].diet;
            let graze_yield = 1.0 - 0.5 * diet;

            // Phase 1: collect overlapping cell indices (immutable borrow of grass).
            let mut cells = std::mem::take(&mut self.scratch_neighbors);
            cells.clear();
            self.grass
                .for_each_cell_overlapping_circle(xi, yi, ri, |cell_idx| {
                    cells.push(cell_idx);
                });

            // Phase 2: consume from each cell (mutable borrow of grass).
            let mut total_gain = 0.0_f32;
            for &cell_idx in &cells {
                total_gain += self.grass.consume(cell_idx, density_chunk, energy_per_bite);
            }
            self.creatures.energy[i] += total_gain * graze_yield;
            // v2.0 Wave 2a: ring-flash green if this graze actually consumed.
            if total_gain > 0.0 {
                self.creatures.set_flash(i, FlashTag::Grazed);
            }

            // Restore the scratch buffer (high-water-mark allocation preserved).
            self.scratch_neighbors = cells;
        }
    }
}

impl World {
    pub(crate) fn apply_movement_and_repulsion(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }

        let move_mult = self.sliders.move_cost_multiplier;
        let world_size = self.dims.world_size;
        let wrap = self.dims.wrap_world;
        {
            crate::profile_span!(&self.profile, "tick.movement.integrate");
            for i in 0..n {
                // v2.0 Wave 2a: speed cap + move cost are genome-scaled by max_speed.
                let speed_factor = self.creatures.genome[i].max_speed_factor();
                let speed_cap = MOVE_SPEED_MAX * speed_factor;
                // v2.0 Wave 2a: GENOME-modulated biome penalty `p` for the cell the
                // creature occupies (reduced by its water_affinity / heat_tolerance).
                // Two of its three effects apply here: a reduced effective speed cap
                // and a higher move-cost multiplier. (The third — extra upkeep — is
                // in energy_bookkeeping.)
                let p = self.movement_penalty_for(i);
                let eff_speed_cap = speed_cap * (1.0 - K_BIOME_SPEED * p);
                // Faster creatures pay proportionally more per distance (max_speed
                // trades raw cap for higher movement cost).
                let eff_move_mult = move_mult * speed_factor * (1.0 + K_BIOME_COST * p);
                let vx = self.creatures.vx[i];
                let vy = self.creatures.vy[i];
                let mag2 = vx * vx + vy * vy;
                let (cvx, cvy) = if eff_speed_cap > 0.0 && mag2 > eff_speed_cap * eff_speed_cap {
                    let s = eff_speed_cap / mag2.sqrt();
                    (vx * s, vy * s)
                } else {
                    (vx, vy)
                };
                let dist = (cvx * cvx + cvy * cvy).sqrt();
                self.creatures.energy[i] -= dist * COST_MOVE_PER_DIST * eff_move_mult;
                self.creatures.distance_travelled[i] += dist;
                // Toroidal: wrap the velocity step into [0, world_size). Walled: the
                // final wall-clamp below keeps positions in bounds.
                let mut nx = self.creatures.x[i] + cvx;
                let mut ny = self.creatures.y[i] + cvy;
                if wrap {
                    nx = nx.rem_euclid(world_size);
                    ny = ny.rem_euclid(world_size);
                }
                self.creatures.x[i] = nx;
                self.creatures.y[i] = ny;
            }
        } // tick.movement.integrate

        // v2.0.x Lever 2: NO rebuild here. Repulsion + attack run on the
        // start-of-tick grid (built in step 1). Integration moves a creature at
        // most MOVE_SPEED_MAX (5u) = 0.25 cell at HASH_CELL=20, so the neighbor
        // buckets are at most a quarter-cell stale vs the 3u repulsion/attack
        // reach — an accepted approximation (consistent with the already-noisy
        // threaded grass field). Eliminates 2 of the 3 grid rebuilds per tick.

        // v2.0 Wave 2a: body radius is genome-scaled, so the search radius uses
        // the maximum possible body radius (body_size trait = 1.0 → factor 1.5)
        // for BOTH creatures to be sure no overlapping pair is missed.
        let max_body_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE * 1.5;
        let search = max_body_r + max_body_r;
        let rep_max = self.sliders.repulsion_max;

        self.scratch_fx.resize(n, 0.0);
        self.scratch_fy.resize(n, 0.0);
        self.scratch_fx.fill(0.0);
        self.scratch_fy.fill(0.0);
        // Skip the per-creature neighbor scan entirely when repulsion is disabled.
        // Every force computed below is clamped to [0, rep_max], so rep_max=0 yields
        // no movement contribution — but the scan itself is O(N·K) and explodes when
        // creatures clump (which happens precisely *because* repulsion is off).
        {
            crate::profile_span!(&self.profile, "tick.movement.repulsion");
            if rep_max > 0.0 {
                for i in 0..n {
                    let xi = self.creatures.x[i];
                    let yi = self.creatures.y[i];
                    let mut neighbors = std::mem::take(&mut self.scratch_neighbors);
                    neighbors.clear();
                    self.grid.for_each_in_radius(xi, yi, search, |j| {
                        if j > i {
                            neighbors.push(j);
                        }
                    });
                    // v2.0 Wave 2a: per-creature genome-scaled body radius.
                    let ri = CREATURE_SIZE
                        * BODY_RADIUS_PER_SIZE
                        * self.creatures.genome[i].body_size_factor();
                    for &j in &neighbors {
                        let xj = self.creatures.x[j];
                        let yj = self.creatures.y[j];
                        // Walled: raw Euclidean displacement. Toroidal: minimum-image
                        // so seam-neighbors repel the short way.
                        let mut dx = xj - xi;
                        let mut dy = yj - yi;
                        if wrap {
                            if dx > world_size * 0.5 {
                                dx -= world_size;
                            } else if dx < -world_size * 0.5 {
                                dx += world_size;
                            }
                            if dy > world_size * 0.5 {
                                dy -= world_size;
                            } else if dy < -world_size * 0.5 {
                                dy += world_size;
                            }
                        }
                        let d2 = dx * dx + dy * dy;
                        // v2.0 Wave 2a: pair sum of genome-scaled radii.
                        let rj = CREATURE_SIZE
                            * BODY_RADIUS_PER_SIZE
                            * self.creatures.genome[j].body_size_factor();
                        let rsum = ri + rj;
                        if d2 < rsum * rsum {
                            if d2 > 1e-8 {
                                let d = d2.sqrt();
                                let overlap = rsum - d;
                                let f = (REPULSION_K * overlap).clamp(0.0, rep_max);
                                let ux = dx / d;
                                let uy = dy / d;
                                self.scratch_fx[i] -= ux * f;
                                self.scratch_fy[i] -= uy * f;
                                self.scratch_fx[j] += ux * f;
                                self.scratch_fy[j] += uy * f;
                            } else {
                                // Co-located (newborn siblings) — deterministic nudge.
                                let angle = ((i as f32) * 0.7 + (j as f32) * 1.3).sin()
                                    * std::f32::consts::PI;
                                let ux = angle.cos();
                                let uy = angle.sin();
                                let f = rep_max;
                                self.scratch_fx[i] -= ux * f;
                                self.scratch_fy[i] -= uy * f;
                                self.scratch_fx[j] += ux * f;
                                self.scratch_fy[j] += uy * f;
                            }
                        }
                    }
                    self.scratch_neighbors = neighbors;
                }
            }
        } // tick.movement.repulsion

        // Apply repulsion forces + wall-clamp/wrap. v2.0.x Lever 2: the grid is
        // NOT rebuilt afterward — the post-repulsion displacement (≤ a few rep_max
        // = 0.1u nudges) is far below the 3u attack reach, and the next consumer
        // (attack) tolerates the sub-unit staleness. Next tick's step-1 rebuild
        // re-syncs from final positions.
        // Walled: wall-clamp to [ri, world_size-ri]. Toroidal: wrap into
        // [0, world_size). v2.0 Wave 2a: ri is genome-scaled per creature.
        {
            crate::profile_span!(&self.profile, "tick.movement.apply");
            for i in 0..n {
                let ri = CREATURE_SIZE
                    * BODY_RADIUS_PER_SIZE
                    * self.creatures.genome[i].body_size_factor();
                let lo = ri;
                let hi = world_size - ri;
                let px = self.creatures.x[i] + self.scratch_fx[i];
                let py = self.creatures.y[i] + self.scratch_fy[i];
                let (new_x, new_y) = if wrap {
                    (px.rem_euclid(world_size), py.rem_euclid(world_size))
                } else {
                    (px.clamp(lo, hi), py.clamp(lo, hi))
                };
                self.creatures.x[i] = new_x;
                self.creatures.y[i] = new_y;
            }
        }
    }

    /// Attack resolution (v2.0 Wave 2a: `eat` renamed to `attack`).
    ///
    /// Parallel scan + sequential apply. The per-attacker search (find an
    /// in-reach target) is the hot loop at high pop — in a clump, every cell
    /// query returns many candidates. Splitting the search across rayon workers
    /// takes that O(N·K) cost down by the worker count.
    ///
    /// The parallel phase only reads from the SoA + grid; writes that touch
    /// shared victim lanes (`scratch_damage[j]`) happen serially afterward from
    /// a per-attacker `AttackPick` buffer.
    ///
    /// v2.0 Wave 2a: attack effectiveness is genome-scaled. The transfer is
    /// `eat_bite_fraction * victim.energy * effectiveness`, where
    /// `effectiveness = (0.5 + diet) * body_size_factor` of the ATTACKER —
    /// predator-diet + larger body hit harder. Single-pool: hits any creature
    /// (W3 adds species gating).
    pub(crate) fn attack(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        self.scratch_damage.resize(n, 0.0);
        self.scratch_gain.resize(n, 0.0);
        self.scratch_cooldown_set.resize(n, false);
        self.scratch_attempted_eat.resize(n, false);
        self.scratch_got_a_bite.resize(n, false);
        self.scratch_attack_picks.resize(n, AttackPick::Skip);
        self.scratch_damage.fill(0.0);
        self.scratch_gain.fill(0.0);
        self.scratch_cooldown_set.fill(false);
        self.scratch_attempted_eat.fill(false);
        self.scratch_got_a_bite.fill(false);
        self.scratch_attack_picks.fill(AttackPick::Skip);

        // v2.0 Wave 2a: max reach uses the largest possible body radius for both
        // attacker and victim (body_size factor ≤ 1.5) so the grid query never
        // misses an in-reach pair; the exact per-pair radius is checked inside.
        let max_body_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE * 1.5;
        let max_range = max_body_r + max_body_r;
        let bite_frac = self.sliders.eat_bite_fraction;
        let world_size = self.dims.world_size;
        let wrap = self.dims.wrap_world;
        // v2.0 Wave 3a: species mode refuses same-species targets (no
        // cannibalism). Single-pool hits any creature (v1 predation, unchanged).
        let species_mode = self.sliders.species_mode;

        // Parallel scan: every per-i body only reads from xs/ys/actions/
        // cooldowns/energies/genome/species/grid (immutable across threads) and
        // writes exclusively to its own `picks[i]` slot.
        {
            let xs = &self.creatures.x[..n];
            let ys = &self.creatures.y[..n];
            let actions = &self.creatures.action_this_tick[..n];
            let cooldowns = &self.creatures.digestion_cooldown[..n];
            let energies = &self.creatures.energy[..n];
            let genome = &self.creatures.genome[..n];
            let species_ids = &self.creatures.species_id[..n];
            let grid = &self.grid;
            let picks = &mut self.scratch_attack_picks[..n];

            let scan_one = |i: usize, slot: &mut AttackPick| {
                if actions[i] != Action::Attack {
                    *slot = AttackPick::Skip;
                    return;
                }
                if cooldowns[i] > 0 {
                    *slot = AttackPick::Miss;
                    return;
                }
                let xi = xs[i];
                let yi = ys[i];
                let ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE * genome[i].body_size_factor();
                let self_species = species_ids[i];
                // First-valid-target attack: bail as soon as we find any
                // in-reach target. The per-cell iteration starts at `i % K_cell`
                // and wraps, so different attackers in the same stack pick
                // different cell-mates first — the bite target spreads across
                // the pile. See decisions/sim.md.
                let pick = grid.find_first_in_radius(xi, yi, max_range, i, |j| {
                    if j == i {
                        return false;
                    }
                    // v2.0 Wave 3a: in species mode, never attack same-species.
                    if species_mode && species_ids[j] == self_species {
                        return false;
                    }
                    let mut dx = xs[j] - xi;
                    let mut dy = ys[j] - yi;
                    // Toroidal minimum-image so a target just over the seam is in reach.
                    if wrap {
                        if dx > world_size * 0.5 {
                            dx -= world_size;
                        } else if dx < -world_size * 0.5 {
                            dx += world_size;
                        }
                        if dy > world_size * 0.5 {
                            dy -= world_size;
                        } else if dy < -world_size * 0.5 {
                            dy += world_size;
                        }
                    }
                    let d2 = dx * dx + dy * dy;
                    let rj = CREATURE_SIZE * BODY_RADIUS_PER_SIZE * genome[j].body_size_factor();
                    let reach = ri + rj;
                    d2 <= reach * reach
                });
                *slot = match pick {
                    Some(j) => {
                        // v2.0 Wave 2a: attack effectiveness scales with the
                        // attacker's diet (predator) + body size.
                        let g = &genome[i];
                        let effectiveness = (0.5 + g.diet) * g.body_size_factor();
                        AttackPick::Hit {
                            j: j as u32,
                            transfer: bite_frac * energies[j] * effectiveness,
                        }
                    }
                    None => AttackPick::Miss,
                };
            };

            #[cfg(feature = "threads")]
            {
                picks
                    .par_iter_mut()
                    .enumerate()
                    .for_each(|(i, slot)| scan_one(i, slot));
            }
            #[cfg(not(feature = "threads"))]
            {
                for (i, slot) in picks.iter_mut().enumerate() {
                    scan_one(i, slot);
                }
            }
        }

        // Sequential apply: now safe to touch the shared damage/gain lanes.
        for i in 0..n {
            match self.scratch_attack_picks[i] {
                AttackPick::Skip => {}
                AttackPick::Miss => {
                    self.scratch_attempted_eat[i] = true;
                }
                AttackPick::Hit { j, transfer } => {
                    let j = j as usize;
                    self.scratch_attempted_eat[i] = true;
                    self.scratch_damage[j] += transfer;
                    // Attacker gains the transferred energy.
                    self.scratch_gain[i] += transfer;
                    self.scratch_cooldown_set[i] = true;
                    self.scratch_got_a_bite[i] = true;
                    // v2.0 Wave 2a: ring-flash the attacker yellow now; the
                    // killed-victim red flash is decided after energy resolves.
                    self.creatures.set_flash(i, FlashTag::Attacked);
                }
            }
        }

        // Resolve cooldowns + energy. Capture each victim's pre-damage energy so
        // the kill-credit pass below can detect the lethal transition.
        for i in 0..n {
            if self.scratch_attempted_eat[i] && self.scratch_cooldown_set[i] {
                self.creatures.digestion_cooldown[i] = self.sliders.digestion_cooldown_ticks;
            }
            self.creatures.energy[i] += self.scratch_gain[i];
        }
        // v2.0 Wave 2a: kill-credit ring-flash. An attacker flashes red
        // ("killed") if the bite it landed drove its victim from alive to dead.
        // We check before subtracting the victim's damage so "alive → dead" is
        // unambiguous, then subtract. Done in two passes: first detect lethal
        // victims, then apply damage.
        for i in 0..n {
            if let AttackPick::Hit { j, .. } = self.scratch_attack_picks[i] {
                let j = j as usize;
                let victim_after = self.creatures.energy[j] - self.scratch_damage[j];
                if self.creatures.energy[j] > 0.0 && victim_after <= 0.0 {
                    self.creatures.set_flash(i, FlashTag::Killed);
                }
            }
        }
        for i in 0..n {
            self.creatures.energy[i] -= self.scratch_damage[i];
        }
    }

    pub(crate) fn energy_bookkeeping(&mut self) {
        // Idle upkeep base: UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + mouth_tax.
        let mouth_tax = self.sliders.mouth_tax;
        let up_base = UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + mouth_tax;
        let mult = self.sliders.upkeep_multiplier;
        let max_age = self.sliders.max_age;
        // v2.0 Wave 2a: per-creature energy cap scales with body_size (bigger
        // bodies hold more). Median genome (0.5 → factor 1.0) reproduces the cap.
        let energy_cap_base = self.sliders.energy_max;
        for i in 0..self.creatures.len() {
            // v2.0 Wave 2a: idle upkeep is multiplied by the metabolism trait.
            let metabolism = self.creatures.genome[i].metabolism_factor();
            let mut up = up_base * mult * metabolism;
            let age = self.creatures.age[i];
            if age > max_age {
                let excess = (age - max_age) as f32;
                let age_mult = PAST_LIFESPAN_MULT.powf(excess / 1000.0);
                up *= age_mult.min(1e6);
            }

            // v2.0 Wave 2a: extra per-tick upkeep for standing in a penalized
            // biome (the third movement-penalty effect), GENOME-modulated by the
            // creature's affinity traits. Added AFTER the age-multiplier so the
            // biome surcharge is a flat additive drain, not amplified by old age.
            let p = self.movement_penalty_for(i);
            up += K_BIOME_UPKEEP * p;

            self.creatures.energy[i] -= up;
            // v2.0 Wave 2a: body_size-scaled energy cap.
            let energy_cap = energy_cap_base * self.creatures.genome[i].body_size_factor();
            if self.creatures.energy[i] > energy_cap {
                self.creatures.energy[i] = energy_cap;
            }
            self.creatures.cumulative_upkeep[i] += up;
            if self.creatures.digestion_cooldown[i] > 0 {
                self.creatures.digestion_cooldown[i] -= 1;
            }
            // v2.0 Wave 3a: decrement the initiator-only mating cooldown
            // (unused / always 0 in single-pool mode).
            if self.creatures.mating_cooldown[i] > 0 {
                self.creatures.mating_cooldown[i] -= 1;
            }
            self.creatures.age[i] += 1;
        }
    }

    /// v2.0 Wave 2a: ring-flash decay. Decrements every creature's
    /// `flash_ticks`; when it reaches 0 the tag clears to `None`. Replaces the
    /// old action-EMA color update (color now derives from the genome at
    /// snapshot write; per-action highlight is the ring flash). Runs once per
    /// tick in the `tick.color_ema` span (kept the span name for the profiler).
    pub(crate) fn flash_decay(&mut self) {
        let n = self.creatures.len();
        for i in 0..n {
            if self.creatures.flash_ticks[i] > 0 {
                self.creatures.flash_ticks[i] -= 1;
                if self.creatures.flash_ticks[i] == 0 {
                    self.creatures.flash_tag[i] = FlashTag::None;
                }
            }
        }
    }

    pub(crate) fn collect_deaths(&mut self) {
        // S27: use promoted scratch_dead buffer instead of a per-call Vec.
        self.scratch_dead.clear();
        let n = self.creatures.len();
        for i in 0..n {
            if self.creatures.energy[i] <= 0.0 {
                self.scratch_dead.push(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    // v2.0 Wave 1a: world dims are runtime. `World::new` builds a walled 1200u
    // world (grass_dim 240), so these module-local constants reproduce the
    // pre-v2.0 walled-world values the position/grass tests assume.
    const WORLD_SIZE: f32 = 1200.0;
    const GRASS_GRID_DIM: usize = 240;

    // ---- E.25.d tests ----
    // NOTE: HallOfFame fields (biggest_ever, weirdest, longest_lived, last_survivor)
    // were deleted in D5. These tests cover simplified death-tracking.

    /// D3/D5: collect_deaths correctly identifies dead creatures.
    #[test]
    fn collect_deaths_identifies_zero_energy() {
        let mut w = World::new("d3-deaths");
        w.creatures.energy[0] = -1.0; // dead
        w.collect_deaths();
        assert_eq!(w.scratch_dead.len(), 1, "one creature should be dead");
        assert_eq!(w.scratch_dead[0], 0, "dead creature must be at index 0");
    }

    /// D3/D5: collect_deaths skips creatures with positive energy.
    #[test]
    fn collect_deaths_skips_alive() {
        let mut w = World::new("d3-alive");
        w.creatures.energy[0] = 100.0; // alive
        w.collect_deaths();
        assert_eq!(w.scratch_dead.len(), 0, "no dead creatures");
    }

    // ---- perf-2 tests ----

    /// perf-2 test 1: scratch_fx / scratch_fy are zeroed at the start of each tick.
    #[test]
    fn scratch_fx_fy_zeroed_at_tick_start() {
        let mut w = World::new("perf2-zeroing");
        for _ in 0..5 {
            w.tick_once();
        }
        let n = w.creatures.len();
        // Pre-poison with out-of-range sentinel values.
        w.scratch_fx.resize(n, 0.0);
        w.scratch_fy.resize(n, 0.0);
        for i in 0..n {
            w.scratch_fx[i] = 999.0;
            w.scratch_fy[i] = -999.0;
        }
        w.tick_once();
        for i in 0..w.creatures.len() {
            assert!(
                w.scratch_fx[i].abs() <= w.sliders.repulsion_max + 1e-3,
                "scratch_fx[{i}] = {} not reset (sentinel survived)",
                w.scratch_fx[i]
            );
            assert!(
                w.scratch_fy[i].abs() <= w.sliders.repulsion_max + 1e-3,
                "scratch_fy[{i}] = {} not reset (sentinel survived)",
                w.scratch_fy[i]
            );
        }
    }

    /// perf-2 test 2: scratch Vecs resize correctly with population each tick.
    /// perf-2: scratch Vecs stay consistent with population size each tick.
    /// D3/D9: split action not reachable via 3-logit decode until D9 collapses enum.
    /// Test verifies scratch vec sizing invariant across 50 ticks.
    #[test]
    fn scratch_grows_with_population() {
        let mut w = World::new("perf2-grow");
        // Give infinite energy so creatures survive; check scratch vec sizes each tick.
        w.creatures.energy[0] = SPLIT_THRESHOLD_DEFAULT + 10_000.0;

        for _ in 0..50 {
            w.tick_once();
            let n = w.creatures.len();
            let floor = n.saturating_sub(1);
            assert!(
                w.scratch_fx.len() >= floor,
                "scratch_fx {} < floor {}, n={}",
                w.scratch_fx.len(),
                floor,
                n
            );
            assert!(
                w.scratch_fy.len() >= floor,
                "scratch_fy {} < floor {}, n={}",
                w.scratch_fy.len(),
                floor,
                n
            );
            assert!(w.scratch_damage.len() >= floor);
            assert!(w.scratch_gain.len() >= floor);
            assert!(w.scratch_cooldown_set.len() >= floor);
            assert!(w.scratch_attempted_eat.len() >= floor);
            assert!(w.scratch_got_a_bite.len() >= floor);
        }
    }

    /// S31: a creature not crossing a seam with no overlapping neighbors should NOT
    /// trigger a grid rebuild (any_wrap = false AND any_repulsion = false).
    #[test]
    fn s31_no_wrap_no_repulsion_grid_still_valid() {
        let mut w = World::new("s31-wall-skip");
        w.creatures.x[0] = WORLD_SIZE * 0.5;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 0.0;
        w.creatures.vy[0] = 0.0;
        // D9: move_bias fields are deleted; creature has zero velocity by default.
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);
        w.apply_movement_and_repulsion();
        let mut found = false;
        w.grid
            .for_each_in_radius(WORLD_SIZE * 0.5, WORLD_SIZE * 0.5, 10.0, |_i| {
                found = true;
            });
        assert!(found, "creature must be found in grid after movement pass");
        assert!(
            (w.creatures.x[0] - WORLD_SIZE * 0.5).abs() < 1.0,
            "creature must remain near center: x={}",
            w.creatures.x[0]
        );
    }

    // ---- P1a toroidal physics tests ----

    /// D7: a creature with positive vx that would cross the right edge is CLAMPED
    /// to WORLD_SIZE - body_radius (walled world, no wrap).
    #[test]
    fn position_clamps_at_wall_edge() {
        let mut w = World::new("d7-wall-clamp");
        // v2.0 Wave 2a: pin median genome so body radius = 1.0 (factor 1.0),
        // matching this test's `ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE` bound.
        w.creatures.genome[0] = crate::creature::Genome::median();
        w.creatures.x[0] = WORLD_SIZE - 1.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 5.0; // would cross the right wall
        w.creatures.vy[0] = 0.0;
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        w.apply_movement_and_repulsion();

        let x = w.creatures.x[0];
        // D7: position must be clamped to [lo, hi] where lo=ri, hi=WORLD_SIZE-ri.
        let ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        assert!(
            x >= ri && x <= WORLD_SIZE - ri,
            "position must be within wall bounds [ri, WORLD_SIZE-ri]; x={x}"
        );
        // Creature stays near the right wall, NOT near x=0 (no wrap).
        assert!(
            x > WORLD_SIZE * 0.5,
            "clamped x should be near the right wall, not wrapped; x={x}"
        );
    }

    /// D7: walled world — split-jitter clamps child position to [0, WORLD_SIZE); no toroidal wrap.
    #[test]
    fn split_jitter_stays_in_bounds() {
        use crate::creature::Action;

        let mut w = World::new("p1a-jitter");
        w.creatures.x[0] = 5.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.energy[0] = SPLIT_THRESHOLD_DEFAULT + 100.0;
        w.creatures.action_this_tick[0] = Action::Split;
        w.handle_births();

        let n = w.creatures.len();
        for i in 1..n {
            let cx = w.creatures.x[i];
            let cy = w.creatures.y[i];
            assert!(
                (0.0..WORLD_SIZE).contains(&cx),
                "child x must be in [0, WORLD_SIZE); x = {cx}"
            );
            assert!(
                (0.0..WORLD_SIZE).contains(&cy),
                "child y must be in [0, WORLD_SIZE); y = {cy}"
            );
        }
    }

    /// D7: walled world — two creatures near opposite walls are repelled outward
    /// and clamped to [0, WORLD_SIZE) after movement; no seam-wrap occurs.
    #[test]
    fn repulsion_clamps_to_walls_after_movement() {
        use crate::brain::{Brain, NnTopology};

        let mut w = World::new("p1a-repulsion-seam");
        w.creatures.x[0] = 2.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 0.0;
        w.creatures.vy[0] = 0.0;

        let mut rng = SimRng::from_u64(123);
        let b2 = Brain::founder(&mut rng, NnTopology::legacy());
        let n_before = w.creatures.len();
        w.creatures.push(
            1,
            598.0,
            WORLD_SIZE * 0.5,
            START_ENERGY_DEFAULT,
            0,
            b2,
            crate::creature::Genome::median(),
            0,
        );
        w.creatures.vx[n_before] = 0.0;
        w.creatures.vy[n_before] = 0.0;

        w.creatures.x[0] = 0.5;
        w.creatures.x[n_before] = WORLD_SIZE - 0.5;

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);
        w.scratch_fx.resize(w.creatures.len(), 0.0);
        w.scratch_fy.resize(w.creatures.len(), 0.0);

        w.apply_movement_and_repulsion();

        for i in 0..w.creatures.len() {
            assert!(
                w.creatures.x[i] >= 0.0 && w.creatures.x[i] < WORLD_SIZE,
                "creature {i} x={} not in [0, WORLD_SIZE) after repulsion",
                w.creatures.x[i]
            );
        }
    }

    // ---- P1e graze tests ----

    /// P1e test 1: barren world → creature with Action::Graze gains zero energy.
    #[test]
    fn graze_no_overlap_yields_zero_gain() {
        let mut w = World::new("graze-barren");
        w.grass.fill_density(0.0);
        w.creatures.action_this_tick[0] = Action::Graze;
        let before = w.creatures.energy[0];
        w.graze();
        assert_eq!(
            before, w.creatures.energy[0],
            "energy must be unchanged when grass is empty"
        );
    }

    /// P1e test 2: creature overlapping seeded cells sums deltas correctly.
    /// D3: CREATURE_SIZE=1.0, BODY_RADIUS_PER_SIZE=1.0 → body radius=1.0.
    /// We position the creature exactly at cell (30,30)'s center so it
    /// overlaps at least one cell.
    #[test]
    fn graze_n_cell_overlap_sums_capped_deltas() {
        use crate::constants::{
            GRASS_BITES_PER_BLOCK_DEFAULT, GRASS_CELL_SIZE, GRASS_ENERGY_PER_BITE_DEFAULT,
            GRASS_MAX,
        };

        let mut w = World::new("graze-patch");
        // v2.0 Wave 2a: median genome (radius 1.0) + pure-grazer diet (full yield).
        let mut g0 = crate::creature::Genome::median();
        g0.diet = 0.0;
        w.creatures.genome[0] = g0;
        // Position creature at cell (30,30) center.
        let cell_ix: usize = 30;
        let cell_iy: usize = 30;
        let cx = (cell_ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (cell_iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        // Seed the entire grass grid with GRASS_MAX.
        w.grass.fill_density(GRASS_MAX);

        // Count how many cells overlap using CREATURE_SIZE body radius.
        let ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        let mut overlap_count = 0usize;
        w.grass.for_each_cell_overlapping_circle(cx, cy, ri, |_| {
            overlap_count += 1;
        });
        assert!(
            overlap_count > 0,
            "creature positioned at cell center with CREATURE_SIZE should overlap at least one cell (ri={ri}, cell_size={})",
            GRASS_CELL_SIZE
        );

        let energy_before = w.creatures.energy[0];
        w.graze();
        let energy_after = w.creatures.energy[0];
        let expected_gain = overlap_count as f32 * GRASS_ENERGY_PER_BITE_DEFAULT;

        assert!(
            (energy_after - energy_before - expected_gain).abs() < 1e-4,
            "energy gain={} expected={} (overlap_count={}, per_bite={})",
            energy_after - energy_before,
            expected_gain,
            overlap_count,
            GRASS_ENERGY_PER_BITE_DEFAULT
        );
        // Confirm grass density actually decreased at the center cell.
        let _density_chunk = GRASS_MAX / GRASS_BITES_PER_BLOCK_DEFAULT as f32;
        let center_cell = cell_iy * GRASS_GRID_DIM + cell_ix;
        // Stage 3: byte-exact assertion.
        // encode(GRASS_MAX=1.0) = 255; chunk = 0.5 → encode(0.5) = 128.
        // post_byte = 255 - 128 = 127.
        assert_eq!(
            w.grass.dget_u8(center_cell),
            127u8,
            "center cell must drop to byte 127 after one bite (255 - 128)"
        );
    }

    /// P1e test 3: creature at world seam (x ≈ WORLD_SIZE) overlaps cells on both sides.
    #[test]
    fn graze_wraps_across_world_seam() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_MAX};

        let mut w = World::new("graze-seam");
        let cx = 0.0_f32;
        let cy = WORLD_SIZE * 0.5;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        // Walled world: a creature at x=0 cannot reach the east seam.
        let iy = ((cy / GRASS_CELL_SIZE).floor() as usize).min(GRASS_GRID_DIM - 1);
        let east_cell = iy * GRASS_GRID_DIM + (GRASS_GRID_DIM - 1);
        let west_cell = iy * GRASS_GRID_DIM;
        w.grass.dset(east_cell, GRASS_MAX);
        w.grass.dset(west_cell, GRASS_MAX);

        w.graze();

        // With the AABB overlap fix, a creature at cx=0 always touches west_cell (ix=0)
        // because its box [0, GRASS_CELL_SIZE] contains the creature position
        // (nearest point = 0, dist = 0). The east cell (ix=dim-1) is far away and
        // not reached (walled world). West cell must be drained.
        let west_drained = w.grass.dget(west_cell) < GRASS_MAX - 1e-6;
        assert!(
            west_drained,
            "west seam cell (ix=0) must be grazed; creature at x=0 is inside cell box [0,5]; density={}",
            w.grass.dget(west_cell)
        );
    }

    /// P1e test 4: single cell at density 1.0 → gain == GRASS_ENERGY_PER_BITE_DEFAULT.
    #[test]
    fn graze_per_cell_cap_holds() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_ENERGY_PER_BITE_DEFAULT, GRASS_MAX};

        let mut w = World::new("graze-cap");
        let mut g0 = crate::creature::Genome::median();
        g0.diet = 0.0; // pure grazer → full graze yield (matches pre-genome)
        w.creatures.genome[0] = g0;
        let ix = 2usize;
        let iy = 2usize;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        let cell_idx = iy * GRASS_GRID_DIM + ix;
        w.grass.dset(cell_idx, GRASS_MAX);

        let energy_before = w.creatures.energy[0];
        w.graze();
        let gained = w.creatures.energy[0] - energy_before;

        assert!(
            (gained - GRASS_ENERGY_PER_BITE_DEFAULT).abs() < 1e-4,
            "gained={gained} expected={GRASS_ENERGY_PER_BITE_DEFAULT}"
        );
        // Stage 3: byte-exact assertion.
        // GRASS_BITES_PER_BLOCK_DEFAULT=2, chunk = GRASS_MAX/2 = 0.5.
        // encode(GRASS_MAX=1.0) = 255; encode(0.5) = 128; post_byte = 255 - 128 = 127.
        assert_eq!(
            w.grass.dget_u8(cell_idx),
            127u8,
            "density must drop to byte 127 after one bite (255 - 128); got byte {}",
            w.grass.dget_u8(cell_idx)
        );
    }

    #[test]
    fn graze_uses_live_grass_bites_per_block() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_ENERGY_PER_BITE_DEFAULT, GRASS_MAX};

        let mut w = World::new("graze-live-bites");
        w.sliders.grass_bites_per_block = 4;
        let mut genome = crate::creature::Genome::median();
        genome.diet = 0.0;
        w.creatures.genome[0] = genome;

        let ix = 5usize;
        let iy = 5usize;
        w.creatures.x[0] = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.y[0] = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.action_this_tick[0] = Action::Graze;
        let cell_idx = iy * GRASS_GRID_DIM + ix;
        w.grass.dset(cell_idx, GRASS_MAX);

        let energy_before = w.creatures.energy[0];
        w.graze();

        assert!(
            (w.creatures.energy[0] - energy_before - GRASS_ENERGY_PER_BITE_DEFAULT).abs() < 1e-4
        );
        assert_eq!(
            w.grass.dget_u8(cell_idx),
            191,
            "bites_per_block=4 must drain one encoded quarter-block (64 bytes)"
        );
    }

    /// P1e test 5: single ripe cell → gain == GRASS_ENERGY_PER_BITE_DEFAULT.
    #[test]
    fn graze_constant_efficiency_produces_correct_gain() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_ENERGY_PER_BITE_DEFAULT, GRASS_MAX};

        let mut w = World::new("graze-const-eff");
        let mut g0 = crate::creature::Genome::median();
        g0.diet = 0.0; // pure grazer → full graze yield (matches pre-genome)
        w.creatures.genome[0] = g0;
        let ix = 5usize;
        let iy = 5usize;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        let cell_idx = iy * GRASS_GRID_DIM + ix;
        w.grass.dset(cell_idx, GRASS_MAX);

        let energy_before = w.creatures.energy[0];
        w.graze();
        let gained = w.creatures.energy[0] - energy_before;
        assert!(
            (gained - GRASS_ENERGY_PER_BITE_DEFAULT).abs() < 1e-4,
            "gained={gained} expected={GRASS_ENERGY_PER_BITE_DEFAULT}"
        );
    }

    /// P1e test 6: bite-count invariant — energy gained and density consumed
    /// agree on the same successful-bite count: gain/per_bite == consumed/chunk.
    #[test]
    fn graze_conserves_bite_count_invariant() {
        use crate::constants::{
            GRASS_BITES_PER_BLOCK_DEFAULT, GRASS_ENERGY_PER_BITE_DEFAULT, GRASS_MAX,
        };

        let mut w = World::new("graze-conserve");
        let mut g0 = crate::creature::Genome::median();
        g0.diet = 0.0; // pure grazer → full graze yield (matches pre-genome)
        w.creatures.genome[0] = g0;
        w.grass.fill_density(GRASS_MAX);

        let cx = WORLD_SIZE * 0.5;
        let cy = WORLD_SIZE * 0.5;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        // v2.0.2 Stream 1a: sum decoded u8 densities.
        let density_before: f32 = (0..w.grass.density.len()).map(|c| w.grass.dget(c)).sum();
        let energy_before = w.creatures.energy[0];

        w.graze();

        let density_after: f32 = (0..w.grass.density.len()).map(|c| w.grass.dget(c)).sum();
        let energy_after = w.creatures.energy[0];

        let density_consumed = density_before - density_after;
        let energy_gained = energy_after - energy_before;
        let density_chunk = GRASS_MAX / GRASS_BITES_PER_BLOCK_DEFAULT as f32;
        let bites_from_energy = energy_gained / GRASS_ENERGY_PER_BITE_DEFAULT;
        let bites_from_density = density_consumed / density_chunk;
        // Stage 3: byte-exact assertion.
        // Every ripe cell starts at byte 255; chunk = 0.5 → byte 128; post = byte 127.
        // decode(255) - decode(127) = (128/255) * GRASS_MAX.
        // density_chunk = 0.5 (exact f32 for GRASS_MAX/2).
        // bites_from_density per cell = (128/255) / 0.5 = 256/255.
        // bites_from_energy per cell = 1.0 (exact, energy = GRASS_ENERGY_PER_BITE_DEFAULT).
        // Exact systematic error = (256/255 - 1) * N = N/255, where N = bites_from_energy.
        let n = bites_from_energy.round();
        let exact_density_side = n * (256.0_f32 / 255.0_f32);
        assert!(
            (bites_from_density - exact_density_side).abs() < 1e-3,
            "bite count from density ({bites_from_density}) must equal N*256/255 = {exact_density_side} \
             (N={n} bites, each draining 128/255 density / chunk 0.5)"
        );

        // Sanity: creature should have gained at least one bite's worth.
        let ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        let mut overlap_count = 0usize;
        w.grass
            .for_each_cell_overlapping_circle(cx, cy, ri, |_| overlap_count += 1);
        let expected_gain = overlap_count as f32 * GRASS_ENERGY_PER_BITE_DEFAULT;
        assert!(
            (energy_gained - expected_gain).abs() < 1e-3,
            "energy_gained={energy_gained} expected={expected_gain} (overlap={overlap_count})"
        );
    }

    // ---- P3a tests ----

    /// P3a test: default eat_bite_fraction is 0.5.
    #[test]
    fn p3a_dev_slider_eat_bite_fraction_default() {
        let w = World::new("p3a-default");
        assert!(
            (w.sliders.eat_bite_fraction - 0.5).abs() < f32::EPSILON,
            "eat_bite_fraction default must be 0.5; got {}",
            w.sliders.eat_bite_fraction
        );
    }

    /// P3a test: basic bite transfer — adjacent predator/prey, armor=0, eff=1.0, bite_frac=0.5.
    /// v2.0 Wave 2a: median-genome attacker (diet=0.5, body_size=0.5 → factor
    /// 1.0) gives effectiveness `(0.5 + 0.5) * 1.0 = 1.0`, reproducing the
    /// pre-genome transfer (0.5 × 100 × 1.0 = 50).
    #[test]
    fn p3a_attack_bite_basic_transfer() {
        use crate::brain::{Brain, NnTopology};

        let mut w = World::new("p3a-basic");
        w.creatures.energy[0] = 100.0;
        w.creatures.digestion_cooldown[0] = 0;
        w.creatures.action_this_tick[0] = Action::Attack;
        // Pin the attacker's genome to median so effectiveness == 1.0.
        w.creatures.genome[0] = crate::creature::Genome::median();
        w.sliders.eat_bite_fraction = 0.5;

        // Add prey (creature 1) adjacent.
        let mut rng = SimRng::from_u64(42);
        let prey_brain = Brain::founder(&mut rng, NnTopology::legacy());
        let pred_x = w.creatures.x[0];
        let pred_y = w.creatures.y[0];
        // Place prey within bite reach (0.0 since bite_reach = 0).
        w.creatures.push(
            1,
            pred_x + 1.5,
            pred_y,
            100.0,
            0,
            prey_brain,
            crate::creature::Genome::median(),
            0,
        );

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        let pred_energy_before = w.creatures.energy[0];
        let prey_energy_before = w.creatures.energy[1];

        w.attack();

        let pred_energy_after = w.creatures.energy[0];
        let prey_energy_after = w.creatures.energy[1];

        // transfer = 0.5 * 100.0 * effectiveness(=1.0 at median genome) = 50.0
        let expected_gain = 50.0_f32;
        let prey_loss = prey_energy_before - prey_energy_after;
        let pred_gain = pred_energy_after - pred_energy_before;

        assert!(
            (prey_loss - 50.0).abs() < 1e-4,
            "prey should lose 50.0; lost {prey_loss}"
        );
        assert!(
            (pred_gain - expected_gain).abs() < 1e-3,
            "predator should gain {expected_gain}; gained {pred_gain}"
        );
    }

    /// P3a test: digestion cooldown still gates bites — only attempt cost charged, no bite.
    #[test]
    fn p3a_attack_cooldown_still_gates() {
        use crate::brain::{Brain, NnTopology};

        let mut w = World::new("p3a-cooldown");
        w.creatures.energy[0] = 100.0;
        // Set active cooldown so bite is gated.
        w.creatures.digestion_cooldown[0] = 10;
        w.creatures.action_this_tick[0] = Action::Attack;
        w.sliders.eat_bite_fraction = 0.5;

        let mut rng = SimRng::from_u64(7);
        let prey_brain = Brain::founder(&mut rng, NnTopology::legacy());
        let pred_x = w.creatures.x[0];
        let pred_y = w.creatures.y[0];
        w.creatures.push(
            1,
            pred_x + 1.5,
            pred_y,
            100.0,
            0,
            prey_brain,
            crate::creature::Genome::median(),
            0,
        );

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        let prey_energy_before = w.creatures.energy[1];
        let pred_energy_before = w.creatures.energy[0];

        w.attack();

        let prey_change = (w.creatures.energy[1] - prey_energy_before).abs();
        let pred_change = pred_energy_before - w.creatures.energy[0];

        // Prey must be unchanged (no bite landed due to cooldown).
        assert!(
            prey_change < 1e-6,
            "prey energy must be unchanged when predator is in cooldown; change = {prey_change}"
        );
        // Predator energy unchanged (no attempt cost in v1.9+).
        assert!(
            pred_change.abs() < 1e-4,
            "predator energy must be unchanged when bite gated; lost {pred_change}"
        );
    }

    // ---- v2.0 Wave 2a ring-flash decay tests ----

    /// A lit ring-flash counts down each tick and clears to `None` at 0.
    #[test]
    fn flash_decay_counts_down_and_clears() {
        use crate::creature::{FlashTag, FLASH_TICKS};
        let mut w = World::new("wave2a-flash-decay");
        // Founder starts with a Born flash from spawn.
        assert_eq!(w.creatures.flash_tag[0], FlashTag::Born);
        assert_eq!(w.creatures.flash_ticks[0], FLASH_TICKS);
        // Decay FLASH_TICKS times → cleared.
        for t in 1..=FLASH_TICKS {
            w.flash_decay();
            let remaining = FLASH_TICKS - t;
            assert_eq!(w.creatures.flash_ticks[0], remaining, "after {t} decays");
            if remaining == 0 {
                assert_eq!(w.creatures.flash_tag[0], FlashTag::None, "tag clears at 0");
            } else {
                assert_eq!(
                    w.creatures.flash_tag[0],
                    FlashTag::Born,
                    "tag persists while lit"
                );
            }
        }
        // Idempotent at 0.
        w.flash_decay();
        assert_eq!(w.creatures.flash_ticks[0], 0);
        assert_eq!(w.creatures.flash_tag[0], FlashTag::None);
    }

    /// A successful graze lights the green ("grazed") flash.
    #[test]
    fn graze_sets_green_flash() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_MAX};
        use crate::creature::FlashTag;
        let mut w = World::new("wave2a-graze-flash");
        let cx = (10.5) * GRASS_CELL_SIZE;
        let cy = (10.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;
        w.grass.fill_density(GRASS_MAX);
        w.graze();
        assert_eq!(
            w.creatures.flash_tag[0],
            FlashTag::Grazed,
            "a consuming graze must light the green flash"
        );
    }
}
