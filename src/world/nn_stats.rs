//! Per-worker + per-sub-phase instrumentation for the parallel NN forward pass.
//!
//! Two layers of counters:
//!
//! 1. **Per-worker lifetime totals** — indexed by `rayon::current_thread_index()`.
//!    Cumulative since `World::new_with_sliders`. Lets a UI render a
//!    health/uptime table (how many creatures each worker has processed, how
//!    long it has been busy, when it first picked up work).
//!
//! 2. **Per-tick sub-phase aggregates** — reset at the top of each NN pass.
//!    After the parallel block returns to the main thread, these are read out
//!    and pushed into the profiler tree under `tick.nn.*` so the existing
//!    perf panel picks them up.
//!
//! All counters are `AtomicU64` with `Relaxed` ordering — chunks contend
//! through `fetch_add` but the ordering only needs to be commutative
//! (sum-of-contributions). Reset happens on the main thread before the
//! parallel pass dispatches, so no race with worker writes.

use std::sync::atomic::{AtomicU64, Ordering};

/// Hard cap on tracked workers. Matches `MAX_CHUNKS = 16` from constants.rs;
/// `rayon::current_thread_index()` may return up to `current_num_threads()`,
/// which equals the JS-side `initThreadPool(navigator.hardwareConcurrency)`.
/// 64 leaves headroom for big-iron CPUs without forcing dynamic alloc.
pub const MAX_TRACKED_WORKERS: usize = 64;

pub struct NnStats {
    /// World construction wall time (us), for "uptime since boot" math.
    pub world_start_us: u64,

    // ── Per-worker lifetime (cumulative since world start) ────────────────
    /// First wall-clock timestamp (us) at which worker `w` picked up an NN
    /// chunk. 0 = never. Computed via compare_exchange so the first writer wins.
    pub worker_first_seen_us: [AtomicU64; MAX_TRACKED_WORKERS],
    /// Latest wall-clock timestamp (us) at which worker `w` finished a chunk.
    /// Updated on every chunk completion. 0 = never.
    pub worker_last_seen_us: [AtomicU64; MAX_TRACKED_WORKERS],
    /// Cumulative chunks processed by worker `w`.
    pub worker_chunks: [AtomicU64; MAX_TRACKED_WORKERS],
    /// Cumulative creatures processed by worker `w`.
    pub worker_creatures: [AtomicU64; MAX_TRACKED_WORKERS],
    /// Cumulative wall time (us) worker `w` has spent inside chunk closures.
    pub worker_busy_us: [AtomicU64; MAX_TRACKED_WORKERS],

    // ── Per-tick sub-phase aggregates (reset each tick) ───────────────────
    pub tick_build_input_other_us: AtomicU64,
    pub tick_proximity_creatures_us: AtomicU64,
    pub tick_proximity_grass_us: AtomicU64,
    pub tick_forward_us: AtomicU64,
    /// Sum of every chunk's wall-clock duration this tick. Often larger than
    /// the tick.nn span itself (chunks run in parallel; sum-of-busy > wall).
    pub tick_chunk_wall_us: AtomicU64,
    /// Number of distinct workers that picked up work this tick (rough
    /// parallelism utilization indicator). Counted via a transient bitset.
    pub tick_workers_used_bitset: AtomicU64,
}

impl NnStats {
    pub fn new(world_start_us: u64) -> Self {
        // Helper closure: [AtomicU64::new(0); N] requires `Copy` which AtomicU64
        // intentionally doesn't have (every element gets its own atomic cell).
        // `std::array::from_fn` initializes each slot independently.
        let zero_array = || std::array::from_fn(|_| AtomicU64::new(0));
        Self {
            world_start_us,
            worker_first_seen_us: zero_array(),
            worker_last_seen_us: zero_array(),
            worker_chunks: zero_array(),
            worker_creatures: zero_array(),
            worker_busy_us: zero_array(),
            tick_build_input_other_us: AtomicU64::new(0),
            tick_proximity_creatures_us: AtomicU64::new(0),
            tick_proximity_grass_us: AtomicU64::new(0),
            tick_forward_us: AtomicU64::new(0),
            tick_chunk_wall_us: AtomicU64::new(0),
            tick_workers_used_bitset: AtomicU64::new(0),
        }
    }

