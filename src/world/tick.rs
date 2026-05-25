//! Per-tick step bodies that mutate `World`. All as `impl World` blocks; private to the `crate::world` parent.

use super::World;
use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::Action;
use crate::events::{Event, EventKind};
use crate::hof::HallOfFame;
use crate::species::species_distance;
use crate::sun::SunMap;

impl World {
    pub(crate) fn apply_movement_and_repulsion(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        // perf-5: hot reads use g_* mirrors; cold reads (bite_reach, etc.) stay on &self.creatures.genomes[i].
        for i in 0..n {
            let speed_cap = self.creatures.g_move_speed[i];
            let vx = self.creatures.vx[i];
            let vy = self.creatures.vy[i];
            let mag2 = vx * vx + vy * vy;
            let (cvx, cvy) = if speed_cap > 0.0 && mag2 > speed_cap * speed_cap {
                let s = speed_cap / mag2.sqrt();
                (vx * s, vy * s)
            } else if speed_cap == 0.0 {
                (0.0, 0.0)
            } else {
                (vx, vy)
            };
            let dist = (cvx * cvx + cvy * cvy).sqrt();
            self.creatures.energy[i] -= dist * COST_MOVE_PER_DIST;
            self.creatures.distance_travelled[i] += dist;
            self.creatures.x[i] += cvx;
            self.creatures.y[i] += cvy;
            // E.25.b: FirstToMove fires here (on the actual crossing tick) per v6 §L.
            if !self.first_move_fired
                && self.creatures.g_move_speed[i] > 0.0
                && self.creatures.distance_travelled[i] >= 5.0
            {
                self.first_move_fired = true;
                let g = self.creatures.genomes[i].clone();
                let species_name = self.species.get(self.creatures.species_id[i]).name.clone();
                // E.25.d: capture first_mover_snapshot for F.28.
                self.first_mover_snapshot = Some(HallOfFame {
                    creature_id: self.creatures.id[i],
                    genome: g.clone(),
                    species_name,
                    captured_tick: self.tick,
                    captured_size: g.size,
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

        // Reset/grow the force accumulators in place. resize() is O(1) when the
        // capacity is already >= n (the common case after the first tick).
        self.scratch_fx.resize(n, 0.0);
        self.scratch_fy.resize(n, 0.0);
        self.scratch_fx.fill(0.0);
        self.scratch_fy.fill(0.0);
        for i in 0..n {
            let xi = self.creatures.x[i];
            let yi = self.creatures.y[i];
            let ri = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
            let search = ri + SIZE_MAX * BODY_RADIUS_PER_SIZE;
            // std::mem::take releases the self.scratch_neighbors borrow so we can
            // simultaneously borrow self.grid and self.creatures inside the closure
            // and the inner loop. The high-water-mark allocation travels with the
            // local binding and is returned to the field at the end of each iteration.
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
                let rj = self.creatures.g_size[j] * BODY_RADIUS_PER_SIZE;
                let dx = xj - xi;
                let dy = yj - yi;
                let d2 = dx * dx + dy * dy;
                let rsum = ri + rj;
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

        // S31: track whether any position changed so we can skip the final grid
        // rebuild when positions are unchanged. Two sources of position change:
        //   1. Wall clamp (any_wall) — clamps x/y beyond what repulsion provided.
        //   2. Nonzero repulsion force (any_repulsion) — moves at least one creature.
        // If neither fired, the pre-repulsion grid rebuild above is still valid.
        // eat_and_scavenge (step 6) uses the grid, so we must not skip when stale.
        let mut any_wall = false;
        let mut any_repulsion = false;
        for i in 0..n {
            if self.scratch_fx[i] != 0.0 || self.scratch_fy[i] != 0.0 {
                any_repulsion = true;
            }
            let mut x = self.creatures.x[i] + self.scratch_fx[i];
            let mut y = self.creatures.y[i] + self.scratch_fy[i];
            let r = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
            if x < r {
                x = r;
                any_wall = true;
                if self.creatures.vx[i] < 0.0 {
                    self.creatures.vx[i] = 0.0;
                }
            } else if x > WORLD_SIZE - r {
                x = WORLD_SIZE - r;
                any_wall = true;
                if self.creatures.vx[i] > 0.0 {
                    self.creatures.vx[i] = 0.0;
                }
            }
            if y < r {
                y = r;
                any_wall = true;
                if self.creatures.vy[i] < 0.0 {
                    self.creatures.vy[i] = 0.0;
                }
            } else if y > WORLD_SIZE - r {
                y = WORLD_SIZE - r;
                any_wall = true;
                if self.creatures.vy[i] > 0.0 {
                    self.creatures.vy[i] = 0.0;
                }
            }
            self.creatures.x[i] = x;
            self.creatures.y[i] = y;
        }

        // S31: skip rebuild when no wall fired AND no repulsion force moved anyone.
        if any_wall || any_repulsion {
            self.grid.rebuild(&self.creatures.x, &self.creatures.y);
        }
    }

    pub(crate) fn photosynth_two_pass(&mut self) {
        // 5a. demand
        // perf-5: hot reads use g_* mirrors; cold reads stay on &self.creatures.genomes[i].
        self.sun.demand.fill(0.0);
        for i in 0..self.creatures.len() {
            if self.creatures.action_this_tick[i] != Action::Photosynth {
                continue;
            }
            let want = PHOTO_GAIN_COEFF * self.creatures.g_photo_eff[i] * self.creatures.g_size[i];
            if want <= 0.0 {
                continue;
            }
            let cell = SunMap::cell_index_for(self.creatures.x[i], self.creatures.y[i]);
            self.sun.demand[cell] += want;
        }
        // 5b. payout
        for i in 0..self.creatures.len() {
            if self.creatures.action_this_tick[i] != Action::Photosynth {
                continue;
            }
            let want = PHOTO_GAIN_COEFF * self.creatures.g_photo_eff[i] * self.creatures.g_size[i];
            if want <= 0.0 {
                continue;
            }
            let cell = SunMap::cell_index_for(self.creatures.x[i], self.creatures.y[i]);
            let demand = self.sun.demand[cell];
            if demand <= 0.0 {
                continue;
            }
            let factor = (self.sun.current[cell] / demand).min(1.0);
            self.creatures.energy[i] += want * factor;
        }
        // 5c. drain
        for k in 0..self.sun.current.len() {
            let d = self.sun.demand[k];
            if d <= 0.0 {
                continue;
            }
            let factor = (self.sun.current[k] / d).min(1.0);
            let paid = d * factor;
            self.sun.current[k] = (self.sun.current[k] - paid).max(0.0);
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

        for i in 0..n {
            match self.creatures.action_this_tick[i] {
                // perf-5: hot reads use g_* mirrors; cold reads (bite_reach, armor) stay on &self.creatures.genomes[i].
                Action::Eat => {
                    self.scratch_attempted_eat[i] = true;
                    if self.creatures.digestion_cooldown[i] > 0 {
                        continue;
                    }
                    let eat_eff_i = self.creatures.g_eat_eff[i];
                    if eat_eff_i <= 0.0 {
                        continue;
                    }
                    let size_i = self.creatures.g_size[i];
                    let bite_reach_i = self.creatures.genomes[i].bite_reach; // COLD field; stays AoS
                    let radius_i = size_i * BODY_RADIUS_PER_SIZE;
                    let reach = bite_reach_i * size_i;
                    let max_range = radius_i + reach + SIZE_MAX * BODY_RADIUS_PER_SIZE;
                    let xi = self.creatures.x[i];
                    let yi = self.creatures.y[i];
                    let mut best: Option<(usize, f32)> = None;
                    let mut candidates: Vec<usize> = Vec::with_capacity(8);
                    self.grid.for_each_in_radius(xi, yi, max_range, |j| {
                        if j != i {
                            candidates.push(j);
                        }
                    });
                    for j in candidates {
                        let dx = self.creatures.x[j] - xi;
                        let dy = self.creatures.y[j] - yi;
                        let d = (dx * dx + dy * dy).sqrt();
                        let rj = self.creatures.g_size[j] * BODY_RADIUS_PER_SIZE;
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
                    if let Some((j, _)) = best {
                        let dmg = EAT_DAMAGE_COEFF * size_i;
                        let armor = self.creatures.genomes[j].armor.clamp(0.0, 1.0); // COLD field; stays AoS
                        self.scratch_damage[j] += dmg * (1.0 - armor);
                        self.scratch_gain[i] += EAT_GAIN_COEFF * eat_eff_i;
                        self.scratch_cooldown_set[i] = true;
                        self.scratch_got_a_bite[i] = true;
                    }
                }
                Action::Scavenge => {
                    self.scratch_attempted_scavenge[i] = true;
                    let scav_eff_i = self.creatures.g_scav_eff[i];
                    if scav_eff_i <= 0.0 {
                        continue;
                    }
                    let r_i = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
                    let r2 = r_i * r_i;
                    let xi = self.creatures.x[i];
                    let yi = self.creatures.y[i];
                    let want = SCAVENGE_GAIN_COEFF * scav_eff_i;
                    // S24: 3×3 cell sweep via cell_to_carrion (O(local) vs O(all carrion)).
                    // Collect the first in-range carrion index before mutating it.
                    let cx = (xi / HASH_CELL).floor() as i32;
                    let cy = (yi / HASH_CELL).floor() as i32;
                    let dim = HASH_DIM as i32;
                    let mut found_ci: Option<usize> = None;
                    'outer: for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let nx = cx + dx;
                            let ny = cy + dy;
                            if nx < 0 || ny < 0 || nx >= dim || ny >= dim {
                                continue;
                            }
                            let cell_idx = ny as usize * HASH_DIM + nx as usize;
                            for &ci in &self.cell_to_carrion[cell_idx] {
                                let c = &self.carrion[ci as usize];
                                let ddx = c.x - xi;
                                let ddy = c.y - yi;
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
        // perf-5: hot reads use g_* mirrors; cold reads (armor, max_age) stay on &self.creatures.genomes[i].
        let mouth_tax = self.sliders.mouth_tax;
        for i in 0..self.creatures.len() {
            let size_i = self.creatures.g_size[i];
            let move_speed_i = self.creatures.g_move_speed[i];
            let eye_count_i = self.creatures.g_eye_count[i];
            let vision_range_i = self.creatures.g_vision_range[i];
            let eat_eff_i = self.creatures.g_eat_eff[i];
            let scav_eff_i = self.creatures.g_scav_eff[i];
            let g = &self.creatures.genomes[i]; // for g.armor, g.max_age, g.clone() (HoF)
            let mut up = UPKEEP_BASE;
            if size_i > 1.0 {
                up += UPKEEP_SIZE_PER_UNIT * (size_i - 1.0);
            }
            if move_speed_i > 0.0 {
                up += UPKEEP_MOBILITY_FLAG;
                up += UPKEEP_MOVE_SPEED_PER_UNIT * move_speed_i;
            }
            up += UPKEEP_PER_EYE * eye_count_i as f32;
            up += UPKEEP_VISION_COEFF * vision_range_i * vision_range_i;
            if eat_eff_i > 0.0 {
                up += mouth_tax;
            }
            if scav_eff_i > 0.0 {
                up += UPKEEP_GUT;
            }
            if g.armor > 0.0 {
                up += UPKEEP_ARMOR_PER_UNIT * g.armor;
            }
            up += UPKEEP_LIFESPAN_PER_1K * (g.max_age as f32 / 1000.0);
            up += UPKEEP_NN_FIXED;

            let age = self.creatures.age[i];
            if age > g.max_age {
                let excess = (age - g.max_age) as f32;
                let mult = PAST_LIFESPAN_MULT.powf(excess / 1000.0);
                up *= mult.min(1e6);
            }

            if size_i > self.creatures.max_size_reached[i] {
                self.creatures.max_size_reached[i] = size_i;
            }
            // E.25.d: biggest_ever hall-of-fame. v1 has no in-life growth, so
            // this fires per-tick against the global champion; any newborn
            // with a larger genome.size becomes the new biggest.
            {
                let current_best = self.biggest_ever.as_ref().map_or(0.0, |h| h.captured_size);
                if size_i > current_best {
                    let species_name = self.species.get(self.creatures.species_id[i]).name.clone();
                    self.biggest_ever = Some(HallOfFame {
                        creature_id: self.creatures.id[i],
                        genome: g.clone(),
                        species_name,
                        captured_tick: self.tick,
                        captured_size: size_i,
                        captured_age: self.creatures.age[i],
                    });
                }
            }
            self.creatures.energy[i] -= up;
            self.creatures.cumulative_upkeep[i] += up;
            if self.creatures.digestion_cooldown[i] > 0 {
                self.creatures.digestion_cooldown[i] -= 1;
            }
            self.creatures.age[i] += 1;
        }
    }

    pub(crate) fn collect_deaths(&mut self) -> Vec<usize> {
        let mut dead: Vec<usize> = Vec::new();
        let n = self.creatures.len();
        let mut species_lost: Vec<u32> = Vec::new();
        for i in 0..n {
            if self.creatures.energy[i] <= 0.0 {
                let pool = (CARRION_POOL_COEFF * self.creatures.cumulative_upkeep[i])
                    .clamp(0.0, CARRION_POOL_CAP);
                let cell = SunMap::cell_index_for(self.creatures.x[i], self.creatures.y[i]);
                self.carrion.push(Carrion {
                    id: self.creatures.id[i],
                    x: self.creatures.x[i],
                    y: self.creatures.y[i],
                    pool,
                    age: 0,
                    sun_cell: cell,
                });
                species_lost.push(self.creatures.species_id[i]);
                dead.push(i);

                // E.25.d: hall-of-fame tracking on death.
                let age = self.creatures.age[i];
                let g_clone = self.creatures.genomes[i].clone();
                let species_name = self.species.get(self.creatures.species_id[i]).name.clone();

                // last_survivor: overwrite on every death; final overwrite is the
                // latest death tick (pop=0 means no more deaths after this).
                self.last_survivor = Some(HallOfFame {
                    creature_id: self.creatures.id[i],
                    genome: g_clone.clone(),
                    species_name: species_name.clone(),
                    captured_tick: self.tick,
                    captured_size: g_clone.size,
                    captured_age: age,
                });

                // longest_lived (unconditional fallback for weirdest).
                if age > self.longest_lived_age {
                    self.longest_lived_age = age;
                    self.longest_lived = Some(HallOfFame {
                        creature_id: self.creatures.id[i],
                        genome: g_clone.clone(),
                        species_name: species_name.clone(),
                        captured_tick: self.tick,
                        captured_size: g_clone.size,
                        captured_age: age,
                    });
                }

                // weirdest: requires age >= 500 (v5 §11.1).
                if age >= 500 {
                    let dist = species_distance(
                        &g_clone,
                        &self.founder_genome_anchor,
                        &self.creatures.brains[i].weights,
                        &self.founder_brain_anchor,
                    );
                    if dist > self.weirdest_distance {
                        self.weirdest_distance = dist;
                        self.weirdest = Some(HallOfFame {
                            creature_id: self.creatures.id[i],
                            genome: g_clone,
                            species_name,
                            captured_tick: self.tick,
                            captured_size: self.creatures.genomes[i].size,
                            captured_age: age,
                        });
                    }
                }
            }
        }
        if !dead.is_empty() {
            self.pending_extinction_check
                .extend_from_slice(&species_lost);
        }
        dead
    }

    pub(crate) fn decay_carrion(&mut self) {
        let mut i = 0;
        while i < self.carrion.len() {
            self.carrion[i].age += 1;
            if self.carrion[i].age > CARRION_MAX_AGE || self.carrion[i].pool <= 0.0 {
                let remaining = self.carrion[i].pool.max(0.0);
                let cell = self.carrion[i].sun_cell;
                if remaining > 0.0 {
                    self.sun.return_to_cell(cell, remaining);
                }
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

    #[test]
    fn energy_conservation_in_photosynth_pass() {
        let mut w = World::new("conserve");
        w.creatures.action_this_tick[0] = Action::Photosynth;
        let before_energy = w.creatures.energy[0];
        let before_sun: f32 = w.sun.current.iter().sum();
        w.photosynth_two_pass();
        let after_energy = w.creatures.energy[0];
        let after_sun: f32 = w.sun.current.iter().sum();
        let gained = after_energy - before_energy;
        let drained = before_sun - after_sun;
        assert!(gained >= 0.0);
        assert!(
            (gained - drained).abs() < 1e-3,
            "gain {gained} vs drain {drained}"
        );
    }

    #[test]
    fn carrion_returns_to_sun_on_decay() {
        let mut w = World::new("decay-test");
        let cell = SunMap::cell_index_for(100.0, 100.0);
        w.sun.current[cell] = 0.0;
        w.carrion.push(Carrion {
            id: 999,
            x: 100.0,
            y: 100.0,
            pool: 0.5,
            age: CARRION_MAX_AGE,
            sun_cell: cell,
        });
        w.decay_carrion();
        assert!(w.sun.current[cell] > 0.0);
        assert!(w.carrion.is_empty());
    }

    // ---- E.25.b test ----

    /// E.25.b: FirstToMove fires inside apply_movement_and_repulsion when
    /// distance_travelled crosses 5.0 (v6 §L: "actually traveled ≥ 5u in lifetime").
    #[test]
    fn e25b_first_to_move_fires_on_movement_step() {
        let mut w = World::new("e25b-move");
        w.events_enabled = true; // enable logging so we can assert event contents
                                 // Give founder move_speed so movement cost applies.
        w.creatures.genomes[0].move_speed = 5.0;
        w.creatures.resync_hot_mirrors_at(0); // perf-5: keep mirrors in sync after genome patch
                                              // Set velocity directly so movement fires deterministically.
        w.creatures.vx[0] = 5.0;
        w.creatures.vy[0] = 0.0;
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

    /// E.25.d: biggest_ever tracks the creature with the highest size ever reached.
    #[test]
    fn e25_biggest_ever_tracks_max_size() {
        let mut w = World::new("e25-biggest");
        // Set founder's size to 5.0.
        w.creatures.genomes[0].size = 5.0;
        w.creatures.resync_hot_mirrors_at(0); // perf-5: keep mirrors in sync after genome patch
        w.energy_bookkeeping();
        assert!(
            w.biggest_ever.is_some(),
            "biggest_ever must be Some after first creature's energy_bookkeeping"
        );
        assert_eq!(
            w.biggest_ever.as_ref().unwrap().captured_size,
            5.0,
            "biggest_ever size must be 5.0"
        );

        // Update to larger size.
        w.creatures.genomes[0].size = 7.0;
        w.creatures.resync_hot_mirrors_at(0); // perf-5: keep mirrors in sync after genome patch
        w.creatures.max_size_reached[0] = 5.0; // reset so next check fires
        w.energy_bookkeeping();
        assert_eq!(
            w.biggest_ever.as_ref().unwrap().captured_size,
            7.0,
            "biggest_ever must update to new max 7.0"
        );
    }

    /// E.25.d: weirdest requires age >= 500 to qualify.
    #[test]
    fn e25_weirdest_requires_500_ticks() {
        let mut w = World::new("e25-weird");
        // Give founder huge genome drift from anchor to qualify if age allows.
        w.creatures.genomes[0].size = SIZE_MAX;
        w.creatures.genomes[0].pigment_r = 1.0 - w.founder_genome_anchor.pigment_r;

        // Age 400 — below threshold, should NOT become weirdest.
        w.creatures.age[0] = 400;
        w.creatures.energy[0] = -1.0; // trigger death
        let dead = w.collect_deaths();
        assert_eq!(dead.len(), 1);
        assert!(
            w.weirdest.is_none(),
            "age 400 must not qualify for weirdest"
        );
        assert!(
            w.longest_lived.is_some(),
            "longest_lived must track unconditionally"
        );

        // Fresh world, age 500 — should qualify.
        let mut w2 = World::new("e25-weird2");
        w2.creatures.genomes[0].size = SIZE_MAX;
        w2.creatures.genomes[0].pigment_r = 1.0 - w2.founder_genome_anchor.pigment_r;
        w2.creatures.age[0] = 500;
        w2.creatures.energy[0] = -1.0;
        let dead2 = w2.collect_deaths();
        assert_eq!(dead2.len(), 1);
        assert!(w2.weirdest.is_some(), "age 500 must qualify for weirdest");
        assert_eq!(w2.weirdest.as_ref().unwrap().captured_age, 500);
    }

    /// E.25.d: last_survivor captures the final death tick.
    #[test]
    fn e25_last_survivor_captures_final_death() {
        use crate::brain::Brain;
        use crate::vision::VISION_LEN;

        let mut w = World::new("e25-last");
        let mut seeder = SimRng::from_string("e25-last-seed");
        // Add a second creature.
        let g2 = Genome::founder();
        let b2 = Brain::founder(&mut seeder);
        w.creatures
            .push(1, 200.0, 200.0, FOUNDER_ENERGY, 0, 0, 0, g2, b2);
        w.vision.push([0.0f32; VISION_LEN]);
        assert_eq!(w.creatures.len(), 2);

        // Kill creature 0 this tick.
        w.creatures.energy[0] = -1.0;
        let id0 = w.creatures.id[0];
        let _dead = w.collect_deaths();

        // last_survivor should be creature 0 (only one dead so far).
        assert!(w.last_survivor.is_some());
        let ls = w.last_survivor.as_ref().unwrap();
        assert_eq!(ls.creature_id, id0);

        // Kill creature 1 on the next "tick" (simulate by bumping tick).
        w.tick = 10;
        let id1 = w.creatures.id[0]; // after swap_remove, index 0 is now creature 1
        w.creatures.energy[0] = -1.0;
        let _dead2 = w.collect_deaths();

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
        // Give the founder scavenge ability and place carrion on top of it.
        w.creatures.genomes[0].scavenge_efficiency = 1.0;
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.action_this_tick[0] = Action::Scavenge;

        let cx = w.creatures.x[0];
        let cy = w.creatures.y[0];
        let cell = crate::sun::SunMap::cell_index_for(cx, cy);
        let carrion_pool = 5.0_f32;
        w.carrion.push(Carrion {
            id: 777,
            x: cx,
            y: cy,
            pool: carrion_pool,
            age: 0,
            sun_cell: cell,
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
        w.creatures.genomes[0].scavenge_efficiency = 1.0;
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.action_this_tick[0] = Action::Scavenge;

        let cx = w.creatures.x[0];
        let cy = w.creatures.y[0];
        // Place carrion far outside the body radius but still within the 3×3 cell window.
        let r_i = w.creatures.g_size[0] * BODY_RADIUS_PER_SIZE;
        let far = r_i * 10.0; // well outside body radius
        let cell = crate::sun::SunMap::cell_index_for(cx, cy);
        let carrion_pool = 5.0_f32;
        w.carrion.push(Carrion {
            id: 888,
            x: cx + far.min(HASH_CELL * 0.9), // within 1-cell offset but outside body radius
            y: cy,
            pool: carrion_pool,
            age: 0,
            sun_cell: cell,
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
    /// Pre-poison the buffers with sentinel values, run one tick, confirm sentinels
    /// are gone (i.e. the resize+fill zeroing in apply_movement_and_repulsion fired).
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
        // After a tick, every entry must be a real repulsion force in
        // [-REPULSION_MAX, REPULSION_MAX] (or zero if no contact).
        // A surviving 999.0 sentinel would corrupt creature positions via the
        // write-back loop and also leave an out-of-range value here.
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

    /// perf-2 test 2: all scratch Vecs grow with the population over 500 ticks.
    #[test]
    fn scratch_grows_with_population() {
        let mut w = World::new("perf2-grow");
        let n0 = w.creatures.len();
        // Run until population grows past the initial scratch capacity.
        for _ in 0..500 {
            w.tick_once();
        }
        let n1 = w.creatures.len();
        assert!(n1 > n0, "test setup: population should have grown");
        // Scratch Vecs MUST be at least as long as the current population.
        assert!(w.scratch_fx.len() >= n1);
        assert!(w.scratch_fy.len() >= n1);
        assert!(w.scratch_damage.len() >= n1);
        assert!(w.scratch_gain.len() >= n1);
        assert!(w.scratch_cooldown_set.len() >= n1);
        assert!(w.scratch_attempted_eat.len() >= n1);
        assert!(w.scratch_attempted_scavenge.len() >= n1);
        assert!(w.scratch_got_a_bite.len() >= n1);
        // No panic across 500 ticks of growth confirms resize handles growth.
    }

    /// S31: a creature far from walls with no overlapping neighbors should NOT
    /// trigger a grid rebuild (any_wall = false AND any_repulsion = false).
    /// We verify correctness by running a tick and confirming the grid is still
    /// valid (correct creature positions) even when the rebuild was skipped.
    #[test]
    fn s31_no_wall_no_repulsion_grid_still_valid() {
        let mut w = World::new("s31-wall-skip");
        // Place the founder at the center with zero velocity so it doesn't move.
        w.creatures.x[0] = WORLD_SIZE * 0.5;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 0.0;
        w.creatures.vy[0] = 0.0;
        // Rebuild grid to match the centered position.
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);
        // Run the movement pass directly. With no velocity and no neighbors,
        // scratch_fx/fy should be zero and no wall should fire.
        w.apply_movement_and_repulsion();
        // After the call, the creature must still be findable in the grid.
        let mut found = false;
        w.grid
            .for_each_in_radius(WORLD_SIZE * 0.5, WORLD_SIZE * 0.5, 10.0, |_i| {
                found = true;
            });
        assert!(found, "creature must be found in grid after movement pass");
        // Position must still be at center (no movement).
        assert!(
            (w.creatures.x[0] - WORLD_SIZE * 0.5).abs() < 1.0,
            "creature must remain near center: x={}",
            w.creatures.x[0]
        );
    }
}
