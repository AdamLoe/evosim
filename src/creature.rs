//! Creature SoA. Layout shared by the live (mutable) tick state.
//!
//! v2.0 Wave 2a: per-creature 6-trait body `Genome` reintroduced (single-pool).
//! Six `f32 ∈ [0,1]` rescaled at each use site — see `Genome` below. Only the
//! brain + genome vary across the population; everything else is constants.
//! v1.5 S5b: vision raycast removed; `eye_trig`, `last2_action`, `prev_energy`
//! columns deleted (no consumer after the 32-input semantic rewrite).

use crate::brain::Brain;
use serde::{Deserialize, Serialize};

/// One discrete action a creature chose this tick.
/// D9: collapsed from 6 variants to 3. Indices 0/1/2.
/// v2.0 Wave 2a: `Eat` renamed to `Attack` (single-pool predation; the action
/// hits any creature in reach). The repr indices are unchanged so the NN action
/// logit order (0=Graze, 1=Attack, 2=Split) and all SIMD numerics are stable.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Action {
    Graze = 0,
    Attack = 1,
    Split = 2,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::Graze, Action::Attack, Action::Split];
}

/// Per-creature evolving body genome (v2.0 Wave 2a, single-pool).
///
/// Six independent traits in `[0, 1]`. Each is rescaled at its use site (the
/// ranges are centered so a trait of 0.5 reproduces the pre-genome constant):
///
/// | trait            | rescale → factor                | plugs into |
/// |------------------|---------------------------------|------------|
/// | `body_size`      | `0.5 + t` ∈ [0.5, 1.5]          | sprite + repulsion radius, energy-cap factor, attack damage |
/// | `max_speed`      | `0.5 + t` ∈ [0.5, 1.5]          | speed cap × factor, move-cost × factor |
/// | `metabolism`     | `0.5 + t` ∈ [0.5, 1.5]          | idle-upkeep multiplier |
/// | `diet`           | `t` (0 grazer … 1 predator)     | graze yield × (1 - 0.5·diet); attack effectiveness × (0.5 + diet) |
/// | `water_affinity` | `t`                             | water move-penalty × (1 - t) |
/// | `heat_tolerance` | `t`                             | desert move-penalty × (1 - t) |
///
/// A median genome (all 0.5) reproduces today's behaviour for body_size /
/// max_speed / metabolism. `diet`/`water_affinity`/`heat_tolerance` at 0.5 sit
/// at the middle of their effect range.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Genome {
    pub body_size: f32,
    pub max_speed: f32,
    pub metabolism: f32,
    pub diet: f32,
    pub water_affinity: f32,
    pub heat_tolerance: f32,
}

impl Genome {
    /// The neutral median genome — every trait at 0.5. Used as the default at
    /// founder seeding before randomization and as a clamp reference.
    pub const fn median() -> Self {
        Self {
            body_size: 0.5,
            max_speed: 0.5,
            metabolism: 0.5,
            diet: 0.5,
            water_affinity: 0.5,
            heat_tolerance: 0.5,
        }
    }

    /// Draw a founder genome uniformly in `[0, 1]` per trait (single-pool:
    /// visible diversity from tick 0). Trait draw order is fixed for RNG
    /// determinism: body_size, max_speed, metabolism, diet, water_affinity,
    /// heat_tolerance.
    pub fn founder(rng: &mut crate::rng::SimRng) -> Self {
        Self {
            body_size: rng.unit(),
            max_speed: rng.unit(),
            metabolism: rng.unit(),
            diet: rng.unit(),
            water_affinity: rng.unit(),
            heat_tolerance: rng.unit(),
        }
    }

