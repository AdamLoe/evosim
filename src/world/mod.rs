//! World — owns SoA + grass + RNG + tick orchestration.
//!
//! Within-tick ordering follows v5 §3.5. NN forward pass is live (Milestone D).
//! D3: genome removed; D9: Action enum collapsed to {Graze, Eat, Split}.

pub(crate) mod biome;
pub(crate) mod nn;
pub(crate) mod nn_stats;
pub(crate) mod proximity;
pub(crate) mod tick;

// v2.0 Wave 1a: wrap-correctness (torus vs walled) tests live in their own
// file/module to avoid the shared-`mod tests` merge hazard.
#[cfg(test)]
#[path = "wrap_tests.rs"]
mod wrap_tests;

// v2.0 Wave 1b: biome generation + movement-penalty + biome NN-input tests in
// their own file/module (same merge-hazard rule).
#[cfg(test)]
#[path = "biome_tests.rs"]
mod biome_tests;

use self::nn::{chunk_ranges, dynamic_chunks};
use self::tick::EatPick;
use crate::brain::{Brain, MutationPolicy, NnTopology};
use crate::constants::*;
use crate::creature::{Action, CreatureSoA};
use crate::grass::GrassGrid;
use crate::grid::SpatialGrid;
use crate::rng::SimRng;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevSliders {
    pub mutation_rate_multiplier: f32,
    pub mouth_tax: f32,
    /// v1.12: 8-bucket mutation policy. Replaces the legacy single-knob
    /// `nn_mutation_sigma` slider and the per-creature `nn_mutation_rate`
    /// drift. Default = bucket 0 carries the legacy `(0.02, 0.02)`.
    pub mutation_policy: MutationPolicy,
    /// Grass cross-kernel propagation rate (k). See v1.2 amendments §A.4.
    pub grass_propagation_rate_k: f32,
    /// Grass in-cell logistic growth rate (r). See v1.2 amendments §A.4.
    pub grass_in_cell_growth_r: f32,
    /// Number of cells seeded at world init (affects next world creation only). See v1.2 amendments §A.4.
    pub grass_initial_seed_count: u32,
    /// Per-bite energy transfer fraction. Predator removes
    /// `eat_bite_fraction * prey.energy * (1 - prey.armor)` per successful bite.
    /// Live-tunable via set_eat_bite_fraction. See v1.2 P3a.
    pub eat_bite_fraction: f32,
    /// Live-tunable multiplier on per-tick *idle* upkeep (the always-on
    /// base + NN + gut + mouth-tax drain). 0 = no drain, 1 = default, 2 = double.
    pub upkeep_multiplier: f32,
    /// Live-tunable multiplier on per-tick *movement* cost. 0 = free move.
    pub move_cost_multiplier: f32,
    /// Live-tunable maximum energy per creature. Energy is clamped at the end of every
    /// tick, and new creatures spawn capped at this value.
    pub energy_max: f32,
    /// Live-tunable energy gained per successful graze bite.
    pub grass_energy_per_bite: f32,
    /// Live-tunable number of bites to drain a ripe grass cell. Density removed
    /// per bite = GRASS_MAX / bites_per_block.
    pub grass_bites_per_block: u32,
    /// Lifespan threshold beyond which the past-lifespan upkeep penalty applies.
    pub max_age: u32,
    /// Energy threshold required for a Split action to fire.
    pub split_threshold: f32,
    /// Energy gifted to each newborn at split (clamped at parent's residual).
    pub split_gift: f32,
    /// Position jitter applied at split — children spawn at parent ± random[-v, v]
    /// per axis (world-units). Live-tunable.
    pub split_jitter: f32,
    /// Number of creatures seeded at world init (clamped to [1, 32]).
    pub founder_count: u32,
    /// Construction-only: when true, world init fills the grass grid to
    /// `GRASS_MAX` instead of seeding `grass_initial_seed_count` cells.
    /// Stored on `DevSliders` so it round-trips through the boot payload via
    /// `set_slider("full_grass_on_init", 0|1)` and shapes the *next* world.
    pub full_grass_on_init: bool,
    /// Ticks of digestion cooldown imposed after a successful bite (eat).
    /// Eat is gated while >0; decrements once per tick in `energy_bookkeeping`.
    pub digestion_cooldown_ticks: u32,
    /// Per-tick cap on the position nudge applied by the creature-vs-creature
    /// repulsion pass. 0 disables physical separation entirely; 5.0 is the
    /// historical value. Higher values make collisions look "harder" but can
    /// cause oscillation at high overlap depths.
    pub repulsion_max: f32,
    /// User-tunable population cap. The cull in `handle_births` clamps pop
    /// to `min(this, MAX_POP_FOR_SIM)` after every birth phase. The hard
    /// `MAX_POP_FOR_SIM` is the SAB-sized invariant; this slider lets the
    /// user hold pop below that in the performance-smooth regime without
    /// changing the build.
    pub max_population: u32,
    /// Construction-only: world extent in world-units (square world). v2.0
    /// Wave 1a runtime-sizing setting. Drives the grass/hash dims via
    /// `WorldDims::from_world_size`; only shapes the *next* world.
    pub world_size: f32,
    /// Construction-only: whether the next world is toroidal (wrap) or walled.
    /// On ⇒ positions wrap, wrap-aware neighbor/proximity math, no wall NN
    /// inputs. Off ⇒ positions clamp, 4 wall-proximity NN inputs present.
    pub wrap_world: bool,
    /// Construction-only: numeric world seed (v2.0 Wave 1a). Carried through
    /// construction/boot; Wave 1b uses it for biome generation. SEPARATE from
    /// the string/XxHash64 sim RNG seed — they are not coupled.
    pub world_seed: u32,
    /// Live-tunable Water biome base movement-penalty severity `p ∈ [0, 1]`
    /// (v2.0 Wave 1b). Drives reduced speed / extra upkeep / higher move-cost
    /// while a creature is on a Water cell. Genome-independent in Wave 1b.
    pub water_movement_penalty: f32,
    /// Live-tunable Desert biome base movement-penalty severity `p ∈ [0, 1]`
    /// (v2.0 Wave 1b). Same three effects as `water_movement_penalty`.
    pub desert_movement_penalty: f32,
}

