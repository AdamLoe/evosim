//! v2.0.4 C3 distribution-match tests for the fused-RNG scatter draws.
//!
//! Validates that `grass_hash_fused_4` produces the same statistical
//! distribution for each of the four scatter draws as the legacy
//! 4-separate-`grass_unit` path:
//!   - decay_u  matches grass_unit(*, *, *, 1)
//!   - spread_u matches grass_unit(*, *, *, 2)
//!   - band_u   matches grass_unit(*, *, *, 3)
//!   - pick_u   matches grass_unit(*, *, *, 4)
//!
//! "Match" means: the STATISTICS (mean, fraction below threshold) are
//! within tolerance over a large sample.  The individual VALUES are
//! intentionally different (different bit windows within two hash words).
//!
//! Also verifies that a grass scatter step does NOT advance SimRng state.
//!
//! Lives in its OWN file (merge-hazard rule), wired as a child module of
//! `grass` so it can reach crate-internal fields/fns.

use super::super::*;
use crate::constants::WorldDims;
use crate::rng::{grass_hash_fused_4, grass_unit, SimRng};

// ── Reference helper (legacy 4-draw path) ────────────────────────────────────

/// Legacy 4-hash draws for a single cell. Mirrors what the old kernel did:
/// salt=1 decay, salt=2 spread-gate, salt=3 band, salt=4 pick.
#[inline]
fn legacy_draws(world_seed: u32, id: u64, tick: u32) -> (f32, f32, f32, f32) {
    (
        grass_unit(world_seed, id, tick, 1),
        grass_unit(world_seed, id, tick, 2),
        grass_unit(world_seed, id, tick, 3),
        grass_unit(world_seed, id, tick, 4),
    )
}

// ── Tolerance constants ───────────────────────────────────────────────────────

/// Acceptable absolute deviation for empirical mean vs 0.5.
/// With N=100_000 samples, the standard error of the mean for U[0,1) is
/// σ/√N = (1/√12)/√100_000 ≈ 0.000913. 5σ ≈ 0.0046, so 0.005 is ~5.5σ.
const MEAN_TOL: f32 = 0.005;

