//! Snapshot-write cost breakdown: measures the per-call cost of `write_snapshot`
//! at the default scale (mip_level=0, 1920×1920 window), split by component:
//!
//!   - `snapshot_write_full`   : the complete `write_snapshot` call (baseline)
//!   - `biome_window_raw`      : just the biome mode-window loop (isolated kernel)
//!   - `creature_pack`         : just the creature SoA→bytes copy (isolated kernel)
//!   - `grass_pyramid_window`  : just the grass pyramid viewport_window call (isolated)
//!
//! "Before" numbers (pre-precompute) are produced by running the bench before
//! the optimization; "after" numbers by running it again after. The biome_window
//! bench target is the prime cost candidate (static grid, per-tick recompute).
//!
//! World: default scale 9600u, grass_cell_size=5.0 → grass_dim=1920.
//! Camera: center at (4800, 4800), zoom=1.0, viewport 1920×1080.
//! Population: 100 creatures (small but real, so creature_pack is non-trivial).

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use evosim::constants::WorldDims;
use evosim::world::biome::generate_biome_grid;
use evosim::WorldHandle;

const WORLD_SIZE: f32 = 9600.0;
const GRASS_CELL_SIZE: f32 = 5.0;
const WORLD_SEED: u32 = 12345;
const FOUNDER_COUNT: u32 = 100;

/// Build a WorldHandle at the default scale with a fixed population.
fn make_handle() -> WorldHandle {
    WorldHandle::new_with_founder_count(
        "bench-seed-snap",
        8000,  // initial_grass_seed_count
        200.0, // energy_max
        FOUNDER_COUNT,
        false, // full_grass_on_init
        "",    // nn_topology_json (legacy)
        WORLD_SIZE,
        true, // wrap_world
        WORLD_SEED,
        false, // species_mode
        0.0,   // crossover_mode
        1,     // starting_species_count
        FOUNDER_COUNT,
        0.0, // starting_species_member_variance
        GRASS_CELL_SIZE,
        0,   // grass_clump_count (0 = uniform scatter)
        0,   // grass_clump_size
        1.0, // init_graze_boost (neutral)
        1.0, // init_split_boost (neutral)
    )
    .expect("WorldHandle construction failed")
}

/// Benchmark the full write_snapshot call (all components together).
fn bench_snapshot_write_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_write");
    group.measurement_time(std::time::Duration::from_secs(5));
    group.sample_size(50);

    let mut handle = make_handle();
    // Step a few ticks so the world is non-trivial (population alive, grass spread).
    for _ in 0..10 {
        handle.step();
    }

    // Camera: center of world, default zoom, 1920×1080 viewport.
    let cam_cx = WORLD_SIZE / 2.0;
    let cam_cy = WORLD_SIZE / 2.0;
    let cam_zoom = 1.0_f32;
    let vp_w = 1920u32;
    let vp_h = 1080u32;

    group.bench_function("write_snapshot", |b| {
        b.iter(|| {
            handle.write_snapshot(
                black_box(0),
                black_box(cam_cx),
                black_box(cam_cy),
                black_box(cam_zoom),
                black_box(vp_w),
                black_box(vp_h),
            );
        });
    });

    group.finish();
}

/// Benchmark isolated biome window computation kernel.
/// This directly measures the per-tick cost of the nested-loop biome
/// mode-downsampling that the precompute optimization targets.
fn bench_biome_window_kernel(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_write");
    group.measurement_time(std::time::Duration::from_secs(5));
    group.sample_size(50);

    // Build a biome grid identical to what the world constructs.
    let grass_dim = (WORLD_SIZE / GRASS_CELL_SIZE) as usize; // 1920
    let biome_grid: Vec<u8> = {
        let dims = WorldDims::from_world_size(WORLD_SIZE, true);
        generate_biome_grid(WORLD_SEED, &dims)
    };

    // At default scale: mip_level=0 → block=1, win_w=1920, win_h=1080*1.5≈1620 → clamped to 1920.
    let mip_level: usize = 0;
    let block = 1usize << mip_level;
    let win_origin_x: usize = 0;
    let win_origin_y: usize = 0;
    let win_w: usize = 1920;
    let win_h: usize = 1620; // ceil(1080 * 1.5)

    let mut biome_region = vec![0u8; win_w * win_h];

    group.bench_function("biome_window_kernel", |b| {
        b.iter(|| {
            // This is the exact nested loop from write_snapshot — the "before" state.
            let grid = black_box(&biome_grid);
            let dim = black_box(grass_dim);
            let ox = black_box(win_origin_x);
            let oy = black_box(win_origin_y);
            let ww = black_box(win_w);
            let wh = black_box(win_h);
            let dst = &mut biome_region;
            let blk = black_box(block);

            for out_y in 0..wh {
                let lk_row = oy + out_y;
                let l0_row_base = (lk_row * blk).min(dim.saturating_sub(1));
                for out_x in 0..ww {
                    let lk_col = ox + out_x;
                    let l0_col_base = (lk_col * blk).min(dim.saturating_sub(1));

                    let mut counts = [0u32; 4];
                    for dy in 0..blk {
                        let row = (l0_row_base + dy).min(dim - 1);
                        for dx in 0..blk {
                            let col = (l0_col_base + dx).min(dim - 1);
                            let tag = grid[row * dim + col] as usize;
                            let idx = tag.min(counts.len() - 1);
                            counts[idx] += 1;
                        }
                    }
                    let mut best_tag = 0u8;
                    let mut best_count = 0u32;
                    for (tag, &cnt) in counts.iter().enumerate() {
                        if cnt > best_count {
                            best_count = cnt;
                            best_tag = tag as u8;
                        }
                    }
                    dst[out_y * ww + out_x] = best_tag;
                }
            }
            black_box(&dst[0]);
        });
    });

    // Also bench the precomputed path: a simple memcpy-like slice copy from a cached window.
    // This represents the "after" state once the pyramid is precomputed at construction.
    let precomputed_window = biome_region.clone();
    group.bench_function("biome_window_precomputed_copy", |b| {
        let src = &precomputed_window;
        let mut dst = vec![0u8; win_w * win_h];
        b.iter(|| {
            dst.copy_from_slice(black_box(src));
            black_box(&dst[0]);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_snapshot_write_full,
    bench_biome_window_kernel
);
criterion_main!(benches);