    /// v2.0 Wave 4: the canonical "first member" genome for a species anchor,
    /// rolled BIASED to survive the biome under the anchor. Only the
    /// terrain-survival traits (`water_affinity`, `heat_tolerance`) are biased;
    /// `body_size` / `max_speed` / `metabolism` / `diet` stay uniform-random so
    /// species still differ in grazer/predator lean + body.
    ///
    /// Bias mapping (LOG + FLAG — feel-affecting):
    ///   * `Biome::Water`  → `water_affinity` ∈ [HIGH_LO, HIGH_HI], `heat_tolerance` random.
    ///   * `Biome::Desert` → `heat_tolerance` ∈ [HIGH_LO, HIGH_HI], `water_affinity` random.
    ///   * `Biome::Plains` → both ∈ [MODERATE_LO, MODERATE_HI].
    ///
    /// RNG draw order is fixed for determinism: body_size, max_speed, metabolism,
    /// diet, then water_affinity, then heat_tolerance (each one draw), so the
    /// stream is well-defined regardless of biome. `rng` is the dedicated
    /// `world_seed`-derived seeding PRNG (NOT the sim RNG).
    pub fn canonical_for_biome(rng: &mut crate::rng::SimRng, biome: crate::constants::Biome) -> Self {
        use crate::constants::{
            Biome, BIOME_BIAS_HIGH_HI, BIOME_BIAS_HIGH_LO, BIOME_BIAS_MODERATE_HI,
            BIOME_BIAS_MODERATE_LO,
        };
        let body_size = rng.unit();
        let max_speed = rng.unit();
        let metabolism = rng.unit();
        let diet = rng.unit();
        // Always draw water then heat in fixed order (even when one is overwritten
        // by the biased band) so the RNG stream is biome-independent in length.
        let water_rand = rng.unit();
        let heat_rand = rng.unit();
        let (water_affinity, heat_tolerance) = match biome {
            Biome::Water => (
                BIOME_BIAS_HIGH_LO + water_rand * (BIOME_BIAS_HIGH_HI - BIOME_BIAS_HIGH_LO),
                heat_rand,
            ),
            Biome::Desert => (
                water_rand,
                BIOME_BIAS_HIGH_LO + heat_rand * (BIOME_BIAS_HIGH_HI - BIOME_BIAS_HIGH_LO),
            ),
            Biome::Plains => (
                BIOME_BIAS_MODERATE_LO + water_rand * (BIOME_BIAS_MODERATE_HI - BIOME_BIAS_MODERATE_LO),
                BIOME_BIAS_MODERATE_LO + heat_rand * (BIOME_BIAS_MODERATE_HI - BIOME_BIAS_MODERATE_LO),
            ),
        };
        Self {
            body_size,
            max_speed,
            metabolism,
            diet,
            water_affinity,
            heat_tolerance,
        }
    }

    /// Mutating child genome: each trait takes one Gaussian nudge
    /// `+= rng.normal() * step` then is clamped to `[0, 1]`. Six normal draws
    /// in fixed trait order (RNG determinism). `step` is the bucket sigma scaled
    /// by `trait_mutation_sigma_multiplier`; if `step <= 0` the genome is copied
    /// verbatim (no draws).
    pub fn mutated(&self, rng: &mut crate::rng::SimRng, step: f32) -> Self {
        // Non-positive OR NaN step → copy verbatim (no draws).
        if step.is_nan() || step <= 0.0 {
            return *self;
        }
        let m = |v: f32, r: &mut crate::rng::SimRng| (v + r.normal() * step).clamp(0.0, 1.0);
        Genome {
            body_size: m(self.body_size, rng),
            max_speed: m(self.max_speed, rng),
            metabolism: m(self.metabolism, rng),
            diet: m(self.diet, rng),
            water_affinity: m(self.water_affinity, rng),
            heat_tolerance: m(self.heat_tolerance, rng),
        }
    }

    /// v2.0 Wave 3a: per-trait crossover of two parent genomes (the genome twin
    /// of `Brain::child_from_crossover_with_sigma`). Applied BEFORE the mutation
    /// step. RNG draw order (deterministic): `FiftyFifty` consumes exactly one
    /// `rng.unit()` per trait, in fixed trait order (body_size, max_speed,
    /// metabolism, diet, water_affinity, heat_tolerance); `Average` consumes NO
    /// RNG (pure midpoint).
    pub fn crossed(
        &self,
        other: &Genome,
        rng: &mut crate::rng::SimRng,
        crossover: crate::constants::CrossoverMode,
    ) -> Self {
        use crate::constants::CrossoverMode;
        match crossover {
            CrossoverMode::Average => Genome {
                body_size: 0.5 * (self.body_size + other.body_size),
                max_speed: 0.5 * (self.max_speed + other.max_speed),
                metabolism: 0.5 * (self.metabolism + other.metabolism),
                diet: 0.5 * (self.diet + other.diet),
                water_affinity: 0.5 * (self.water_affinity + other.water_affinity),
                heat_tolerance: 0.5 * (self.heat_tolerance + other.heat_tolerance),
            },
            CrossoverMode::FiftyFifty => {
                // One draw per trait, fixed order. `pick(a, b)` first so the
                // draw happens in declaration order regardless of which wins.
                let pick = |a: f32, b: f32, r: &mut crate::rng::SimRng| {
                    if r.unit() < 0.5 {
                        a
                    } else {
                        b
                    }
                };
                Genome {
                    body_size: pick(self.body_size, other.body_size, rng),
                    max_speed: pick(self.max_speed, other.max_speed, rng),
                    metabolism: pick(self.metabolism, other.metabolism, rng),
                    diet: pick(self.diet, other.diet, rng),
                    water_affinity: pick(self.water_affinity, other.water_affinity, rng),
                    heat_tolerance: pick(self.heat_tolerance, other.heat_tolerance, rng),
                }
            }
        }
    }

