//! Per-tick step bodies that mutate `World`. All as `impl World` blocks; private to the `crate::world` parent.

use super::World;
use crate::constants::*;
use crate::creature::Action;
use crate::torus::{torus_delta, wrap_pos};

impl World {
    /// Multi-cell graze: for every creature that chose `Action::Graze` this tick,
    /// consume grass density from every `GrassGrid` cell whose center overlaps the
    /// creature's body circle (toroidal distance ≤ body radius).
    ///
    /// Per-cell delta = `min(grass[cell], GRAZE_MAX_PER_TICK * graze_efficiency)`.
    /// Total energy gained = sum of deltas across all overlapping cells.
    /// Energy gained equals exactly the grass density consumed (conservation property).
    ///
    /// Formula locked per v1.2 amendments §A.1 + p1e §5:
    /// efficiency applies in the per-cell cap only (NOT as a post-sum multiplier).
    ///
    /// Runs SEQUENTIALLY regardless of `--features threads` (two creatures may overlap
    /// the same grass cell; parallel writes would race on `density[cell]`). See p1e §3.
    ///
    /// Borrow-workaround: Option B (collect-then-iterate). We collect overlapping cell
    /// indices into a scratch `Vec` using `for_each_cell_overlapping_circle` (which
    /// takes `&self.grass`), then release that immutable borrow before calling
    /// `consume` (which needs `&mut self.grass`). This avoids refactoring the
    /// `for_each_cell_overlapping_circle` API into a free function.
    pub(crate) fn graze(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }

