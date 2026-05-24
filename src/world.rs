//! World — owns SoA + sun + carrion + species + RNG + tick orchestration.
//!
//! Within-tick ordering follows v5 §3.5 exactly. Milestone B stubs vision
//! and NN forward (no inputs needed yet) and uses a hardcoded action
//! selector ("split if energy ≥ SPLIT_PLACEHOLDER, else photosynth").
//! Milestone D swaps in the real NN forward pass.

use crate::brain::Brain;
use crate::carrion::Carrion;
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::events::{Event, EventKind, EventLog};
use crate::genome::Genome;
use crate::grid::SpatialGrid;
use crate::rng::SimRng;
use crate::species::SpeciesRegistry;
use crate::sun::SunMap;
use crate::vision::{
    build_cell_to_carrion, sector_to_angle, VisionBuf, VisionPass, SECTORS, VISION_LEN,
};
use serde::{Deserialize, Serialize};

/// Internal body radius (world units) per genome `size` unit. Render layer
/// scales this further (`size × 2.5` pixels per v6 §B at base zoom).
pub const BODY_RADIUS_PER_SIZE: f32 = 1.0;

/// Placeholder split threshold used by Milestone B's hardcoded action picker.
/// Higher than the spec's 50 floor so the parent retains a 30-energy buffer
/// (gift = 30) and the world doesn't immediately oscillate to extinction.
const SPLIT_PLACEHOLDER: f32 = 80.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevSliders {
    pub base_sun_rate: f32,
    pub mutation_rate_multiplier: f32,
    pub sun_gradient_strength: f32,
    pub mouth_tax: f32,
    pub nn_mutation_sigma: f32,
}

impl Default for DevSliders {
    fn default() -> Self {
        Self {
            base_sun_rate: SUN_REFILL_RATE,
            mutation_rate_multiplier: 1.0,
            sun_gradient_strength: 1.0,
            mouth_tax: UPKEEP_MOUTH_DEFAULT,
            nn_mutation_sigma: NN_MUT_SIGMA_DEFAULT,
        }
    }
}

pub struct World {
    pub tick: u32,
    pub seed: String,
    pub rng: SimRng,
    pub sun: SunMap,
    pub grid: SpatialGrid,
    pub creatures: CreatureSoA,
    pub carrion: Vec<Carrion>,
    pub species: SpeciesRegistry,
    pub events: EventLog,
    pub sliders: DevSliders,
    pub next_creature_id: u64,
    pub peak_population: u32,
    pub peak_species_count: u32,
    pub world_ended: bool,
    pub live_species_count: u32,
    pub first_move_fired: bool,
    pub first_eat_fired: bool,
    /// Per-creature vision cache (index-aligned with CreatureSoA). Milestone C.12.
    pub vision: Vec<VisionBuf>,
    /// Per-cell carrion lookup, rebuilt each tick before vision pass. Size = HASH_DIM².
    cell_to_carrion: Vec<Vec<u32>>,
    /// Species ids that lost a creature this tick; checked after removals to
    /// emit extinction events. Drained each `finalize_extinctions`.
    pending_extinction_check: Vec<u32>,
}

impl World {
    pub fn new(seed: impl Into<String>) -> Self {
        let seed_string = seed.into();
        let mut rng = SimRng::from_string(&seed_string);
        let sun = SunMap::new(&mut rng);
        let mut creatures = CreatureSoA::with_capacity(2048);
        let mut species = SpeciesRegistry::new();
        let founder_genome = Genome::founder();
        let founder_brain = Brain::founder(&mut rng);
        let founder_species = species.founder(&founder_genome, &founder_brain, 0);
        let cx = WORLD_SIZE * 0.5;
        let cy = WORLD_SIZE * 0.5;
        creatures.push(
            0,
            cx,
            cy,
            FOUNDER_ENERGY,
            founder_species,
            founder_species,
            0,
            founder_genome,
            founder_brain,
        );
        let mut grid = SpatialGrid::new();
        grid.rebuild(&creatures.x, &creatures.y);
        Self {
            tick: 0,
            seed: seed_string,
            rng,
            sun,
            grid,
            creatures,
            carrion: Vec::new(),
            species,
            events: EventLog::new(),
            sliders: DevSliders::default(),
            next_creature_id: 1,
            peak_population: 1,
            peak_species_count: 1,
            world_ended: false,
            live_species_count: 1,
            first_move_fired: false,
            first_eat_fired: false,
            vision: vec![[0.0f32; VISION_LEN]], // 1 for the founder
            cell_to_carrion: Vec::new(),
            pending_extinction_check: Vec::new(),
        }
    }