    /// Sprite/body radius factor (× `CREATURE_SIZE`). 0.5 trait → 1.0 (today).
    #[inline]
    pub fn body_size_factor(&self) -> f32 {
        0.5 + self.body_size
    }
    /// Speed-cap + move-cost factor (× `MOVE_SPEED_MAX` / × move cost).
    #[inline]
    pub fn max_speed_factor(&self) -> f32 {
        0.5 + self.max_speed
    }
    /// Idle-upkeep multiplier. 0.5 trait → 1.0 (today).
    #[inline]
    pub fn metabolism_factor(&self) -> f32 {
        0.5 + self.metabolism
    }
}

/// Transient action tag recorded for the ring-flash (v2.0 Wave 2a). Stored
/// per-creature alongside a small ticks-remaining countdown; packed into the
/// snapshot for the renderer (2b draws the ring). Priority when several fire in
/// one tick (highest wins): Killed > CreatedChild > Attacked > Grazed. `Born`
/// is set once at spawn. `None` = no active flash.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum FlashTag {
    None = 0,
    Born = 1,        // teal — just born
    Grazed = 2,      // green — grazed
    Attacked = 3,    // yellow — attacked
    CreatedChild = 4, // blue — created a child (split initiator)
    Killed = 5,      // red — killed another creature
}

impl FlashTag {
    /// Priority for "which flash wins when multiple fire this tick". Higher =
    /// stronger. `Born` is set at spawn and never competes within a tick.
    #[inline]
    pub fn priority(self) -> u8 {
        match self {
            FlashTag::None => 0,
            FlashTag::Born => 1,
            FlashTag::Grazed => 2,
            FlashTag::Attacked => 3,
            FlashTag::CreatedChild => 4,
            FlashTag::Killed => 5,
        }
    }
}

/// Ticks a ring-flash stays lit after the action that set it (v2.0 Wave 2a).
pub const FLASH_TICKS: u8 = 5;

/// Hot per-creature scalars promoted into SoA arrays for cache friendliness
/// in the per-tick inner loops.
///
/// v2.0 Wave 2a: `genome` column added; the action-EMA `color_r/g/b` columns
/// were deleted (display color now derives from the genome at snapshot write,
/// and per-action highlight lives in the ring-flash `flash_*` columns).
pub struct CreatureSoA {
    pub id: Vec<u64>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub energy: Vec<f32>,
    pub age: Vec<u32>,
    pub digestion_cooldown: Vec<u32>,
    pub cumulative_upkeep: Vec<f32>,
    pub last_action: Vec<Action>,
    pub action_this_tick: Vec<Action>,
    pub distance_travelled: Vec<f32>,
    /// Tick at which this creature was born (for lifespan stats).
    pub birth_tick: Vec<u32>,
    /// Ticks elapsed since the creature last performed a Split action (or
    /// since birth, if it has never split). Reset to 0 on the tick a parent
    /// splits; incremented in bookkeeping_tail. Drives NN input slot 6.
    pub ticks_since_split: Vec<u32>,
    pub brains: Vec<Brain>,
    /// v2.0 Wave 2a: per-creature evolving 6-trait body genome. Sim-side only —
    /// never rides the snapshot (the render-side color is *derived* from it at
    /// write time). Founders draw uniformly in [0,1]; children mutate off the
    /// same per-birth mutation bucket as the brain (sigma × the trait-sigma
    /// slider).
    pub genome: Vec<Genome>,
    /// v2.0 Wave 2a: transient ring-flash action tag + ticks remaining.
    /// `flash_tag[i]` is the highest-priority action that fired recently;
    /// `flash_ticks[i]` counts down to 0 (cleared in bookkeeping_tail). Packed
    /// into the snapshot for the renderer (2b draws the ring).
    pub flash_tag: Vec<FlashTag>,
    pub flash_ticks: Vec<u8>,
    /// v2.0 Wave 3a: per-creature species id. Unused (always 0) in single-pool
    /// mode; in `species_mode` it keys the species color + the Attack /
    /// same-species gates and rides the snapshot `packed_u32` (bits 7..23).
    pub species_id: Vec<u16>,
    /// v2.0 Wave 3a: initiator-only post-mating cooldown (ticks). Decrements once
    /// per tick in `energy_bookkeeping`. Gates whether `Mate` is a legal NN pick
    /// (species_mode only). Distinct from `digestion_cooldown` (which gates
    /// Attack). Always 0 / unused in single-pool mode.
    pub mating_cooldown: Vec<u32>,
}