        // Reuse the scratch_neighbors buffer to collect cell indices — avoids
        // a per-creature heap allocation. The buffer is cleared each iteration.
        // (scratch_neighbors holds `usize` already; no type conflict.)
        for i in 0..n {
            if self.creatures.action_this_tick[i] != Action::Graze {
                continue;
            }
            let eff_i = self.creatures.g_graze_eff[i];
            let max_per_cell = GRAZE_MAX_PER_TICK * eff_i;
            if max_per_cell <= 0.0 {
                continue;
            }
            let xi = self.creatures.x[i];
            let yi = self.creatures.y[i];
            let ri = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;

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

        // D6 stub: move_bias deleted. P2f re-roll removed.

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
                // Walled: raw Euclidean displacement for repulsion.
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
        // rebuild when positions are unchanged.
        // Walled: clamp positions to [radius, WORLD_SIZE - radius] after repulsion forces.
        // eat_and_scavenge (step 6) uses the grid, so we must not skip when stale.
        let mut any_moved = false;
        for i in 0..n {
            if self.scratch_fx[i] != 0.0 || self.scratch_fy[i] != 0.0 {
                any_moved = true;
            }
            let ri = self.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
            let lo = ri;
            let hi = WORLD_SIZE - ri;
            let new_x = (self.creatures.x[i] + self.scratch_fx[i]).clamp(lo, hi);
            let new_y = (self.creatures.y[i] + self.scratch_fy[i]).clamp(lo, hi);
            if new_x != self.creatures.x[i] || new_y != self.creatures.y[i] {
                any_moved = true;
            }
            self.creatures.x[i] = new_x;
            self.creatures.y[i] = new_y;
        }

        // S31: skip rebuild when no position change occurred (no repulsion, no wall contact).
        if any_moved {
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
                    // S25: use pooled scratch buffer instead of per-Eat Vec::with_capacity(8).
                    let mut candidates = std::mem::take(&mut self.scratch_eat_candidates);
                    candidates.clear();
                    self.grid.for_each_in_radius(xi, yi, max_range, |j| {
                        if j != i {
                            candidates.push(j);
                        }
                    });
                    for j in candidates.iter().copied() {
                        // Walled: raw Euclidean distance to candidate.
                        let ddx = self.creatures.x[j] - xi;
                        let ddy = self.creatures.y[j] - yi;
                        let d = (ddx * ddx + ddy * ddy).sqrt();
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
                    // S25: restore the pooled buffer (high-water-mark allocation preserved).
                    self.scratch_eat_candidates = candidates;
                    if let Some((j, _)) = best {
                        let armor = self.creatures.genomes[j].armor.clamp(0.0, 1.0); // COLD field; stays AoS
                        let bite_frac = self.sliders.eat_bite_fraction;
                        let prey_energy = self.creatures.energy[j];
                        let transfer = bite_frac * prey_energy * (1.0 - armor);
                        self.scratch_damage[j] += transfer;
                        self.scratch_gain[i] += transfer * eat_eff_i;
                        self.scratch_cooldown_set[i] = true;
                        self.scratch_got_a_bite[i] = true;
                    }
                }
                // D9: Action::Scavenge deleted from enum.
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
            // Size is genome-determined and never changes after birth.
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
        // Clear it before each use; high-water-mark allocation is reused.
        self.scratch_dead.clear();
        let n = self.creatures.len();
        for i in 0..n {
            if self.creatures.energy[i] <= 0.0 {
                // S27: push directly to pending_extinction_check (no species_lost intermediate Vec).
                self.pending_extinction_check
                    .push(self.creatures.species_id[i]);
                self.scratch_dead.push(i);
            }
        }
        // S27: dead indices are in self.scratch_dead; pending_extinction_check was
        // already written directly above (no intermediate species_lost Vec drain).
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;


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

    /// perf-2 test 2: scratch Vecs resize correctly with population each tick.
    ///
    /// P1b/P1e: photosynth income deleted (P1b); graze replacement lands in P1e.
    /// Without energy income, population blooms from pre-loaded energy then crashes.
    /// Verifies: (a) peak population > n0 (splits fired), (b) scratch Vecs >= n-1
    /// each tick (one-tick lag from births is acceptable — movement step catches up),
    /// (c) no panic over the full bloom-crash lifecycle.
    #[test]
    fn scratch_grows_with_population() {
        let mut w = World::new("perf2-grow");
        let n0 = w.creatures.len();
        // Pre-load energy so creatures split rapidly; captures the resize path.
        for i in 0..w.creatures.len() {
            w.creatures.energy[i] = 10_000.0;
        }
        let mut peak_n = n0;
        for _ in 0..500 {
            // Check invariant BEFORE tick: at this point handle_births from the
            // PREVIOUS tick may have added creatures. Scratch Vecs were sized
            // by apply_movement_and_repulsion in the previous tick for n_prev_births.
            // new_births = w.creatures.len() - n_at_last_movement_step; allow lag of 1.
            let n = w.creatures.len();
            peak_n = peak_n.max(n);
            // Each scratch Vec must be at least n-1 (one-tick lag from births is acceptable).
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
                break; // world-end is expected after the energy boom
            }
        }
        // The population must have peaked above the initial count (splits fired).
        assert!(
            peak_n > n0,
            "population should have peaked above initial n0={n0}"
        );
    }

    /// S31: a stationary creature with no overlapping neighbors should NOT
    /// trigger a grid rebuild (any_moved = false AND any_repulsion = false).
    /// We verify correctness by running a tick and confirming the grid is still
    /// valid (correct creature positions) even when the rebuild was skipped.
    #[test]
    fn s31_no_move_no_repulsion_grid_still_valid() {
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

    // ---- D7 walled physics tests ----

    /// D7: a creature with positive vx that would cross the right edge should
    /// be clamped to WORLD_SIZE - radius (not wrap to x ≈ 0).
    #[test]
    fn creature_clamped_at_right_edge() {
        let mut w = World::new("d7-clamp-right");
        let ri = w.creatures.g_size[0] * BODY_RADIUS_PER_SIZE;
        // Move the founder to just inside the right edge, then give it a large vx.
        w.creatures.x[0] = WORLD_SIZE - 1.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 5.0; // would cross the right edge
        w.creatures.vy[0] = 0.0;
        w.creatures.genomes[0].move_speed = 5.0;
        w.creatures.resync_hot_mirrors_at(0);
        // Zero move bias so it doesn't interfere.
        w.creatures.move_bias_x[0] = 0.0;
        w.creatures.move_bias_y[0] = 0.0;
        w.creatures.move_bias_reroll_at[0] = u32::MAX;
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        w.apply_movement_and_repulsion();

        let x = w.creatures.x[0];
        // After wall-clamp, x must be <= WORLD_SIZE - radius.
        assert!(
            x <= WORLD_SIZE - ri + 1e-4,
            "creature must be clamped at right wall; x={x}, limit={}",
            WORLD_SIZE - ri
        );
        // Must not have wrapped to near 0.
        assert!(
            x > 10.0,
            "creature must NOT wrap to near x=0 (walled world); x={x}"
        );
    }

    /// D7: a creature at the left edge moving left is clamped at x=radius.
    #[test]
    fn creature_clamped_at_left_edge() {
        let mut w = World::new("d7-clamp-left");
        let ri = w.creatures.g_size[0] * BODY_RADIUS_PER_SIZE;
        w.creatures.x[0] = 1.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = -5.0; // moving left toward x=0
        w.creatures.vy[0] = 0.0;
        w.creatures.genomes[0].move_speed = 5.0;
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.move_bias_x[0] = 0.0;
        w.creatures.move_bias_y[0] = 0.0;
        w.creatures.move_bias_reroll_at[0] = u32::MAX;
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        w.apply_movement_and_repulsion();

        let x = w.creatures.x[0];
        // Must be at least ri (radius).
        assert!(
            x >= ri - 1e-4,
            "creature must be clamped at left wall; x={x}, limit={ri}"
        );
    }

    /// D7: split-jitter clamps the child position to [radius, WORLD_SIZE - radius].
    #[test]
    fn split_jitter_clamped() {
        use crate::creature::Action;

        let mut w = World::new("d7-jitter-clamp");
        // Place the founder near the left wall.
        w.creatures.x[0] = 2.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.energy[0] = SPLIT_THRESHOLD + 100.0;
        w.creatures.action_this_tick[0] = Action::Split;
        w.handle_births();

        let n = w.creatures.len();
        for i in 1..n {
            let ri = w.creatures.g_size[i] * BODY_RADIUS_PER_SIZE;
            let cx = w.creatures.x[i];
            let cy = w.creatures.y[i];
            // Walled: child must be in [ri, WORLD_SIZE - ri].
            assert!(
                cx >= ri - 1e-4 && cx <= WORLD_SIZE - ri + 1e-4,
                "child x={cx} must be in [ri={ri}, WORLD_SIZE-ri={}]",
                WORLD_SIZE - ri
            );
            assert!(
                cy >= ri - 1e-4 && cy <= WORLD_SIZE - ri + 1e-4,
                "child y={cy} must be in [ri={ri}, WORLD_SIZE-ri={}]",
                WORLD_SIZE - ri
            );
        }
    }

    /// D7: two creatures far on opposite sides of the world (x=2 vs x=598)
    /// do NOT repel each other via phantom seam crossing in the walled world.
    #[test]
    fn repulsion_does_not_cross_seam() {
        use crate::brain::Brain;
        use crate::genome::Genome;

        let mut w = World::new("d7-repulsion-no-seam");
        w.creatures.x[0] = 2.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 0.0;
        w.creatures.vy[0] = 0.0;

        let mut rng = SimRng::from_u64(123);
        let g2 = Genome::founder();
        let b2 = Brain::founder(&mut rng);
        let n_before = w.creatures.len();
        use crate::vision::VISION_LEN;
        w.creatures
            .push(1, 598.0, WORLD_SIZE * 0.5, FOUNDER_ENERGY, 0, 0, 0, g2, b2);
        w.vision.push([0.0f32; VISION_LEN]);
        w.creatures.vx[n_before] = 0.0;
        w.creatures.vy[n_before] = 0.0;
        w.creatures.move_bias_x[0] = 0.0;
        w.creatures.move_bias_y[0] = 0.0;
        w.creatures.move_bias_reroll_at[0] = u32::MAX;
        w.creatures.move_bias_x[n_before] = 0.0;
        w.creatures.move_bias_y[n_before] = 0.0;
        w.creatures.move_bias_reroll_at[n_before] = u32::MAX;

        let x0_before = w.creatures.x[0];
        let x1_before = w.creatures.x[n_before];

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);
        w.apply_movement_and_repulsion();

        // In the walled world, A at x=2 and B at x=598 are 596 units apart —
        // far beyond any repulsion range. Neither should have moved due to repulsion.
        let dx0 = (w.creatures.x[0] - x0_before).abs();
        let dx1 = (w.creatures.x[n_before] - x1_before).abs();
        assert!(
            dx0 < 1e-4,
            "creature A at x=2 must not be repelled by far-side B; moved {dx0}"
        );
        assert!(
            dx1 < 1e-4,
            "creature B at x=598 must not be repelled by far-side A; moved {dx1}"
        );
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

    /// P1e test 2: creature overlapping multiple seeded cells sums deltas correctly.
    ///
    /// Place creature at the center of a 5×5 seeded patch (density = GRASS_MAX).
    /// Every overlapping cell should yield exactly GRAZE_MAX_PER_TICK (since
    /// graze_eff = 1.0 and density >= cap). Total gain = overlap_count × cap.
    #[test]
    fn graze_n_cell_overlap_sums_capped_deltas() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX, GRAZE_MAX_PER_TICK};

        let mut w = World::new("graze-patch");
        // Place creature at center of world, radius from size=4.0 body.
        let cx = WORLD_SIZE * 0.5;
        let cy = WORLD_SIZE * 0.5;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.genomes[0].size = 4.0;
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.genomes[0].graze_efficiency = 1.0;
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.action_this_tick[0] = Action::Graze;

        // Seed the entire grass grid with GRASS_MAX so every overlapping cell yields the cap.
        w.grass.density.fill(GRASS_MAX);

        // Count how many cells overlap (use the same method under test).
        let ri = w.creatures.g_size[0] * BODY_RADIUS_PER_SIZE;
        let mut overlap_count = 0usize;
        w.grass.for_each_cell_overlapping_circle(cx, cy, ri, |_| {
            overlap_count += 1;
        });
        assert!(
            overlap_count > 0,
            "creature with size=4 should overlap at least one cell"
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
        // Confirm grass density actually decreased.
        let expected_density = GRASS_MAX - GRAZE_MAX_PER_TICK;
        let center_cell = (((cy / GRASS_CELL_SIZE).floor() as usize) * GRASS_GRID_DIM)
            + ((cx / GRASS_CELL_SIZE).floor() as usize);
        assert!(
            (w.grass.density[center_cell] - expected_density).abs() < 1e-5,
            "center cell density should have dropped by GRAZE_MAX_PER_TICK; got {}",
            w.grass.density[center_cell]
        );
    }

    /// P1e test 4: single cell at density 1.0 with eff=1.0 → gain == GRAZE_MAX_PER_TICK exactly.
    #[test]
    fn graze_per_cell_cap_holds() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX, GRAZE_MAX_PER_TICK};