    pub fn population(&self) -> u32 {
        self.creatures.len() as u32
    }

    /// Run one sim tick. Returns true if the world is still alive afterwards.
    pub fn step(&mut self) -> bool {
        if self.world_ended {
            return false;
        }

        self.sun.refill_rate = self.sliders.base_sun_rate;

        // 1. Rebuild spatial hash grid from start-of-tick positions.
        self.grid.rebuild(&self.creatures.x, &self.creatures.y);

        // 2. Vision pass (Milestone C.12).
        self.run_vision_pass();

        // 3. Choose action (Milestone C hardcoded picker — replaced in D by NN).
        let n = self.creatures.len();
        for i in 0..n {
            let (vx, vy, action) = pick_action_c(
                i,
                &self.vision[i],
                &self.creatures.genomes[i],
                self.creatures.energy[i],
                self.creatures.digestion_cooldown[i],
                self.creatures.id[i],
            );
            self.creatures.vx[i] = vx;
            self.creatures.vy[i] = vy;
            self.creatures.action_this_tick[i] = action;
        }

        // 4. Apply velocities + soft repulsion + wall clamp; rebuild grid.
        self.apply_movement_and_repulsion();

        // 5. Photosynth two-pass.
        self.photosynth_two_pass();

        // 6. Eat / scavenge resolution.
        self.eat_and_scavenge();

        // 7. Sun refill.
        self.sun.refill();

        // 8. Energy bookkeeping.
        self.energy_bookkeeping();

        // 9. Deaths → carrion.
        let died = self.collect_deaths();
        if !died.is_empty() {
            // Mirror swap_remove on vision vec to keep it index-aligned.
            // Walk from back just like remove_indices does.
            for &k in died.iter().rev() {
                if k < self.vision.len() {
                    self.vision.swap_remove(k);
                }
            }
            self.creatures.remove_indices(&died);
        }

        // 10. Carrion decay.
        self.decay_carrion();

        // 11. Births.
        self.handle_births();

        // 12. Species detection (full §12 metric lands in Milestone E).

        // Promote this-tick action → next-tick last_action (v6 §1 / §E).
        for i in 0..self.creatures.len() {
            self.creatures.last_action[i] = self.creatures.action_this_tick[i];
        }

        self.tick = self.tick.saturating_add(1);
        if self.creatures.len() as u32 > self.peak_population {
            self.peak_population = self.creatures.len() as u32;
        }
        if self.live_species_count > self.peak_species_count {
            self.peak_species_count = self.live_species_count;
        }

        if self.creatures.is_empty() {
            self.world_ended = true;
            self.events.push(Event {
                tick: self.tick,
                kind: EventKind::WorldEnded {
                    ticks_lived: self.tick,
                    peak_population: self.peak_population,
                    peak_species: self.peak_species_count,
                },
            });
            return false;
        }
        true
    }

