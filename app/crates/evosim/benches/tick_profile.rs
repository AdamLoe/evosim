//! Per-phase tick cost breakdown at the default world scale, native.
//!
//! Reproduces the in-app profiler's `tick.*` numbers without a browser so we
//! can measure before/after a perf change. Builds a single-pool world with a
//! fixed founder population, warms it up so creatures spread and the grass
//! field reaches a steady state, clears the profiler, runs a measured window,
//! then prints µs/tick per phase (total_us / call_count).
//!
//! Run: `cargo bench --bench tick_profile` (release timing; harness = false).
//! Optional env: `TICK_FOUNDERS` (default 1500), `TICK_WARMUP` (default 400),
//! `TICK_MEASURE` (default 300).

use evosim::WorldHandle;

const WORLD_SEED: u32 = 12345;

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn make_handle(founders: u32, world_size: f32, grass_cell: f32, full_grass: bool) -> WorldHandle {
    WorldHandle::new_with_founder_count(
        "bench-tick-profile",
        8000,  // initial_grass_seed_count
        200.0, // energy_max
        founders,
        full_grass, // full_grass_on_init (fills the map → dense grass_step/pyramid regime)
        "",         // nn_topology_json (legacy)
        world_size,
        true, // wrap_world
        WORLD_SEED,
        false, // species_mode
        0.0,   // crossover_mode
        1,     // starting_species_count
        founders,
        0.0, // starting_species_member_variance
        grass_cell,
        40,  // grass_clump_count (default seeded clumps)
        8,   // grass_clump_size
        1.0, // init_graze_boost (neutral)
        1.0, // init_split_boost (neutral)
    )
    .expect("WorldHandle construction failed")
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Recursively collect (name, total_us, call_count) for every node whose name
/// starts with `tick`.
fn collect_tick_nodes(node: &serde_json::Value, out: &mut Vec<(String, u64, u64)>) {
    if let Some(name) = node.get("name").and_then(|v| v.as_str()) {
        if name.starts_with("tick") {
            let total_us = node.get("total_us").and_then(|v| v.as_u64()).unwrap_or(0);
            // total_call_count is the honest per-invocation count; for tick.*
            // spans (RAII, one per tick) it equals the number of measured ticks.
            let calls = node
                .get("total_call_count")
                .and_then(|v| v.as_u64())
                .or_else(|| node.get("call_count").and_then(|v| v.as_u64()))
                .unwrap_or(1)
                .max(1);
            out.push((name.to_string(), total_us, calls));
        }
    }
    if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
        for c in children {
            collect_tick_nodes(c, out);
        }
    }
}

fn main() {
    let founders = env_u32("TICK_FOUNDERS", 1500);
    let warmup = env_u32("TICK_WARMUP", 400);
    let measure = env_u32("TICK_MEASURE", 300);

    let world_size = env_f32("TICK_WORLD", 9600.0);
    let grass_cell = env_f32("TICK_GRASS", 5.0);
    let full_grass = env_u32("TICK_FULLGRASS", 0) != 0;
    let max_pop = env_u32("TICK_MAXPOP", 1500);
    let mut handle = make_handle(founders, world_size, grass_cell, full_grass);
    // Pin the population cap to the user's regime (~1500) so movement's
    // population-dependent repulsion scan matches the real profile; the grid
    // rebuild is population-independent either way.
    let _ = handle.set_slider("max_population", max_pop as f32);
    handle.profile_enable(true);

    for _ in 0..warmup {
        handle.step();
    }
    let pop_after_warmup = handle.population();

    handle.profile_clear();
    let t0 = std::time::Instant::now();
    for _ in 0..measure {
        handle.step();
    }
    let wall = t0.elapsed();
    let pop_after_measure = handle.population();

    let report: serde_json::Value =
        serde_json::from_str(&handle.profile_report_json()).expect("profile report parse");

    // The report is { now_ms, window_ms, enabled, tree: [roots...] }.
    let mut nodes = Vec::new();
    if let Some(tree) = report.get("tree").and_then(|v| v.as_array()) {
        for root in tree {
            collect_tick_nodes(root, &mut nodes);
        }
    }

    // Sort by cost desc, but keep the `tick` root first.
    nodes.sort_by_key(|n| std::cmp::Reverse(n.1));

    println!();
    println!("=== tick_profile ===");
    println!(
        "world_size={world_size}  grass_cell={grass_cell}  full_grass={full_grass}  founders={founders}  warmup={warmup}  measure={measure}"
    );
    println!("population: after warmup = {pop_after_warmup}, after measure = {pop_after_measure}");
    println!(
        "wall: {:.2} ms for {measure} ticks = {:.3} ms/tick",
        wall.as_secs_f64() * 1e3,
        wall.as_secs_f64() * 1e3 / measure as f64
    );
    println!();
    println!("{:<28} {:>12} {:>10}", "phase", "us/tick", "% of tick");

    let tick_total = nodes
        .iter()
        .find(|(n, _, _)| n == "tick")
        .map(|(_, us, c)| *us as f64 / *c as f64)
        .unwrap_or(0.0);

    for (name, total_us, calls) in &nodes {
        let us_per_tick = *total_us as f64 / *calls as f64;
        let pct = if tick_total > 0.0 {
            100.0 * us_per_tick / tick_total
        } else {
            0.0
        };
        println!("{name:<28} {us_per_tick:>12.1} {pct:>9.1}%");
    }
    println!();
}
