//! v2.1 P3 — population-equilibrium exit-gate harness.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard.
//!
//! # What this file asserts
//!
//! Runs the sim headless across K seeds × T ticks and asserts the P3 exit gate
//! with HONEST metrics (red-team rework 2026-06-11, replacing the original
//! tautological "avg cumulative_upkeep of LIVING creatures" proxy):
//!
//!   1. **Steady-state band gate**: in the last-quarter window the population
//!      holds a tight band — coefficient of variation (std/mean) below
//!      `MAX_STEADY_COV` and mean within `[STEADY_MEAN_LO, STEADY_MEAN_HI]`.
//!      This proves a *held* steady state, not merely "never 0 / never capped"
//!      over the whole run (which a boom-bust trajectory also passes).
//!   2. **No extinction**: population never reaches 0.
//!   3. **Backstop-cull gate**: the random `MAX_POP_FOR_SIM` cull in
//!      `handle_births` must NOT fire — population must hold BELOW
//!      `max_population` every tick (food-limited density-dependence is the
//!      binding ceiling, not the random cull).
//!   4. **Foraging-skill trend via DEATH COHORTS**: we record `age_at_death`
//!      for every creature that dies, via an id-diff across each `step()`
//!      (no SoA/wire-layout change). Comparing the EARLY death cohort (first
//!      20% of completed lives) to the LATE cohort (last 20%), mean
//!      age-at-death must RISE by at least `MIN_AGE_TREND` — i.e. later-
//!      generation creatures, having inherited better-foraging brains, survive
//!      longer. This is a genuine selection signal (completed lives, cohort-
//!      compared), unlike "age of currently-living creatures" which rises by
//!      construction (survivors are simply older than fresh founders).
//!
//! ## Why age-at-death, not energy-gathered-per-tick
//!
//! The red-team harness (`equilibrium_measure.rs`) also measured
//! energy-gathered-per-tick-lived. In steady state that metric is FLAT
//! (ratio ≈ 0.90–0.96 early→late) because total food is pinned by the
//! density-dependent equilibrium — every creature is competing for the same
//! bounded food budget, so raw intake-per-tick cannot rise. Skill shows up as
//! *living longer on that same budget*, i.e. age-at-death. We therefore use
//! age-at-death as the primary skill signal. Measured early→late age ratio is
//! ~2.0–2.2 over a 6000-tick run and ~4.2–4.4 over a 2000-tick run (the early
//! cohort dies during the food-rich, low-competition startup transient); the
//! `MIN_AGE_TREND = 1.30` threshold leaves a large honest margin.
//!
//! # Run time
//!
//! T = 3000 ticks × K = 4 seeds on the small 1200u world finishes in ~100s in
//! the dev profile (optimized + debuginfo) and <50s in `--release`.

use super::*;
use crate::constants::*;
use std::collections::HashSet;

// ── Harness parameters ─────────────────────────────────────────────────────

/// Number of distinct seeds to run.
const K_SEEDS: usize = 4;
/// Number of ticks per run. 3000 is enough for the population to clear the
/// startup transient and hold steady for many generation waves (turn-over ≈
/// every 120–150 ticks at equilibrium). NOTE: 2000 was too short — under the
/// `threads` feature the parallel-birth RNG path gives some seeds a *slower*
/// founder-bottleneck transient (e.g. eq-seed-2 is still climbing at tick 2000
/// threaded, mean≈38; by tick 3000 it has reached the same ~1700 band as every
/// other seed). 3000 makes the steady-state window robust across BOTH the
/// single-thread and `--features threads` RNG paths.
const T_TICKS: usize = 3_000;
/// Steady-state window = the final `STEADY_FRAC` of the run (post-transient).
const STEADY_FRAC: f64 = 0.25;
/// Max coefficient of variation (std/mean) of population in the steady-state
/// window. Measured ≤ 0.025 across all four seeds on both RNG paths at T=3000;
/// 0.12 leaves a generous margin while still rejecting a boom-bust trajectory
/// (a still-collapsing seed has steady-window CoV ≫ 0.1 — e.g. ~0.96 for the
/// threaded eq-seed-2 at the too-short T=2000).
const MAX_STEADY_COV: f64 = 0.12;
/// Steady-state mean population must land in this band — above extinction and
/// far below the `max_population` cap (10 000 in this harness), confirming food
/// limits the population, not the cull. The current tuned defaults produce a
/// deliberately sparse steady state in this small harness.
const STEADY_MEAN_LO: f64 = 10.0;
const STEADY_MEAN_HI: f64 = 100.0;
/// Fraction of completed lives in the early / late death cohort.
const COHORT_FRAC: f64 = 0.20;
/// Minimum late/early ratio of mean age-at-death (foraging-skill trend).
/// Measured ~2.0–4.4 depending on T; 1.30 is a deliberately loose floor that
/// still fails flat-or-declining learning.
const MIN_AGE_TREND: f64 = 1.30;