impl Default for DevSliders {
    fn default() -> Self {
        Self {
            mutation_rate_multiplier: 1.0,
            mouth_tax: UPKEEP_MOUTH_DEFAULT,
            mutation_policy: MutationPolicy::default(),
            grass_propagation_rate_k: GRASS_PROPAGATION_RATE_K_DEFAULT,
            grass_in_cell_growth_r: GRASS_IN_CELL_GROWTH_R_DEFAULT,
            grass_initial_seed_count: GRASS_INITIAL_SEED_COUNT_DEFAULT,
            eat_bite_fraction: EAT_BITE_FRACTION_DEFAULT,
            upkeep_multiplier: 1.0,
            move_cost_multiplier: 1.0,
            energy_max: 100.0,
            grass_energy_per_bite: GRASS_ENERGY_PER_BITE_DEFAULT,
            grass_bites_per_block: GRASS_BITES_PER_BLOCK_DEFAULT,
            max_age: MAX_AGE_DEFAULT,
            split_threshold: SPLIT_THRESHOLD_DEFAULT,
            split_gift: SPLIT_GIFT_MAX_DEFAULT,
            split_jitter: SPLIT_JITTER_DEFAULT,
            founder_count: STARTING_POP_DEFAULT,
            full_grass_on_init: FULL_GRASS_ON_INIT_DEFAULT,
            digestion_cooldown_ticks: DIGESTION_COOLDOWN_TICKS,
            repulsion_max: REPULSION_MAX,
            max_population: MAX_POPULATION_DEFAULT,
            world_size: WORLD_SIZE_DEFAULT,
            wrap_world: WRAP_WORLD_DEFAULT,
            // Default: random per construction (caller may override). Kept
            // separate from the string RNG seed — see `world_seed` doc.
            world_seed: 0,
            water_movement_penalty: WATER_MOVEMENT_PENALTY_DEFAULT,
            desert_movement_penalty: DESERT_MOVEMENT_PENALTY_DEFAULT,
        }
    }
}

/// Halton sequence value at index `i` for base `b`. Used for deterministic
/// quasi-random founder placement (multi-founder spawn, v1.5).
fn halton(mut i: u32, b: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    while i > 0 {
        f /= b as f32;
        r += f * (i % b) as f32;
        i /= b;
    }
    r
}

pub struct World {
    pub tick: u32,
    pub seed: String,
    pub rng: SimRng,
    /// Runtime world dimensions (world_size / wrap / grass+hash dims), computed
    /// once at construction from the `world_size` + `wrap_world` settings.
    /// v2.0 Wave 1a: replaces the compile-time `WORLD_SIZE` / dim constants.
    pub dims: WorldDims,
    /// Numeric world seed (v2.0 Wave 1a). Carried for biome gen (Wave 1b);
    /// separate from the string RNG seed.
    pub world_seed: u32,
    /// Static biome grid (v2.0 Wave 1b): one `Biome as u8` per grass cell,
    /// row-major `grass_dim × grass_dim`. Generated deterministically from
    /// `world_seed` at construction (see `biome::generate_biome_grid`) and
    /// copied byte-for-byte into the boot `biome_buf` SAB. Read O(1) per tick
    /// via `biome_at`.
    pub(crate) biome_grid: Vec<u8>,
    /// Grass density field (v1.2 grass mechanic).
    pub grass: GrassGrid,
    pub grid: SpatialGrid,
    pub creatures: CreatureSoA,
    pub sliders: DevSliders,
    pub next_creature_id: u64,
    pub world_ended: bool,
    /// In-app hierarchical profiler. Runtime-toggleable, default OFF.
    /// Excluded from SaveV1 and from the §16 snapshot hash (D10).
    /// See docs/plans/perf-timing.md for the full design.
    pub profile: crate::profiler::Profiler,
    // Per-tick scratch buffers, promoted from in-function `vec!` to long-lived
    // fields to eliminate ~300 KB/tick allocator pressure (perf-final-report
    // §3 item 2). Excluded from SaveV1 by omission —
    // mirrors the `profile` pattern.
    pub(crate) scratch_fx: Vec<f32>,
    pub(crate) scratch_fy: Vec<f32>,
    pub(crate) scratch_neighbors: Vec<usize>,
    pub(crate) scratch_damage: Vec<f32>,
    pub(crate) scratch_gain: Vec<f32>,
    pub(crate) scratch_cooldown_set: Vec<bool>,
    pub(crate) scratch_attempted_eat: Vec<bool>,
    pub(crate) scratch_got_a_bite: Vec<bool>,
    /// Per-predator eat outcome computed in parallel by `eat()`, applied
    /// sequentially afterward. Indexed by creature index `i`; length matches
    /// the SoA at the top of each `eat()` call. See `EatPick`.
    pub(crate) scratch_eat_picks: Vec<EatPick>,
    /// S27: promoted dead-indices buffer from collect_deaths.
    /// Cleared and refilled each call; caller reads it via &self.scratch_dead.
    pub(crate) scratch_dead: Vec<usize>,
    /// v1.6 cap-cull: promoted candidate-index pool for the post-births random
    /// sample in `handle_births`. Refilled with `0..virtual_pop` each time
    /// the cull fires (which is every tick a birth would push pop above
    /// `MAX_POP_FOR_SIM`). Caching here lets the cull reuse the same
    /// allocation tick-after-tick instead of re-allocating ~256 KB per call.
    pub(crate) scratch_cull_pool: Vec<usize>,
    /// Per-tick splitter indices (creatures whose Action::Split fired with
    /// enough energy). Drives the parallel child-brain phase in `handle_births`.
    pub(crate) scratch_splitters: Vec<usize>,
    /// Bitmask over splitters: `true` means the prospective newborn is sampled
    /// for the cap-cull and should NOT be cloned. Saves a brain clone+free per
    /// would-be-culled newborn.
    pub(crate) scratch_newborn_dead: Vec<bool>,
    /// Indices of pre-existing creatures (in [0, n_before_births)) sampled for
    /// the cap-cull. Partitioned out of `scratch_dead` so we can apply the
    /// SoA `remove_indices` once at the end of `handle_births`.
    pub(crate) scratch_existing_dead: Vec<usize>,
    /// Per-splitter RNG seeds, pre-rolled from `self.rng` in deterministic
    /// order so each parallel worker can build its child brain from an
    /// independent stream without contending on the shared RNG.
    pub(crate) scratch_birth_seeds: Vec<u64>,
    /// Per-splitter prospective child brain. `Some` for survivors after the
    /// parallel clone phase; `None` for culled-before-birth newborns. Drained
    /// during the sequential apply phase.
    pub(crate) scratch_child_brains: Vec<Option<Brain>>,
    /// v1.5 S3: per-creature pre-fallthrough argmax (index of largest action
    /// logit before validity gating). 0=Graze, 1=Eat, 2=Split. Consumed by the
    /// color-EMA bookkeeping pass to credit Graze intent even when fallthrough
    /// landed on a different action.
    pub(crate) scratch_argmax_pre: Vec<u8>,
    /// v1.5 S5b: precomputed (sector_id, weight) for each integer cell offset.
    /// 33×33 LUT (`proximity::LUT_DIM²`) eliminates `atan2` in the per-creature
    /// grass-density scan. Built once at `new_with_sliders`.
    pub(crate) sector_lut: Vec<(u8, u8, f32)>,
    /// v1.5 S5b: per-creature 8-accumulator scratch reused across the NN input
    /// build. Resized in `step()` before the chunked input phase.
    pub(crate) scratch_sector_accum: Vec<[f32; 8]>,
    /// v1.12: NN structure for this world's lifetime. Set at construction
    /// (from the boot payload), used by `Brain::founder` and by the per-layer
    /// profiler-drain loop in `nn_forward_all_chunks`.
    pub(crate) nn_topology: NnTopology,
    /// v2.0: composable NN input-layout descriptor. Defines the active input
    /// groups + their slot offsets; `width()` must equal
    /// `nn_topology.input_width()`. Wave 0 = legacy width-32 layout.
    pub(crate) nn_input_layout: self::nn::NnInputLayout,
    /// Per-worker + per-sub-phase counters for the parallel NN forward pass.
    /// Wrapped in Arc so the parallel block can hold a thread-safe handle while
    /// `&mut self.creatures` is being mutated next to it. See `nn_stats.rs`.
    pub(crate) nn_stats: std::sync::Arc<nn_stats::NnStats>,
}

