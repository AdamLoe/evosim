//! v5 §16 / v6 §M acceptance test. Pinned seed, 10k ticks, perf gate,
//! golden snapshot hash. Run with `cargo test --test acceptance`.
//!
//! Bootstrap: `EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance`
//! writes `tests/golden_snapshot_t10000.txt`. Subsequent runs assert against it.

// D6 stub: use evosim::save::SaveV1; — deleted by D1, test deleted by D6
use evosim::snapshot_hash::snapshot_hash;
use evosim::world::World;
use std::time::Instant;

const SEED: &str = "evosim-test-001";
const TICKS: u32 = 10_000;
const PERF_BUDGET_MS: u128 = 8_000;
#[cfg(not(feature = "threads"))]
const GOLDEN_PATH: &str = "tests/golden_snapshot_t10000.txt";

#[cfg(not(feature = "threads"))]
#[test]
fn acceptance_t10000() {
    let mut w = World::new(SEED);

    let started = Instant::now();
    for _ in 0..TICKS {
        if !w.tick_once() {
            // World ended early — still run the assertions so we report the
            // actual population / species counts rather than silently exiting.
            break;
        }
    }
    let elapsed_ms = started.elapsed().as_millis();

    // (a) population > 0 at tick 10_000 (or whenever the world stopped).
    assert!(
        w.population() > 0,
        "(a) population must be > 0 at tick {TICKS}; got {} at tick {}",
        w.population(),
        w.tick,
    );

    // (b) ≥ 2 distinct live species.
    assert!(
        w.live_species_count >= 2,
        "(b) live_species_count must be ≥ 2; got {} at tick {}",
        w.live_species_count,
        w.tick,
    );

    // (c) wall-clock under budget.
    assert!(
        elapsed_ms < PERF_BUDGET_MS,
        "(c) sim took {elapsed_ms} ms; budget {PERF_BUDGET_MS} ms",
    );

    // (d) snapshot hash matches pinned golden value.
    let hash = snapshot_hash(&w);

    // Bootstrap mode: env var writes the golden file instead of asserting.
    if std::env::var("EVOSIM_WRITE_GOLDEN").is_ok() {
        std::fs::write(GOLDEN_PATH, format!("{hash:#018x}\n")).expect("write golden");
        eprintln!("wrote golden snapshot: {hash:#018x}  (tick={})", w.tick);
        return;
    }

    let golden_str = std::fs::read_to_string(GOLDEN_PATH).expect(
        "golden file missing; bootstrap with: \
         EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance",
    );
    let golden_str = golden_str.trim();
    let golden: u64 = u64::from_str_radix(golden_str.trim_start_matches("0x"), 16)
        .expect("malformed golden hash in tests/golden_snapshot_t10000.txt");

    assert_eq!(
        hash, golden,
        "(d) snapshot hash mismatch: got {hash:#018x}, golden {golden:#018x}",
    );
}

/// Test 7 (profiler plan): enabling the profiler at t=0 must NOT change
/// the golden snapshot hash. This asserts the observation-purity guarantee
/// (D10): profiler is a pure observer and has zero effect on sim state.
#[cfg(not(feature = "threads"))]
#[test]
fn profile_does_not_change_hash() {
    let mut w = World::new(SEED);

    // Enable the profiler from tick 0 — the strongest possible test.
    w.profile.set_enabled(true);

    for _ in 0..TICKS {
        if !w.tick_once() {
            break;
        }
    }

    let hash = evosim::snapshot_hash::snapshot_hash(&w);

    let golden_str = std::fs::read_to_string(GOLDEN_PATH).expect(
        "golden file missing; bootstrap with: \
         EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance",
    );
    let golden_str = golden_str.trim();
    let golden: u64 = u64::from_str_radix(golden_str.trim_start_matches("0x"), 16)
        .expect("malformed golden hash in tests/golden_snapshot_t10000.txt");

    assert_eq!(
        hash, golden,
        "profiler-enabled hash mismatch: got {hash:#018x}, golden {golden:#018x}; \
         the profiler must be a pure observer (D10)",
    );
}