        let mut w = World::new("graze-cap");
        // Place creature exactly at cell center of cell (ix=2, iy=2).
        let ix = 2usize;
        let iy = 2usize;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        // Use a tiny radius (< half cell size) so only the center cell is overlapped.
        w.creatures.genomes[0].size = 0.5; // radius = 0.5 < GRASS_CELL_SIZE/2 = 1.25
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.genomes[0].graze_efficiency = 1.0;
        w.creatures.resync_hot_mirrors_at(0);
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

    /// P1e test 5: graze_efficiency = 0 → no energy gained, no density change.
    #[test]
    fn graze_zero_efficiency_is_noop() {
        use crate::constants::{GRASS_CELL_SIZE, GRASS_GRID_DIM, GRASS_MAX};

        let mut w = World::new("graze-zero-eff");
        let ix = 5usize;
        let iy = 5usize;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.genomes[0].size = 1.0;
        w.creatures.genomes[0].graze_efficiency = 0.0;
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.action_this_tick[0] = Action::Graze;

        let cell_idx = iy * GRASS_GRID_DIM + ix;
        w.grass.density[cell_idx] = GRASS_MAX;

        let energy_before = w.creatures.energy[0];
        let density_before = w.grass.density[cell_idx];
        w.graze();

        assert_eq!(
            w.creatures.energy[0], energy_before,
            "zero efficiency must yield no energy gain"
        );
        assert_eq!(
            w.grass.density[cell_idx], density_before,
            "zero efficiency must leave density unchanged"
        );
    }