    /// Reset only the per-tick aggregates. Lifetime counters are preserved.
    pub fn reset_tick(&self) {
        self.tick_build_input_other_us.store(0, Ordering::Relaxed);
        self.tick_proximity_creatures_us.store(0, Ordering::Relaxed);
        self.tick_proximity_grass_us.store(0, Ordering::Relaxed);
        self.tick_forward_us.store(0, Ordering::Relaxed);
        self.tick_chunk_wall_us.store(0, Ordering::Relaxed);
        self.tick_workers_used_bitset.store(0, Ordering::Relaxed);
    }

    /// Record one chunk's worth of work. Called by every worker thread on every
    /// chunk; uses `Relaxed` atomics so the contention is fetch_add-only.
    #[allow(clippy::too_many_arguments)]
    pub fn record_chunk(
        &self,
        worker_idx: usize,
        chunk_start_us: u64,
        chunk_end_us: u64,
        creatures_in_chunk: u64,
        build_input_other_us: u64,
        proximity_creatures_us: u64,
        proximity_grass_us: u64,
        forward_us: u64,
    ) {
        let w = worker_idx.min(MAX_TRACKED_WORKERS - 1);
        let chunk_us = chunk_end_us.saturating_sub(chunk_start_us);

        // First-seen: cmpxchg 0 → chunk_start. Only the first writer wins.
        let _ = self.worker_first_seen_us[w].compare_exchange(
            0,
            chunk_start_us,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        self.worker_last_seen_us[w].store(chunk_end_us, Ordering::Relaxed);
        self.worker_chunks[w].fetch_add(1, Ordering::Relaxed);
        self.worker_creatures[w].fetch_add(creatures_in_chunk, Ordering::Relaxed);
        self.worker_busy_us[w].fetch_add(chunk_us, Ordering::Relaxed);

        self.tick_build_input_other_us
            .fetch_add(build_input_other_us, Ordering::Relaxed);
        self.tick_proximity_creatures_us
            .fetch_add(proximity_creatures_us, Ordering::Relaxed);
        self.tick_proximity_grass_us
            .fetch_add(proximity_grass_us, Ordering::Relaxed);
        self.tick_forward_us
            .fetch_add(forward_us, Ordering::Relaxed);
        self.tick_chunk_wall_us
            .fetch_add(chunk_us, Ordering::Relaxed);

        if w < 64 {
            self.tick_workers_used_bitset
                .fetch_or(1u64 << w, Ordering::Relaxed);
        }
    }

    /// JSON snapshot for the dev panel / inspector. One object with a
    /// `workers` array (only entries with non-zero `chunks`) plus the latest
    /// per-tick aggregates and the global world uptime.
    pub fn to_json(&self, now_us: u64) -> String {
        let mut s = String::with_capacity(1024);
        s.push_str("{\"world_uptime_us\":");
        s.push_str(&now_us.saturating_sub(self.world_start_us).to_string());
        s.push_str(",\"tick\":{\"build_input_other_us\":");
        s.push_str(
            &self
                .tick_build_input_other_us
                .load(Ordering::Relaxed)
                .to_string(),
        );
        s.push_str(",\"proximity_creatures_us\":");
        s.push_str(
            &self
                .tick_proximity_creatures_us
                .load(Ordering::Relaxed)
                .to_string(),
        );
        s.push_str(",\"proximity_grass_us\":");
        s.push_str(
            &self
                .tick_proximity_grass_us
                .load(Ordering::Relaxed)
                .to_string(),
        );
        s.push_str(",\"forward_us\":");
        s.push_str(&self.tick_forward_us.load(Ordering::Relaxed).to_string());
        s.push_str(",\"chunk_wall_us\":");
        s.push_str(&self.tick_chunk_wall_us.load(Ordering::Relaxed).to_string());
        s.push_str(",\"workers_used\":");
        s.push_str(
            &(self
                .tick_workers_used_bitset
                .load(Ordering::Relaxed)
                .count_ones() as u64)
                .to_string(),
        );
        s.push_str("},\"workers\":[");

        let mut first = true;
        for w in 0..MAX_TRACKED_WORKERS {
            let chunks = self.worker_chunks[w].load(Ordering::Relaxed);
            if chunks == 0 {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let first_seen = self.worker_first_seen_us[w].load(Ordering::Relaxed);
            let last_seen = self.worker_last_seen_us[w].load(Ordering::Relaxed);
            let creatures = self.worker_creatures[w].load(Ordering::Relaxed);
            let busy_us = self.worker_busy_us[w].load(Ordering::Relaxed);
            s.push_str("{\"idx\":");
            s.push_str(&w.to_string());
            s.push_str(",\"first_seen_us\":");
            s.push_str(&first_seen.to_string());
            s.push_str(",\"last_seen_us\":");
            s.push_str(&last_seen.to_string());
            s.push_str(",\"uptime_us\":");
            s.push_str(&now_us.saturating_sub(first_seen).to_string());
            s.push_str(",\"chunks\":");
            s.push_str(&chunks.to_string());
            s.push_str(",\"creatures\":");
            s.push_str(&creatures.to_string());
            s.push_str(",\"busy_us\":");
            s.push_str(&busy_us.to_string());
            s.push_str(",\"creatures_per_chunk\":");
            s.push_str(&(creatures / chunks.max(1)).to_string());
            s.push('}');
        }
        s.push_str("]}");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_chunk_accumulates_per_worker() {
        let s = NnStats::new(0);
        s.record_chunk(0, 100, 250, 50, 30, 40, 50, 20);
        s.record_chunk(0, 300, 500, 80, 60, 70, 50, 40);
        s.record_chunk(2, 110, 180, 30, 10, 20, 30, 15);

        assert_eq!(s.worker_chunks[0].load(Ordering::Relaxed), 2);
        assert_eq!(s.worker_creatures[0].load(Ordering::Relaxed), 130);
        assert_eq!(s.worker_busy_us[0].load(Ordering::Relaxed), 350); // (250-100) + (500-300)
        assert_eq!(s.worker_first_seen_us[0].load(Ordering::Relaxed), 100);
        assert_eq!(s.worker_chunks[2].load(Ordering::Relaxed), 1);
        assert_eq!(s.worker_chunks[1].load(Ordering::Relaxed), 0);

        assert_eq!(s.tick_forward_us.load(Ordering::Relaxed), 75);
        // workers 0 and 2 used → bitset = 0b101 = 5; popcount = 2
        let used = s.tick_workers_used_bitset.load(Ordering::Relaxed);
        assert_eq!(used.count_ones(), 2);
    }

    #[test]
    fn reset_tick_clears_per_tick_only() {
        let s = NnStats::new(0);
        s.record_chunk(0, 100, 200, 50, 10, 20, 30, 40);
        s.reset_tick();
        assert_eq!(s.worker_chunks[0].load(Ordering::Relaxed), 1); // preserved
        assert_eq!(s.tick_forward_us.load(Ordering::Relaxed), 0); // cleared
        assert_eq!(s.tick_workers_used_bitset.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn first_seen_only_set_once() {
        let s = NnStats::new(0);
        s.record_chunk(3, 500, 700, 10, 0, 0, 0, 0);
        s.record_chunk(3, 800, 900, 10, 0, 0, 0, 0);
        assert_eq!(s.worker_first_seen_us[3].load(Ordering::Relaxed), 500);
        assert_eq!(s.worker_last_seen_us[3].load(Ordering::Relaxed), 900);
    }
}
