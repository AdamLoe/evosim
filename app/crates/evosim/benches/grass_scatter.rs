//! Criterion benchmark: scatter vs blur grass-propagation kernel speedup.
//!
//! Grid choice: `world_size = 2560.0` → `grass_dim = round(2560 / 5) = 512`.
//! That is 512×512 = 262 144 cells, 16×16 = 256 tiles.
//!
//! Active-tile set: `initial_seed_count = 16 384` (≈ 6.25 % of cells) placed by
//! a seeded SimRng. After `resync_active_from_density()`, the frontier tiles that
//! border the seeded patches are active (typically 40–120 tiles at this density).
//! This represents a realistic mid-game frontier: not all-empty (trivial skip) and
//! not fully saturated (also skipped). A denser seeding of ~25 % (65 536 seeds)
//! is used for the blur bench because the blur kernel's active-tile set tends to
//! shrink to a thin fringe at saturation, which would under-represent its true
//! production cost; both benches use the same grid so the comparison is fair.
//!
//! NOTE: both functions are called via the public `compute_propagation` dispatcher
//! (which routes via `GrassPropagation`), so no private-function exposure is needed.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use evosim::constants::WorldDims;
use evosim::grass::{GrassGrid, GrassPropagation};
use evosim::rng::SimRng;

/// Build a seeded GrassGrid at the given world_size with `seed_count` random
/// cells set to full grass. The active-tile set is resynced from the density
/// so the first `compute_propagation` call sees a realistic frontier.
fn make_grid(world_size: f32, seed_count: u32, rng_seed: u64) -> GrassGrid {
    let dims = WorldDims::from_world_size(world_size, false);
    let mut rng = SimRng::from_u64(rng_seed);
    GrassGrid::new(&mut rng, seed_count, dims)
}

fn bench_grass_propagation(c: &mut Criterion) {
    // ── Grid parameters ─────────────────────────────────────────────────────
    // world_size=2560  →  grass_dim=512  →  512²=262 144 cells, 16×16=256 tiles
    // seed_count=16 384 → ≈6.25% fill → realistic mid-game frontier (~40-120
    // active tiles depending on seeding layout).
    const WORLD_SIZE: f32 = 2560.0;
    const SEED_COUNT: u32 = 16_384;
    const RNG_SEED: u64 = 0xDEAD_BEEF_1234_5678;

    // Scatter params: defaults from ScatterParams::default() (these mirror the
    // live sim constants: decay_pct=0.02, spread_pct=0.55, etc.).

    // ── SCATTER benchmark ────────────────────────────────────────────────────
    // Use iter_batched so the clone overhead is measured in the SETUP phase
    // (excluded from timing), and only compute_propagation is timed.
    c.bench_function("grass_propagation_scatter", |b| {
        let base = make_grid(WORLD_SIZE, SEED_COUNT, RNG_SEED);
        b.iter_batched(
            || {
                let mut g = base.clone();
                g.set_propagation(GrassPropagation::Scatter);
                g.world_seed = 42;
                g.scatter_tick = 1;
                g
            },
            |mut g| g.compute_propagation(black_box(0.05), black_box(0.02)),
            criterion::BatchSize::SmallInput,
        );
    });

    // ── BLUR benchmark ───────────────────────────────────────────────────────
    // Use iter_batched for the same reason: clone is setup, not measured.
    // r_in_cell=0.05, k_propagate=0.02 match the production World defaults.
    c.bench_function("grass_propagation_blur", |b| {
        let base = make_grid(WORLD_SIZE, SEED_COUNT, RNG_SEED);
        b.iter_batched(
            || {
                let mut g = base.clone();
                g.set_propagation(GrassPropagation::Blur);
                g
            },
            |mut g| g.compute_propagation(black_box(0.05), black_box(0.02)),
            criterion::BatchSize::SmallInput,
        );
    });

    // ── Full step_n(1) proxy: scatter vs blur on a single tick ───────────────
    // Mirrors what World::tick_once does (propagation + rebuild_row_bitset).
    // Both use the same 512² grid with default scatter params.
    let mut group = c.benchmark_group("grass_full_tick");
    for mode in &[GrassPropagation::Scatter, GrassPropagation::Blur] {
        let label = match mode {
            GrassPropagation::Scatter => "scatter",
            GrassPropagation::Blur => "blur",
        };
        let base = make_grid(WORLD_SIZE, SEED_COUNT, RNG_SEED);
        group.bench_with_input(BenchmarkId::from_parameter(label), mode, |b, &mode| {
            b.iter_batched(
                || {
                    let mut g = base.clone();
                    g.set_propagation(mode);
                    if mode == GrassPropagation::Scatter {
                        g.world_seed = 42;
                        g.scatter_tick = 1;
                    }
                    g
                },
                |mut g| {
                    g.compute_propagation(black_box(0.05), black_box(0.02));
                    g.rebuild_row_bitset();
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_grass_propagation);
criterion_main!(benches);
