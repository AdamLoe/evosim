//! v2.1 P3 — HONEST equilibrium measurement harness (red-team instrument).
//!
//! This is a measurement / reporting harness, NOT a gate. It is `#[ignore]`d so
//! it never runs in `cargo test --lib`; invoke explicitly with
//! `cargo test --lib --release -- --ignored --nocapture p3_measure`.
//!
//! Unlike the shipped `equilibrium_tests.rs` proxy (avg `cumulative_upkeep` of
//! LIVING creatures — tautological, since survivors are simply older), this
//! harness records **age-at-death** for every creature that dies, via an
//! id-diff across each `step()`:
//!
//!   * Before each step, snapshot `id -> age` for all living creatures.
//!   * After the step, any id present before but absent after died THIS tick.
//!     Its age-at-death is its pre-step age + 1 (energy_bookkeeping increments
//!     age before collect_deaths, both inside one step()).
//!
//! Creature ids are monotonic (`next_creature_id`) and never reused, so a
//! disappeared id is a genuine death, never a swap_remove artifact.
//!
//! We also track energy-gathered-per-lifespan WITHOUT touching the shipped SoA:
//! before the graze contributes, energy only rises via grazing/attack and falls
//! via upkeep. We approximate "energy gathered" by accumulating positive energy
//! deltas per creature across its life — measured purely in the harness via the
//! id->energy snapshot. (This is an approximation: attack steals energy too. We
//! report it as a secondary signal and lean on age-at-death as primary.)

use super::*;
use crate::constants::*;
use std::collections::HashMap;

const K_SEEDS: usize = 4;
const SEEDS: [&str; K_SEEDS] = ["eq-seed-1", "eq-seed-2", "eq-seed-3", "eq-seed-4"];

/// Build the same small world the shipped harness uses, so numbers are
/// comparable. Mirrors `equilibrium_tests::equilibrium_world`.
fn measure_world(seed: &str) -> World {
    World::new_with_sliders(
        seed,
        DevSliders {
            world_size: 1200.0,
            wrap_world: true,
            founder_count: 48,
            max_population: 10_000,
            energy_max: 100.0,
            upkeep_multiplier: 1.0,
            grass_clump_count: 8,
            grass_clump_size: 5,
            crowding_strength: CROWDING_STRENGTH_DEFAULT,
            crowding_radius: CROWDING_RADIUS_DEFAULT,
            starvation_threshold: STARVATION_THRESHOLD_DEFAULT,
            starvation_drain_rate: STARVATION_DRAIN_RATE_DEFAULT,
            grass_capacity_scale: GRASS_CAPACITY_SCALE_DEFAULT,
            grass_regrowth_rate: GRASS_REGROWTH_RATE_DEFAULT,
            ..Default::default()
        },
    )
}

struct DeathRecord {
    death_tick: u32,
    age_at_death: u32,
    energy_gathered: f64,
}

struct SeedResult {
    seed: String,
    pop_samples: Vec<usize>, // sampled every 100 ticks
    final_pop: usize,
    extinct_tick: Option<u32>,
    cap_hits: u32,
    deaths: Vec<DeathRecord>,
}