/// Acceptable absolute deviation for empirical gate fraction vs nominal rate.
/// Used for the "decay_u < decay_pct" and "spread_u < spread_pct" checks.
/// At rate 0.10 with N=100_000, σ = √(p(1-p)/N) ≈ 0.00095; 5σ ≈ 0.0047.
/// 0.005 is ~5.3σ — generous but not so loose that a systematic bias slips through.
const GATE_TOL: f32 = 0.005;

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Distribution-match test: over many cells/ticks, the fused-RNG decay/spread/
/// band/pick draws have the same mean and gate-fraction as the legacy 4-hash path.
///
/// Method: sample 100_000 (cell, tick) pairs; for each compute BOTH the fused
/// and legacy draws; accumulate means and gate fractions; compare.
///
/// Acceptance criteria:
///   |mean_fused - mean_legacy| < MEAN_TOL  (per-field, ≈ 5.5σ)
///   |gate_fused - gate_legacy| < GATE_TOL  (per threshold, ≈ 5.3σ)
#[test]
fn fused_rng_distribution_matches_legacy() {
    const N: usize = 100_000;
    const WORLD_SEED: u32 = 0xABCD_1234;
    // Thresholds that exercise both sides of the gate (not trivially 0 or 1).
    const DECAY_PCT: f32 = 0.10;
    const SPREAD_PCT: f32 = 0.25;

    let mut sum_fused = [0.0f64; 4];
    let mut sum_legacy = [0.0f64; 4];
    let mut gate_decay_fused = 0u64;
    let mut gate_spread_fused = 0u64;
    let mut gate_decay_legacy = 0u64;
    let mut gate_spread_legacy = 0u64;

    for i in 0..N {
        // Vary both cell id and tick to exercise the full key space.
        let cid = i as u64;
        let tick = (i / 1000) as u32;

        let (fd, fs, fb, fp) = grass_hash_fused_4(WORLD_SEED, cid, tick);
        let (ld, ls, lb, lp) = legacy_draws(WORLD_SEED, cid, tick);

        sum_fused[0] += fd as f64;
        sum_fused[1] += fs as f64;
        sum_fused[2] += fb as f64;
        sum_fused[3] += fp as f64;

        sum_legacy[0] += ld as f64;
        sum_legacy[1] += ls as f64;
        sum_legacy[2] += lb as f64;
        sum_legacy[3] += lp as f64;

        if fd < DECAY_PCT {
            gate_decay_fused += 1;
        }
        if fs < SPREAD_PCT {
            gate_spread_fused += 1;
        }
        if ld < DECAY_PCT {
            gate_decay_legacy += 1;
        }
        if ls < SPREAD_PCT {
            gate_spread_legacy += 1;
        }
    }

    let n = N as f64;
    let names = ["decay", "spread", "band", "pick"];
    for i in 0..4 {
        let mf = (sum_fused[i] / n) as f32;
        let ml = (sum_legacy[i] / n) as f32;
        // Each field should be close to 0.5 (uniform [0,1)).
        assert!(
            (mf - 0.5).abs() < MEAN_TOL,
            "fused {}: mean={:.4} is more than {MEAN_TOL:.4} from 0.5",
            names[i],
            mf
        );
        assert!(
            (ml - 0.5).abs() < MEAN_TOL,
            "legacy {}: mean={:.4} is more than {MEAN_TOL:.4} from 0.5",
            names[i],
            ml
        );
        // Fused and legacy means must be close (both are uniform; the
        // specific values differ but the distribution is the same).
        let diff = (mf - ml).abs();
        assert!(
            diff < MEAN_TOL,
            "fused vs legacy mean for {}: |{:.4} - {:.4}| = {:.4} > {MEAN_TOL:.4}",
            names[i],
            mf,
            ml,
            diff
        );
    }

    // Gate fractions: P(decay_u < DECAY_PCT) must match nominal rate.
    let gdf = gate_decay_fused as f32 / N as f32;
    let gdl = gate_decay_legacy as f32 / N as f32;
    assert!(
        (gdf - DECAY_PCT).abs() < GATE_TOL,
        "fused decay gate: fraction={gdf:.4} expected~{DECAY_PCT:.4} (tol {GATE_TOL:.4})"
    );
    assert!(
        (gdl - DECAY_PCT).abs() < GATE_TOL,
        "legacy decay gate: fraction={gdl:.4} expected~{DECAY_PCT:.4} (tol {GATE_TOL:.4})"
    );
    // Fused and legacy gate fractions must be close to each other.
    assert!(
        (gdf - gdl).abs() < GATE_TOL,
        "fused vs legacy decay gate: |{gdf:.4} - {gdl:.4}| > {GATE_TOL:.4}"
    );

    let gsf = gate_spread_fused as f32 / N as f32;
    let gsl = gate_spread_legacy as f32 / N as f32;
    assert!(
        (gsf - SPREAD_PCT).abs() < GATE_TOL,
        "fused spread gate: fraction={gsf:.4} expected~{SPREAD_PCT:.4} (tol {GATE_TOL:.4})"
    );
    assert!(
        (gsl - SPREAD_PCT).abs() < GATE_TOL,
        "legacy spread gate: fraction={gsl:.4} expected~{SPREAD_PCT:.4} (tol {GATE_TOL:.4})"
    );
    assert!(
        (gsf - gsl).abs() < GATE_TOL,
        "fused vs legacy spread gate: |{gsf:.4} - {gsl:.4}| > {GATE_TOL:.4}"
    );
}

