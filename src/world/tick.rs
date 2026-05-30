//! Per-tick step bodies that mutate `World`. All as `impl World` blocks; private to the `crate::world` parent.
//!
//! D3: Genome removed. All body trait reads replaced with constants:
//!   g_graze_eff → 1.0, g_size → CREATURE_SIZE,
//!   g_move_speed → MOVE_SPEED_MAX,
//!   g_eat_eff → 1.0 (always enabled), armor/bite_reach → 0.0.
//! D9: Scavenge action removed; eat_and_scavenge renamed to eat.

use super::World;
use crate::constants::*;
use crate::creature::Action;

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

        let ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE; // D3: size = CREATURE_SIZE always
        let bites_per_block = self.sliders.grass_bites_per_block.max(1) as f32;
        let density_chunk = GRASS_MAX / bites_per_block;
        let energy_per_bite = self.sliders.grass_energy_per_bite;

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
                total_gain += self.grass.consume(cell_idx, density_chunk, energy_per_bite);
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
        let move_mult = self.sliders.move_cost_multiplier * self.current_curriculum_factor;
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
            self.creatures.energy[i] -= dist * COST_MOVE_PER_DIST * move_mult;
            self.creatures.distance_travelled[i] += dist;
            self.creatures.x[i] += cvx;
            self.creatures.y[i] += cvy;
        }

        self.grid.rebuild(&self.creatures.x, &self.creatures.y);

        // D3: ri is constant for all creatures.
        let ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        let search = ri + ri; // rsum_max = 2 * ri (all same size)
        let rep_max = self.sliders.repulsion_max;

        self.scratch_fx.resize(n, 0.0);
        self.scratch_fy.resize(n, 0.0);
        self.scratch_fx.fill(0.0);
        self.scratch_fy.fill(0.0);
        // Skip the per-creature neighbor scan entirely when repulsion is disabled.
        // Every force computed below is clamped to [0, rep_max], so rep_max=0 yields
        // no movement contribution — but the scan itself is O(N·K) and explodes when
        // creatures clump (which happens precisely *because* repulsion is off).
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
                            let f = (REPULSION_K * overlap).clamp(0.0, rep_max);
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

        // S31: track whether any position changed to skip the final grid rebuild.
        // D7: wall-clamp (no wrap). D3: ri is constant CREATURE_SIZE * BODY_RADIUS_PER_SIZE.
        let mut any_moved = false;
        let ri = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
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

    /// Eat resolution. Scavenge action removed in D9; function renamed from eat_and_scavenge.
    pub(crate) fn eat(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        self.scratch_damage.resize(n, 0.0);
        self.scratch_gain.resize(n, 0.0);
        self.scratch_cooldown_set.resize(n, false);
        self.scratch_attempted_eat.resize(n, false);
        self.scratch_got_a_bite.resize(n, false);
        self.scratch_damage.fill(0.0);
        self.scratch_gain.fill(0.0);
        self.scratch_cooldown_set.fill(false);
        self.scratch_attempted_eat.fill(false);
        self.scratch_got_a_bite.fill(false);

        // D3: all creatures have eat_eff = 1.0, armor = 0.0, bite_reach = 0.0,
        // size = CREATURE_SIZE (all constant).
        let eat_eff = 1.0_f32;
        let size = CREATURE_SIZE;
        let bite_reach = 0.0_f32;
        let armor = 0.0_f32;

        for i in 0..n {
            if self.creatures.action_this_tick[i] == Action::Eat {
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
        }

        for i in 0..n {
            if self.scratch_attempted_eat[i] && self.scratch_cooldown_set[i] {
                self.creatures.digestion_cooldown[i] = self.sliders.digestion_cooldown_ticks;
            }
            self.creatures.energy[i] += self.scratch_gain[i];
            self.creatures.energy[i] -= self.scratch_damage[i];
        }
    }

    pub(crate) fn energy_bookkeeping(&mut self) {
        // D3: all variable body-trait upkeep terms removed. Flat formula:
        //   up = UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + mouth_tax
        // (UPKEEP_GUT for gut always present, mouth_tax for eat_eff always > 0)
        let mouth_tax = self.sliders.mouth_tax;
        let up_base = UPKEEP_BASE + UPKEEP_NN_FIXED + UPKEEP_GUT + mouth_tax;
        let mult = self.sliders.upkeep_multiplier * self.current_curriculum_factor;
        let energy_cap = self.sliders.energy_max;
        let max_age = self.sliders.max_age;
        for i in 0..self.creatures.len() {
            let mut up = up_base * mult;
            let age = self.creatures.age[i];
            if age > max_age {
                let excess = (age - max_age) as f32;
                let age_mult = PAST_LIFESPAN_MULT.powf(excess / 1000.0);
                up *= age_mult.min(1e6);
            }

            // D3: max_size_reached removed (was always CREATURE_SIZE anyway).
            self.creatures.energy[i] -= up;
            // Live-tunable energy cap (clamp gains at end of tick).
            if self.creatures.energy[i] > energy_cap {
                self.creatures.energy[i] = energy_cap;
            }
            self.creatures.cumulative_upkeep[i] += up;
            if self.creatures.digestion_cooldown[i] > 0 {
                self.creatures.digestion_cooldown[i] -= 1;
            }
            self.creatures.age[i] += 1;
        }
    }

    /// v1.5 S3: per-creature action-EMA color update.
    /// Green (Graze): +0.01 if argmax-pre==Graze else -0.01.
    /// Blue (Split):  +0.5 on Split tick else -0.005.
    /// Red (Eat):     +0.1 per successful bite else -0.001.
    /// All channels clamped to [0, 1]. Scratch buffers (`scratch_argmax_pre`,
    /// `scratch_got_a_bite`) must already be index-aligned with `creatures`
    /// over `0..creatures.len()`. Newborns appended this tick (indices beyond
    /// the scratch length) keep their initial (0,0,0) EMA.
    pub(crate) fn color_ema_update(&mut self) {
        let n = self.creatures.len();
        let m = n
            .min(self.scratch_argmax_pre.len())
            .min(self.scratch_got_a_bite.len());
        for i in 0..m {
            let dg = if self.scratch_argmax_pre[i] == Action::Graze as u8 {
                0.5
            } else {
                -0.05
            };
            self.creatures.color_g[i] = (self.creatures.color_g[i] + dg).clamp(0.0, 1.0);

            let db = if self.creatures.action_this_tick[i] == Action::Split {
                1.0
            } else {
                -0.01
            };
            self.creatures.color_b[i] = (self.creatures.color_b[i] + db).clamp(0.0, 1.0);

            let dr = if self.scratch_got_a_bite[i] {
                1.0
            } else {
                -0.01
            };
            self.creatures.color_r[i] = (self.creatures.color_r[i] + dr).clamp(0.0, 1.0);
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
        use crate::brain::Brain;

        let mut w = World::new("p1a-repulsion-seam");
        w.creatures.x[0] = 2.0;
        w.creatures.y[0] = WORLD_SIZE * 0.5;
        w.creatures.vx[0] = 0.0;
        w.creatures.vy[0] = 0.0;

        let mut rng = SimRng::from_u64(123);
        let b2 = Brain::founder(&mut rng);
        let n_before = w.creatures.len();
        w.creatures
            .push(1, 598.0, WORLD_SIZE * 0.5, START_ENERGY_DEFAULT, 0, b2);
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
    /// D3: CREATURE_SIZE=1.0, BODY_RADIUS_PER_SIZE=1.0 → body radius=1.0.
    /// We position the creature exactly at cell (30,30)'s center so it
    /// overlaps at least one cell.
    #[test]
    fn graze_n_cell_overlap_sums_capped_deltas() {
        use crate::constants::{
            GRASS_BITES_PER_BLOCK_DEFAULT, GRASS_CELL_SIZE, GRASS_ENERGY_PER_BITE_DEFAULT,
            GRASS_GRID_DIM, GRASS_MAX,
        };

        let mut w = World::new("graze-patch");
        // D3: size = CREATURE_SIZE always. Position creature at cell (30,30) center.
        let cell_ix: usize = 30;
        let cell_iy: usize = 30;
        let cx = (cell_ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (cell_iy as f32 + 0.5) * GRASS_CELL_SIZE;
        w.creatures.x[0] = cx;
        w.creatures.y[0] = cy;
        w.creatures.action_this_tick[0] = Action::Graze;

        // Seed the entire grass grid with GRASS_MAX.
        w.grass.density.fill(GRASS_MAX);

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
        let density_chunk = GRASS_MAX / GRASS_BITES_PER_BLOCK_DEFAULT as f32;
        let expected_density = GRASS_MAX - density_chunk;
        let center_cell = cell_iy * GRASS_GRID_DIM + cell_ix;
        assert!(
            (w.grass.density[center_cell] - expected_density).abs() < 1e-5,
            "center cell density should have dropped by one chunk ({density_chunk}); got {}",
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

        // Walled world: a creature at x=0 cannot reach the east seam.
        let iy = ((cy / GRASS_CELL_SIZE).floor() as usize).min(GRASS_GRID_DIM - 1);
        let east_cell = iy * GRASS_GRID_DIM + (GRASS_GRID_DIM - 1);
        let west_cell = iy * GRASS_GRID_DIM;
        w.grass.density[east_cell] = GRASS_MAX;
        w.grass.density[west_cell] = GRASS_MAX;

        w.graze();

        // With the AABB overlap fix, a creature at cx=0 always touches west_cell (ix=0)
        // because its box [0, GRASS_CELL_SIZE] contains the creature position
        // (nearest point = 0, dist = 0). The east cell (ix=dim-1) is far away and
        // not reached (walled world). West cell must be drained.
        let west_drained = w.grass.density[west_cell] < GRASS_MAX - 1e-6;
        assert!(
            west_drained,
            "west seam cell (ix=0) must be grazed; creature at x=0 is inside cell box [0,5]; density={}",
            w.grass.density[west_cell]
        );
    }

    /// P1e test 4: single cell at density 1.0 → gain == GRASS_ENERGY_PER_BITE_DEFAULT.
    #[test]
    fn graze_per_cell_cap_holds() {
        use crate::constants::{
            GRASS_BITES_PER_BLOCK_DEFAULT, GRASS_CELL_SIZE, GRASS_ENERGY_PER_BITE_DEFAULT,
            GRASS_GRID_DIM, GRASS_MAX,
        };

        let mut w = World::new("graze-cap");
        let ix = 2usize;
        let iy = 2usize;
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

        assert!(
            (gained - GRASS_ENERGY_PER_BITE_DEFAULT).abs() < 1e-4,
            "gained={gained} expected={GRASS_ENERGY_PER_BITE_DEFAULT}"
        );
        let density_chunk = GRASS_MAX / GRASS_BITES_PER_BLOCK_DEFAULT as f32;
        assert!(
            (w.grass.density[cell_idx] - (GRASS_MAX - density_chunk)).abs() < 1e-5,
            "density should drop by one chunk ({density_chunk}); got {}",
            w.grass.density[cell_idx]
        );
    }

    /// P1e test 5: single ripe cell → gain == GRASS_ENERGY_PER_BITE_DEFAULT.
    #[test]
    fn graze_constant_efficiency_produces_correct_gain() {
        use crate::constants::{
            GRASS_CELL_SIZE, GRASS_ENERGY_PER_BITE_DEFAULT, GRASS_GRID_DIM, GRASS_MAX,
        };

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
        let density_chunk = GRASS_MAX / GRASS_BITES_PER_BLOCK_DEFAULT as f32;
        let bites_from_energy = energy_gained / GRASS_ENERGY_PER_BITE_DEFAULT;
        let bites_from_density = density_consumed / density_chunk;
        assert!(
            (bites_from_energy - bites_from_density).abs() < 1e-3,
            "bite count from energy ({bites_from_energy}) must match density side ({bites_from_density})"
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
    /// D3: all creatures have eat_eff=1.0, armor=0.0 as constants.
    #[test]
    fn p3a_eat_bite_basic_transfer() {
        use crate::brain::Brain;

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
        // Place prey within bite reach (0.0 since bite_reach = 0).
        w.creatures
            .push(1, pred_x + 1.5, pred_y, 100.0, 0, prey_brain);

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        let pred_energy_before = w.creatures.energy[0];
        let prey_energy_before = w.creatures.energy[1];

        w.eat();

        let pred_energy_after = w.creatures.energy[0];
        let prey_energy_after = w.creatures.energy[1];

        // transfer = 0.5 * 100.0 * (1.0 - 0.0) = 50.0
        // predator gain = transfer * eat_eff
        // prey loss = transfer
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
    fn p3a_eat_cooldown_still_gates() {
        use crate::brain::Brain;

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

        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        let prey_energy_before = w.creatures.energy[1];
        let pred_energy_before = w.creatures.energy[0];

        w.eat();

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

    // ---- v1.5 S3 color-EMA tests ----

    /// v1.5 S3: one EMA tick where argmax-pre==Graze, action==Graze, no bite.
    /// Green +0.01, blue −0.005 (clamped to 0), red −0.001 (clamped to 0).
    /// Then a Split tick from non-zero blue bumps blue by +0.5 (clamped to 1).
    #[test]
    fn color_ema_increments_on_action() {
        let mut w = World::new("s3-color-ema");
        let n = w.creatures.len();

        // --- Tick 1: Graze pre-argmax, Graze action, no bite ---
        w.creatures.action_this_tick[0] = Action::Graze;
        w.scratch_argmax_pre.resize(n, Action::Graze as u8);
        w.scratch_got_a_bite.resize(n, false);

        w.color_ema_update();

        assert!(
            (w.creatures.color_g[0] - 0.5).abs() < 1e-6,
            "green expected 0.5 got {}",
            w.creatures.color_g[0]
        );
        assert_eq!(w.creatures.color_b[0], 0.0, "blue clamped at 0");
        assert_eq!(w.creatures.color_r[0], 0.0, "red clamped at 0");

        // --- Tick 2: Split action with Split pre-argmax + a bite ---
        w.creatures.action_this_tick[0] = Action::Split;
        w.scratch_argmax_pre[0] = Action::Split as u8;
        w.scratch_got_a_bite[0] = true;

        w.color_ema_update();

        // green: was 0.5, argmax-pre != Graze → -0.05 → 0.45.
        assert!(
            (w.creatures.color_g[0] - 0.45).abs() < 1e-6,
            "green expected 0.45 got {}",
            w.creatures.color_g[0]
        );
        // blue: was 0.0, +1.0 (Split tick) → clamps to 1.0.
        assert!(
            (w.creatures.color_b[0] - 1.0).abs() < 1e-6,
            "blue expected 1.0 got {}",
            w.creatures.color_b[0]
        );
        // red: was 0.0, +1.0 (bite) → clamps to 1.0.
        assert!(
            (w.creatures.color_r[0] - 1.0).abs() < 1e-6,
            "red expected 1.0 got {}",
            w.creatures.color_r[0]
        );
    }
}