    fn apply_movement_and_repulsion(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        for i in 0..n {
            let speed_cap = self.creatures.genomes[i].move_speed;
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
        }

        self.grid.rebuild(&self.creatures.x, &self.creatures.y);

        let mut fx = vec![0.0_f32; n];
        let mut fy = vec![0.0_f32; n];
        for i in 0..n {
            let xi = self.creatures.x[i];
            let yi = self.creatures.y[i];
            let ri = self.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
            let search = ri + SIZE_MAX * BODY_RADIUS_PER_SIZE;
            let mut neighbors: Vec<usize> = Vec::with_capacity(16);
            self.grid.for_each_in_radius(xi, yi, search, |j| {
                if j > i {
                    neighbors.push(j);
                }
            });
            for j in neighbors {
                let xj = self.creatures.x[j];
                let yj = self.creatures.y[j];
                let rj = self.creatures.genomes[j].size * BODY_RADIUS_PER_SIZE;
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
                        fx[i] -= ux * f;
                        fy[i] -= uy * f;
                        fx[j] += ux * f;
                        fy[j] += uy * f;
                    } else {
                        // Co-located (newborn siblings) — deterministic nudge.
                        let angle =
                            ((i as f32) * 0.7 + (j as f32) * 1.3).sin() * std::f32::consts::PI;
                        let ux = angle.cos();
                        let uy = angle.sin();
                        let f = REPULSION_MAX;
                        fx[i] -= ux * f;
                        fy[i] -= uy * f;
                        fx[j] += ux * f;
                        fy[j] += uy * f;
                    }
                }
            }
        }

        for i in 0..n {
            let mut x = self.creatures.x[i] + fx[i];
            let mut y = self.creatures.y[i] + fy[i];
            let r = self.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
            if x < r {
                x = r;
                if self.creatures.vx[i] < 0.0 {
                    self.creatures.vx[i] = 0.0;
                }
            } else if x > WORLD_SIZE - r {
                x = WORLD_SIZE - r;
                if self.creatures.vx[i] > 0.0 {
                    self.creatures.vx[i] = 0.0;
                }
            }
            if y < r {
                y = r;
                if self.creatures.vy[i] < 0.0 {
                    self.creatures.vy[i] = 0.0;
                }
            } else if y > WORLD_SIZE - r {
                y = WORLD_SIZE - r;
                if self.creatures.vy[i] > 0.0 {
                    self.creatures.vy[i] = 0.0;
                }
            }
            self.creatures.x[i] = x;
            self.creatures.y[i] = y;
        }

        self.grid.rebuild(&self.creatures.x, &self.creatures.y);
    }

    fn photosynth_two_pass(&mut self) {
        // 5a. demand
        self.sun.demand.fill(0.0);
        for i in 0..self.creatures.len() {
            if self.creatures.action_this_tick[i] != Action::Photosynth {
                continue;
            }
            let g = &self.creatures.genomes[i];
            let want = PHOTO_GAIN_COEFF * g.photosynth_efficiency * g.size;
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
            let g = &self.creatures.genomes[i];
            let want = PHOTO_GAIN_COEFF * g.photosynth_efficiency * g.size;
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

    fn eat_and_scavenge(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        let mut damage = vec![0.0_f32; n];
        let mut gain = vec![0.0_f32; n];
        let mut cooldown_set = vec![false; n];
        let mut attempted_eat = vec![false; n];
        let mut attempted_scavenge = vec![false; n];
        let mut got_a_bite = vec![false; n];

        for i in 0..n {
            match self.creatures.action_this_tick[i] {
                Action::Eat => {
                    attempted_eat[i] = true;
                    if self.creatures.digestion_cooldown[i] > 0 {
                        continue;
                    }
                    let g_i = self.creatures.genomes[i].clone();
                    if g_i.eat_efficiency <= 0.0 {
                        continue;
                    }
                    let radius_i = g_i.size * BODY_RADIUS_PER_SIZE;
                    let reach = g_i.bite_reach * g_i.size;
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
                        let rj = self.creatures.genomes[j].size * BODY_RADIUS_PER_SIZE;
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
                        let dmg = EAT_DAMAGE_COEFF * g_i.size;
                        let armor = self.creatures.genomes[j].armor.clamp(0.0, 1.0);
                        damage[j] += dmg * (1.0 - armor);
                        gain[i] += EAT_GAIN_COEFF * g_i.eat_efficiency;
                        cooldown_set[i] = true;
                        got_a_bite[i] = true;
                    }
                }
                Action::Scavenge => {
                    attempted_scavenge[i] = true;
                    let g_i = self.creatures.genomes[i].clone();
                    if g_i.scavenge_efficiency <= 0.0 {
                        continue;
                    }
                    let r_i = g_i.size * BODY_RADIUS_PER_SIZE;
                    let xi = self.creatures.x[i];
                    let yi = self.creatures.y[i];
                    let want = SCAVENGE_GAIN_COEFF * g_i.scavenge_efficiency;
                    for c in &mut self.carrion {
                        let dx = c.x - xi;
                        let dy = c.y - yi;
                        let d2 = dx * dx + dy * dy;
                        if d2 <= r_i * r_i {
                            let take = c.pool.min(want);
                            c.pool -= take;
                            gain[i] += take;
                            break;
                        }
                    }
                }
                _ => {}
            }
        }

        for i in 0..n {
            if attempted_eat[i] {
                self.creatures.energy[i] -= COST_EAT_ATTEMPT;
                if cooldown_set[i] {
                    self.creatures.digestion_cooldown[i] = DIGESTION_COOLDOWN_TICKS;
                }
                if got_a_bite[i] && !self.first_eat_fired {
                    self.first_eat_fired = true;
                    self.events.push(Event {
                        tick: self.tick,
                        kind: EventKind::FirstToEat {
                            creature_id: self.creatures.id[i],
                        },
                    });
                }
            }
            if attempted_scavenge[i] {
                self.creatures.energy[i] -= COST_SCAVENGE_ATTEMPT;
            }
            self.creatures.energy[i] += gain[i];
            self.creatures.energy[i] -= damage[i];
        }
    }

    fn energy_bookkeeping(&mut self) {
        let mouth_tax = self.sliders.mouth_tax;
        for i in 0..self.creatures.len() {
            let g = &self.creatures.genomes[i];
            let mut up = UPKEEP_BASE;
            if g.size > 1.0 {
                up += UPKEEP_SIZE_PER_UNIT * (g.size - 1.0);
            }
            if g.move_speed > 0.0 {
                up += UPKEEP_MOBILITY_FLAG;
                up += UPKEEP_MOVE_SPEED_PER_UNIT * g.move_speed;
            }
            up += UPKEEP_PER_EYE * g.eye_count as f32;
            up += UPKEEP_VISION_COEFF * g.vision_range * g.vision_range;
            if g.eat_efficiency > 0.0 {
                up += mouth_tax;
            }
            if g.scavenge_efficiency > 0.0 {
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

            if g.size > self.creatures.max_size_reached[i] {
                self.creatures.max_size_reached[i] = g.size;
            }
            self.creatures.energy[i] -= up;
            self.creatures.cumulative_upkeep[i] += up;
            if self.creatures.digestion_cooldown[i] > 0 {
                self.creatures.digestion_cooldown[i] -= 1;
            }
            self.creatures.age[i] += 1;
        }
    }

    fn collect_deaths(&mut self) -> Vec<usize> {
        let mut dead: Vec<usize> = Vec::new();
        let n = self.creatures.len();
        if !self.first_move_fired {
            for i in 0..n {
                if self.creatures.genomes[i].move_speed > 0.0
                    && self.creatures.distance_travelled[i] >= 5.0
                {
                    self.first_move_fired = true;
                    self.events.push(Event {
                        tick: self.tick,
                        kind: EventKind::FirstToMove {
                            creature_id: self.creatures.id[i],
                        },
                    });
                    break;
                }
            }
        }
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
            }
        }
        if !dead.is_empty() {
            self.pending_extinction_check
                .extend_from_slice(&species_lost);
        }
        dead
    }

    fn decay_carrion(&mut self) {
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

    fn handle_births(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        for i in 0..n {
            if self.creatures.action_this_tick[i] != Action::Split {
                continue;
            }
            if self.creatures.energy[i] < SPLIT_THRESHOLD {
                continue;
            }
            let parent_energy_after_cost = self.creatures.energy[i] - SPLIT_THRESHOLD;
            let gift = parent_energy_after_cost.clamp(0.0, SPLIT_GIFT_MAX);
            self.creatures.energy[i] = parent_energy_after_cost - gift;
            let child_energy = gift;

            let mut child_genome = self.creatures.genomes[i].clone();
            child_genome.mutate_in_place(&mut self.rng, self.sliders.mutation_rate_multiplier);
            let child_brain = Brain::child_from(
                &self.creatures.brains[i],
                &mut self.rng,
                self.sliders.nn_mutation_sigma,
            );
            let jitter_x = self.rng.symm() * FOUNDER_SPLIT_JITTER;
            let jitter_y = self.rng.symm() * FOUNDER_SPLIT_JITTER;
            let parent_radius = self.creatures.genomes[i].size * BODY_RADIUS_PER_SIZE;
            let cx =
                (self.creatures.x[i] + jitter_x).clamp(parent_radius, WORLD_SIZE - parent_radius);
            let cy =
                (self.creatures.y[i] + jitter_y).clamp(parent_radius, WORLD_SIZE - parent_radius);
            let new_id = self.next_creature_id;
            self.next_creature_id += 1;
            let parent_species = self.creatures.species_id[i];
            self.creatures.push(
                new_id,
                cx,
                cy,
                child_energy,
                parent_species,
                parent_species,
                self.tick,
                child_genome,
                child_brain,
            );
        }
    }

    pub fn finalize_extinctions(&mut self) {
        let mut alive = std::collections::HashSet::<u32>::new();
        for i in 0..self.creatures.len() {
            alive.insert(self.creatures.species_id[i]);
        }
        let candidates = std::mem::take(&mut self.pending_extinction_check);
        for cand in candidates {
            if !alive.contains(&cand) {
                let species = &mut self.species.list[cand as usize];
                if species.died_tick.is_none() {
                    species.died_tick = Some(self.tick);
                    let name = species.name.clone();
                    self.events.push(Event {
                        tick: self.tick,
                        kind: EventKind::Extinction {
                            species_id: cand,
                            species_name: name,
                        },
                    });
                }
            }
        }
        self.live_species_count = alive.len() as u32;
    }

    pub fn tick_once(&mut self) -> bool {
        let alive = self.step();
        self.finalize_extinctions();
        alive
    }

    /// Rebuild the per-cell carrion index and run the vision pass.
    /// Ensures self.vision is index-aligned with self.creatures.
    fn run_vision_pass(&mut self) {
        let n = self.creatures.len();
        // Ensure vision buffer is large enough.
        if self.vision.len() < n {
            self.vision.resize(n, [0.0f32; VISION_LEN]);
        }
        // Rebuild per-cell carrion index (O(N+C), reuses cached Vec<Vec<u32>>).
        build_cell_to_carrion(&self.carrion, &mut self.cell_to_carrion);
        let pass = VisionPass {
            creatures: &self.creatures,
            carrion: &self.carrion,
            grid: &self.grid,
            cell_to_carrion: &self.cell_to_carrion,
        };
        pass.run(&mut self.vision[..n]);
    }
}

