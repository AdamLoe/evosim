//! SaveV1 ↔ World conversion. Hardening lands in S12; S1 only moves the existing bodies.

use super::World;
use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grass::GrassGrid;
use crate::grid::SpatialGrid;
use crate::save::{rehydrate_event_log, validate_save, LoadError, SCHEMA_VERSION};
use crate::vision::{CarrionIndex, VisionBuf, VISION_LEN};

impl World {
    /// Serialize the world to a `SaveV1` snapshot (F.26).
    pub fn to_save_v1(&self) -> crate::save::SaveV1 {
        crate::save::SaveV1::from_world(self)
    }

    /// Reconstruct a World from a `SaveV1` (F.26). Returns `LoadError` on schema
    /// mismatch or structural problems. Transient fields are rebuilt or zeroed.
    pub fn from_save_v1(save: crate::save::SaveV1) -> Result<Self, crate::save::LoadError> {
        // Schema version check first, before any other parsing.
        if save.schema_version != SCHEMA_VERSION {
            return Err(LoadError::SchemaVersionMismatch {
                found: save.schema_version,
                expected: SCHEMA_VERSION,
            });
        }

        // S12: comprehensive structural validation (position/energy/brain/slider checks).
        let n = validate_save(&save)?;

        // Rebuild SoA.
        let mut creatures = CreatureSoA::with_capacity(n.max(16));
        for i in 0..n {
            let g = &save.creatures.genomes[i];
            let b = &save.creatures.brains[i];
            // Brain weight count already validated by validate_save; this indexing is safe.
            creatures.push(
                save.creatures.id[i],
                save.creatures.x[i],
                save.creatures.y[i],
                save.creatures.energy[i],
                save.creatures.birth_tick[i],
                g.clone(),
                b.clone(),
            );
            creatures.vx[i] = save.creatures.vx[i];
            creatures.vy[i] = save.creatures.vy[i];
            creatures.age[i] = save.creatures.age[i];
            creatures.digestion_cooldown[i] = save.creatures.digestion_cooldown[i];
            creatures.cumulative_upkeep[i] = save.creatures.cumulative_upkeep[i];
            creatures.last_action[i] = save.creatures.last_action[i];
            creatures.action_this_tick[i] = save.creatures.action_this_tick[i];
            creatures.max_size_reached[i] = save.creatures.max_size_reached[i];
            creatures.distance_travelled[i] = save.creatures.distance_travelled[i];
            // P2g: restore move_bias fields from snapshot (replaces P2f zero-init placeholder).
            creatures.move_bias_x[i] = save.creatures.move_bias_x[i];
            creatures.move_bias_y[i] = save.creatures.move_bias_y[i];
            creatures.move_bias_reroll_at[i] = save.creatures.move_bias_reroll_at[i];
        }

        // Rebuild spatial grid from positions.
        let mut grid = SpatialGrid::new();
        grid.rebuild(&creatures.x, &creatures.y);

        // Rebuild vision Vec (zeros — overwritten on next tick's vision pass).
        let vision: Vec<VisionBuf> = vec![[0.0f32; VISION_LEN]; n];

        // Restore GrassGrid from snapshot — direct move, zero copy.
        let grass = GrassGrid {
            density: save.grass.density,
            scratch: vec![0.0f32; GRASS_CELL_COUNT],
        };

        // Rehydrate event log.
        let events = rehydrate_event_log(save.events);

        Ok(World {
            tick: save.tick,
            seed: save.seed,
            rng: save.rng,
            grass,
            grid,
            creatures,
            carrion: save.carrion,
            events,
            events_enabled: false,
            sliders: save.sliders,
            next_creature_id: save.next_creature_id,
            peak_population: save.peak_population,
            world_ended: save.world_ended,
            first_move_fired: save.first_move_fired,
            first_eat_fired: save.first_eat_fired,
            population_milestones_fired: save.population_milestones_fired,
            biggest_ever: save.biggest_ever,
            last_survivor: save.last_survivor,
            weirdest: save.weirdest,
            weirdest_distance: save.weirdest_distance,
            longest_lived: save.longest_lived,
            longest_lived_age: save.longest_lived_age,
            first_mover_snapshot: save.first_mover_snapshot,
            founder_genome_anchor: save.founder_genome_anchor,
            founder_brain_anchor: save.founder_brain_anchor,
            vision,
            cell_to_carrion: CarrionIndex::new(),
            force_sequential_nn: false, // S39: observation-only; never saved
            // Profiler is never saved/loaded — always start fresh (D9/D10).
            profile: crate::profiler::Profiler::new(),
            scratch_fx: Vec::new(),
            scratch_fy: Vec::new(),
            scratch_neighbors: Vec::new(),
            scratch_damage: Vec::new(),
            scratch_gain: Vec::new(),
            scratch_cooldown_set: Vec::new(),
            scratch_attempted_eat: Vec::new(),
            scratch_attempted_scavenge: Vec::new(),
            scratch_got_a_bite: Vec::new(),
            scratch_eat_candidates: Vec::new(), // S25: pool, not saved
            scratch_dead: Vec::new(),           // S27: pool, not saved
        })
    }
}
