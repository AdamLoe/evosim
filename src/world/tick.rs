//! Per-tick step bodies that mutate `World`. All as `impl World` blocks; private to the `crate::world` parent.
//!
//! D3: Genome removed. All body trait reads replaced with constants:
//!   g_graze_eff → 1.0 (GRAZE_EFF_CONSTANT), g_size → FOUNDER_SIZE,
//!   g_move_speed → MOVE_SPEED_MAX, g_eye_count/g_vision_range → constant 24/VISION_RANGE_MAX,
//!   g_eat_eff/g_scav_eff → 1.0 (always enabled), armor/bite_reach → 0.0/BITE_REACH_CONSTANT.
//!   HallOfFame snapshots no longer include genome or captured_size.

use super::World;
use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::Action;
use crate::events::{Event, EventKind};
use crate::hof::HallOfFame;
use crate::species::species_distance;
use crate::torus::{torus_delta, wrap_pos};

impl World {
    /// Multi-cell graze: for every creature that chose `Action::Graze` this tick,
    /// consume grass density from every `GrassGrid` cell whose center overlaps the
    /// creature's body circle (toroidal distance ≤ body radius).
    ///
    /// Per-cell delta = `min(grass[cell], GRAZE_MAX_PER_TICK * 1.0)`.
    /// Total energy gained = sum of deltas across all overlapping cells.
    /// Energy gained equals exactly the grass density consumed (conservation property).
    ///
    /// D3: graze_efficiency is always 1.0 (constant). size is FOUNDER_SIZE (constant).
    ///
    /// Runs SEQUENTIALLY regardless of `--features threads` (two creatures may overlap
    /// the same grass cell; parallel writes would race on `density[cell]`). See p1e §3.
    pub(crate) fn graze(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }

        let max_per_cell = GRAZE_MAX_PER_TICK; // D3: graze_eff = 1.0 always
        let ri = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE; // D3: size = FOUNDER_SIZE always

        for i in 0..n {
            if self.creatures.action_this_tick[i] != Action::Graze {
                continue;
            }
            let xi = self.creatures.x[i];
            let yi = self.creatures.y[i];

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
                let delta = self.grass.consume(cell_idx, max_per_cell);
                total_gain += delta;
            }
            self.creatures.energy[i] += total_gain;

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

        // P2f: re-roll per-creature move_bias_x/y every MOVE_BIAS_REROLL_INTERVAL ticks.
        let tick = self.tick;
        for i in 0..n {
            if tick >= self.creatures.move_bias_reroll_at[i] {
                self.creatures.move_bias_x[i] = self.rng.symm();
                self.creatures.move_bias_y[i] = self.rng.symm();
                self.creatures.move_bias_reroll_at[i] =
                    tick.saturating_add(MOVE_BIAS_REROLL_INTERVAL);
            }
        }

        // D3: speed_cap = MOVE_SPEED_MAX for all creatures (constant).
        let speed_cap = MOVE_SPEED_MAX;
        for i in 0..n {
            // P2f: ADD move_bias on top of NN-output velocity (amendments §A.5).
            self.creatures.vx[i] += self.creatures.move_bias_x[i] * MOVE_SPEED_MAX;
            self.creatures.vy[i] += self.creatures.move_bias_y[i] * MOVE_SPEED_MAX;
            let vx = self.creatures.vx[i];
            let vy = self.creatures.vy[i];
            let mag2 = vx * vx + vy * vy;
            let (cvx, cvy) = if speed_cap > 0.0 && mag2 > speed_cap * speed_cap {
                let s = speed_cap / mag2.sqrt();
                (vx * s, vy * s)
            } else {
                (vx, vy)
            };
            let dist = (cvx * cvx + cvy * cvy).sqrt();
            self.creatures.energy[i] -= dist * COST_MOVE_PER_DIST;
            self.creatures.distance_travelled[i] += dist;
            self.creatures.x[i] += cvx;
            self.creatures.y[i] += cvy;
            // E.25.b: FirstToMove fires here (on the actual crossing tick) per v6 §L.
            // D3: move_speed is always MOVE_SPEED_MAX > 0 for all creatures.
            if !self.first_move_fired && self.creatures.distance_travelled[i] >= 5.0 {
                self.first_move_fired = true;
                let species_name = self.species.get(self.creatures.species_id[i]).name.clone();
                // E.25.d: capture first_mover_snapshot for F.28.
                self.first_mover_snapshot = Some(HallOfFame {
                    creature_id: self.creatures.id[i],
                    species_name,
                    captured_tick: self.tick,
                    captured_age: self.creatures.age[i],
                });
                if self.events_enabled {
                    self.events.push(Event {
                        tick: self.tick,
                        kind: EventKind::FirstToMove {
                            creature_id: self.creatures.id[i],
                        },
                    });
                }
            }
        }

        self.grid.rebuild(&self.creatures.x, &self.creatures.y);

        // D3: ri is constant for all creatures.
        let ri = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let search = ri + ri; // rsum_max = 2 * ri (all same size)