/// D6 stub: F.26 round-trip test disabled (SaveV1 deleted by D1; test deleted by D6).
#[cfg(any())]
#[cfg(not(feature = "threads"))]
#[test]
fn save_load_step_preserves_determinism() {
    const SAVE_AT: u32 = 1_000;
    const RESUME_STEPS: u32 = 500;

    // Reference path: run SAVE_AT + RESUME_STEPS straight through.
    let mut reference = World::new(SEED);
    for _ in 0..(SAVE_AT + RESUME_STEPS) {
        if !reference.tick_once() {
            break;
        }
    }
    let ref_hash = snapshot_hash(&reference);

    // Save/load path: run SAVE_AT, save, load into a fresh World, run RESUME_STEPS.
    let mut original = World::new(SEED);
    for _ in 0..SAVE_AT {
        assert!(original.tick_once(), "original died before save point");
    }
    let save_json = serde_json::to_string(&SaveV1::from_world(&original)).expect("serialize");
    let save: SaveV1 = serde_json::from_str(&save_json).expect("deserialize");
    let mut loaded = World::from_save_v1(save).expect("from_save_v1");
    for _ in 0..RESUME_STEPS {
        if !loaded.tick_once() {
            break;
        }
    }
    let loaded_hash = snapshot_hash(&loaded);

    assert_eq!(
        ref_hash, loaded_hash,
        "save→load→step{RESUME_STEPS} diverged from reference at tick {}; \
         ref={ref_hash:#018x}, loaded={loaded_hash:#018x}",
        loaded.tick,
    );
}

/// D6 stub: S39 save/load hash test disabled (SaveV1 deleted by D1; test deleted by D6).
#[cfg(any())]
#[test]
fn save_load_hash_equal_immediately_after_load() {
    for &n in &[0u32, 1, 200, 2000] {
        let mut w = World::new(SEED);
        for tick in 0..n {
            assert!(
                w.tick_once(),
                "world died before n={n} ticks (at tick {tick}); pinned seed should survive"
            );
        }
        let h_orig = snapshot_hash(&w);

        let json = serde_json::to_string(&SaveV1::from_world(&w)).expect("serialize");
        let save: SaveV1 = serde_json::from_str(&json).expect("deserialize");
        let loaded = World::from_save_v1(save).expect("from_save_v1 at n={n}");
        let h_loaded = snapshot_hash(&loaded);

        assert_eq!(
            h_orig, h_loaded,
            "save→load at n={n} diverged before any further step; \
             orig={h_orig:#018x}, loaded={h_loaded:#018x}. \
             A field silently zeroed on load will surface here."
        );
    }
}

