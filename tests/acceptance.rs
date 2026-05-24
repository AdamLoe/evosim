//! v5 §16 / v6 §M acceptance test. Pinned seed, 10k ticks, perf gate,
//! golden snapshot hash. Run with `cargo test --test acceptance`.
//!
//! Bootstrap: `EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance`
//! writes `tests/golden_snapshot_t10000.txt`. Subsequent runs assert against it.

use evosim::snapshot_hash::snapshot_hash;
use evosim::world::World;
use std::time::Instant;

const SEED: &str = "evosim-test-001";
const TICKS: u32 = 10_000;
const PERF_BUDGET_MS: u128 = 8_000;
const GOLDEN_PATH: &str = "tests/golden_snapshot_t10000.txt";

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