    /// P1e test 6: energy gained == grass density consumed (conservation property).
    ///
    /// Formula: gain = Σ min(grass[k], GRAZE_MAX_PER_TICK * eff).
    /// Efficiency enters only in the per-cell cap; energy gained equals grass consumed.
    #[test]
    fn graze_conserves_energy_with_grass_drain() {
        use crate::constants::{GRASS_MAX, GRAZE_MAX_PER_TICK};

        let mut w = World::new("graze-conserve");
        // Seed a large area so many cells are overlapped.
        w.grass.density.fill(GRASS_MAX);

        // Use size=3 to ensure multi-cell overlap.
        let cx = WORLD_SIZE * 0.5;
        let cy = WORLD_SIZE * 0.5;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.genomes[0].size = 3.0;
        w.creatures.genomes[0].graze_efficiency = 0.8; // non-trivial efficiency
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.action_this_tick[0] = Action::Graze;

        // Snapshot total density before graze.
        let density_before: f32 = w.grass.density.iter().sum();
        let energy_before = w.creatures.energy[0];

        w.graze();

        // Snapshot total density after.
        let density_after: f32 = w.grass.density.iter().sum();
        let energy_after = w.creatures.energy[0];

        let density_consumed = density_before - density_after;
        let energy_gained = energy_after - energy_before;

        // Conservation: energy_gained must equal density_consumed up to float-sum precision.
        // Tolerance scales with cap because larger per-tick deltas accumulate larger rounding.
        assert!(
            (energy_gained - density_consumed).abs() < 1e-2,
            "conservation violated: gained={energy_gained} consumed={density_consumed}"
        );
        // Sanity: the creature should have gained something (it's on full grass).
        let ri = w.creatures.g_size[0] * BODY_RADIUS_PER_SIZE;
        let eff = w.creatures.g_graze_eff[0];
        let expected_cap = GRAZE_MAX_PER_TICK * eff;
        let mut overlap_count = 0usize;
        w.grass
            .for_each_cell_overlapping_circle(cx, cy, ri, |_| overlap_count += 1);
        let expected_gain = overlap_count as f32 * expected_cap.min(GRASS_MAX);
        assert!(
            (energy_gained - expected_gain).abs() < 1e-3,
            "energy_gained={energy_gained} expected={expected_gain}"
        );
    }