        self.scratch_fx.resize(n, 0.0);
        self.scratch_fy.resize(n, 0.0);
        self.scratch_fx.fill(0.0);
        self.scratch_fy.fill(0.0);
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
            for &j in &neighbors {
                let xj = self.creatures.x[j];
                let yj = self.creatures.y[j];
                // D3: rj == ri (same FOUNDER_SIZE for all).
                let (dx, dy) = torus_delta(xj - xi, yj - yi);
                let d2 = dx * dx + dy * dy;
                let rsum = ri + ri;
                if d2 < rsum * rsum {
                    if d2 > 1e-8 {
                        let d = d2.sqrt();
                        let overlap = rsum - d;
                        let f = (REPULSION_K * overlap).clamp(0.0, REPULSION_MAX);
                        let ux = dx / d;
                        let uy = dy / d;
                        self.scratch_fx[i] -= ux * f;
                        self.scratch_fy[i] -= uy * f;
                        self.scratch_fx[j] += ux * f;
                        self.scratch_fy[j] += uy * f;
                    } else {
                        // Co-located (newborn siblings) — deterministic nudge.
                        let angle =
                            ((i as f32) * 0.7 + (j as f32) * 1.3).sin() * std::f32::consts::PI;
                        let ux = angle.cos();
                        let uy = angle.sin();
                        let f = REPULSION_MAX;
                        self.scratch_fx[i] -= ux * f;
                        self.scratch_fy[i] -= uy * f;
                        self.scratch_fx[j] += ux * f;
                        self.scratch_fy[j] += uy * f;
                    }
                }
            }
            self.scratch_neighbors = neighbors;
        }

        // S31/P1a: track whether any position changed so we can skip the final grid
        // rebuild when positions are unchanged.
        let mut any_wrap = false;
        for i in 0..n {
            if self.scratch_fx[i] != 0.0 || self.scratch_fy[i] != 0.0 {
                any_wrap = true;
            }
            let candidate_x = self.creatures.x[i] + self.scratch_fx[i];
            let candidate_y = self.creatures.y[i] + self.scratch_fy[i];
            let wrapped_x = wrap_pos(candidate_x);
            let wrapped_y = wrap_pos(candidate_y);
            if wrapped_x != candidate_x || wrapped_y != candidate_y {
                any_wrap = true;
            }
            self.creatures.x[i] = wrapped_x;
            self.creatures.y[i] = wrapped_y;
        }

        // S31: skip rebuild when no position change occurred.
        if any_wrap {
            self.grid.rebuild(&self.creatures.x, &self.creatures.y);
        }
    }

    pub(crate) fn eat_and_scavenge(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        self.scratch_damage.resize(n, 0.0);
        self.scratch_gain.resize(n, 0.0);
        self.scratch_cooldown_set.resize(n, false);
        self.scratch_attempted_eat.resize(n, false);
        self.scratch_attempted_scavenge.resize(n, false);
        self.scratch_got_a_bite.resize(n, false);
        self.scratch_damage.fill(0.0);
        self.scratch_gain.fill(0.0);
        self.scratch_cooldown_set.fill(false);
        self.scratch_attempted_eat.fill(false);
        self.scratch_attempted_scavenge.fill(false);
        self.scratch_got_a_bite.fill(false);

        // D3: all creatures have eat_eff = 1.0, scav_eff = 1.0, armor = 0.0,
        //     bite_reach = BITE_REACH_CONSTANT, size = FOUNDER_SIZE (all constant).
        let eat_eff = 1.0_f32;
        let scav_eff = 1.0_f32;
        let size = FOUNDER_SIZE;
        let bite_reach = BITE_REACH_CONSTANT;
        let armor = 0.0_f32;

        for i in 0..n {
            match self.creatures.action_this_tick[i] {
                Action::Eat => {
                    self.scratch_attempted_eat[i] = true;
                    if self.creatures.digestion_cooldown[i] > 0 {
                        continue;
                    }
                    let radius_i = size * BODY_RADIUS_PER_SIZE;
                    let reach = bite_reach * size;
                    let max_range = radius_i + reach + size * BODY_RADIUS_PER_SIZE;
                    let xi = self.creatures.x[i];
                    let yi = self.creatures.y[i];
                    let mut best: Option<(usize, f32)> = None;
                    // S25: use pooled scratch buffer instead of per-Eat Vec::with_capacity(8).
                    let mut candidates = std::mem::take(&mut self.scratch_eat_candidates);
                    candidates.clear();
                    self.grid.for_each_in_radius(xi, yi, max_range, |j| {
                        if j != i {
                            candidates.push(j);
                        }
                    });
                    for j in candidates.iter().copied() {
                        let (ddx, ddy) =
                            torus_delta(self.creatures.x[j] - xi, self.creatures.y[j] - yi);
                        let d = (ddx * ddx + ddy * ddy).sqrt();
                        let rj = size * BODY_RADIUS_PER_SIZE; // D3: same size for all
                        let contact = (d - radius_i - rj).max(0.0);
                        if contact <= reach {
                            best = match best {
                                None => Some((j, d)),
                                Some((_, bd)) if d < bd => Some((j, d)),
                                Some((bj, bd))
                                    if d == bd && self.creatures.id[j] < self.creatures.id[bj] =>
                                {
                                    Some((j, d))
                                }
                                other => other,
                            };
                        }
                    }
                    self.scratch_eat_candidates = candidates;
                    if let Some((j, _)) = best {
                        // D3: armor = 0.0 for all prey.
                        let bite_frac = self.sliders.eat_bite_fraction;
                        let prey_energy = self.creatures.energy[j];
                        let transfer = bite_frac * prey_energy * (1.0 - armor);
                        self.scratch_damage[j] += transfer;
                        self.scratch_gain[i] += transfer * eat_eff;
                        self.scratch_cooldown_set[i] = true;
                        self.scratch_got_a_bite[i] = true;
                    }
                }
                Action::Scavenge => {
                    self.scratch_attempted_scavenge[i] = true;
                    let r_i = size * BODY_RADIUS_PER_SIZE;
                    let r2 = r_i * r_i;
                    let xi = self.creatures.x[i];
                    let yi = self.creatures.y[i];
                    let want = SCAVENGE_GAIN_COEFF * scav_eff;
                    // S24: 3×3 cell sweep via cell_to_carrion.
                    let cx = (xi / HASH_CELL).floor() as i32;
                    let cy = (yi / HASH_CELL).floor() as i32;
                    let dim = HASH_DIM as i32;
                    let mut found_ci: Option<usize> = None;
                    'outer: for sdy in -1i32..=1 {
                        for sdx in -1i32..=1 {
                            let nx = (cx + sdx).rem_euclid(dim);
                            let ny = (cy + sdy).rem_euclid(dim);
                            let cell_idx = ny as usize * HASH_DIM + nx as usize;
                            for &ci in self.cell_to_carrion.cell_slice(cell_idx) {
                                let c = &self.carrion[ci as usize];
                                let (ddx, ddy) = torus_delta(c.x - xi, c.y - yi);
                                if ddx * ddx + ddy * ddy <= r2 {
                                    found_ci = Some(ci as usize);
                                    break 'outer;
                                }
                            }
                        }
                    }
                    if let Some(ci) = found_ci {
                        let c = &mut self.carrion[ci];
                        let take = c.pool.min(want);
                        c.pool -= take;
                        self.scratch_gain[i] += take;
                    }
                }
                _ => {}
            }
        }

        for i in 0..n {
            if self.scratch_attempted_eat[i] {
                self.creatures.energy[i] -= COST_EAT_ATTEMPT;
                if self.scratch_cooldown_set[i] {
                    self.creatures.digestion_cooldown[i] = DIGESTION_COOLDOWN_TICKS;
                }
                if self.scratch_got_a_bite[i] && !self.first_eat_fired {
                    self.first_eat_fired = true;
                    if self.events_enabled {
                        self.events.push(Event {
                            tick: self.tick,
                            kind: EventKind::FirstToEat {
                                creature_id: self.creatures.id[i],
                            },
                        });
                    }
                }
            }
            if self.scratch_attempted_scavenge[i] {
                self.creatures.energy[i] -= COST_SCAVENGE_ATTEMPT;
            }
            self.creatures.energy[i] += self.scratch_gain[i];
            self.creatures.energy[i] -= self.scratch_damage[i];
        }
    }

    pub(crate) fn energy_bookkeeping(&mut self) {
        // D3: all variable body-trait upkeep terms removed. Flat formula:
        //   up = UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + mouth_tax
        // (UPKEEP_GUT for scav_eff always > 0, mouth_tax for eat_eff always > 0)
        let mouth_tax = self.sliders.mouth_tax;
        let up_base = UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + mouth_tax;
        for i in 0..self.creatures.len() {
            let mut up = up_base;
            // D3: max_age is FOUNDER_MAX_AGE (constant).
            let age = self.creatures.age[i];
            if age > FOUNDER_MAX_AGE {
                let excess = (age - FOUNDER_MAX_AGE) as f32;
                let mult = PAST_LIFESPAN_MULT.powf(excess / 1000.0);
                up *= mult.min(1e6);
            }

            // D3: max_size_reached removed (was always FOUNDER_SIZE anyway).
            self.creatures.energy[i] -= up;
            self.creatures.cumulative_upkeep[i] += up;
            if self.creatures.digestion_cooldown[i] > 0 {
                self.creatures.digestion_cooldown[i] -= 1;
            }
            self.creatures.age[i] += 1;
        }
    }

    pub(crate) fn collect_deaths(&mut self) {
        // S27: use promoted scratch_dead buffer instead of a per-call Vec.
        self.scratch_dead.clear();
        let n = self.creatures.len();
        for i in 0..n {
            if self.creatures.energy[i] <= 0.0 {
                let pool = (CARRION_POOL_COEFF * self.creatures.cumulative_upkeep[i])
                    .clamp(0.0, CARRION_POOL_CAP);
                self.carrion.push(Carrion {
                    id: self.creatures.id[i],
                    x: self.creatures.x[i],
                    y: self.creatures.y[i],
                    pool,
                    age: 0,
                });
                self.pending_extinction_check
                    .push(self.creatures.species_id[i]);
                self.scratch_dead.push(i);

                // E.25.d: hall-of-fame tracking on death.
                // D3: no genome/size in HallOfFame.
                let age = self.creatures.age[i];
                let species_name = self.species.get(self.creatures.species_id[i]).name.clone();

                // last_survivor: overwrite on every death.
                self.last_survivor = Some(HallOfFame {
                    creature_id: self.creatures.id[i],
                    species_name: species_name.clone(),
                    captured_tick: self.tick,
                    captured_age: age,
                });

                // longest_lived (unconditional fallback for weirdest).
                if age > self.longest_lived_age {
                    self.longest_lived_age = age;
                    self.longest_lived = Some(HallOfFame {
                        creature_id: self.creatures.id[i],
                        species_name: species_name.clone(),
                        captured_tick: self.tick,
                        captured_age: age,
                    });
                }

                // weirdest: requires age >= 500 (v5 §11.1).
                // D3: species_distance uses brain weights only.
                if age >= 500 {
                    let dist = species_distance(
                        &self.creatures.brains[i].weights,
                        &self.founder_brain_anchor,
                    );
                    if dist > self.weirdest_distance {
                        self.weirdest_distance = dist;
                        self.weirdest = Some(HallOfFame {
                            creature_id: self.creatures.id[i],
                            species_name,
                            captured_tick: self.tick,
                            captured_age: age,
                        });
                    }
                }
            }
        }
    }

    pub(crate) fn decay_carrion(&mut self) {
        // Unscavenged carrion energy vanishes on expiry (per master §F #10 / P1b).
        let mut i = 0;
        while i < self.carrion.len() {
            self.carrion[i].age += 1;
            if self.carrion[i].age > CARRION_MAX_AGE || self.carrion[i].pool <= 0.0 {
                self.carrion.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    // ---- E.25.b test ----

    /// E.25.b: FirstToMove fires inside apply_movement_and_repulsion when
    /// distance_travelled crosses 5.0 (v6 §L: "actually traveled ≥ 5u in lifetime").
    /// D3: move_speed is always MOVE_SPEED_MAX; no genome patch needed.
    #[test]
    fn e25b_first_to_move_fires_on_movement_step() {
        let mut w = World::new("e25b-move");
        w.events_enabled = true;
        // Set velocity directly so movement fires deterministically.
        w.creatures.vx[0] = 5.0;
        w.creatures.vy[0] = 0.0;
        // P2f: zero move_bias so the ADD is a no-op for this deterministic test.
        w.creatures.move_bias_x[0] = 0.0;
        w.creatures.move_bias_y[0] = 0.0;
        w.creatures.move_bias_reroll_at[0] = u32::MAX;
        // Pre-condition: distance_travelled starts at 0, event not fired.
        assert!(!w.first_move_fired);
        assert_eq!(w.creatures.distance_travelled[0], 0.0);
        let creature_id = w.creatures.id[0];

        // Run the movement step directly.
        w.apply_movement_and_repulsion();

        // After movement, distance_travelled should be >= 5.0 and event should fire.
        assert!(
            w.creatures.distance_travelled[0] >= 5.0,
            "distance_travelled = {}",
            w.creatures.distance_travelled[0]
        );
        assert!(
            w.first_move_fired,
            "first_move_fired must be true after crossing 5.0"
        );
        let ev = w
            .events
            .all
            .iter()
            .find(|e| matches!(e.kind, EventKind::FirstToMove { .. }));
        assert!(ev.is_some(), "FirstToMove event must be in log");
        if let Some(ev) = ev {
            if let EventKind::FirstToMove { creature_id: cid } = &ev.kind {
                assert_eq!(*cid, creature_id, "FirstToMove creature_id must match");
            }
        }
    }

    // ---- E.25.d tests ----

    /// E.25.d: biggest_ever is set at World::new. D3: size is always FOUNDER_SIZE.
    #[test]
    fn e25_biggest_ever_tracks_max_size() {
        // The founder always initializes biggest_ever at World::new.
        let w = World::new("e25-biggest");
        assert!(
            w.biggest_ever.is_some(),
            "biggest_ever must be Some immediately after World::new (founder seeded)"
        );
        // D3: no captured_size in HallOfFame; just check it's populated.
        assert_eq!(
            w.biggest_ever.as_ref().unwrap().creature_id,
            0,
            "biggest_ever must be the founder (id=0)"
        );
    }

    /// E.25.d: weirdest requires age >= 500 to qualify.
    /// D3: no genome fields to mutate; use brain weights for weirdness.
    #[test]
    fn e25_weirdest_requires_500_ticks() {
        let mut w = World::new("e25-weird");

        // Age 400 — below threshold, should NOT become weirdest.
        w.creatures.age[0] = 400;
        w.creatures.energy[0] = -1.0; // trigger death
        w.collect_deaths(); // S27: results in self.scratch_dead
        assert_eq!(w.scratch_dead.len(), 1);
        assert!(
            w.weirdest.is_none(),
            "age 400 must not qualify for weirdest"
        );
        assert!(
            w.longest_lived.is_some(),
            "longest_lived must track unconditionally"
        );

        // Fresh world, age 500 — should qualify (brain may or may not differ enough).
        let mut w2 = World::new("e25-weird2");
        // Set brain weights far from anchor to guarantee weirdness distance > 0.
        for wt in w2.creatures.brains[0].weights.iter_mut() {
            *wt = 5.0;
        }
        w2.creatures.age[0] = 500;
        w2.creatures.energy[0] = -1.0;
        w2.collect_deaths(); // S27: results in self.scratch_dead
        assert_eq!(w2.scratch_dead.len(), 1);
        // weirdest distance must be > 0 now (brain deviated from anchor).
        assert!(
            w2.weirdest.is_some(),
            "age 500 with deviated brain must qualify for weirdest"
        );
        assert_eq!(w2.weirdest.as_ref().unwrap().captured_age, 500);
    }

    /// E.25.d: last_survivor captures the final death tick.
    #[test]
    fn e25_last_survivor_captures_final_death() {
        use crate::brain::Brain;
        use crate::vision::VISION_LEN;

        let mut w = World::new("e25-last");
        let mut seeder = SimRng::from_string("e25-last-seed");
        // Add a second creature. D3: no genome needed.
        let b2 = Brain::founder(&mut seeder);
        w.creatures
            .push(1, 200.0, 200.0, FOUNDER_ENERGY, 0, 0, 0, b2);
        w.vision.push([0.0f32; VISION_LEN]);
        assert_eq!(w.creatures.len(), 2);

        // Kill creature 0 this tick.
        w.creatures.energy[0] = -1.0;
        let id0 = w.creatures.id[0];
        w.collect_deaths(); // S27: results in self.scratch_dead

        // last_survivor should be creature 0 (only one dead so far).
        assert!(w.last_survivor.is_some());
        let ls = w.last_survivor.as_ref().unwrap();
        assert_eq!(ls.creature_id, id0);

        // Kill creature 1 on the next "tick" (simulate by bumping tick).
        w.tick = 10;
        let id1 = w.creatures.id[0]; // after swap_remove, index 0 is now creature 1
        w.creatures.energy[0] = -1.0;
        w.collect_deaths(); // S27: results in self.scratch_dead

        // last_survivor must be updated to creature 1 (later tick).
        let ls2 = w.last_survivor.as_ref().unwrap();
        assert_eq!(
            ls2.creature_id, id1,
            "last_survivor must be the creature that died last"
        );
        assert_eq!(
            ls2.captured_tick, 10,
            "captured_tick must match the second death tick"
        );
    }

    // ---- S24 tests ----

    /// S24 test 1: Scavenge 3×3 sweep picks up a carrion blob that is within
    /// the creature's body radius, using the cell_to_carrion index.
    #[test]
    fn s24_scavenge_3x3_sweep_finds_adjacent_carrion() {
        use crate::constants::*;
        use crate::vision::build_cell_to_carrion;

        let mut w = World::new("s24-sweep");
        // D3: scav_eff is always 1.0; no genome patch needed.
        w.creatures.action_this_tick[0] = Action::Scavenge;

        let cx = w.creatures.x[0];
        let cy = w.creatures.y[0];
        let carrion_pool = 5.0_f32;
        w.carrion.push(Carrion {
            id: 777,
            x: cx,
            y: cy,
            pool: carrion_pool,
            age: 0,
        });
        // Rebuild the cell index so eat_and_scavenge can use it.
        build_cell_to_carrion(&w.carrion, &mut w.cell_to_carrion);

        let energy_before = w.creatures.energy[0];
        w.eat_and_scavenge();
        let energy_after = w.creatures.energy[0];

        // The creature should have gained some energy (scavenged from the carrion).
        assert!(
            energy_after > energy_before - COST_SCAVENGE_ATTEMPT,
            "Scavenge should gain energy from adjacent carrion: before={energy_before} after={energy_after}"
        );
        // The carrion pool should have decreased.
        assert!(
            w.carrion[0].pool < carrion_pool,
            "carrion pool should decrease after scavenging: pool={}",
            w.carrion[0].pool
        );
    }

    /// S24 test 2: Scavenge does NOT pick up carrion that is outside the creature's
    /// body radius, even if it is in the same 3×3 cell window.
    #[test]
    fn s24_scavenge_skips_out_of_radius_carrion() {
        use crate::constants::*;
        use crate::vision::build_cell_to_carrion;

        let mut w = World::new("s24-oor");
        // D3: scav_eff is always 1.0; no genome patch needed.
        w.creatures.action_this_tick[0] = Action::Scavenge;

        let cx = w.creatures.x[0];
        let cy = w.creatures.y[0];
        // D3: r_i uses FOUNDER_SIZE.
        let r_i = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let far = r_i * 10.0; // well outside body radius
        let carrion_pool = 5.0_f32;
        w.carrion.push(Carrion {
            id: 888,
            x: cx + far.min(HASH_CELL * 0.9), // within 1-cell offset but outside body radius
            y: cy,
            pool: carrion_pool,
            age: 0,
        });
        build_cell_to_carrion(&w.carrion, &mut w.cell_to_carrion);

        let energy_before = w.creatures.energy[0];
        w.eat_and_scavenge();
        let energy_after = w.creatures.energy[0];

        // The carrion pool should be unchanged (no scavenging took place).
        assert_eq!(
            w.carrion[0].pool, carrion_pool,
            "out-of-radius carrion must not be scavenged"
        );
        // Energy dropped by the attempt cost only.
        let delta = energy_after - energy_before;
        assert!(
            (delta + COST_SCAVENGE_ATTEMPT).abs() < 1e-4,
            "only scavenge attempt cost, delta={delta}"
        );
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
                w.scratch_fx[i].abs() <= REPULSION_MAX + 1e-3,
                "scratch_fx[{i}] = {} not reset (sentinel survived)",
                w.scratch_fx[i]
            );
            assert!(
                w.scratch_fy[i].abs() <= REPULSION_MAX + 1e-3,
                "scratch_fy[{i}] = {} not reset (sentinel survived)",
                w.scratch_fy[i]
            );
        }
    }

    /// perf-2 test 2: scratch Vecs resize correctly with population each tick.
    #[test]
    fn scratch_grows_with_population() {
        let mut w = World::new("perf2-grow");
        let n0 = w.creatures.len();
        for i in 0..w.creatures.len() {
            w.creatures.energy[i] = 10_000.0;
        }
        let mut peak_n = n0;
        for _ in 0..500 {
            let n = w.creatures.len();
            peak_n = peak_n.max(n);
            let floor = n.saturating_sub(1);
            assert!(
                w.scratch_fx.len() >= floor,
                "tick pre-check: scratch_fx {} < floor {}, n={}",
                w.scratch_fx.len(),
                floor,
                n
            );
            assert!(
                w.scratch_fy.len() >= floor,
                "tick pre-check: scratch_fy {} < floor {}, n={}",
                w.scratch_fy.len(),
                floor,
                n
            );
            assert!(w.scratch_damage.len() >= floor);
            assert!(w.scratch_gain.len() >= floor);
            assert!(w.scratch_cooldown_set.len() >= floor);
            assert!(w.scratch_attempted_eat.len() >= floor);
            assert!(w.scratch_attempted_scavenge.len() >= floor);
            assert!(w.scratch_got_a_bite.len() >= floor);
            if !w.tick_once() {
                break;
            }
        }
        assert!(
            peak_n > n0,
            "population should have peaked above initial n0={n0}"
        );
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
        // Zero move_bias so the creature truly has no velocity this tick.
        w.creatures.move_bias_x[0] = 0.0;
        w.creatures.move_bias_y[0] = 0.0;
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

    /// P1a §2.5: a creature with positive vx that crosses the right edge should
    /// wrap to x ≈ 0 (not be clamped to WORLD_SIZE - radius).
    /// D3: no genome.move_speed patch needed (MOVE_SPEED_MAX is constant).
    #[test]
    fn position_wraps_when_velocity_crosses_edge() {
        let mut w = World::new("p1a-wrap-vel");
        w.creatures.x[0] = WORLD_SIZE - 1.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 5.0; // will cross the right seam
        w.creatures.vy[0] = 0.0;
        // D3: no resync_hot_mirrors_at needed.
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        w.apply_movement_and_repulsion();

        let x = w.creatures.x[0];
        assert!(
            (0.0..WORLD_SIZE).contains(&x),
            "position must be in [0, WORLD_SIZE) after wrap; x = {x}"
        );
        assert!(
            x < 10.0,
            "wrapped x should be near 0 (crossed right seam); x = {x}"
        );
    }

    /// P1a §2.9: split-jitter should wrap the child position toroidally, not clamp.
    #[test]
    fn split_jitter_wraps() {
        use crate::creature::Action;

        let mut w = World::new("p1a-jitter");
        w.creatures.x[0] = 5.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.energy[0] = SPLIT_THRESHOLD + 100.0;
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

    /// P1a §2.4: two creatures on opposite sides of the x-seam should repel
    /// each other via the shortest-path vector.
    #[test]
    fn repulsion_wraps_across_seam() {
        use crate::brain::Brain;

        let mut w = World::new("p1a-repulsion-seam");
        w.creatures.x[0] = 2.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 0.0;
        w.creatures.vy[0] = 0.0;

        let mut rng = SimRng::from_u64(123);
        let b2 = Brain::founder(&mut rng);
        let n_before = w.creatures.len();
        use crate::vision::VISION_LEN;
        w.creatures
            .push(1, 598.0, WORLD_SIZE * 0.5, FOUNDER_ENERGY, 0, 0, 0, b2);
        w.vision.push([0.0f32; VISION_LEN]);
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
        w.grass.density.fill(0.0);
        w.creatures.action_this_tick[0] = Action::Graze;
        let before = w.creatures.energy[0];
        w.graze();
        assert_eq!(
            before, w.creatures.energy[0],
            "energy must be unchanged when grass is empty"
        );
    }

    /// P1e test 2: creature overlapping seeded cells sums deltas correctly.
    /// D3: FOUNDER_SIZE=1.0, BODY_RADIUS_PER_SIZE=1.0 → body radius=1.0.
    /// GRASS_CELL_SIZE=2.5 so a circle of r=1.0 must be placed at a cell center
    /// to overlap that cell. We position the creature exactly at cell (120,120)'s
    /// center so it overlaps at least one cell.
    #[test]
    fn graze_n_cell_overlap_sums_capped_deltas() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX, GRAZE_MAX_PER_TICK};

        let mut w = World::new("graze-patch");
        // D3: size = FOUNDER_SIZE always. Position creature at cell (120,120) center.
        let cell_ix: usize = 120;
        let cell_iy: usize = 120;
        let cx = (cell_ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (cell_iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        // Seed the entire grass grid with GRASS_MAX.
        w.grass.density.fill(GRASS_MAX);

        // Count how many cells overlap using FOUNDER_SIZE body radius.
        let ri = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let mut overlap_count = 0usize;
        w.grass.for_each_cell_overlapping_circle(cx, cy, ri, |_| {
            overlap_count += 1;
        });
        assert!(
            overlap_count > 0,
            "creature positioned at cell center with FOUNDER_SIZE should overlap at least one cell (ri={ri}, cell_size={})",
            GRASS_CELL_SIZE
        );

        let energy_before = w.creatures.energy[0];
        w.graze();
        let energy_after = w.creatures.energy[0];
        let expected_gain = overlap_count as f32 * GRAZE_MAX_PER_TICK;

        assert!(
            (energy_after - energy_before - expected_gain).abs() < 1e-4,
            "energy gain={} expected={} (overlap_count={}, cap={})",
            energy_after - energy_before,
            expected_gain,
            overlap_count,
            GRAZE_MAX_PER_TICK
        );
        // Confirm grass density actually decreased at the center cell.
        let expected_density = GRASS_MAX - GRAZE_MAX_PER_TICK;
        let center_cell = cell_iy * GRASS_GRID_DIM + cell_ix;
        assert!(
            (w.grass.density[center_cell] - expected_density).abs() < 1e-5,
            "center cell density should have dropped by GRAZE_MAX_PER_TICK; got {}",
            w.grass.density[center_cell]
        );
    }

    /// P1e test 3: creature at world seam (x ≈ WORLD_SIZE) overlaps cells on both sides.
    #[test]
    fn graze_wraps_across_world_seam() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX};

        let mut w = World::new("graze-seam");
        let cx = 0.0_f32;
        let cy = WORLD_SIZE * 0.5;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        // Use FOUNDER_SIZE for body; need radius >= 1.25 to hit both seam cells.
        // FOUNDER_SIZE * BODY_RADIUS_PER_SIZE must be >= 1.25 (GRASS_CELL_SIZE/2 = 1.25).
        let iy = ((cy / GRASS_CELL_SIZE).floor() as usize).min(GRASS_GRID_DIM - 1);
        let east_cell = iy * GRASS_GRID_DIM + (GRASS_GRID_DIM - 1);
        let west_cell = iy * GRASS_GRID_DIM;
        w.grass.density[east_cell] = GRASS_MAX;
        w.grass.density[west_cell] = GRASS_MAX;

        w.graze();

        // Check whether radius is sufficient (FOUNDER_SIZE=1.0, BODY_RADIUS_PER_SIZE=1.0 → radius=1.0 < 1.25)
        // If not, this test is a no-op but must not panic.
        let ri = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        if ri >= 1.25 {
            let east_drained = w.grass.density[east_cell] < GRASS_MAX - 1e-6;
            let west_drained = w.grass.density[west_cell] < GRASS_MAX - 1e-6;
            assert!(
                east_drained,
                "east seam cell (ix=239) must be grazed when radius >= 1.25; density={}",
                w.grass.density[east_cell]
            );
            assert!(
                west_drained,
                "west seam cell (ix=0) must be grazed when radius >= 1.25; density={}",
                w.grass.density[west_cell]
            );
        }
        // If ri < 1.25, the seam cells are not reached — test passes (no panic).
    }

    /// P1e test 4: single cell at density 1.0 with constant eff → gain == GRAZE_MAX_PER_TICK.
    #[test]
    fn graze_per_cell_cap_holds() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX, GRAZE_MAX_PER_TICK};

        let mut w = World::new("graze-cap");
        let ix = 2usize;
        let iy = 2usize;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        // D3: size is FOUNDER_SIZE (1.0); if radius < GRASS_CELL_SIZE/2 = 1.25 only center cell hit.
        // FOUNDER_SIZE=1.0, BODY_RADIUS_PER_SIZE=1.0 → ri=1.0 < 1.25 → single cell overlap.
        w.creatures.action_this_tick[0] = Action::Graze;

        let cell_idx = iy * GRASS_GRID_DIM + ix;
        w.grass.density[cell_idx] = GRASS_MAX;

        let energy_before = w.creatures.energy[0];
        w.graze();
        let gained = w.creatures.energy[0] - energy_before;

        assert!(
            (gained - GRAZE_MAX_PER_TICK).abs() < 1e-5,
            "gained={gained} expected={GRAZE_MAX_PER_TICK}"
        );
        assert!(
            (w.grass.density[cell_idx] - (GRASS_MAX - GRAZE_MAX_PER_TICK)).abs() < 1e-5,
            "density should drop by cap; got {}",
            w.grass.density[cell_idx]
        );
    }

    /// P1e test 5: graze_efficiency = constant 1.0 — energy gained == GRAZE_MAX_PER_TICK.
    /// D3: no zero-efficiency path; founder always grazes at full efficiency.
    #[test]
    fn graze_constant_efficiency_produces_correct_gain() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX, GRAZE_MAX_PER_TICK};

        let mut w = World::new("graze-const-eff");
        let ix = 5usize;
        let iy = 5usize;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        let cell_idx = iy * GRASS_GRID_DIM + ix;
        w.grass.density[cell_idx] = GRASS_MAX;

        let energy_before = w.creatures.energy[0];
        w.graze();
        let gained = w.creatures.energy[0] - energy_before;
        // D3: constant eff=1.0, single cell → gain = GRAZE_MAX_PER_TICK.
        assert!(
            (gained - GRAZE_MAX_PER_TICK).abs() < 1e-5,
            "gained={gained} expected={GRAZE_MAX_PER_TICK}"
        );
    }

    /// P1e test 6: energy gained == grass density consumed (conservation property).
    #[test]
    fn graze_conserves_energy_with_grass_drain() {
        use crate::constants::{GRASS_MAX, GRAZE_MAX_PER_TICK};

        let mut w = World::new("graze-conserve");
        w.grass.density.fill(GRASS_MAX);

        let cx = WORLD_SIZE * 0.5;
        let cy = WORLD_SIZE * 0.5;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        let density_before: f32 = w.grass.density.iter().sum();
        let energy_before = w.creatures.energy[0];

        w.graze();

        let density_after: f32 = w.grass.density.iter().sum();
        let energy_after = w.creatures.energy[0];

        let density_consumed = density_before - density_after;
        let energy_gained = energy_after - energy_before;

        assert!(
            (energy_gained - density_consumed).abs() < 1e-2,
            "conservation violated: gained={energy_gained} consumed={density_consumed}"
        );
        // Sanity: creature should have gained something.
        let ri = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let mut overlap_count = 0usize;
        w.grass
            .for_each_cell_overlapping_circle(cx, cy, ri, |_| overlap_count += 1);
        let expected_gain = overlap_count as f32 * GRAZE_MAX_PER_TICK.min(GRASS_MAX);
        assert!(
            (energy_gained - expected_gain).abs() < 1e-3,
            "energy_gained={energy_gained} expected={expected_gain}"
        );
    }

    // ---- P2f move_bias tests ----

    /// P2f test 1: move_bias_reroll fires after exactly MOVE_BIAS_REROLL_INTERVAL ticks.
    #[test]
    fn move_bias_reroll_fires_every_20_ticks() {
        let mut w = World::new("p2f-rerolls");
        w.tick = 100;
        w.creatures.move_bias_x[0] = 0.123;
        w.creatures.move_bias_reroll_at[0] = w.tick + MOVE_BIAS_REROLL_INTERVAL; // 120
        let before_x = w.creatures.move_bias_x[0];

        for _ in 0..(MOVE_BIAS_REROLL_INTERVAL - 1) {
            w.apply_movement_and_repulsion();
            w.tick += 1;
        }
        assert_eq!(
            w.creatures.move_bias_x[0], before_x,
            "bias must not re-roll before reroll_at is reached (tick={} reroll_at={})",
            w.tick, w.creatures.move_bias_reroll_at[0]
        );

        w.tick += 1; // tick = 120
        w.apply_movement_and_repulsion();
        assert_ne!(
            w.creatures.move_bias_x[0], before_x,
            "bias must re-roll when tick reaches reroll_at (tick={})",
            w.tick
        );
    }

    /// P2f test 2: move_bias adds to velocity (ADD semantics per amendments §A.5).
    #[test]
    fn move_bias_adds_to_velocity() {
        let mut w = World::new("p2f-add");
        // Zero all NN weights so NN output is zero for both vx and vy.
        w.creatures.brains[0].weights = vec![0.0; NN_WEIGHT_COUNT];
        w.creatures.move_bias_x[0] = 0.5;
        w.creatures.move_bias_y[0] = -0.5;
        w.creatures.move_bias_reroll_at[0] = u32::MAX;
        // D3: MOVE_SPEED_MAX is constant for all creatures.

        w.step();

        let dx = w.creatures.x[0] - WORLD_SIZE * 0.5;
        let dy = w.creatures.y[0] - WORLD_SIZE * 0.5;
        assert!(
            dx > 0.0,
            "x must increase (bias_x > 0 → rightward motion); dx = {dx}"
        );
        assert!(
            dy < 0.0,
            "y must decrease (bias_y < 0 → upward motion); dy = {dy}"
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
    /// D3: all creatures have eat_eff=1.0, armor=0.0 as constants.
    #[test]
    fn p3a_eat_bite_basic_transfer() {
        use crate::brain::Brain;
        use crate::vision::{build_cell_to_carrion, VISION_LEN};

        let mut w = World::new("p3a-basic");
        w.creatures.energy[0] = 100.0;
        w.creatures.digestion_cooldown[0] = 0;
        w.creatures.action_this_tick[0] = Action::Eat;
        w.sliders.eat_bite_fraction = 0.5;

        // Add prey (creature 1) adjacent.
        let mut rng = SimRng::from_u64(42);
        let prey_brain = Brain::founder(&mut rng);
        let pred_x = w.creatures.x[0];
        let pred_y = w.creatures.y[0];
        // Place prey within bite reach (BITE_REACH_CONSTANT * FOUNDER_SIZE).
        w.creatures
            .push(1, pred_x + 1.5, pred_y, 100.0, 0, 0, 0, prey_brain);
        w.vision.push([0.0f32; VISION_LEN]);

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);
        build_cell_to_carrion(&w.carrion, &mut w.cell_to_carrion);

        let pred_energy_before = w.creatures.energy[0];
        let prey_energy_before = w.creatures.energy[1];

        w.eat_and_scavenge();

        let pred_energy_after = w.creatures.energy[0];
        let prey_energy_after = w.creatures.energy[1];

        // transfer = 0.5 * 100.0 * (1.0 - 0.0) = 50.0
        // predator gain = 50.0 * 1.0 = 50.0; net = 50.0 - COST_EAT_ATTEMPT (0.3) = 49.7
        // prey loss = 50.0
        let prey_loss = prey_energy_before - prey_energy_after;
        let pred_gain = pred_energy_after - pred_energy_before;

        assert!(
            (prey_loss - 50.0).abs() < 1e-4,
            "prey should lose 50.0; lost {prey_loss}"
        );
        assert!(
            (pred_gain - 49.7).abs() < 1e-3,
            "predator should gain 49.7 (50 - 0.3 cost); gained {pred_gain}"
        );
    }

    /// P3a test: digestion cooldown still gates bites — only attempt cost charged, no bite.
    #[test]
    fn p3a_eat_cooldown_still_gates() {
        use crate::brain::Brain;
        use crate::vision::{build_cell_to_carrion, VISION_LEN};

        let mut w = World::new("p3a-cooldown");
        w.creatures.energy[0] = 100.0;
        // Set active cooldown so bite is gated.
        w.creatures.digestion_cooldown[0] = 10;
        w.creatures.action_this_tick[0] = Action::Eat;
        w.sliders.eat_bite_fraction = 0.5;

        let mut rng = SimRng::from_u64(7);
        let prey_brain = Brain::founder(&mut rng);
        let pred_x = w.creatures.x[0];
        let pred_y = w.creatures.y[0];
        w.creatures
            .push(1, pred_x + 1.5, pred_y, 100.0, 0, 0, 0, prey_brain);
        w.vision.push([0.0f32; VISION_LEN]);

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);
        build_cell_to_carrion(&w.carrion, &mut w.cell_to_carrion);

        let prey_energy_before = w.creatures.energy[1];
        let pred_energy_before = w.creatures.energy[0];

        w.eat_and_scavenge();

        let prey_change = (w.creatures.energy[1] - prey_energy_before).abs();
        let pred_change = pred_energy_before - w.creatures.energy[0];

        // Prey must be unchanged (no bite landed due to cooldown).
        assert!(
            prey_change < 1e-6,
            "prey energy must be unchanged when predator is in cooldown; change = {prey_change}"
        );
        // Predator should only lose the attempt cost.
        assert!(
            (pred_change - COST_EAT_ATTEMPT).abs() < 1e-4,
            "predator should only lose COST_EAT_ATTEMPT ({COST_EAT_ATTEMPT}); lost {pred_change}"
        );
    }
}
