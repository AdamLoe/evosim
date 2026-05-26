//! Save/load support for v1. Schema version: 4. JSON via serde_json.
//! v5 §13, v6 §I. Snapshot structs are explicit so future versions can add
//! SaveV2 alongside without disturbing SaveV1. From<&World> conversions kept
//! in this module; World re-exports via to_save_v1 / from_save_v1 helpers.

use crate::brain::Brain;
use crate::creature::Action;
use crate::events::{Event, EventLog};
use crate::genome::Genome;
use crate::hof::HallOfFame;
use crate::rng::SimRng;
use crate::species::Species;
use crate::world::{DevSliders, World};
use serde::{Deserialize, Serialize};

/// Current schema version. Bump on any save-shape change that breaks compatibility.
/// v1 → v2 (P1b+P1f): dropped SunMapSnapshot; added GrassGridSnapshot; dropped Carrion.sun_cell.
/// v2 → v3 (P2g): added nose_count + move_bias_x/y/reroll_at columns; NN weight count now 4032.
/// v3 → v4 (P3a): added eat_bite_fraction to DevSliders.
pub const SCHEMA_VERSION: u32 = 4;

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
    pub grass: GrassGridSnapshot,

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
    // v3 fields (P2g): genome nose_count mirror + move-bias state.
    pub nose_count: Vec<u8>,
    pub move_bias_x: Vec<f32>,
    pub move_bias_y: Vec<f32>,
    pub move_bias_reroll_at: Vec<u32>,
}

/// GrassGrid snapshot — density only; scratch is recomputed on load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrassGridSnapshot {
    pub density: Vec<f32>,
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
                write!(
                    f,
                    "schema version mismatch: found {found}, expected {expected}"
                )
            }
            LoadError::InvalidJson(e) => write!(f, "invalid json: {e}"),
            LoadError::StructuralError(s) => write!(f, "structural error: {s}"),
        }
    }
}

impl std::error::Error for LoadError {}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot construction from World

impl SaveV1 {
    pub fn from_world(w: &World) -> Self {
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
                // v3 fields (P2g).
                nose_count: w.creatures.g_nose_count.clone(),
                move_bias_x: w.creatures.move_bias_x.clone(),
                move_bias_y: w.creatures.move_bias_y.clone(),
                move_bias_reroll_at: w.creatures.move_bias_reroll_at.clone(),
            },
            grass: GrassGridSnapshot {
                density: w.grass.density.clone(),
            },
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
    // v3 columns (P2g).
    check!(nose_count);
    check!(move_bias_x);
    check!(move_bias_y);
    check!(move_bias_reroll_at);
    Ok(n)
}

