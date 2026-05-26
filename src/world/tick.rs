//! Per-tick step bodies that mutate `World`. All as `impl World` blocks; private to the `crate::world` parent.
//!
//! D3: Genome removed. All body trait reads replaced with constants:
//!   g_graze_eff → 1.0 (GRAZE_EFF_CONSTANT), g_size → FOUNDER_SIZE,
//!   g_move_speed → MOVE_SPEED_MAX, g_eye_count/g_vision_range → constant 24/VISION_RANGE_MAX,
//!   g_eat_eff/g_scav_eff → 1.0 (always enabled), armor/bite_reach → 0.0/BITE_REACH_CONSTANT.
//!   HallOfFame snapshots no longer include genome or captured_size.

use super::World;
use crate::constants::*;
use crate::creature::Action;

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

        // D3: speed_cap = MOVE_SPEED_MAX for all creatures (constant).
        let speed_cap = MOVE_SPEED_MAX;
        for i in 0..n {
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
                // D3: rj == ri. D7: walled, raw Euclidean displacement.
                let dx = xj - xi;
                let dy = yj - yi;
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

        // S31: track whether any position changed to skip the final grid rebuild.
        // D7: wall-clamp (no wrap). D3: ri is constant FOUNDER_SIZE * BODY_RADIUS_PER_SIZE.
        let mut any_moved = false;
        let ri = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
        let lo = ri;
        let hi = WORLD_SIZE - ri;
        for i in 0..n {
            if self.scratch_fx[i] != 0.0 || self.scratch_fy[i] != 0.0 {
                any_moved = true;
            }
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

        // D3: all creatures have eat_eff = 1.0, scav_eff = 1.0, armor = 0.0,
        //     bite_reach = BITE_REACH_CONSTANT, size = FOUNDER_SIZE (all constant).
        let eat_eff = 1.0_f32;
        let scav_eff = 1.0_f32;
        let size = FOUNDER_SIZE;
        let bite_reach = FOUNDER_BITE_REACH;
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
                        // D7: walled, raw Euclidean distance.
                        let ddx = self.creatures.x[j] - xi;
                        let ddy = self.creatures.y[j] - yi;
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
                self.scratch_dead.push(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    // ---- E.25.b test ----

    /// E.25.b: first_move_fired flips after distance_travelled crosses 5.0.
    /// D3: move_speed is always MOVE_SPEED_MAX; events system deleted by D4.
    #[test]
    fn e25b_first_to_move_fires_on_movement_step() {
        let mut w = World::new("e25b-move");
        // Set velocity directly so movement fires deterministically.
        w.creatures.vx[0] = 5.0;
        w.creatures.vy[0] = 0.0;
        // Pre-condition: distance_travelled starts at 0, flag not set.
        assert!(!w.first_move_fired);
        assert_eq!(w.creatures.distance_travelled[0], 0.0);

        // Run the movement step directly.
        w.apply_movement_and_repulsion();

        // After movement, distance_travelled should be >= 5.0 and flag should flip.
        assert!(
            w.creatures.distance_travelled[0] >= 5.0,
            "distance_travelled = {}",
            w.creatures.distance_travelled[0]
        );
        assert!(
            w.first_move_fired,
            "first_move_fired must be true after crossing 5.0"
        );
    }

    // ---- E.25.d tests ----
    // NOTE: biggest_ever, weirdest, longest_lived, last_survivor were HallOfFame fields
    // deleted by D5. These tests are replaced by simplified death-tracking tests.

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

    // ---- S24 tests ----
    // NOTE: Carrion was deleted by D2. Scavenge tests that required carrion are removed.
    /// D2/S24/D9: eat_and_scavenge on an empty world does not panic.
    /// D2 deleted carrion; D9 collapsed Action to 3 variants (no Scavenge).
    #[test]
    fn scavenge_on_empty_world_does_not_panic() {
        let mut w = World::new("s24-empty");
        w.creatures.action_this_tick[0] = Action::Graze;
        // Should not panic; no carrion in D2+ codebase.
        w.eat_and_scavenge();
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
    /// perf-2: scratch Vecs stay consistent with population size each tick.
    /// D3/D9: split action not reachable via 3-logit decode until D9 collapses enum.
    /// Test verifies scratch vec sizing invariant across 50 ticks.
    #[test]
    fn scratch_grows_with_population() {
        let mut w = World::new("perf2-grow");
        // Give infinite energy so creatures survive; check scratch vec sizes each tick.
        w.creatures.energy[0] = SPLIT_THRESHOLD + 10_000.0;

        for _ in 0..50 {
            w.tick_once();
            let n = w.creatures.len();
            let floor = n.saturating_sub(1);
            assert!(
                w.scratch_fx.len() >= floor,
                "scratch_fx {} < floor {}, n={}",
                w.scratch_fx.len(), floor, n
            );
            assert!(
                w.scratch_fy.len() >= floor,
                "scratch_fy {} < floor {}, n={}",
                w.scratch_fy.len(), floor, n
            );
            assert!(w.scratch_damage.len() >= floor);
            assert!(w.scratch_gain.len() >= floor);
            assert!(w.scratch_cooldown_set.len() >= floor);
            assert!(w.scratch_attempted_eat.len() >= floor);
            assert!(w.scratch_attempted_scavenge.len() >= floor);
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
        w.creatures.x[0] = WORLD_SIZE - 1.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 5.0; // would cross the right wall
        w.creatures.vy[0] = 0.0;
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        w.apply_movement_and_repulsion();

        let x = w.creatures.x[0];
        // D7: position must be clamped to [lo, hi] where lo=ri, hi=WORLD_SIZE-ri.
        let ri = FOUNDER_SIZE * BODY_RADIUS_PER_SIZE;
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
            .push(1, 598.0, WORLD_SIZE * 0.5, FOUNDER_ENERGY, 0, b2);
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
    // NOTE: MOVE_BIAS_REROLL_INTERVAL constant and its re-roll logic were deleted by D6/D8.
    // The move_bias fields remain in CreatureSoA until D9 removes them.

    /// D9: move_bias fields are deleted from CreatureSoA entirely.
    /// This test verifies no-panic on zero-velocity creature.
    #[test]
    fn move_bias_fields_deleted_d9() {
        let w = World::new("p2f-fields");
        // D9 removed move_bias_x/y/reroll_at; just confirm creature exists at index 0.
        assert!(w.creatures.len() > 0, "world must spawn at least one creature");
        // vx/vy are the only velocity fields now.
        let _vx = w.creatures.vx[0];
        let _vy = w.creatures.vy[0];
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
    /// D3: all creatures have eat_eff=1.0, armor=0.0 as constants. D2: no carrion.
    #[test]
    fn p3a_eat_bite_basic_transfer() {
        use crate::brain::Brain;
        use crate::vision::VISION_LEN;

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
            .push(1, pred_x + 1.5, pred_y, 100.0, 0, prey_brain);
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

    /// P3a test: digestion cooldown still gates bites — only attempt cost charged, no bite.
    #[test]
    fn p3a_eat_cooldown_still_gates() {
        use crate::brain::Brain;
        use crate::vision::VISION_LEN;

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
            .push(1, pred_x + 1.5, pred_y, 100.0, 0, prey_brain);
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
        // Predator should only lose the attempt cost.
        assert!(
            (pred_change - COST_EAT_ATTEMPT).abs() < 1e-4,
            "predator should only lose COST_EAT_ATTEMPT ({COST_EAT_ATTEMPT}); lost {pred_change}"
        );
    }
}