impl CreatureSoA {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            id: Vec::with_capacity(cap),
            x: Vec::with_capacity(cap),
            y: Vec::with_capacity(cap),
            vx: Vec::with_capacity(cap),
            vy: Vec::with_capacity(cap),
            energy: Vec::with_capacity(cap),
            age: Vec::with_capacity(cap),
            digestion_cooldown: Vec::with_capacity(cap),
            cumulative_upkeep: Vec::with_capacity(cap),
            last_action: Vec::with_capacity(cap),
            action_this_tick: Vec::with_capacity(cap),
            distance_travelled: Vec::with_capacity(cap),
            birth_tick: Vec::with_capacity(cap),
            ticks_since_split: Vec::with_capacity(cap),
            brains: Vec::with_capacity(cap),
            genome: Vec::with_capacity(cap),
            flash_tag: Vec::with_capacity(cap),
            flash_ticks: Vec::with_capacity(cap),
            species_id: Vec::with_capacity(cap),
            mating_cooldown: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.x.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append one creature. Returns new index. v2.0 Wave 2a: takes the
    /// per-creature `genome`. v2.0 Wave 3a: takes `species_id` (0 = single-pool
    /// / unused). New creatures are born with a `Born` ring-flash (teal) lit for
    /// `FLASH_TICKS`.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        id: u64,
        x: f32,
        y: f32,
        energy: f32,
        birth_tick: u32,
        brain: Brain,
        genome: Genome,
        species_id: u16,
    ) -> usize {
        self.id.push(id);
        self.x.push(x);
        self.y.push(y);
        self.vx.push(0.0);
        self.vy.push(0.0);
        self.energy.push(energy);
        self.age.push(0);
        self.digestion_cooldown.push(0);
        self.cumulative_upkeep.push(0.0);
        self.last_action.push(Action::Graze);
        self.action_this_tick.push(Action::Graze);
        self.distance_travelled.push(0.0);
        self.birth_tick.push(birth_tick);
        self.ticks_since_split.push(0);
        self.genome.push(genome);
        // v2.0 Wave 2a: newborns flash teal ("born").
        self.flash_tag.push(FlashTag::Born);
        self.flash_ticks.push(FLASH_TICKS);
        self.species_id.push(species_id);
        self.mating_cooldown.push(0);
        self.brains.push(brain);
        self.x.len() - 1
    }

    /// Record an action ring-flash for creature `i`, keeping the
    /// highest-priority tag if one is already lit this tick. Resets the
    /// countdown to `FLASH_TICKS`.
    #[inline]
    pub fn set_flash(&mut self, i: usize, tag: FlashTag) {
        // Only overwrite if the new tag has >= priority OR the current flash
        // has expired. This makes "killed" beat "attacked" beat "grazed" within
        // a single tick regardless of write order.
        if self.flash_ticks[i] == 0 || tag.priority() >= self.flash_tag[i].priority() {
            self.flash_tag[i] = tag;
            self.flash_ticks[i] = FLASH_TICKS;
        }
    }

    /// Remove indices `dead` (must be sorted ascending). Uses swap_remove
    /// from the back so we only touch O(K) entries.
    pub fn remove_indices(&mut self, dead: &[usize]) {
        // walk dead from the back so swap-remove doesn't disturb earlier indices.
        for &k in dead.iter().rev() {
            self.id.swap_remove(k);
            self.x.swap_remove(k);
            self.y.swap_remove(k);
            self.vx.swap_remove(k);
            self.vy.swap_remove(k);
            self.energy.swap_remove(k);
            self.age.swap_remove(k);
            self.digestion_cooldown.swap_remove(k);
            self.cumulative_upkeep.swap_remove(k);
            self.last_action.swap_remove(k);
            self.action_this_tick.swap_remove(k);
            self.distance_travelled.swap_remove(k);
            self.birth_tick.swap_remove(k);
            self.ticks_since_split.swap_remove(k);
            self.genome.swap_remove(k);
            self.flash_tag.swap_remove(k);
            self.flash_ticks.swap_remove(k);
            self.species_id.swap_remove(k);
            self.mating_cooldown.swap_remove(k);
            self.brains.swap_remove(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Brain, NnTopology};
    use crate::rng::SimRng;

    /// v1.5 S5b: remove_indices keeps every SoA column length-consistent.
    #[test]
    fn remove_indices_preserves_column_lengths() {
        let mut soa = CreatureSoA::with_capacity(4);
        let mut rng = SimRng::from_u64(1);
        for i in 0..4u64 {
            let b = Brain::founder(&mut rng, NnTopology::legacy());
            soa.push(i, i as f32 * 10.0, 0.0, 100.0, 0, b, Genome::median(), 0);
        }
        assert_eq!(soa.len(), 4);
        soa.remove_indices(&[1]);
        assert_eq!(soa.len(), 3);
        assert_eq!(soa.x.len(), 3);
        assert_eq!(soa.genome.len(), 3);
        assert_eq!(soa.flash_tag.len(), 3);
        assert_eq!(soa.ticks_since_split.len(), 3);
    }

    /// v2.0 Wave 2a: push takes a Genome; len round-trips correctly.
    #[test]
    fn push_with_genome_roundtrips_len() {
        let mut soa = CreatureSoA::with_capacity(4);
        let mut rng = SimRng::from_u64(42);
        for k in 0u64..5 {
            let b = Brain::founder(&mut rng, NnTopology::legacy());
            let g = Genome::founder(&mut rng);
            soa.push(k, k as f32, 0.0, 100.0, 0, b, g, 0);
        }
        assert_eq!(soa.len(), 5);
        soa.remove_indices(&[0, 2]);
        assert_eq!(soa.len(), 3);
    }

    /// v2.0 Wave 2a: newborns flash "Born" (teal) at spawn.
    #[test]
    fn newborns_flash_born_at_birth() {
        let mut soa = CreatureSoA::with_capacity(2);
        let mut rng = SimRng::from_u64(7);
        for k in 0u64..3 {
            let b = Brain::founder(&mut rng, NnTopology::legacy());
            soa.push(k, k as f32, 0.0, 100.0, 0, b, Genome::median(), 0);
        }
        for i in 0..soa.len() {
            assert_eq!(soa.flash_tag[i], FlashTag::Born, "flash_tag[{i}] must be Born");
            assert_eq!(soa.flash_ticks[i], FLASH_TICKS, "flash_ticks[{i}] must be FLASH_TICKS");
        }
    }

    /// v2.0 Wave 2a: set_flash respects priority (Killed beats Grazed within a tick).
    #[test]
    fn set_flash_keeps_highest_priority() {
        let mut soa = CreatureSoA::with_capacity(1);
        let mut rng = SimRng::from_u64(3);
        let b = Brain::founder(&mut rng, NnTopology::legacy());
        soa.push(0, 0.0, 0.0, 100.0, 0, b, Genome::median(), 0);
        // Lower-priority Grazed over the Born-at-spawn (Born p=1 < Grazed p=2).
        soa.set_flash(0, FlashTag::Grazed);
        assert_eq!(soa.flash_tag[0], FlashTag::Grazed);
        // Killed (p=5) beats Grazed.
        soa.set_flash(0, FlashTag::Killed);
        assert_eq!(soa.flash_tag[0], FlashTag::Killed);
        // Attacked (p=3) does NOT downgrade Killed within the same tick.
        soa.set_flash(0, FlashTag::Attacked);
        assert_eq!(soa.flash_tag[0], FlashTag::Killed);
    }
}
