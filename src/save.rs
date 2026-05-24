//! Save/load support for v1. Schema version: 1. JSON via serde_json.
//! v5 §13, v6 §I. Snapshot structs are explicit so future versions can add
//! SaveV2 alongside without disturbing SaveV1. From<&World> conversions kept
//! in this module; World re-exports via to_save_v1 / from_save_v1 helpers.

use crate::brain::Brain;
use crate::carrion::Carrion;
use crate::creature::Action;
use crate::events::{Event, EventLog};
use crate::genome::Genome;
use crate::hof::HallOfFame;
use crate::rng::SimRng;
use crate::species::Species;
use crate::world::{DevSliders, World};
use serde::{Deserialize, Serialize};

/// Current schema version. Bump on any save-shape change that breaks compatibility.
pub const SCHEMA_VERSION: u32 = 1;

/// Wire shape on disk. Camera / UI state lives on the JS side.
/// All fields are owned Vecs so we can serialize without referencing &World.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveV1 {
    pub schema_version: u32,
    pub seed: String,
    pub tick: u32,

    // RNG state — exact xoshiro256++ state via serde1 feature.
    pub rng: SimRng,

    // SoA — one Vec per column.
    pub creatures: CreatureSoASnapshot,

    // Maps.
    pub sun: SunMapSnapshot,
    pub carrion: Vec<Carrion>,

    // Bookkeeping.
    pub species: SpeciesSnapshot,
    pub events: EventLogSnapshot,
    pub sliders: DevSliders,

    pub next_creature_id: u64,
    pub peak_population: u32,
    pub peak_species_count: u32,
    pub world_ended: bool,
    pub live_species_count: u32,
    pub first_move_fired: bool,
    pub first_eat_fired: bool,
    pub population_milestones_fired: u32,

    // Hall-of-fame snapshots (E.25.d, consumed by F.28).
    pub biggest_ever: Option<HallOfFame>,
    pub last_survivor: Option<HallOfFame>,
    pub weirdest: Option<HallOfFame>,
    pub weirdest_distance: f32,
    pub longest_lived: Option<HallOfFame>,
    pub longest_lived_age: u32,
    pub first_mover_snapshot: Option<HallOfFame>,
    pub founder_genome_anchor: Genome,
    pub founder_brain_anchor: Vec<f32>,
}

/// Per-creature SoA snapshot — one Vec per column, index-aligned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatureSoASnapshot {
    pub id: Vec<u64>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub energy: Vec<f32>,
    pub age: Vec<u32>,
    pub digestion_cooldown: Vec<u32>,
    pub cumulative_upkeep: Vec<f32>,
    pub species_id: Vec<u32>,
    pub parent_species_id: Vec<u32>,
    pub last_action: Vec<Action>,
    pub action_this_tick: Vec<Action>,
    pub max_size_reached: Vec<f32>,
    pub distance_travelled: Vec<f32>,
    pub birth_tick: Vec<u32>,
    pub genomes: Vec<Genome>,
    pub brains: Vec<Brain>,
}

/// SunMap snapshot — hotspots stored for replay.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SunMapSnapshot {
    pub capacity: Vec<f32>,
    pub current: Vec<f32>,
    pub hotspots: Vec<(f32, f32)>,
    pub demand: Vec<f32>,
    pub gradient_strength: f32,
    pub refill_rate: f32,
}

/// SpeciesRegistry snapshot — next_id is recomputed on load as max(id)+1.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeciesSnapshot {
    pub list: Vec<Species>,
}

/// EventLog snapshot — full history; recent ring is rebuilt from tail on load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventLogSnapshot {
    pub all: Vec<Event>,
    pub ring_cap: usize,
}