/// Hardcoded vision-driven action picker (Milestone C placeholder).
/// Replaced wholesale by NN forward pass in Milestone D.
///
/// Priority order:
/// 1. Split if energy >= SPLIT_PLACEHOLDER.
/// 2. Scavenge if nearest visible target is carrion-colored AND scavenge_efficiency > 0.
/// 3. Eat if nearest visible target is a creature AND eat_efficiency > 0 AND cooldown == 0.
/// 4. Photosynth (fallback).
///
/// Velocity: head toward closest visible target at genome.move_speed; if no
/// target, drift at 30% speed along a deterministic per-id angle so
/// distance_travelled accumulates. Uses creature id (not loop index i) to
/// stay stable across swap_removes.
fn pick_action_c(
    i: usize,
    vision_buf: &VisionBuf,
    genome: &Genome,
    energy: f32,
    cooldown: u32,
    creature_id: u64,
) -> (f32, f32, Action) {
    let speed = genome.move_speed;

    // 1. Find closest visible target.
    let mut best_sector: Option<usize> = None;
    let mut best_dist = f32::MAX;
    let mut best_features = [0.0f32; 5];
    for s in 0..SECTORS {
        let off = s * 5;
        let d = vision_buf[off];
        if d <= 0.0 {
            continue;
        }
        if d < best_dist {
            best_dist = d;
            best_sector = Some(s);
            best_features.copy_from_slice(&vision_buf[off..off + 5]);
        }
    }

    // Helper: is this sector's target carrion-colored?
    let target_is_carrion = best_sector.is_some()
        && (best_features[2] - crate::vision::CARRION_R).abs() < 1e-4
        && (best_features[3] - crate::vision::CARRION_G).abs() < 1e-4
        && (best_features[4] - crate::vision::CARRION_B).abs() < 1e-4;

    // 2. Action selection.
    let action = if energy >= SPLIT_PLACEHOLDER {
        Action::Split
    } else if target_is_carrion && genome.scavenge_efficiency > 0.0 {
        Action::Scavenge
    } else if best_sector.is_some()
        && !target_is_carrion
        && genome.eat_efficiency > 0.0
        && cooldown == 0
    {
        Action::Eat
    } else {
        Action::Photosynth
    };

    // 3. Velocity.
    let (vx, vy) = if let Some(s) = best_sector {
        if speed > 0.0 {
            let theta = sector_to_angle(s, &genome.eye_offsets, genome.eye_count);
            (speed * theta.cos(), speed * theta.sin())
        } else {
            (0.0, 0.0)
        }
    } else if speed > 0.0 {
        // Drift: deterministic per creature id so it's stable across swap_removes.
        let theta = (creature_id as f32 * 1.618_034).rem_euclid(std::f32::consts::TAU);
        (0.3 * speed * theta.cos(), 0.3 * speed * theta.sin())
    } else {
        (0.0, 0.0)
    };

    // Suppress unused parameter warning for i (kept for future D use).
    let _ = i;
    (vx, vy, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_initializes_with_one_creature() {
        let w = World::new("test-seed");
        assert_eq!(w.population(), 1);
        assert_eq!(w.species.list.len(), 1);
    }

    #[test]
    fn lone_creature_eventually_splits() {
        let mut w = World::new("split-test");
        for _ in 0..5000 {
            if !w.tick_once() {
                break;
            }
            if w.population() > 1 {
                return;
            }
        }
        panic!("never split — final pop {}", w.population());
    }

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

    /// C.12 test 6: vision + movement loop under real-ish conditions.
    /// Force the founder to have move_speed + eyes so that the vision loop
    /// and movement cost accounting both exercise the hot paths.
    #[test]
    fn world_runs_2000_ticks_with_movement() {
        let mut w = World::new("movement-smoke");
        // Patch the founder's genome to have eyes and move_speed.
        w.creatures.genomes[0].move_speed = 1.0;
        w.creatures.genomes[0].vision_range = 30.0;
        w.creatures.genomes[0].eye_count = 4;
        for _ in 0..2000 {
            if !w.tick_once() {
                break;
            }
        }
        // World ran for at least 1 tick without panicking.
        assert!(w.tick > 0);
    }

    /// C.12 test 7: vision layout contract for D.
    /// VISION_LEN == 120 and NN_INPUTS == 10 + VISION_LEN + 6 (== 136).
    #[test]
    fn vision_layout_matches_nn_input_block() {
        use crate::vision::VISION_LEN;
        assert_eq!(VISION_LEN, 120);
        // D's NN input block: 10 self-state + 120 vision + 6 last_action one-hot.
        assert_eq!(NN_INPUTS, 10 + VISION_LEN + 6);
    }
}