/// Threaded acceptance test (perf-4 dual-golden protocol).
///
/// Runs the same 10k-tick scenario as `acceptance_t10000` but with the
/// `threads` feature active (parallel NN forward + parallel vision).
/// Asserts against a SEPARATE pinned golden file. Static inspection of
/// the threaded NN-forward block (src/world.rs:351–424) vs the
/// sequential helpers (src/world.rs:1117–1161) shows bit-identical
/// arithmetic, and vision is RNG-free, so the bootstrapped threaded
/// hash MAY equal the sequential one. If it differs, the source is
/// parallel f32 reduction order across chunks. Either outcome is
/// acceptable; each codepath is deterministic against itself.
///
/// Bootstrap:
///   EVOSIM_WRITE_GOLDEN_THREADED=1 cargo test --release \
///       --features threads --test acceptance acceptance_t10000_threaded
///
/// See docs/plans/perf-4-threads.md §7 for the full bootstrap recipe.
#[cfg(feature = "threads")]
#[test]
fn acceptance_t10000_threaded() {
    const GOLDEN_PATH_THREADED: &str = "tests/golden_snapshot_t10000_threaded.txt";

    let mut w = World::new(SEED);
    let started = Instant::now();
    for _ in 0..TICKS {
        if !w.tick_once() {
            break;
        }
    }
    let elapsed_ms = started.elapsed().as_millis();

    assert!(
        w.population() > 0,
        "(a) population must be > 0 at tick {TICKS}"
    );
    assert!(
        w.live_species_count >= 2,
        "(b) live_species_count must be ≥ 2"
    );

    let hash = snapshot_hash(&w);

    // Bootstrap mode: write the golden file and skip the perf/hash assertions.
    // Run with: EVOSIM_WRITE_GOLDEN_THREADED=1 cargo test --release
    //            --features threads --test acceptance acceptance_t10000_threaded
    if std::env::var("EVOSIM_WRITE_GOLDEN_THREADED").is_ok() {
        std::fs::write(GOLDEN_PATH_THREADED, format!("{hash:#018x}\n"))
            .expect("write threaded golden");
        eprintln!(
            "wrote threaded golden snapshot: {hash:#018x}  (tick={})",
            w.tick
        );
        return;
    }

    // (c) wall-clock under budget — checked after bootstrap path so bootstrap
    // always succeeds even on slow environments (WSL2, low-core CI). On real
    // Linux CI this comfortably fits in 8 s; rayon's thread-pool overhead
    // in WSL2 can exceed the budget, but WSL2 is not a CI target.
    assert!(
        elapsed_ms < PERF_BUDGET_MS,
        "(c) threaded sim took {elapsed_ms} ms; budget {PERF_BUDGET_MS} ms",
    );

    let golden_str = std::fs::read_to_string(GOLDEN_PATH_THREADED).expect(
        "threaded golden file missing; bootstrap with: \
         EVOSIM_WRITE_GOLDEN_THREADED=1 cargo test --release \
         --features threads --test acceptance acceptance_t10000_threaded",
    );
    let golden_str = golden_str.trim();
    let golden: u64 = u64::from_str_radix(golden_str.trim_start_matches("0x"), 16)
        .expect("malformed threaded golden hash");

    assert_eq!(
        hash, golden,
        "(d) threaded snapshot hash mismatch: got {hash:#018x}, golden {golden:#018x}",
    );
}

/// S39 test (a): threaded NN forward produces a bit-identical hash to the
/// sequential NN forward at t=TICKS under the pinned seed.
///
/// In one test process: world A runs the normal threaded NN path; world B sets
/// `force_sequential_nn = true` so the threaded branch short-circuits to the
/// sequential body. A divergence in the threaded NN code causes hash_a ≠ hash_b.
/// NO golden file is read — the assertion is between two live runs, so the
/// regen ceremony cannot make this test tautological.
///
/// Only compiled under --features threads (comparing threaded vs sequential is
/// meaningless when the threaded code path is not compiled in).
#[cfg(feature = "threads")]
#[test]
fn acceptance_threaded_actually_matches_sequential_t10000() {
    // World A — threaded NN forward (production path under --features threads).
    let mut wa = World::new(SEED);
    debug_assert!(!wa.force_sequential_nn, "default toggle must be false");
    for _ in 0..TICKS {
        if !wa.tick_once() {
            break;
        }
    }
    let hash_a = snapshot_hash(&wa);

    // World B — same seed, but the threaded NN branch short-circuits to the
    // sequential body via the S39 observation-only knob.
    let mut wb = World::new(SEED);
    wb.force_sequential_nn = true;
    for _ in 0..TICKS {
        if !wb.tick_once() {
            break;
        }
    }
    let hash_b = snapshot_hash(&wb);

    assert_eq!(
        hash_a, hash_b,
        "threaded NN forward must produce a bit-identical hash to the \
         sequential NN forward at t={TICKS} under the pinned seed; \
         got threaded={hash_a:#018x}, sequential={hash_b:#018x}. \
         A divergence here means nn_forward_all_chunks's threaded branch \
         has drifted from the sequential branch; bisect src/world/nn.rs.",
    );
}