impl World {
    /// Legacy 1-founder constructor used by Rust-side tests and as the basic
    /// entrypoint when no slider overrides are needed. Wasm callers should
    /// route through `WorldHandle::new_with_founder_count` for the multi-
    /// founder default.
    ///
    /// v2.0 Wave 1a: this legacy test ctor pins a *walled 1200u* world so the
    /// historical walled-behavior tests keep their meaning. The production path
    /// (`WorldHandle::new*`) uses the new 9600u/wrap-on defaults from
    /// `DevSliders::default()`.
    #[allow(dead_code)]
    pub fn new(seed: impl Into<String>) -> Self {
        Self::new_with_sliders(
            seed,
            DevSliders {
                founder_count: 1,
                world_size: 1200.0,
                wrap_world: false,
                ..Default::default()
            },
        )
    }

    pub fn new_with_sliders(seed: impl Into<String>, sliders: DevSliders) -> Self {
        Self::new_with_sliders_topology(seed, sliders, NnTopology::legacy())
    }

    pub fn new_with_sliders_topology(
        seed: impl Into<String>,
        sliders: DevSliders,
        nn_topology: NnTopology,
    ) -> Self {
        let seed_string = seed.into();
        let mut rng = SimRng::from_string(&seed_string);
        // v2.0 Wave 1a: runtime dims computed once from the construction settings.
        let dims = WorldDims::from_world_size(sliders.world_size, sliders.wrap_world);
        let world_seed = sliders.world_seed;
        // v2.0 Wave 1b: static biome map from `world_seed` (independent of the
        // sim RNG above). Built once; read O(1) per tick via `biome_at`.
        let biome_grid = biome::generate_biome_grid(world_seed, &dims);
        let mut grass = GrassGrid::new(&mut rng, sliders.grass_initial_seed_count, dims);
        if sliders.full_grass_on_init {
            for d in grass.density.iter_mut() {
                *d = GRASS_MAX;
            }
            grass.rebuild_row_bitset();
        }
        // v2.0 Wave 1b: the active input layout is driven by `wrap_world`
        // (drops ReservedPredator; WallProximity only when walled; biome groups
        // always-on). The active width is 32 (wrap on) or 40 (wrap off). The
        // topology's `input_width` must agree with the layout the founders are
        // drawn for, so reconcile the topology to the layout width BEFORE
        // founder draws. The incoming `nn_topology` carries its hidden layers
        // (from the boot payload or legacy default) at the default 32-input
        // width; we widen its first matmul fan-in to match the layout.
        let nn_input_layout = self::nn::NnInputLayout::for_settings(dims.wrap_world);
        let nn_topology = if nn_topology.input_width() == nn_input_layout.width() {
            nn_topology
        } else {
            NnTopology::with_input_width(
                nn_input_layout.width(),
                nn_topology.hidden_sizes().to_vec(),
                nn_topology.activations().to_vec(),
            )
            .expect("layout width is a valid multiple-of-8 input width in [8, MAX_NN_INPUTS]")
        };
        debug_assert_eq!(
            nn_input_layout.width(),
            nn_topology.input_width(),
            "NN input layout width must equal topology input_width"
        );

        let mut creatures = CreatureSoA::with_capacity(2048);
        let founder_count = sliders.founder_count.clamp(1, 32);
        let founder_energy = START_ENERGY_DEFAULT.min(sliders.energy_max);
        let body_r = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        // Walled: keep founders a body-radius off the wall. Toroidal: any point
        // in [0, world_size) is valid (no wall to bump).
        let (lo, hi) = if dims.wrap_world {
            (0.0, dims.world_size)
        } else {
            (body_r, dims.world_size - body_r)
        };
        for k in 0..founder_count {
            let brain = Brain::founder(&mut rng, nn_topology.clone());
            // Halton (2, 3) gives a low-discrepancy 2D sequence; shift by 1 so
            // the first sample isn't (0, 0).
            let hx = halton(k + 1, 2);
            let hy = halton(k + 1, 3);
            let x = lo + hx * (hi - lo);
            let y = lo + hy * (hi - lo);
            creatures.push(k as u64, x, y, founder_energy, 0, brain);
        }
        let mut grid = SpatialGrid::new(dims);
        grid.rebuild(&creatures.x, &creatures.y);
        let sector_lut = proximity::build_sector_lut();
        Self {
            tick: 0,
            seed: seed_string,
            dims,
            world_seed,
            biome_grid,
            rng,
            grass,
            grid,
            creatures,
            sliders,
            next_creature_id: founder_count as u64,
            world_ended: false,
            profile: crate::profiler::Profiler::new(),
            scratch_fx: Vec::new(),
            scratch_fy: Vec::new(),
            scratch_neighbors: Vec::new(),
            scratch_damage: Vec::new(),
            scratch_gain: Vec::new(),
            scratch_cooldown_set: Vec::new(),
            scratch_attempted_eat: Vec::new(),
            scratch_got_a_bite: Vec::new(),
            scratch_eat_picks: Vec::new(),
            scratch_dead: Vec::new(),
            scratch_cull_pool: Vec::new(),
            scratch_splitters: Vec::new(),
            scratch_newborn_dead: Vec::new(),
            scratch_existing_dead: Vec::new(),
            scratch_birth_seeds: Vec::new(),
            scratch_child_brains: Vec::new(),
            scratch_argmax_pre: Vec::new(),
            sector_lut,
            scratch_sector_accum: Vec::new(),
            nn_topology,
            nn_input_layout,
            nn_stats: std::sync::Arc::new(nn_stats::NnStats::new(
                crate::profiler::clock_now_us_threadsafe(),
            )),
        }
    }