/// Errors returned by `World::from_save_v1`.
#[derive(Debug)]
pub enum LoadError {
    SchemaVersionMismatch { found: u32, expected: u32 },
    InvalidJson(serde_json::Error),
    StructuralError(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::SchemaVersionMismatch { found, expected } => {
                write!(f, "schema version mismatch: found {found}, expected {expected}")
            }
            LoadError::InvalidJson(e) => write!(f, "invalid json: {e}"),
            LoadError::StructuralError(s) => write!(f, "structural error: {s}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot construction from World

impl SaveV1 {
    pub fn from_world(w: &World) -> Self {
        let hotspots = w.sun.hotspots.to_vec();
        SaveV1 {
            schema_version: SCHEMA_VERSION,
            seed: w.seed.clone(),
            tick: w.tick,
            rng: w.rng.clone(),
            creatures: CreatureSoASnapshot {
                id: w.creatures.id.clone(),
                x: w.creatures.x.clone(),
                y: w.creatures.y.clone(),
                vx: w.creatures.vx.clone(),
                vy: w.creatures.vy.clone(),
                energy: w.creatures.energy.clone(),
                age: w.creatures.age.clone(),
                digestion_cooldown: w.creatures.digestion_cooldown.clone(),
                cumulative_upkeep: w.creatures.cumulative_upkeep.clone(),
                species_id: w.creatures.species_id.clone(),
                parent_species_id: w.creatures.parent_species_id.clone(),
                last_action: w.creatures.last_action.clone(),
                action_this_tick: w.creatures.action_this_tick.clone(),
                max_size_reached: w.creatures.max_size_reached.clone(),
                distance_travelled: w.creatures.distance_travelled.clone(),
                birth_tick: w.creatures.birth_tick.clone(),
                genomes: w.creatures.genomes.clone(),
                brains: w.creatures.brains.clone(),
            },
            sun: SunMapSnapshot {
                capacity: w.sun.capacity.clone(),
                current: w.sun.current.clone(),
                hotspots,
                demand: w.sun.demand.clone(),
                gradient_strength: w.sun.gradient_strength,
                refill_rate: w.sun.refill_rate,
            },
            carrion: w.carrion.clone(),
            species: SpeciesSnapshot {
                list: w.species.list.clone(),
            },
            events: EventLogSnapshot {
                all: w.events.all.clone(),
                ring_cap: w.events.ring_cap,
            },
            sliders: w.sliders.clone(),
            next_creature_id: w.next_creature_id,
            peak_population: w.peak_population,
            peak_species_count: w.peak_species_count,
            world_ended: w.world_ended,
            live_species_count: w.live_species_count,
            first_move_fired: w.first_move_fired,
            first_eat_fired: w.first_eat_fired,
            population_milestones_fired: w.population_milestones_fired,
            biggest_ever: w.biggest_ever.clone(),
            last_survivor: w.last_survivor.clone(),
            weirdest: w.weirdest.clone(),
            weirdest_distance: w.weirdest_distance,
            longest_lived: w.longest_lived.clone(),
            longest_lived_age: w.longest_lived_age,
            first_mover_snapshot: w.first_mover_snapshot.clone(),
            founder_genome_anchor: w.founder_genome_anchor.clone(),
            founder_brain_anchor: w.founder_brain_anchor.clone(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers used by world.rs::from_save_v1

/// Validate that all creature SoA columns have equal length.
pub fn validate_soa_lengths(s: &CreatureSoASnapshot) -> Result<usize, LoadError> {
    let n = s.id.len();
    macro_rules! check {
        ($col:ident) => {
            if s.$col.len() != n {
                return Err(LoadError::StructuralError(format!(
                    "column '{}' len {} != id len {}",
                    stringify!($col),
                    s.$col.len(),
                    n
                )));
            }
        };
    }
    check!(x);
    check!(y);
    check!(vx);
    check!(vy);
    check!(energy);
    check!(age);
    check!(digestion_cooldown);
    check!(cumulative_upkeep);
    check!(species_id);
    check!(parent_species_id);
    check!(last_action);
    check!(action_this_tick);
    check!(max_size_reached);
    check!(distance_travelled);
    check!(birth_tick);
    check!(genomes);
    check!(brains);
    Ok(n)
}

/// Rebuild an EventLog from a snapshot: full `all` history preserved; `recent`
/// ring recomputed from the tail (last `ring_cap` entries).
pub fn rehydrate_event_log(snap: EventLogSnapshot) -> EventLog {
    let mut log = EventLog::new();
    log.ring_cap = snap.ring_cap;
    log.recent.clear();
    let n = snap.all.len();
    let start = n.saturating_sub(snap.ring_cap);
    for ev in snap.all[start..].iter() {
        log.recent.push_back(ev.clone());
    }
    log.all = snap.all;
    log
}

// ─────────────────────────────────────────────────────────────────────────────
// Inline tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    fn make_world_500() -> World {
        let mut w = World::new("f26-rt");
        for _ in 0..500 {
            w.tick_once();
        }
        w
    }

    #[test]
    fn f26_round_trip_preserves_population() {
        let w = make_world_500();
        let pop_before = w.population();
        let tick_before = w.tick;
        let seed_before = w.seed.clone();
        let peak_before = w.peak_population;

        let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");
        let save2: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let w2 = World::from_save_v1(save2).expect("from_save_v1");

        assert_eq!(w2.population(), pop_before, "population must match");
        assert_eq!(w2.tick, tick_before, "tick must match");
        assert_eq!(w2.seed, seed_before, "seed must match");
        assert_eq!(w2.peak_population, peak_before, "peak_population must match");
    }

    #[test]
    fn f26_round_trip_preserves_creature_state() {
        let w = make_world_500();
        let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");
        let save2: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let w2 = World::from_save_v1(save2).expect("from_save_v1");
        let n = w.creatures.len();
        assert_eq!(w2.creatures.len(), n, "creature count must match");
        for i in 0..n {
            assert_eq!(w2.creatures.id[i], w.creatures.id[i], "id[{i}]");
            assert_eq!(w2.creatures.x[i], w.creatures.x[i], "x[{i}]");
            assert_eq!(w2.creatures.y[i], w.creatures.y[i], "y[{i}]");
            assert_eq!(w2.creatures.vx[i], w.creatures.vx[i], "vx[{i}]");
            assert_eq!(w2.creatures.vy[i], w.creatures.vy[i], "vy[{i}]");
            assert_eq!(w2.creatures.energy[i], w.creatures.energy[i], "energy[{i}]");
            assert_eq!(w2.creatures.age[i], w.creatures.age[i], "age[{i}]");
            assert_eq!(
                w2.creatures.digestion_cooldown[i], w.creatures.digestion_cooldown[i],
                "digestion_cooldown[{i}]"
            );
            assert_eq!(
                w2.creatures.cumulative_upkeep[i], w.creatures.cumulative_upkeep[i],
                "cumulative_upkeep[{i}]"
            );
            assert_eq!(w2.creatures.species_id[i], w.creatures.species_id[i], "species_id[{i}]");
            assert_eq!(
                w2.creatures.parent_species_id[i], w.creatures.parent_species_id[i],
                "parent_species_id[{i}]"
            );
            assert_eq!(
                w2.creatures.last_action[i], w.creatures.last_action[i],
                "last_action[{i}]"
            );
            assert_eq!(
                w2.creatures.action_this_tick[i], w.creatures.action_this_tick[i],
                "action_this_tick[{i}]"
            );
            assert_eq!(
                w2.creatures.max_size_reached[i], w.creatures.max_size_reached[i],
                "max_size_reached[{i}]"
            );
            assert_eq!(
                w2.creatures.distance_travelled[i], w.creatures.distance_travelled[i],
                "distance_travelled[{i}]"
            );
            assert_eq!(w2.creatures.birth_tick[i], w.creatures.birth_tick[i], "birth_tick[{i}]");
            assert_eq!(
                w2.creatures.genomes[i].size, w.creatures.genomes[i].size,
                "genome.size[{i}]"
            );
            assert_eq!(
                w2.creatures.brains[i].weights[0], w.creatures.brains[i].weights[0],
                "brain.weights[0][{i}]"
            );
        }
    }

    #[test]
    fn f26_round_trip_preserves_rng() {
        // Tick 100, snapshot, restore, then tick 100 more in both worlds.
        // Population should match — divergence-free RNG.
        let mut w1 = World::new("f26-rng");
        for _ in 0..100 {
            w1.tick_once();
        }
        let json = serde_json::to_string(&SaveV1::from_world(&w1)).expect("serialize");
        let save2: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let mut w2 = World::from_save_v1(save2).expect("from_save_v1");

        for _ in 0..100 {
            w1.tick_once();
            w2.tick_once();
        }
        assert_eq!(
            w1.population(), w2.population(),
            "population must match after 100 more ticks (RNG divergence canary)"
        );
    }

    #[test]
    fn f26_schema_version_mismatch_errs() {
        let mut w = World::new("f26-schema");
        w.tick_once();
        let mut save = SaveV1::from_world(&w);
        save.schema_version = 999;
        let result = World::from_save_v1(save);
        match result {
            Err(LoadError::SchemaVersionMismatch { found: 999, expected: 1 }) => {}
            Err(e) => panic!("expected SchemaVersionMismatch, got: {e}"),
            Ok(_) => panic!("expected Err, got Ok"),
        }
    }

    #[test]
    fn f26_invalid_json_errs() {
        let result: Result<SaveV1, _> = serde_json::from_str("not valid json {{}}");
        assert!(result.is_err(), "must fail on garbage JSON");
    }

    #[test]
    fn f26_full_event_log_round_trips() {
        // Run with elevated mutation rate to generate events faster.
        let mut w = World::new("f26-events");
        w.sliders.mutation_rate_multiplier = 5.0;
        for _ in 0..5000 {
            if !w.tick_once() {
                break;
            }
        }
        let all_len_before = w.events.all.len();
        // Only assert if some events were generated.
        if all_len_before == 0 {
            return;
        }
        let last_event_tick = w.events.all.last().map(|e| e.tick);

        let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");
        let save2: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let w2 = World::from_save_v1(save2).expect("from_save_v1");

        assert_eq!(
            w2.events.all.len(), all_len_before,
            "full event log length must match"
        );
        assert_eq!(
            w2.events.all.last().map(|e| e.tick), last_event_tick,
            "last event tick must match"
        );
    }

    #[test]
    fn f26_sun_round_trips_exactly() {
        let w = make_world_500();
        let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");
        let save2: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let w2 = World::from_save_v1(save2).expect("from_save_v1");
        let n = w.sun.current.len();
        assert_eq!(w2.sun.current.len(), n, "sun len must match");
        for k in 0..n {
            assert_eq!(w2.sun.current[k], w.sun.current[k], "sun.current[{k}]");
            assert_eq!(w2.sun.capacity[k], w.sun.capacity[k], "sun.capacity[{k}]");
        }
    }
}