fn run_seed(seed: &str, t_ticks: u32) -> SeedResult {
    let mut world = measure_world(seed);
    let cap = world.sliders.max_population as usize;

    // id -> (age, cumulative energy gathered so far) for currently-living.
    let mut alive: HashMap<u64, (u32, f64)> = HashMap::new();
    // id -> last-seen energy, for delta accounting.
    let mut last_energy: HashMap<u64, f32> = HashMap::new();

    let snap = |world: &World,
                alive: &mut HashMap<u64, (u32, f64)>,
                last_energy: &mut HashMap<u64, f32>| {
        let n = world.creatures.len();
        for i in 0..n {
            let id = world.creatures.id[i];
            let age = world.creatures.age[i];
            let e = world.creatures.energy[i];
            alive.entry(id).or_insert((age, 0.0)).0 = age;
            last_energy.entry(id).or_insert(e);
        }
    };
    snap(&world, &mut alive, &mut last_energy);

    let mut result = SeedResult {
        seed: seed.to_string(),
        pop_samples: Vec::new(),
        final_pop: 0,
        extinct_tick: None,
        cap_hits: 0,
        deaths: Vec::new(),
    };

    for tick in 0..t_ticks {
        // Pre-step living id set.
        let pre_ids: std::collections::HashSet<u64> = (0..world.creatures.len())
            .map(|i| world.creatures.id[i])
            .collect();

        world.step();
        let pop = world.creatures.len();

        if pop >= cap {
            result.cap_hits += 1;
        }

        // Accumulate energy-gathered for survivors (positive deltas) and refresh ages.
        let n = world.creatures.len();
        let mut post_ids = std::collections::HashSet::with_capacity(n);
        for i in 0..n {
            let id = world.creatures.id[i];
            post_ids.insert(id);
            let e = world.creatures.energy[i];
            let entry = alive.entry(id).or_insert((world.creatures.age[i], 0.0));
            entry.0 = world.creatures.age[i];
            let prev = last_energy.entry(id).or_insert(e);
            let delta = e - *prev;
            if delta > 0.0 {
                entry.1 += delta as f64;
            }
            *prev = e;
        }

        // Deaths: ids present pre-step but not post-step.
        for id in pre_ids.difference(&post_ids) {
            if let Some((age, gathered)) = alive.remove(id) {
                // age was last set to its value the previous tick; energy_bookkeeping
                // increments age inside this step before death, so add 1.
                result.deaths.push(DeathRecord {
                    death_tick: tick,
                    age_at_death: age.saturating_add(1),
                    energy_gathered: gathered,
                });
            }
            last_energy.remove(id);
        }

        if tick % 100 == 0 {
            result.pop_samples.push(pop);
        }

        if pop == 0 {
            result.extinct_tick = Some(tick);
            break;
        }
    }

    result.final_pop = world.creatures.len();
    result
}

fn stats(v: &[f64]) -> (f64, f64, f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64;
    let std = var.sqrt();
    let min = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mean, std, min, max)
}