    // ---- P2f move_bias tests ----

    /// P2f test 1: move_bias_reroll fires after exactly MOVE_BIAS_REROLL_INTERVAL ticks.
    /// Pins the re-roll cadence: bias stays constant until tick == reroll_at, then fires.
    /// Uses apply_movement_and_repulsion directly to isolate the mechanism.
    #[test]
    fn move_bias_reroll_fires_every_20_ticks() {
        let mut w = World::new("p2f-rerolls");
        // Start with tick = 100 to avoid zero-tick edge cases.
        w.tick = 100;
        // Fix a known bias value; set reroll_at to tick + interval = 120.
        w.creatures.move_bias_x[0] = 0.123;
        w.creatures.move_bias_reroll_at[0] = w.tick + MOVE_BIAS_REROLL_INTERVAL; // 120
        let before_x = w.creatures.move_bias_x[0];

        // Run (interval - 1) ticks (ticks 100..119): tick < reroll_at each time, no re-roll.
        for _ in 0..(MOVE_BIAS_REROLL_INTERVAL - 1) {
            w.apply_movement_and_repulsion(); // tick=100..118 each call
            w.tick += 1; // advance tick AFTER movement (mirrors World::step ordering)
        }
        // Now tick = 119. reroll_at = 120. Condition: 119 >= 120 → false. No re-roll yet.
        assert_eq!(
            w.creatures.move_bias_x[0], before_x,
            "bias must not re-roll before reroll_at is reached (tick={} reroll_at={})",
            w.tick, w.creatures.move_bias_reroll_at[0]
        );

        // Advance tick to 120 (= reroll_at), then call movement: re-roll fires.
        w.tick += 1; // tick = 120
        w.apply_movement_and_repulsion(); // tick=120 >= reroll_at=120 → fires
        assert_ne!(
            w.creatures.move_bias_x[0], before_x,
            "bias must re-roll when tick reaches reroll_at (tick={})",
            w.tick
        );
    }

