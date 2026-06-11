//! A/B microbench for the grass scatter step at full coverage.
//!
//! Zero founders → population is 0 from tick 0 → the sim runs its thin path
//! (grass_step + bitset rebuild only), so a timed `step()` is essentially the
//! grass kernel in isolation — no NN / movement / births noise. We warm up
//! until grass saturates the whole field, then time a block of steps and report
//! mean µs/step. Deterministic and browser-noise-free; the relevant cost axis
//! (memory traffic + the per-tile source read + the spread RMWs) is identical
//! between native and x86-hosted wasm because `Relaxed` atomic loads lower to
//! plain `mov`s on x86 in both.
//!
//! Run: `cargo bench --bench grass_step_ab --features threads`
//! Env: AB_WORLD (default 9600), AB_WARMUP (default 2500), AB_TIMED (default 400)

use evosim::WorldHandle;
use std::time::Instant;

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let world = env_f32("AB_WORLD", 9600.0);
    let warmup = env_u32("AB_WARMUP", 2500);
    let timed = env_u32("AB_TIMED", 400);

    // founder_count = 0 → no creatures ever → thin (grass-only) path.
    let mut h = WorldHandle::new_with_founder_count(
        "grass-step-ab",
        0, // founder_count
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
        1.0,
        1.0,
    )
    .expect("ctor");

    // Force the population to die out → thin (grass-only) path, so a timed step
    // is the grass kernel in isolation.
    let _ = h.set_slider("max_population", 0.0);
    for _ in 0..warmup {
        h.step();
    }
    let g0 = h.live_grass_cell_count();
    let d0 = h.total_grass_density();
    let pop = h.population();

    let t0 = Instant::now();
    for _ in 0..timed {
        h.step();
    }
    let elapsed = t0.elapsed();
    let us_per_step = elapsed.as_micros() as f64 / timed as f64;

    println!(
        "\n=== grass_step_ab (world={world}, threads={}) ===",
        cfg!(feature = "threads")
    );
    println!(
        "after {warmup} warmup ticks: pop={pop}  live_grass_cells={g0}  total_density={d0:.0}"
    );
    println!(
        "timed {timed} steps: {:.3} ms total → {us_per_step:.1} µs/step",
        elapsed.as_secs_f64() * 1000.0
    );
}