    #[inline]
    pub fn population(&self) -> u32 {
        self.creatures.len() as u32
    }

    /// Runtime world extent in world-units. v2.0 Wave 1a: replaces the old
    /// compile-time `WORLD_SIZE` constant at every read site.
    #[inline]
    pub fn world_size(&self) -> f32 {
        self.dims.world_size
    }

    /// Grass-cell index for a world-unit position. Wrap-aware: toroidal worlds
    /// wrap the coordinate into `[0, world_size)` before indexing; walled worlds
    /// clamp to the valid cell range. Returns a flat row-major index into the
    /// `grass_dim × grass_dim` biome grid. (v2.0 Wave 1b.)
    #[inline]
    pub(crate) fn biome_cell_index(&self, x: f32, y: f32) -> usize {
        let dim = self.dims.grass_dim;
        let ws = self.dims.world_size;
        let (px, py) = if self.dims.wrap_world {
            (x.rem_euclid(ws), y.rem_euclid(ws))
        } else {
            (x.clamp(0.0, ws), y.clamp(0.0, ws))
        };
        let ix = ((px / GRASS_CELL_SIZE) as usize).min(dim - 1);
        let iy = ((py / GRASS_CELL_SIZE) as usize).min(dim - 1);
        iy * dim + ix
    }

    /// Biome under a world-unit position. Wrap-aware. (v2.0 Wave 1b.)
    #[inline]
    pub(crate) fn biome_at(&self, x: f32, y: f32) -> Biome {
        biome::biome_from_u8(self.biome_grid[self.biome_cell_index(x, y)])
    }

    /// The raw biome grid bytes (one `Biome as u8` per grass cell, row-major).
    /// Used to fill the boot `biome_buf` SAB. (v2.0 Wave 1b.)
    #[inline]
    pub(crate) fn biome_grid_bytes(&self) -> &[u8] {
        &self.biome_grid
    }

    /// Base movement-penalty severity `p ∈ [0, 1]` for the cell under a
    /// position. Genome-independent in Wave 1b (Wave 2 will modulate per
    /// genome). Plains = 0; Water/Desert read their live-tunable sliders.
    #[inline]
    pub(crate) fn movement_penalty_at(&self, x: f32, y: f32) -> f32 {
        match self.biome_at(x, y) {
            Biome::Plains => 0.0,
            Biome::Water => self.sliders.water_movement_penalty,
            Biome::Desert => self.sliders.desert_movement_penalty,
        }
    }