#[test]
#[ignore = "measurement harness; run explicitly with --ignored --nocapture"]
fn p3_measure_equilibrium() {
    // Push T high; report wall time. 6000 ticks × 4 seeds.
    let t_ticks: u32 = std::env::var("P3_TICKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6_000);

    let wall = std::time::Instant::now();
    eprintln!("\n=== P3 HONEST EQUILIBRIUM MEASUREMENT ===");
    eprintln!("T={t_ticks} ticks × K={K_SEEDS} seeds (small 1200u world)\n");

    for &seed in &SEEDS {
        let r = run_seed(seed, t_ticks);

        // Steady-state window = last 40% of pop samples.
        let n = r.pop_samples.len();
        let ss_start = (n as f64 * 0.6) as usize;
        let ss: Vec<f64> = r.pop_samples[ss_start..]
            .iter()
            .map(|&p| p as f64)
            .collect();
        let (ss_mean, ss_std, ss_min, ss_max) = stats(&ss);
        let cov = if ss_mean > 0.0 { ss_std / ss_mean } else { 0.0 };

        // Also report last-25% window CoV (deeper past the transient).
        let ss25_start = (n as f64 * 0.75) as usize;
        let ss25: Vec<f64> = r.pop_samples[ss25_start..]
            .iter()
            .map(|&p| p as f64)
            .collect();
        let (s25_mean, s25_std, _, _) = stats(&ss25);
        let cov25 = if s25_mean > 0.0 {
            s25_std / s25_mean
        } else {
            0.0
        };
        eprintln!("  [last25%] mean={s25_mean:.1} CoV={cov25:.3}");

        // Whole-run pop range.
        let whole: Vec<f64> = r.pop_samples.iter().map(|&p| p as f64).collect();
        let (_, _, w_min, w_max) = stats(&whole);

        // Death-cohort foraging: sort deaths by death_tick, compare early 20% vs late 20%.
        let mut deaths = r.deaths;
        deaths.sort_by_key(|d| d.death_tick);
        let nd = deaths.len();
        let k = (nd as f64 * 0.20).max(1.0) as usize;
        let early: Vec<&DeathRecord> = deaths.iter().take(k).collect();
        let late: Vec<&DeathRecord> = deaths.iter().rev().take(k).collect();

        let early_age: Vec<f64> = early.iter().map(|d| d.age_at_death as f64).collect();
        let late_age: Vec<f64> = late.iter().map(|d| d.age_at_death as f64).collect();
        let (ea_mean, _, _, _) = stats(&early_age);
        let (la_mean, _, _, _) = stats(&late_age);

        // energy gathered per tick lived.
        let early_egpt: Vec<f64> = early
            .iter()
            .map(|d| d.energy_gathered / (d.age_at_death.max(1) as f64))
            .collect();
        let late_egpt: Vec<f64> = late
            .iter()
            .map(|d| d.energy_gathered / (d.age_at_death.max(1) as f64))
            .collect();
        let (ee_mean, _, _, _) = stats(&early_egpt);
        let (le_mean, _, _, _) = stats(&late_egpt);

        let age_ratio = if ea_mean > 0.0 {
            la_mean / ea_mean
        } else {
            0.0
        };
        let egpt_ratio = if ee_mean > 0.0 {
            le_mean / ee_mean
        } else {
            0.0
        };

        // Steady-state-only cohort comparison: restrict to deaths in the last
        // 60% of ticks (post-transient), then compare early-20% vs late-20% of
        // THAT window. Removes the food-rich-startup confound from the
        // energy/tick metric.
        let ss_tick = (t_ticks as f64 * 0.40) as u32;
        let ss_deaths: Vec<&DeathRecord> =
            deaths.iter().filter(|d| d.death_tick >= ss_tick).collect();
        let nss = ss_deaths.len();
        let kss = (nss as f64 * 0.20).max(1.0) as usize;
        let ss_early: Vec<&&DeathRecord> = ss_deaths.iter().take(kss).collect();
        let ss_late: Vec<&&DeathRecord> = ss_deaths.iter().rev().take(kss).collect();
        let sse_age: Vec<f64> = ss_early.iter().map(|d| d.age_at_death as f64).collect();
        let ssl_age: Vec<f64> = ss_late.iter().map(|d| d.age_at_death as f64).collect();
        let (sse_a, _, _, _) = stats(&sse_age);
        let (ssl_a, _, _, _) = stats(&ssl_age);
        let sse_eg: Vec<f64> = ss_early
            .iter()
            .map(|d| d.energy_gathered / (d.age_at_death.max(1) as f64))
            .collect();
        let ssl_eg: Vec<f64> = ss_late
            .iter()
            .map(|d| d.energy_gathered / (d.age_at_death.max(1) as f64))
            .collect();
        let (sse_e, _, _, _) = stats(&sse_eg);
        let (ssl_e, _, _, _) = stats(&ssl_eg);

        eprintln!("seed={}", r.seed);
        eprintln!(
            "  whole-run pop:   min={w_min:.0} max={w_max:.0} final={}",
            r.final_pop
        );
        eprintln!(
            "  steady-state(last40%): mean={ss_mean:.1} std={ss_std:.1} \
             min={ss_min:.0} max={ss_max:.0} CoV={cov:.3}"
        );
        eprintln!(
            "  extinct={:?} cap_hits={} deaths_recorded={nd}",
            r.extinct_tick, r.cap_hits
        );
        eprintln!(
            "  age-at-death cohorts: early(n={})={ea_mean:.1}  late(n={})={la_mean:.1}  \
             ratio={age_ratio:.3}",
            early.len(),
            late.len()
        );
        eprintln!(
            "  energy/tick-lived:    early={ee_mean:.4}  late={le_mean:.4}  ratio={egpt_ratio:.3}"
        );
        let ss_age_ratio = if sse_a > 0.0 { ssl_a / sse_a } else { 0.0 };
        let ss_eg_ratio = if sse_e > 0.0 { ssl_e / sse_e } else { 0.0 };
        eprintln!(
            "  [STEADY-STATE ONLY, deaths in last60%] age: early={sse_a:.1} late={ssl_a:.1} \
             ratio={ss_age_ratio:.3}  energy/tick: early={sse_e:.4} late={ssl_e:.4} \
             ratio={ss_eg_ratio:.3}"
        );
        eprintln!();
    }

    eprintln!("=== wall time: {:.1}s ===\n", wall.elapsed().as_secs_f64());
}