/// One seed in the `K_SEEDS` array.
const SEEDS: [&str; K_SEEDS] = ["eq-seed-1", "eq-seed-2", "eq-seed-3", "eq-seed-4"];

// ── World construction helper ─────────────────────────────────────────────

/// Build a world tuned for the equilibrium harness.
///
/// Uses a *small* world (1200u wrapped) so debug-mode runs finish quickly.
/// The equilibrium mechanism is the same; only the absolute population numbers
/// differ from a default 9600u run.
fn equilibrium_world(seed: &str) -> World {
    World::new_with_sliders(
        seed,
        DevSliders {
            // Small world for speed in debug builds.
            world_size: 1200.0,
            wrap_world: true,
            founder_count: 48,
            // Lower the user-visible cap well below MAX_POP_FOR_SIM so the
            // backstop-cull test is meaningful. Food-limited density-dependence
            // must do all the work below this cap.
            max_population: 10_000,
            // Energy economy — tighter than default so starvation actually kills.
            energy_max: 100.0,
            upkeep_multiplier: 1.0,
            // Grass tuning — small clump count limits total food on the 1200u world.
            grass_clump_count: 8,
            grass_clump_size: 5,
            // v2.1 P3 equilibrium knobs (canonical defaults from constants.rs).
            crowding_strength: CROWDING_STRENGTH_DEFAULT,
            crowding_radius: CROWDING_RADIUS_DEFAULT,
            starvation_threshold: STARVATION_THRESHOLD_DEFAULT,
            starvation_drain_rate: STARVATION_DRAIN_RATE_DEFAULT,
            ..Default::default()
        },
    )
}

// ── Per-seed run ───────────────────────────────────────────────────────────

/// Population samples (every tick) + the ordered list of age-at-death values
/// for every creature that died, in death order.
struct SeedRun {
    /// Population after each tick.
    pop_per_tick: Vec<usize>,
    /// `(death_tick, age_at_death)` for every completed life, in tick order.
    deaths: Vec<u32>,
    /// True if the population ever reached the `max_population` cap (the random
    /// backstop cull would be the binding limiter).
    cull_fired: bool,
    /// True if the population ever hit 0.
    extinct: bool,
}

/// Run one seed for `T_TICKS`, recording population and age-at-death via an
/// id-diff hook. Creature ids are monotonic and never reused, so an id present
/// before `step()` but absent after died this tick; its age-at-death is its
/// pre-step age + 1 (`energy_bookkeeping` increments age before
/// `collect_deaths`, both inside one `step()`).
fn run_seed(seed: &str) -> SeedRun {
    let mut world = equilibrium_world(seed);
    let cap = world.sliders.max_population as usize;

    let mut run = SeedRun {
        pop_per_tick: Vec::with_capacity(T_TICKS),
        deaths: Vec::new(),
        cull_fired: false,
        extinct: false,
    };

    // Pre-step (id -> age) for the currently-living population.
    let mut prev_age: std::collections::HashMap<u64, u32> = (0..world.creatures.len())
        .map(|i| (world.creatures.id[i], world.creatures.age[i]))
        .collect();

    for tick in 0..T_TICKS {
        let pre_ids: HashSet<u64> = prev_age.keys().copied().collect();

        world.step();
        let pop = world.creatures.len();
        run.pop_per_tick.push(pop);

        if pop >= cap {
            run.cull_fired = true;
        }

        // Build post-step id set and refresh the age map.
        let mut post_ids = HashSet::with_capacity(pop);
        let mut next_age = std::collections::HashMap::with_capacity(pop);
        for i in 0..pop {
            let id = world.creatures.id[i];
            post_ids.insert(id);
            next_age.insert(id, world.creatures.age[i]);
        }

        // Deaths = ids present pre-step but not post-step.
        for id in pre_ids.difference(&post_ids) {
            if let Some(age) = prev_age.get(id) {
                run.deaths.push(age.saturating_add(1));
            }
        }
        prev_age = next_age;

        if pop == 0 {
            run.extinct = true;
            let _ = tick;
            break;
        }
    }

    run
}