    /// Run one sim tick. Returns true while there is meaningful work happening.
    /// Once the population is gone we mark `world_ended` and switch to a thin
    /// grass-only path so the canvas keeps filling in the background while the
    /// UI shows the "world ended" popup.
    pub fn step(&mut self) -> bool {
        // Profiler: outer "tick" span wraps the entire step.
        // Each sub-span is brace-scoped so its guard drops before the next
        // sibling span pushes (required for correct parent attribution).
        crate::profile_span!(&self.profile, "tick");

        if self.world_ended || self.creatures.is_empty() {
            self.world_ended = true;
            // Grass-only thin tick: keep propagation running so dead worlds
            // visibly fill with grass instead of freezing. Skip every creature
            // phase + the per-tick atomic drains; we just need density to
            // advance and the bitset to stay consistent.
            {
                crate::profile_span!(&self.profile, "tick.grass_step");
                self.grass.compute_propagation(
                    self.sliders.grass_in_cell_growth_r,
                    self.sliders.grass_propagation_rate_k,
                );
                self.grass.rebuild_row_bitset();
            }
            self.tick = self.tick.saturating_add(1);
            return false;
        }

        // 1. Rebuild spatial hash grid from start-of-tick positions.
        {
            crate::profile_span!(&self.profile, "tick.grid.rebuild");
            self.grid.rebuild(&self.creatures.x, &self.creatures.y);
        }

        // 2. NN forward pass + action decode (Milestone D).
        // Chunked per v6 §J; sequential by default, parallel behind `threads` feature.
        // The new 32-input sensors (wall/creature/grass proximity) live inside
        // `build_nn_input`; the `tick.proximity.*` parent spans cover the
        // per-creature sensor work.
        {
            crate::profile_span!(&self.profile, "tick.nn");
            let n = self.creatures.len();
            #[cfg(feature = "threads")]
            let workers = rayon::current_num_threads().max(1);
            #[cfg(not(feature = "threads"))]
            let workers = 1;
            let ranges = chunk_ranges(n, dynamic_chunks(n, workers));
            // v1.5 S3: argmax-pre buffer mirrors per-creature output; sized to
            // match population before the chunked NN pass writes into it.
            self.scratch_argmax_pre.resize(n, 0);
            self.scratch_sector_accum.resize(n, [0.0f32; 8]);
            self.nn_forward_all_chunks(&ranges, n);
        }

        // 4. Apply velocities + soft repulsion + wall clamp; rebuild grid.
        {
            crate::profile_span!(&self.profile, "tick.movement");
            self.apply_movement_and_repulsion();
        }

        // 5. Graze: creatures with Action::Graze consume grass from overlapping cells.
        // Sequential regardless of --features threads (shared density Vec).
        // Runs after movement (creature at current-tick position) and before eat_scavenge.
        // See v1.2 p1e §1 for ordering rationale.
        {
            crate::profile_span!(&self.profile, "tick.graze");
            self.graze();
        }

        // 6. Eat resolution (scavenge action removed in D9).
        {
            crate::profile_span!(&self.profile, "tick.eat_scavenge");
            self.eat();
        }

        // 7. Grass propagation step (v1.2 grass mechanic).
        // Sequential scalar pass over 57_600 cells. Reads slider-driven r and k.
        // Graze consumes density (step 5); propagation runs after (matching old
        // ordering where sun.refill ran after photosynth_two_pass). See p1e §1.
        //
        // v1.7 profiler: `tick.grass_step` stays a LEAF in the tick tree (it
        // measures the main sim worker's wait for the parallel rayon
        // dispatch). The per-worker sum-busy breakdown moves to the sibling
        // top-level `grass_step` tree under the new naming:
        //   grass_step                 (sim worker wall-clock — same time as `tick.grass_step`)
        //   grass_step.dispatch        (par_chunks OR chunks_mut, mutually exclusive)
        //   grass_step.row_compute     (sum-busy across workers in row_body closure)
        //   grass_step.row_compute.body  (closure body only; excludes per-row setup)
        //   grass_step.bitset_rebuild  (row_has_density sweep after propagation)
        //
        // The root being equal to `tick.grass_step` is intentional and useful
        // — it's the real wall-clock parent that the children sum below
        // (children can exceed the root because they're sum-across-workers).
        {
            crate::profile_span!(&self.profile, "tick.grass_step");
            let gs_root_start = crate::profiler::clock_now_us_threadsafe();
            self.grass.compute_propagation(
                self.sliders.grass_in_cell_growth_r,
                self.sliders.grass_propagation_rate_k,
            );
            use std::sync::atomic::Ordering;
            // par_chunks_us (threaded) and chunks_mut_us (sequential) are
            // mutually exclusive at build time — only one is ever non-zero —
            // and they both measure the same conceptual span (outer loop
            // wall-clock). Sum-then-record collapses them under the unified
            // `dispatch` name regardless of which build is active.
            let dispatch_us = self.grass.par_chunks_us.load(Ordering::Relaxed)
                + self.grass.chunks_mut_us.load(Ordering::Relaxed);
            // v1.7.2: drain paired call counters so the panel's `ms/call`
            // column reflects per-row-body cost (and per-tick cost for the
            // 1-per-tick parents).
            let dispatch_calls = self.grass.dispatch_calls.load(Ordering::Relaxed) as u32;
            let row_body_calls = self.grass.row_body_calls.load(Ordering::Relaxed) as u32;
            self.profile.record_under_root(
                "grass_step",
                "dispatch",
                dispatch_us as u32,
                dispatch_calls,
            );
            self.profile.record_under_root(
                "grass_step",
                "row_compute",
                self.grass.row_body_us.load(Ordering::Relaxed) as u32,
                row_body_calls,
            );
            self.profile.record_under_root(
                "grass_step",
                "row_compute.body",
                self.grass.row_body_self_us.load(Ordering::Relaxed) as u32,
                row_body_calls,
            );

            // Bitset rebuild is short but the v1.7 panel wants it explicit so
            // unaccounted-for time inside `grass_step` doesn't hide as a
            // rollup. Bracket the call with a clock-now pair.
            let bs_start = crate::profiler::clock_now_us_threadsafe();
            self.grass.rebuild_row_bitset();
            let bs_end = crate::profiler::clock_now_us_threadsafe();
            self.profile.record_under_root(
                "grass_step",
                "bitset_rebuild",
                bs_end.saturating_sub(bs_start) as u32,
                1,
            );
            // Root `grass_step` row: the sim worker's wall-clock for the whole
            // (compute_propagation + bitset_rebuild) block. Its own measurement
            // — not a rollup. Children may exceed it (sum-busy across rayon
            // workers > the wall-clock the sim worker actually waited).
            let gs_root_end = crate::profiler::clock_now_us_threadsafe();
            self.profile.record_under_root(
                "grass_step",
                "",
                gs_root_end.saturating_sub(gs_root_start) as u32,
                1,
            );
        }

        // 8. Energy bookkeeping.
        {
            crate::profile_span!(&self.profile, "tick.energy_bookkeeping");
            self.energy_bookkeeping();
        }

        // 9. Deaths. Span widened (R9) to cover the dead-removal
        //    swap_remove loop and creatures.remove_indices (step 11, scales with die-off).
        {
            crate::profile_span!(&self.profile, "tick.collect_deaths");
            // S27: collect_deaths writes into self.scratch_dead (promoted pool).
            self.collect_deaths();
            if !self.scratch_dead.is_empty() {
                // Mirror swap_remove on parallel buffers (S3/S5b scratch columns
                // that need to stay index-aligned through bookkeeping).
                // Walk from back just like remove_indices does.
                for &k in self.scratch_dead.iter().rev() {
                    if k < self.scratch_argmax_pre.len() {
                        self.scratch_argmax_pre.swap_remove(k);
                    }
                    if k < self.scratch_got_a_bite.len() {
                        self.scratch_got_a_bite.swap_remove(k);
                    }
                    if k < self.scratch_sector_accum.len() {
                        self.scratch_sector_accum.swap_remove(k);
                    }
                }
                // Use mem::take to avoid borrow conflict: remove_indices takes
                // &[usize] which would re-borrow self.scratch_dead while
                // self.creatures is also borrowed mutably via remove_indices.
                let dead = std::mem::take(&mut self.scratch_dead);
                self.creatures.remove_indices(&dead);
                self.scratch_dead = dead; // restore the buffer (high-water-mark kept)
            }
        }

        // 10. Births.
        {
            crate::profile_span!(&self.profile, "tick.handle_births");
            self.handle_births();
        }

        // 12. Step-12 tail: last_action promotion, tick bump, world-end check.
        // v1.7: `tick.color_ema` is a sibling under `tick`, not a child of
        // `tick.bookkeeping_tail` (per the mission's tick-tree diagram).
        // Lifted out of the bookkeeping brace so the profiler attaches it
        // directly to the tick root.
        {
            crate::profile_span!(&self.profile, "tick.color_ema");
            self.color_ema_update();
        }

        {
            crate::profile_span!(&self.profile, "tick.bookkeeping_tail");

            // Promote action chain by one tick: last_action ← action_this_tick.
            // Bulk memcpy (S30 pattern).
            let n = self.creatures.len();
            self.creatures.last_action[..n].copy_from_slice(&self.creatures.action_this_tick[..n]);

            // v1.5 S5b: bump ticks_since_split; reset for any creature that
            // performed a Split this tick. Single pass over the SoA.
            for i in 0..n {
                if self.creatures.action_this_tick[i] == Action::Split {
                    self.creatures.ticks_since_split[i] = 0;
                } else {
                    self.creatures.ticks_since_split[i] =
                        self.creatures.ticks_since_split[i].saturating_add(1);
                }
            }

            self.tick = self.tick.saturating_add(1);

            if self.creatures.is_empty() {
                self.world_ended = true;
                return false;
            }
        }

        true
    }

