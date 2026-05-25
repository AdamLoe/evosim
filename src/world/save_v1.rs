//! SaveV1 ↔ World conversion. Hardening lands in S12; S1 only moves the existing bodies.

use super::World;
use crate::constants::*;
use crate::creature::CreatureSoA;
use crate::grid::SpatialGrid;
use crate::save::{rehydrate_event_log, validate_save, LoadError, SCHEMA_VERSION};
use crate::species::SpeciesRegistry;
use crate::sun::SunMap;
use crate::vision::{VisionBuf, VISION_LEN};

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
                save.creatures.species_id[i],
                save.creatures.parent_species_id[i],
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
        }

        // Rebuild spatial grid from positions.
        let mut grid = SpatialGrid::new();
        grid.rebuild(&creatures.x, &creatures.y);

        // Rebuild vision Vec (zeros — overwritten on next tick's vision pass).
        let vision: Vec<VisionBuf> = vec![[0.0f32; VISION_LEN]; n];

        // Rebuild SunMap from snapshot.
        let mut hotspots = [(0.0f32, 0.0f32); HOTSPOT_COUNT];
        for (k, &hp) in save.sun.hotspots.iter().enumerate().take(HOTSPOT_COUNT) {
            hotspots[k] = hp;
        }
        let sun = SunMap {
            capacity: save.sun.capacity,
            current: save.sun.current,
            hotspots,
            demand: save.sun.demand,
            gradient_strength: save.sun.gradient_strength,
            refill_rate: save.sun.refill_rate,
        };

        // Rebuild SpeciesRegistry — next_id recomputed as max(id) + 1.
        let max_id = save.species.list.iter().map(|s| s.id).max().unwrap_or(0);
        let species = SpeciesRegistry::from_snapshot(save.species.list, max_id + 1);

        // Rehydrate event log.
        let events = rehydrate_event_log(save.events);

        Ok(World {
            tick: save.tick,
            seed: save.seed,
            rng: save.rng,
            sun,
            grid,
            creatures,
            carrion: save.carrion,
            species,
            events,
            events_enabled: false,
            sliders: save.sliders,
            next_creature_id: save.next_creature_id,
            peak_population: save.peak_population,
            peak_species_count: save.peak_species_count,
            world_ended: save.world_ended,
            live_species_count: save.live_species_count,
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
            cell_to_carrion: Vec::new(),
            pending_extinction_check: Vec::new(),
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
        })
    }
}