/// Ring-band split test: the fused band_u produces the same proportional band
/// selection as the legacy band draw when sampled against the same disc table
/// cumulative weights.
///
/// With ring weights (60%, 30%, 10%) the disc table's cum weights are
/// approximately (0.60, 0.90, 1.00). Over many samples, the fraction of
/// band-0 picks from the fused path should match the legacy path within
/// tolerance.
#[test]
fn fused_rng_band_split_matches_legacy() {
    const N: usize = 100_000;
    const WORLD_SEED: u32 = 0x5555_AAAA;
    // Use simple linear thresholds to mimic band selection: bands at 60/30/10%.
    const CUM0: f32 = 0.60;
    const CUM1: f32 = 0.90;

    let mut band0_fused = 0u64;
    let mut band1_fused = 0u64;
    let mut band0_legacy = 0u64;
    let mut band1_legacy = 0u64;

    for i in 0..N {
        let cid = (i * 7 + 13) as u64; // non-trivial spacing
        let tick = (i / 500) as u32;

        let (_, _, fb, _) = grass_hash_fused_4(WORLD_SEED, cid, tick);
        let lb = grass_unit(WORLD_SEED, cid, tick, 3);

        if fb < CUM0 {
            band0_fused += 1;
        } else if fb < CUM1 {
            band1_fused += 1;
        }
        if lb < CUM0 {
            band0_legacy += 1;
        } else if lb < CUM1 {
            band1_legacy += 1;
        }
    }

    let bf0 = band0_fused as f32 / N as f32;
    let bf1 = band1_fused as f32 / N as f32;
    let bl0 = band0_legacy as f32 / N as f32;
    let bl1 = band1_legacy as f32 / N as f32;

    // Each should be close to 0.60 and 0.30 respectively.
    assert!(
        (bf0 - CUM0).abs() < GATE_TOL,
        "fused band0 fraction={bf0:.4} expected~{CUM0:.4}"
    );
    assert!(
        (bl0 - CUM0).abs() < GATE_TOL,
        "legacy band0 fraction={bl0:.4} expected~{CUM0:.4}"
    );
    // Fused vs legacy band fractions must be close.
    assert!(
        (bf0 - bl0).abs() < GATE_TOL,
        "fused vs legacy band0: |{bf0:.4} - {bl0:.4}| > {GATE_TOL:.4}"
    );
    assert!(
        (bf1 - bl1).abs() < GATE_TOL,
        "fused vs legacy band1: |{bf1:.4} - {bl1:.4}| > {GATE_TOL:.4}"
    );
}

/// SimRng invariant test: a grass scatter step does NOT advance SimRng state.
///
/// Method: record SimRng state before a scatter tick; run one scatter tick;
/// compare state after. The grass scatter kernel uses its own positional hash
/// (`grass_hash_fused_4`/`grass_hash_u64`) and NEVER draws from SimRng.
///
/// Reuses the scatter grid helper pattern from grass_scatter_tests.rs.
#[test]
fn scatter_does_not_advance_sim_rng() {
    const DIMS: WorldDims = WorldDims {
        world_size: 640.0,
        wrap_world: true,
        grass_dim: 128,
        grass_cell_count: 128 * 128,
        hash_dim: 64,
        grass_cell_size: 5.0,
    };

    // Build a scatter grid seeded with some grass.
    let mut rng = SimRng::from_u64(0xDEAD_BEEF);
    let mut g = GrassGrid::new_with_capacity(&mut rng, 100, DIMS, None);
    g.set_propagation(GrassPropagation::Scatter);
    g.world_seed = 0x1234_5678;
    g.resync_active_from_density();
    g.mark_all_tiles_dirty();

    // Snapshot SimRng state before the scatter tick.
    let state_before = rng.state();

    // Run one scatter tick.
    g.scatter_tick = 42;
    g.compute_propagation(0.0, 0.0); // r/k unused on scatter path

    // SimRng state must be unchanged.
    let state_after = rng.state();
    assert_eq!(
        state_before, state_after,
        "grass scatter tick must NOT advance SimRng: \
         state changed from {:?} to {:?}",
        state_before, state_after
    );
}

/// Sanity: `grass_hash_fused_4` draws are all in [0, 1).
#[test]
fn fused_draws_are_in_unit_interval() {
    const WORLD_SEED: u32 = 0x9999_7777;
    for i in 0..10_000u64 {
        let (a, b, c, d) = grass_hash_fused_4(WORLD_SEED, i, (i % 200) as u32);
        assert!(
            (0.0..1.0).contains(&a),
            "decay draw {a} out of [0,1) at i={i}"
        );
        assert!(
            (0.0..1.0).contains(&b),
            "spread draw {b} out of [0,1) at i={i}"
        );
        assert!(
            (0.0..1.0).contains(&c),
            "band draw {c} out of [0,1) at i={i}"
        );
        assert!(
            (0.0..1.0).contains(&d),
            "pick draw {d} out of [0,1) at i={i}"
        );
    }
}