// ── Stats helpers ──────────────────────────────────────────────────────────

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn std_dev(v: &[f64], m: f64) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
    }
}

// ── Exit-gate test ─────────────────────────────────────────────────────────

/// P3 exit gate: across K seeds × T ticks the population holds a tight
/// steady-state band below the cap, never goes extinct, the backstop cull never
/// fires, and the death-cohort age-at-death trends upward (foraging improves).
///
/// This is the AUTHORITATIVE assertion that ships with P3. Do not weaken the
/// criteria without an explicit decision from the lead. (Honest-metric rework
/// 2026-06-11 — see module docs and docs/plans/v2.1-p3-equilibrium.md report.)
#[test]
fn p3_equilibrium_band_and_foraging_trend() {
    let mut failures: Vec<String> = Vec::new();

    for &seed in &SEEDS {
        let run = run_seed(seed);

        if run.extinct {
            failures.push(format!("seed={seed}: EXTINCT during run"));
            continue;
        }

        // ── Steady-state band ──────────────────────────────────────────────
        let n = run.pop_per_tick.len();
        let ss_start = (n as f64 * (1.0 - STEADY_FRAC)) as usize;
        let ss: Vec<f64> = run.pop_per_tick[ss_start..]
            .iter()
            .map(|&p| p as f64)
            .collect();
        let ss_mean = mean(&ss);
        let ss_std = std_dev(&ss, ss_mean);
        let cov = if ss_mean > 0.0 {
            ss_std / ss_mean
        } else {
            f64::INFINITY
        };
        let ss_min = ss.iter().cloned().fold(f64::INFINITY, f64::min);
        let ss_max = ss.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        if cov > MAX_STEADY_COV {
            failures.push(format!(
                "seed={seed}: steady-state CoV {cov:.3} > {MAX_STEADY_COV} (boom-bust, not a held band)"
            ));
        }
        if !(STEADY_MEAN_LO..=STEADY_MEAN_HI).contains(&ss_mean) {
            failures.push(format!(
                "seed={seed}: steady-state mean pop {ss_mean:.0} outside \
                 [{STEADY_MEAN_LO},{STEADY_MEAN_HI}]"
            ));
        }

        // ── Backstop-cull gate ─────────────────────────────────────────────
        if run.cull_fired {
            failures.push(format!(
                "seed={seed}: random backstop cull fired (pop reached max_population)"
            ));
        }

        // ── Foraging-skill trend via death cohorts ─────────────────────────
        // run.deaths is already in tick order (deaths recorded as they happen).
        let nd = run.deaths.len();
        let k = ((nd as f64) * COHORT_FRAC).max(1.0) as usize;
        let early: Vec<f64> = run.deaths[..k].iter().map(|&a| a as f64).collect();
        let late: Vec<f64> = run.deaths[nd - k..].iter().map(|&a| a as f64).collect();
        let early_mean = mean(&early);
        let late_mean = mean(&late);
        let age_ratio = if early_mean > 0.0 {
            late_mean / early_mean
        } else {
            0.0
        };

        if age_ratio < MIN_AGE_TREND {
            failures.push(format!(
                "seed={seed}: age-at-death did NOT improve \
                 (early={early_mean:.1}, late={late_mean:.1}, ratio={age_ratio:.3} < {MIN_AGE_TREND})"
            ));
        }

        // Report per-seed stats (visible in test output).
        eprintln!(
            "[equilibrium] seed={seed} steady_pop[mean={ss_mean:.0} CoV={cov:.3} \
             min={ss_min:.0} max={ss_max:.0}] age_at_death[early={early_mean:.1} \
             late={late_mean:.1} ratio={age_ratio:.2}] deaths={nd} cull_fired={}",
            run.cull_fired
        );
    }

    assert!(
        failures.is_empty(),
        "P3 equilibrium gate FAILED:\n{}",
        failures.join("\n")
    );
}