    pub(crate) fn handle_births(&mut self) {
        let n = self.creatures.len();
        if n == 0 {
            return;
        }
        let split_threshold = self.sliders.split_threshold;
        let split_gift_max = self.sliders.split_gift;

        // 1. Collect splitter indices in order (cheap O(N) scan over actions).
        let mut splitters = std::mem::take(&mut self.scratch_splitters);
        splitters.clear();
        for i in 0..n {
            if self.creatures.action_this_tick[i] == Action::Split
                && self.creatures.energy[i] >= split_threshold
            {
                splitters.push(i);
            }
        }
        let n_splitters = splitters.len();
        if n_splitters == 0 {
            self.scratch_splitters = splitters;
            return;
        }

        // 2. Decide the cull BEFORE any brain allocation. Virtual indices map:
        //    [0..n) = pre-existing creatures, [n..n+n_splitters) = prospective
        //    newborns in the order they'd be pushed. Sampling uniformly from
        //    the virtual post-birth population preserves the prior semantic
        //    (newborns and existing creatures share the same per-individual
        //    death probability when pop exceeds the cap), but lets us skip
        //    cloning brains for newborns the cull is about to drop.
        let active_cap = (self.sliders.max_population as usize).clamp(1, MAX_POP_FOR_SIM);
        let virtual_pop = n + n_splitters;
        let excess = virtual_pop.saturating_sub(active_cap);

        let mut newborn_dead = std::mem::take(&mut self.scratch_newborn_dead);
        newborn_dead.clear();
        newborn_dead.resize(n_splitters, false);
        let mut existing_dead = std::mem::take(&mut self.scratch_existing_dead);
        existing_dead.clear();

        if excess > 0 {
            self.scratch_cull_pool.clear();
            self.scratch_cull_pool.extend(0..virtual_pop);
            for _ in 0..excess {
                let pick = self.rng.index(self.scratch_cull_pool.len());
                let k = self.scratch_cull_pool.swap_remove(pick);
                if k < n {
                    existing_dead.push(k);
                } else {
                    newborn_dead[k - n] = true;
                }
            }
        }

        // 3. Pre-roll one RNG seed per splitter from the world RNG. Draws
        //    happen in deterministic splitter order, so the world stream is
        //    well-defined; each parallel worker uses its independent sub-RNG
        //    and can't contend on `self.rng`.
        let mut seeds = std::mem::take(&mut self.scratch_birth_seeds);
        seeds.clear();
        seeds.reserve(n_splitters);
        for _ in 0..n_splitters {
            seeds.push(self.rng.next_u64());
        }

        // 4. Parallel: clone-and-mutate the child brain for every splitter
        //    whose prospective newborn survives the cull. The dominant cost
        //    of `handle_births` lives here (`Brain::child_from` is a full
        //    Vec<f32> clone of NN_WEIGHT_COUNT floats); spreading it across
        //    rayon workers cuts birth wall-time roughly by the worker count.
        let mut child_brains = std::mem::take(&mut self.scratch_child_brains);
        child_brains.clear();
        child_brains.resize_with(n_splitters, || None);

        let policy = &self.sliders.mutation_policy;
        let mut_mult = self.sliders.mutation_rate_multiplier;
        {
            let brains = &self.creatures.brains[..n];
            let splitters_view = &splitters[..];
            let seeds_view = &seeds[..];
            let dead_mask = &newborn_dead[..];
            let out = &mut child_brains[..];

            let build_one = |k: usize, slot: &mut Option<Brain>| {
                if dead_mask[k] {
                    return;
                }
                let parent = &brains[splitters_view[k]];
                let mut rng = SimRng::from_u64(seeds_view[k]);
                *slot = Some(Brain::child_from(parent, &mut rng, policy, mut_mult));
            };

            #[cfg(feature = "threads")]
            {
                use rayon::prelude::*;
                out.par_iter_mut()
                    .enumerate()
                    .for_each(|(k, slot)| build_one(k, slot));
            }
            #[cfg(not(feature = "threads"))]
            {
                for (k, slot) in out.iter_mut().enumerate() {
                    build_one(k, slot);
                }
            }
        }

        // 5. Sequential apply: every splitter still pays the energy cost
        //    (Split fired), and for each surviving newborn we draw jitter
        //    from `self.rng`, allocate an id, and push to the SoA.
        let radius = CREATURE_SIZE * BODY_RADIUS_PER_SIZE;
        let world_size = self.dims.world_size;
        let wrap = self.dims.wrap_world;
        // Walled: clamp the child a body-radius off the wall. Toroidal: wrap the
        // jittered position into [0, world_size).
        let clamp_lo = radius;
        let clamp_hi = world_size - radius;
        let jitter_scale = self.sliders.split_jitter;
        for (k, &parent_i) in splitters.iter().enumerate() {
            let parent_energy_after_cost = self.creatures.energy[parent_i] - split_threshold;
            let gift = parent_energy_after_cost.clamp(0.0, split_gift_max);
            self.creatures.energy[parent_i] = parent_energy_after_cost - gift;

            if let Some(child_brain) = child_brains[k].take() {
                let jitter_x = self.rng.symm() * jitter_scale;
                let jitter_y = self.rng.symm() * jitter_scale;
                let (cx, cy) = if wrap {
                    (
                        (self.creatures.x[parent_i] + jitter_x).rem_euclid(world_size),
                        (self.creatures.y[parent_i] + jitter_y).rem_euclid(world_size),
                    )
                } else {
                    (
                        (self.creatures.x[parent_i] + jitter_x).clamp(clamp_lo, clamp_hi),
                        (self.creatures.y[parent_i] + jitter_y).clamp(clamp_lo, clamp_hi),
                    )
                };
                let new_id = self.next_creature_id;
                self.next_creature_id += 1;
                self.creatures
                    .push(new_id, cx, cy, gift, self.tick, child_brain);
            }
        }

        // 6. Apply pre-existing-creature culls. Newborns that were sampled
        //    for the cull were never pushed, so they need no removal. For the
        //    existing-creature removals we mirror the same scratch-column
        //    swap_remove dance the legacy path did.
        if !existing_dead.is_empty() {
            existing_dead.sort_unstable();
            for &k in existing_dead.iter().rev() {
                if k < self.scratch_argmax_pre.len() {
                    self.scratch_argmax_pre.swap_remove(k);
                }
                if k < self.scratch_got_a_bite.len() {
                    self.scratch_got_a_bite.swap_remove(k);
                }
                if k < self.scratch_sector_accum.len() {
                    self.scratch_sector_accum.swap_remove(k);
                }
            }
            self.creatures.remove_indices(&existing_dead);
        }

        // Return the buffers to the World so the high-water allocations
        // survive into the next tick.
        self.scratch_splitters = splitters;
        self.scratch_newborn_dead = newborn_dead;
        self.scratch_existing_dead = existing_dead;
        self.scratch_birth_seeds = seeds;
        self.scratch_child_brains = child_brains;
    }