    /// P2f test 2: move_bias adds to velocity (ADD semantics per amendments §A.5).
    /// With all-zero NN weights, the NN contributes nothing; only move_bias drives motion.
    /// Pins that SET is NOT implemented (a pure SET would also pass, but the ADD
    /// semantics are confirmed by the NN-output being zero before the bias ADD).
    #[test]
    fn move_bias_adds_to_velocity() {
        let mut w = World::new("p2f-add");
        // Zero all NN weights so NN output is zero for both vx and vy.
        w.creatures.brains[0].weights = vec![0.0; NN_WEIGHT_COUNT];
        // Set a known bias; prevent re-roll during the test.
        w.creatures.move_bias_x[0] = 0.5;
        w.creatures.move_bias_y[0] = -0.5;
        w.creatures.move_bias_reroll_at[0] = u32::MAX;
        // Give the creature move_speed > 0 so bias contributes.
        w.creatures.genomes[0].move_speed = MOVE_SPEED_MAX;
        w.creatures.resync_hot_mirrors_at(0);

        w.step();

        // NN output is zero → only move_bias contributes.
        // Expected: vx > 0 (bias_x=0.5), vy < 0 (bias_y=-0.5).
        // (vx/vy may have been clamped by speed cap or zeroed by Action::Rest,
        //  but the bias was added before the clamp — track the step result.)
        // The movement pass adds bias then clamps; with speed_cap = MOVE_SPEED_MAX
        // and combined input = 0 + 0.5 * MOVE_SPEED_MAX = 2.5 (< cap=5), no clamping.
        // After movement, vx is reset by the NN in the next tick's forward pass.
        // We check the creature moved in the expected direction.
        // NOTE: step() runs the full tick (NN → movement → ...), so by the END of the
        // tick the NN has already been run for tick+1 and overwritten vx/vy.
        // Instead, we verify by checking x/y moved relative to start.
        // x increased (bias_x > 0 → moved right) and y decreased (bias_y < 0 → moved up).
        // The exact position shift should be ≈ 0.5 * MOVE_SPEED_MAX = 2.5 world units.
        // We assert direction only (sign), not magnitude.
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
    /// P3e may revise this; update in lockstep.
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
    /// Prey loses 50, predator gains 50 - COST_EAT_ATTEMPT = 49.7.
    #[test]
    fn p3a_eat_bite_basic_transfer() {
        use crate::brain::Brain;
        use crate::genome::Genome;
        use crate::vision::VISION_LEN;

        let mut w = World::new("p3a-basic");
        // Set predator (creature 0) properties.
        w.creatures.energy[0] = 100.0;
        w.creatures.genomes[0].eat_efficiency = 1.0;
        w.creatures.genomes[0].armor = 0.0;
        w.creatures.genomes[0].size = 1.0;
        w.creatures.genomes[0].bite_reach = 3.0; // generous reach
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.digestion_cooldown[0] = 0;
        w.creatures.action_this_tick[0] = Action::Eat;
        w.sliders.eat_bite_fraction = 0.5;

        // Add prey (creature 1) adjacent.
        let mut rng = SimRng::from_u64(42);
        let mut prey_genome = Genome::founder();
        prey_genome.armor = 0.0;
        prey_genome.size = 1.0;
        let prey_brain = Brain::founder(&mut rng);
        let pred_x = w.creatures.x[0];
        let pred_y = w.creatures.y[0];
        // Place prey within bite reach.
        w.creatures.push(
            1,
            pred_x + 1.5,
            pred_y,
            100.0,
            0,
            0,
            0,
            prey_genome,
            prey_brain,
        );
        w.vision.push([0.0f32; VISION_LEN]);

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

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

    /// P3a test: bite with armor attenuation — armor=0.3, eff=0.8, bite_frac=0.5, prey energy=100.
    /// transfer = 0.5 * 100 * 0.7 = 35; prey loses 35, predator gains 35 * 0.8 = 28.
    #[test]
    fn p3a_eat_bite_with_armor() {
        use crate::brain::Brain;
        use crate::genome::Genome;
        use crate::vision::VISION_LEN;

        let mut w = World::new("p3a-armor");
        w.creatures.energy[0] = 100.0;
        w.creatures.genomes[0].eat_efficiency = 0.8;
        w.creatures.genomes[0].armor = 0.0;
        w.creatures.genomes[0].size = 1.0;
        w.creatures.genomes[0].bite_reach = 3.0;
        w.creatures.resync_hot_mirrors_at(0);
        w.creatures.digestion_cooldown[0] = 0;
        w.creatures.action_this_tick[0] = Action::Eat;
        w.sliders.eat_bite_fraction = 0.5;

        let mut rng = SimRng::from_u64(99);
        let mut prey_genome = Genome::founder();
        prey_genome.armor = 0.3;
        prey_genome.size = 1.0;
        let prey_brain = Brain::founder(&mut rng);
        let pred_x = w.creatures.x[0];
        let pred_y = w.creatures.y[0];
        w.creatures.push(
            1,
            pred_x + 1.5,
            pred_y,
            100.0,
            0,
            0,
            0,
            prey_genome,
            prey_brain,
        );
        w.vision.push([0.0f32; VISION_LEN]);

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        let prey_energy_before = w.creatures.energy[1];
        let pred_energy_before = w.creatures.energy[0];

        w.eat_and_scavenge();

        let prey_loss = prey_energy_before - w.creatures.energy[1];
        // predator net = gain - COST_EAT_ATTEMPT; compute gain from total change
        let pred_net = w.creatures.energy[0] - pred_energy_before;

        // transfer = 0.5 * 100 * (1 - 0.3) = 35
        // prey loss = 35; predator gain = 35 * 0.8 = 28; net = 28 - 0.3 = 27.7
        assert!(
            (prey_loss - 35.0).abs() < 1e-4,
            "prey should lose 35.0; lost {prey_loss}"
        );
        assert!(
            (pred_net - 27.7).abs() < 1e-3,
            "predator net should be 27.7 (28 - 0.3 cost); got {pred_net}"
        );
    }

    /// P3a test: digestion cooldown still gates bites — only attempt cost charged, no bite.
    #[test]
    fn p3a_eat_cooldown_still_gates() {
        use crate::brain::Brain;
        use crate::genome::Genome;
        use crate::vision::VISION_LEN;

        let mut w = World::new("p3a-cooldown");
        w.creatures.energy[0] = 100.0;
        w.creatures.genomes[0].eat_efficiency = 1.0;
        w.creatures.genomes[0].size = 1.0;
        w.creatures.genomes[0].bite_reach = 3.0;
        w.creatures.resync_hot_mirrors_at(0);
        // Set active cooldown so bite is gated.
        w.creatures.digestion_cooldown[0] = 10;
        w.creatures.action_this_tick[0] = Action::Eat;
        w.sliders.eat_bite_fraction = 0.5;

        let mut rng = SimRng::from_u64(7);
        let prey_genome = Genome::founder();
        let prey_brain = Brain::founder(&mut rng);
        let pred_x = w.creatures.x[0];
        let pred_y = w.creatures.y[0];
        w.creatures.push(
            1,
            pred_x + 1.5,
            pred_y,
            100.0,
            0,
            0,
            0,
            prey_genome,
            prey_brain,
        );
        w.vision.push([0.0f32; VISION_LEN]);

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

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
        // Predator should only lose the attempt cost (0.3).
        assert!(
            (pred_change - COST_EAT_ATTEMPT).abs() < 1e-4,
            "predator should only lose COST_EAT_ATTEMPT ({COST_EAT_ATTEMPT}); lost {pred_change}"
        );
    }
}
