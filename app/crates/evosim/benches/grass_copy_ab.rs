//! A/B microbench for the snapshot grass-window copy (`grass_copy`).
//!
//! Replicates the level-0 `GrassPyramid::viewport_window` copy at default scale
//! (1920×1920 full field, wrap_world = true — the default) two ways:
//!
//!   OLD: the original per-cell loop — an integer `% lw` (wrap) plus an
//!        `AtomicU8::load` for every one of the 3.68M cells, every tick.
//!   NEW: reinterpret the AtomicU8 field as `&[u8]` and copy row-by-row with
//!        `copy_from_slice` (memcpy); wrap handled with at most two slice copies
//!        per row, so the per-cell `%` is gone entirely.
//!
//! This isolates exactly what changed in the snapshot grass copy, with zero
//! browser noise. Run: `cargo bench --bench grass_copy_ab --features threads`
//! Env: CP_DIM (default 1920), CP_ITERS (default 200)

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let dim = env_usize("CP_DIM", 1920);
    let iters = env_usize("CP_ITERS", 200);
    let n = dim * dim;

    // Build a density field with a non-trivial pattern (so the optimizer can't
    // fold it away). Stored as AtomicU8, exactly like GrassGrid::density.
    let density: Vec<AtomicU8> = (0..n)
        .map(|i| AtomicU8::new(((i * 2654435761) >> 13) as u8))
        .collect();
    let mut dst = vec![0u8; n];

    let (lw, lh) = (dim, dim);
    let (origin_x, origin_y) = (0usize, 0usize);
    let (win_w, win_h) = (dim, dim);

    // ---- OLD: per-cell `% lw` + atomic load (wrap path) ----
    let t0 = Instant::now();
    let mut old_checksum = 0u64;
    for _ in 0..iters {
        for row in 0..win_h {
            let sy = (origin_y + row) % lh;
            let dst_row = &mut dst[row * win_w..(row + 1) * win_w];
            for (col, byte) in dst_row.iter_mut().enumerate() {
                let sx = (origin_x + col) % lw;
                *byte = density[sy * lw + sx].load(Ordering::Relaxed);
            }
        }
        old_checksum = old_checksum.wrapping_add(dst[n / 2] as u64);
    }
    let old_us = t0.elapsed().as_micros() as f64 / iters as f64;

    // ---- NEW: reinterpret as &[u8], row-wise copy_from_slice (memcpy) ----
    let t1 = Instant::now();
    let mut new_checksum = 0u64;
    for _ in 0..iters {
        // SAFETY: AtomicU8 has the same layout as u8; no concurrent writer here.
        let src: &[u8] = unsafe { std::slice::from_raw_parts(density.as_ptr() as *const u8, n) };
        for row in 0..win_h {
            let sy = (origin_y + row) % lh;
            let dst_row = &mut dst[row * win_w..(row + 1) * win_w];
            let src_row = &src[sy * lw..(sy + 1) * lw];
            let first_len = win_w.min(lw - origin_x);
            dst_row[..first_len].copy_from_slice(&src_row[origin_x..origin_x + first_len]);
            if first_len < win_w {
                dst_row[first_len..].copy_from_slice(&src_row[..win_w - first_len]);
            }
        }
        new_checksum = new_checksum.wrapping_add(dst[n / 2] as u64);
    }
    let new_us = t1.elapsed().as_micros() as f64 / iters as f64;

    println!("\n=== grass_copy_ab (dim={dim}, {n} cells, {iters} iters) ===");
    println!("OLD (per-cell % + atomic load): {old_us:8.1} µs/copy");
    println!("NEW (reinterpret + memcpy rows): {new_us:8.1} µs/copy");
    println!("speedup: {:.1}×", old_us / new_us);
    assert_eq!(
        old_checksum, new_checksum,
        "kernels must produce identical bytes"
    );
}