/// Comprehensive structural validation of a `SaveV1` before reconstruction.
///
/// S12: Centralises all loader hardening checks (previously scattered inline
/// in `from_save_v1`). Returns `LoadError::StructuralError` on any violation.
///
/// Checks performed:
/// 1. SoA column length parity (delegates to `validate_soa_lengths`).
/// 2. Grass cell count == GRASS_CELL_COUNT; all densities finite and in `[0, GRASS_MAX]`.
/// 3. Brain weight count == NN_WEIGHT_COUNT per creature.
/// 4. Brain `nn_mutation_rate` is finite per creature.
/// 5. Creature positions (x, y) are finite per creature.
/// 6. Creature energies are finite per creature.
/// 7. No `parent_species_id` is `u32::MAX` (old sentinel value).
/// 8. All `DevSliders` fields are finite.
pub fn validate_save(save: &SaveV1) -> Result<usize, LoadError> {
    use crate::constants::{GRASS_CELL_COUNT, GRASS_MAX, NN_WEIGHT_COUNT};

    // 1. SoA column parity.
    let n = validate_soa_lengths(&save.creatures)?;

    // 2. Grass cell count.
    if save.grass.density.len() != GRASS_CELL_COUNT {
        return Err(LoadError::StructuralError(format!(
            "grass.density len {} != expected {}",
            save.grass.density.len(),
            GRASS_CELL_COUNT
        )));
    }

    // 2b. Grass densities finite + in [0, GRASS_MAX].
    for (k, &d) in save.grass.density.iter().enumerate() {
        if !d.is_finite() {
            return Err(LoadError::StructuralError(format!(
                "grass.density[{k}] is non-finite: {d}"
            )));
        }
        if !(0.0..=GRASS_MAX).contains(&d) {
            return Err(LoadError::StructuralError(format!(
                "grass.density[{k}] out of range [0, {GRASS_MAX}]: {d}"
            )));
        }
    }

    // 3-4. Per-creature brain checks.
    for i in 0..n {
        let b = &save.creatures.brains[i];
        if b.weights.len() != NN_WEIGHT_COUNT {
            return Err(LoadError::StructuralError(format!(
                "creature {i} brain weight count {} != {NN_WEIGHT_COUNT}",
                b.weights.len()
            )));
        }
        if !b.nn_mutation_rate.is_finite() {
            return Err(LoadError::StructuralError(format!(
                "creature {i} brain.nn_mutation_rate is non-finite: {}",
                b.nn_mutation_rate
            )));
        }
    }

    // 5. Per-creature v3 field range checks (P2g).
    use crate::constants::NOSE_VALID;
    let nose_max = *NOSE_VALID.iter().max().unwrap_or(&5);
    for i in 0..n {
        if save.creatures.nose_count[i] > nose_max {
            return Err(LoadError::StructuralError(format!(
                "creature {i} nose_count {} exceeds max {nose_max}",
                save.creatures.nose_count[i]
            )));
        }
        if !save.creatures.move_bias_x[i].is_finite() {
            return Err(LoadError::StructuralError(format!(
                "creature {i} move_bias_x is non-finite: {}",
                save.creatures.move_bias_x[i]
            )));
        }
        if !save.creatures.move_bias_y[i].is_finite() {
            return Err(LoadError::StructuralError(format!(
                "creature {i} move_bias_y is non-finite: {}",
                save.creatures.move_bias_y[i]
            )));
        }
    }

    // 6. Positions finite.
    for i in 0..n {
        if !save.creatures.x[i].is_finite() {
            return Err(LoadError::StructuralError(format!(
                "creature {i} x is non-finite: {}",
                save.creatures.x[i]
            )));
        }
        if !save.creatures.y[i].is_finite() {
            return Err(LoadError::StructuralError(format!(
                "creature {i} y is non-finite: {}",
                save.creatures.y[i]
            )));
        }
    }

    // 7. Energy finite.
    for i in 0..n {
        if !save.creatures.energy[i].is_finite() {
            return Err(LoadError::StructuralError(format!(
                "creature {i} energy is non-finite: {}",
                save.creatures.energy[i]
            )));
        }
    }

    // 8. parent_species_id sentinel check (u32::MAX was an old placeholder).
    for i in 0..n {
        if save.creatures.parent_species_id[i] == u32::MAX {
            return Err(LoadError::StructuralError(format!(
                "creature {i} parent_species_id is u32::MAX (invalid sentinel)"
            )));
        }
    }

    // 8. Slider fields finite.
    let s = &save.sliders;
    for (name, val) in [
        ("mutation_rate_multiplier", s.mutation_rate_multiplier),
        ("mouth_tax", s.mouth_tax),
        ("nn_mutation_sigma", s.nn_mutation_sigma),
        ("grass_propagation_rate_k", s.grass_propagation_rate_k),
        ("grass_in_cell_growth_r", s.grass_in_cell_growth_r),
        ("eat_bite_fraction", s.eat_bite_fraction),
    ] {
        if !val.is_finite() {
            return Err(LoadError::StructuralError(format!(
                "slider '{name}' is non-finite: {val}"
            )));
        }
    }
    // 9b. Grass slider range checks (per v1.2 amendments §A.4 + p3b slider ranges).
    if !(0.0..=0.2).contains(&s.grass_propagation_rate_k) {
        return Err(LoadError::StructuralError(format!(
            "slider 'grass_propagation_rate_k' out of range [0, 0.2]: {}",
            s.grass_propagation_rate_k
        )));
    }
    if !(0.0..=0.05).contains(&s.grass_in_cell_growth_r) {
        return Err(LoadError::StructuralError(format!(
            "slider 'grass_in_cell_growth_r' out of range [0, 0.05]: {}",
            s.grass_in_cell_growth_r
        )));
    }
    // 9c. eat_bite_fraction range check (P3a).
    if !(0.0..=1.0).contains(&s.eat_bite_fraction) {
        return Err(LoadError::StructuralError(format!(
            "slider 'eat_bite_fraction' out of range [0, 1]: {}",
            s.eat_bite_fraction
        )));
    }

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
        assert_eq!(
            w2.peak_population, peak_before,
            "peak_population must match"
        );
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
            assert_eq!(
                w2.creatures.species_id[i], w.creatures.species_id[i],
                "species_id[{i}]"
            );
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
            assert_eq!(
                w2.creatures.birth_tick[i], w.creatures.birth_tick[i],
                "birth_tick[{i}]"
            );
            assert_eq!(
                w2.creatures.genomes[i].size, w.creatures.genomes[i].size,
                "genome.size[{i}]"
            );
            assert_eq!(
                w2.creatures.brains[i].weights[0], w.creatures.brains[i].weights[0],
                "brain.weights[0][{i}]"
            );
            // v3 fields (P2g).
            assert_eq!(
                w2.creatures.genomes[i].nose_count, w.creatures.genomes[i].nose_count,
                "nose_count[{i}]"
            );
            assert_eq!(
                w2.creatures.move_bias_x[i], w.creatures.move_bias_x[i],
                "move_bias_x[{i}]"
            );
            assert_eq!(
                w2.creatures.move_bias_y[i], w.creatures.move_bias_y[i],
                "move_bias_y[{i}]"
            );
            assert_eq!(
                w2.creatures.move_bias_reroll_at[i], w.creatures.move_bias_reroll_at[i],
                "move_bias_reroll_at[{i}]"
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
            w1.population(),
            w2.population(),
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
            Err(LoadError::SchemaVersionMismatch {
                found: 999,
                expected: 4,
            }) => {}
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
            w2.events.all.len(),
            all_len_before,
            "full event log length must match"
        );
        assert_eq!(
            w2.events.all.last().map(|e| e.tick),
            last_event_tick,
            "last event tick must match"
        );
    }

    #[test]
    fn p1f_grass_round_trips_exactly() {
        let w = make_world_500();
        let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");
        let save2: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let w2 = World::from_save_v1(save2).expect("from_save_v1");
        assert_eq!(
            w2.grass.density, w.grass.density,
            "grass.density must match"
        );
        assert_eq!(
            w2.grass.scratch.len(),
            w.grass.density.len(),
            "scratch.len must equal density.len"
        );
    }

    /// S34: LoadError implements std::error::Error so it can be boxed.
    #[test]
    fn load_error_implements_std_error() {
        let _: Box<dyn std::error::Error> = Box::new(LoadError::StructuralError("x".into()));
    }

    // ─────────── S12: validate_save tests ───────────

    fn make_valid_save() -> crate::save::SaveV1 {
        let mut w = World::new("s12-valid");
        w.tick_once();
        SaveV1::from_world(&w)
    }

    /// S12 positive: a freshly serialised world passes validate_save.
    #[test]
    fn s12_valid_save_passes() {
        let save = make_valid_save();
        assert!(
            validate_save(&save).is_ok(),
            "freshly serialised world must pass validate_save"
        );
    }

    /// S12 negative 1: mismatched SoA column lengths.
    #[test]
    fn s12_soa_length_mismatch_fails() {
        let mut save = make_valid_save();
        // Drop one entry from x so len != id.len().
        save.creatures.x.pop();
        let result = validate_save(&save);
        assert!(
            matches!(result, Err(LoadError::StructuralError(_))),
            "SoA length mismatch must yield StructuralError, got: {result:?}"
        );
    }

    /// P1f negative: wrong grass cell count.
    #[test]
    fn p1f_grass_cell_count_mismatch_fails() {
        let mut save = make_valid_save();
        save.grass.density.pop(); // one short
        let result = validate_save(&save);
        assert!(
            matches!(result, Err(LoadError::StructuralError(_))),
            "wrong grass density len must yield StructuralError"
        );
    }

    /// P1f negative: NaN in grass density.
    #[test]
    fn p1f_nan_grass_density_fails() {
        let mut save = make_valid_save();
        save.grass.density[0] = f32::NAN;
        assert!(
            matches!(validate_save(&save), Err(LoadError::StructuralError(_))),
            "NaN grass density must yield StructuralError"
        );
    }

    /// P1f negative: out-of-range grass density.
    #[test]
    fn p1f_out_of_range_grass_density_fails() {
        let mut save = make_valid_save();
        save.grass.density[0] = 2.0;
        assert!(
            matches!(validate_save(&save), Err(LoadError::StructuralError(_))),
            "out-of-range grass density must yield StructuralError"
        );
    }

    /// P3a: old-schema version 3 returns SchemaVersionMismatch { found: 3, expected: 4 }.
    #[test]
    fn load_old_schema_returns_version_mismatch_error() {
        let mut w = World::new("p2g-old-schema");
        w.tick_once();
        let mut save = SaveV1::from_world(&w);
        save.schema_version = 3;
        match World::from_save_v1(save) {
            Err(LoadError::SchemaVersionMismatch {
                found: 3,
                expected: 4,
            }) => {}
            Ok(_) => panic!("expected SchemaVersionMismatch but got Ok"),
            Err(e) => {
                panic!("expected SchemaVersionMismatch {{ found: 3, expected: 4 }} but got {e:?}")
            }
        }
    }

    /// S12: non-finite nn_mutation_rate.
    #[test]
    fn s12_nan_nn_mutation_rate_fails() {
        let mut save = make_valid_save();
        if !save.creatures.brains.is_empty() {
            save.creatures.brains[0].nn_mutation_rate = f32::NAN;
        } else {
            // No creatures: inject a dummy brain with NaN rate.
            save.creatures.brains.push({
                let mut b = crate::brain::Brain::founder(&mut crate::rng::SimRng::from_u64(0));
                b.nn_mutation_rate = f32::NAN;
                b
            });
            // keep ids in sync so soa check passes
            save.creatures.id.push(999);
            save.creatures.x.push(0.0);
            save.creatures.y.push(0.0);
            save.creatures.vx.push(0.0);
            save.creatures.vy.push(0.0);
            save.creatures.energy.push(1.0);
            save.creatures.age.push(0);
            save.creatures.digestion_cooldown.push(0);
            save.creatures.cumulative_upkeep.push(0.0);
            save.creatures.species_id.push(0);
            save.creatures.parent_species_id.push(0);
            save.creatures
                .last_action
                .push(crate::creature::Action::Rest);
            save.creatures
                .action_this_tick
                .push(crate::creature::Action::Rest);
            save.creatures.max_size_reached.push(0.0);
            save.creatures.distance_travelled.push(0.0);
            save.creatures.birth_tick.push(0);
            save.creatures
                .genomes
                .push(crate::genome::Genome::founder());
            // v3 columns (P2g).
            save.creatures.nose_count.push(0);
            save.creatures.move_bias_x.push(0.0);
            save.creatures.move_bias_y.push(0.0);
            save.creatures.move_bias_reroll_at.push(0);
        }
        let result = validate_save(&save);
        assert!(
            matches!(result, Err(LoadError::StructuralError(_))),
            "NaN nn_mutation_rate must yield StructuralError"
        );
    }

    /// S12 negative 5: non-finite creature position.
    #[test]
    fn s12_nan_position_fails() {
        let mut save = make_valid_save();
        if !save.creatures.x.is_empty() {
            save.creatures.x[0] = f32::NAN;
        }
        let result = validate_save(&save);
        assert!(
            matches!(result, Err(LoadError::StructuralError(_))),
            "NaN position must yield StructuralError"
        );
    }

    /// S12 negative 6: non-finite creature energy.
    #[test]
    fn s12_nan_energy_fails() {
        let mut save = make_valid_save();
        if !save.creatures.energy.is_empty() {
            save.creatures.energy[0] = f32::INFINITY;
        }
        let result = validate_save(&save);
        assert!(
            matches!(result, Err(LoadError::StructuralError(_))),
            "Inf energy must yield StructuralError"
        );
    }

    /// S12 negative 7: parent_species_id == u32::MAX sentinel.
    #[test]
    fn s12_parent_species_id_sentinel_fails() {
        let mut save = make_valid_save();
        if !save.creatures.parent_species_id.is_empty() {
            save.creatures.parent_species_id[0] = u32::MAX;
        }
        let result = validate_save(&save);
        assert!(
            matches!(result, Err(LoadError::StructuralError(_))),
            "u32::MAX parent_species_id must yield StructuralError"
        );
    }

    /// S12 negative 8: non-finite slider value.
    #[test]
    fn s12_nan_slider_fails() {
        let mut save = make_valid_save();
        save.sliders.mutation_rate_multiplier = f32::NAN;
        let result = validate_save(&save);
        assert!(
            matches!(result, Err(LoadError::StructuralError(_))),
            "NaN slider must yield StructuralError"
        );
    }

    // ─────────── P2g: v3 column validation tests ───────────

    /// P2g: nose_count exceeding NOSE_VALID max is rejected.
    #[test]
    fn p2g_invalid_nose_count_fails() {
        let mut save = make_valid_save();
        if !save.creatures.nose_count.is_empty() {
            save.creatures.nose_count[0] = 6; // > NOSE_VALID max (5)
        } else {
            return; // no creatures in world — skip
        }
        assert!(
            matches!(validate_save(&save), Err(LoadError::StructuralError(_))),
            "nose_count > 5 must yield StructuralError"
        );
    }

    /// P2g: non-finite move_bias_x is rejected.
    #[test]
    fn p2g_nan_move_bias_x_fails() {
        let mut save = make_valid_save();
        if !save.creatures.move_bias_x.is_empty() {
            save.creatures.move_bias_x[0] = f32::NAN;
        } else {
            return;
        }
        assert!(
            matches!(validate_save(&save), Err(LoadError::StructuralError(_))),
            "NaN move_bias_x must yield StructuralError"
        );
    }

    /// P2g: non-finite move_bias_y is rejected.
    #[test]
    fn p2g_nan_move_bias_y_fails() {
        let mut save = make_valid_save();
        if !save.creatures.move_bias_y.is_empty() {
            save.creatures.move_bias_y[0] = f32::INFINITY;
        } else {
            return;
        }
        assert!(
            matches!(validate_save(&save), Err(LoadError::StructuralError(_))),
            "Inf move_bias_y must yield StructuralError"
        );
    }

    /// P2g: mismatched move_bias_x column length fails.
    #[test]
    fn p2g_move_bias_length_mismatch_fails() {
        let mut save = make_valid_save();
        save.creatures.move_bias_x.pop();
        assert!(
            matches!(validate_save(&save), Err(LoadError::StructuralError(_))),
            "move_bias_x length mismatch must yield StructuralError"
        );
    }

    /// P2g: nose_count and move_bias_* round-trip through save/load.
    #[test]
    fn p2g_v3_fields_round_trip() {
        let w = make_world_500();
        let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");
        let save2: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let w2 = World::from_save_v1(save2).expect("from_save_v1");
        let n = w.creatures.len();
        assert_eq!(w2.creatures.len(), n);
        for i in 0..n {
            assert_eq!(
                w2.creatures.genomes[i].nose_count, w.creatures.genomes[i].nose_count,
                "nose_count[{i}]"
            );
            assert_eq!(
                w2.creatures.move_bias_x[i], w.creatures.move_bias_x[i],
                "move_bias_x[{i}]"
            );
            assert_eq!(
                w2.creatures.move_bias_y[i], w.creatures.move_bias_y[i],
                "move_bias_y[{i}]"
            );
            assert_eq!(
                w2.creatures.move_bias_reroll_at[i], w.creatures.move_bias_reroll_at[i],
                "move_bias_reroll_at[{i}]"
            );
        }
    }
}
