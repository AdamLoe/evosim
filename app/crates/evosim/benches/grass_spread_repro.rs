//! Repro: does lowering `grass_spread_pct` (the "Scatter spread prob" slider)
//! kill the whole population? Builds a world, warms it to a living steady state,
//! snapshots pop + grass, applies a new spread_pct (env TICK_SPREAD, default
//! 0.1), then steps and prints the pop/grass trajectory so we can see whether
//! the collapse is a gradual starvation (emergent) or an instant cliff (bug).
//!
//! Run: `cargo bench --bench grass_spread_repro --features threads`
//! Env: TICK_SPREAD (new spread_pct, default 0.1), TICK_WORLD (default 4800),
//!      TICK_WARMUP (default 600), TICK_AFTER (default 400).

use evosim::WorldHandle;

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn make(world: f32) -> WorldHandle {
    WorldHandle::new_with_founder_count(
        "grass-spread-repro",
        8000,
        200.0,
        600,
        false,
        "",
        world,
        true,
        12345,
        false,
        0.0,
        1,
        600,
        0.0,
        5.0,
        true,
        40,
        8,
        1.0, // init_graze_boost (neutral)
        1.0, // init_split_boost (neutral)
    )
    .expect("ctor")
}

fn snap(h: &WorldHandle) -> (u32, u32, f32) {
    (
        h.population(),
        h.live_grass_cell_count(),
        h.total_grass_density(),
    )
}

fn main() {
    let world = env_f32("TICK_WORLD", 4800.0);
    let warmup = env_u32("TICK_WARMUP", 600);
    let after = env_u32("TICK_AFTER", 400);
    let new_spread = env_f32("TICK_SPREAD", 0.1);

    let mut h = make(world);
    let _ = h.set_slider("max_population", 4000.0);
    for _ in 0..warmup {
        h.step();
    }
    let (p0, g0, d0) = snap(&h);
    println!("\n=== grass_spread_repro (world={world}) ===");
    println!("default spread_pct=0.55. After {warmup} warmup ticks:");
    println!("  pop={p0}  live_grass_cells={g0}  total_density={d0:.0}");
    // Faithful repro of the live SAB epoch-drain bug: the worker applies ALL
    // slider lanes on any Apply, and lane 17 (max_population) is never seeded
    // (its control lives in the perf panel, omitted from initial_sliders) so it
    // drains as 0 → apply_max_population clamps to cap=1 → mass cull.
    if env_u32("TICK_CULLBUG", 0) != 0 {
        println!("\n--- simulating SAB drain: set_slider_by_index(17 max_population, 0.0) ---");
        h.set_slider_by_index(17, 0.0);
    } else {
        println!("\n--- applying grass_spread_pct = {new_spread} (via set_slider) ---");
        let _ = h.set_slider("grass_spread_pct", new_spread);
    }

    println!(
        "{:>5}  {:>7}  {:>14}  {:>12}",
        "tick", "pop", "grass_cells", "density"
    );
    for t in 1..=after {
        h.step();
        if t <= 10 || t % 25 == 0 || t == after {
            let (p, g, d) = snap(&h);
            println!("{t:>5}  {p:>7}  {g:>14}  {d:>12.0}");
            if p == 0 {
                println!("  >>> population hit ZERO at tick {t} after the change");
                break;
            }
        }
    }
    println!();
}