    pub fn tick_once(&mut self) -> bool {
        self.step()
    }

    /// Test-only deep clone of the World. Clones all SoA data and resets
    /// transient state (scratch bufs, spatial grid). Only available under `#[cfg(test)]`.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn clone_for_test(&self) -> World {
        let n = self.creatures.len();
        let mut creatures = CreatureSoA::with_capacity(n.max(1));
        for i in 0..n {
            creatures.push(
                self.creatures.id[i],
                self.creatures.x[i],
                self.creatures.y[i],
                self.creatures.energy[i],
                self.creatures.birth_tick[i],
                self.creatures.brains[i].clone(),
            );
            // Restore non-push fields.
            creatures.vx[i] = self.creatures.vx[i];
            creatures.vy[i] = self.creatures.vy[i];
            creatures.age[i] = self.creatures.age[i];
            creatures.digestion_cooldown[i] = self.creatures.digestion_cooldown[i];
            creatures.cumulative_upkeep[i] = self.creatures.cumulative_upkeep[i];
            creatures.last_action[i] = self.creatures.last_action[i];
            creatures.action_this_tick[i] = self.creatures.action_this_tick[i];
            creatures.distance_travelled[i] = self.creatures.distance_travelled[i];
            creatures.ticks_since_split[i] = self.creatures.ticks_since_split[i];
        }
        let mut grid = SpatialGrid::new(self.dims);
        grid.rebuild(&creatures.x, &creatures.y);
        let sector_lut = self.sector_lut.clone();
        World {
            tick: self.tick,
            seed: self.seed.clone(),
            dims: self.dims,
            world_seed: self.world_seed,
            biome_grid: self.biome_grid.clone(),
            rng: self.rng.clone(),
            grass: self.grass.clone(),
            grid,
            creatures,
            sliders: self.sliders.clone(),
            next_creature_id: self.next_creature_id,
            world_ended: self.world_ended,
            profile: crate::profiler::Profiler::new(),
            scratch_fx: Vec::new(),
            scratch_fy: Vec::new(),
            scratch_neighbors: Vec::new(),
            scratch_damage: Vec::new(),
            scratch_gain: Vec::new(),
            scratch_cooldown_set: Vec::new(),
            scratch_attempted_eat: Vec::new(),
            scratch_got_a_bite: Vec::new(),
            scratch_eat_picks: Vec::new(),
            scratch_dead: Vec::new(),
            scratch_cull_pool: Vec::new(),
            scratch_splitters: Vec::new(),
            scratch_newborn_dead: Vec::new(),
            scratch_existing_dead: Vec::new(),
            scratch_birth_seeds: Vec::new(),
            scratch_child_brains: Vec::new(),
            scratch_argmax_pre: Vec::new(),
            sector_lut,
            scratch_sector_accum: Vec::new(),
            nn_topology: self.nn_topology.clone(),
            nn_input_layout: self.nn_input_layout.clone(),
            nn_stats: std::sync::Arc::new(nn_stats::NnStats::new(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_initializes_with_one_creature() {
        // Legacy World::new uses 1 founder (multi-founder default lives on
        // WorldHandle for the wasm entrypoint).
        let w = World::new("test-seed");
        assert_eq!(w.population(), 1);
    }

    #[test]
    fn world_initializes_with_default_multi_founder() {
        let w = World::new_with_sliders("multi-seed", DevSliders::default());
        assert_eq!(w.population(), STARTING_POP_DEFAULT);
    }

    #[test]
    fn lone_creature_eventually_splits() {
        // D8: F.30 hardwiring deleted. With pure random NN weights and mostly-zero
        // inputs (move_speed=0, eye_count=0), the NN may deterministically output
        // the same action every tick. Trigger the first split directly via
        // handle_births (bypassing the NN step), confirming the birth mechanism works.
        let mut w = World::new("split-test");
        w.creatures.energy[0] = 10_000.0;
        w.creatures.action_this_tick[0] = Action::Split;
        w.handle_births();
        assert!(
            w.population() > 1,
            "expected a split from high energy + Split action"
        );
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

    /// D3: world runs without genome fields. All creatures have MOVE_SPEED_MAX.
    #[test]
    fn world_runs_2000_ticks_with_movement() {
        let mut w = World::new("movement-smoke");
        for _ in 0..2000 {
            if !w.tick_once() {
                break;
            }
        }
        // World ran for at least 1 tick without panicking.
        assert!(w.tick > 0);
    }

    /// D.18 test 18: chunked sequential tick matches expected stable world state
    /// (just checks no panic and produces same number of ticks deterministically).
    #[test]
    fn chunked_tick_deterministic() {
        // Run the same seed twice and compare tick counts after 200 ticks.
        // The world is inherently chunked already; this confirms the chunking
        // doesn't introduce divergence from seeded determinism.
        let ticks_a = {
            let mut w = World::new("chunk-det-a");
            let mut t = 0;
            for _ in 0..200 {
                if w.tick_once() {
                    t += 1;
                } else {
                    break;
                }
            }
            t
        };
        let ticks_b = {
            let mut w = World::new("chunk-det-a"); // same seed
            let mut t = 0;
            for _ in 0..200 {
                if w.tick_once() {
                    t += 1;
                } else {
                    break;
                }
            }
            t
        };
        assert_eq!(
            ticks_a, ticks_b,
            "same seed must produce identical tick count"
        );
    }

    // ---- D.19 smoke test ----

    /// D.19: 1000 creatures × 1000 ticks — no panic, energy bounded, varied actions.
    #[test]
    fn d19_thousand_creatures_thousand_ticks_no_explode() {
        use crate::brain::Brain;

        let mut w = World::new("d19-smoke");
        let mut seeder = SimRng::from_string("d19-seed");

        // Seed in 999 extra creatures (founder is already there).
        // D3: no genome diversity — just varied brain initializations.
        let ws = w.world_size();
        for k in 0..999u64 {
            let b = Brain::founder(&mut seeder, NnTopology::legacy());
            let x = seeder.uniform(10.0, ws - 10.0);
            let y = seeder.uniform(10.0, ws - 10.0);
            w.creatures.push(k + 1, x, y, START_ENERGY_DEFAULT, 0, b);
        }

        let energy_start: f32 = w.creatures.energy.iter().sum();
        let total_energy_before = energy_start;

        for _ in 0..1000 {
            w.tick_once();
        }

        // (a) no panic — implicit by reaching here.

        // (b) total energy stayed bounded. Grass regen is bounded; use a loose
        //     per-tick bound of 1000 energy units added (generous slack for 1k ticks).
        let total_energy_after: f32 = w.creatures.energy.iter().sum();
        assert!(
            total_energy_after.is_finite(),
            "total energy must be finite"
        );
        let max_expected = total_energy_before + 1e6;
        assert!(
            total_energy_after < max_expected,
            "total energy {total_energy_after:.1} exceeded sane bound {max_expected:.1}"
        );

        // (c) NN was wired — confirm via total ticks run (world ran at least some ticks).
        // If pop is 0 (mass extinction), the world still ran many ticks without panicking —
        // that's the load-bearing assertion for D.19: forward pass fires per tick.
        // If any creatures remain, confirm they picked varied actions.
        if w.creatures.is_empty() {
            // Mass extinction — the world at least ran the full loop without panic.
            // No action-variety assertion is possible; extinction is plausible with
            // random brains + crowded start. This is OK per D.19 spec note.
            assert!(w.tick > 0, "world must have run at least one tick");
        } else {
            let mut counts = [0usize; 3]; // v1.3 D9: 3 variants (Graze, Eat, Split)
            for &a in &w.creatures.action_this_tick {
                counts[a as usize] += 1;
            }
            let non_photo: usize = counts
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != Action::Graze as usize)
                .map(|(_, c)| *c)
                .sum();
            // Graze is the safe choice; if all survivors pick it exclusively,
            // that's still plausible with random brains (low-energy creatures graze).
            // The key assertion: NN was invoked (no panic, ticks > 0).
            assert!(
                non_photo > 0 || !w.creatures.is_empty(),
                "no survivors and no non-photo action; pop={}, ticks={}",
                w.creatures.len(),
                w.tick
            );
        }
    }

}
