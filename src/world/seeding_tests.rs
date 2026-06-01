//! v2.0 Wave 4 — species seeding & balance tests.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard. Covers:
//!   * Seeding determinism: same `world_seed` ⇒ identical anchors / species /
//!     founder genomes + brains; a different `world_seed` ⇒ a different layout —
//!     while the run (sim RNG) stays decoupled from `world_seed`.
//!   * Founders cluster near their species anchor.
//!   * Biome-adapted canonical genome (water anchor → high water_affinity,
//!     desert → high heat_tolerance, plains → moderate both).
//!   * `member_variance` produces measurable spread off the canonical member.
//!   * Balance smoke: a multi-thousand-tick species-mode run stays under the
//!     soft pop cap (no random-cull binding) and ≥1 species persists a while.

use super::*;
use crate::constants::{Biome, BIOME_BIAS_HIGH_LO, MAX_POP_FOR_SIM};
use crate::creature::Genome;
use crate::rng::SimRng;

/// Production-style species-mode sliders at a chosen world size + seed. Uses the
/// real species-seeding defaults (10 × 10) unless overridden by the caller.
fn species_sliders(world_size: f32, world_seed: u32) -> DevSliders {
    DevSliders {
        species_mode: true,
        world_size,
        world_seed,
        wrap_world: true,
        ..Default::default()
    }
}

/// Build a species-mode world. The string seed (sim RNG) is held constant so the
/// ONLY thing varying between determinism cases is `world_seed`.
fn species_world(world_size: f32, world_seed: u32) -> World {
    World::new_with_sliders(
        format!("seed-string-{world_seed}"),
        species_sliders(world_size, world_seed),
    )
}

// ─── Seeding determinism ────────────────────────────────────────────────────

/// Same `world_seed` ⇒ byte-identical tick-0 species layout: anchors (positions),
/// per-creature genomes, and founder brain weights all match. This is the
/// Wave-4 "seeding deterministic from `world_seed`" verification.
#[test]
fn same_world_seed_reproduces_identical_seeding() {
    // Different STRING seeds (different sim RNG) but the SAME world_seed → the
    // seeding must still match, proving seeding rides world_seed, not the sim RNG.
    let a = World::new_with_sliders("string-A", species_sliders(4800.0, 12345));
    let b = World::new_with_sliders("string-B", species_sliders(4800.0, 12345));

    assert_eq!(
        a.creatures.len(),
        b.creatures.len(),
        "same world_seed must seed the same population"
    );
    assert!(
        a.creatures.len() > 0,
        "species seeding must produce founders"
    );

    for i in 0..a.creatures.len() {
        assert_eq!(a.creatures.x[i], b.creatures.x[i], "x mismatch at {i}");
        assert_eq!(a.creatures.y[i], b.creatures.y[i], "y mismatch at {i}");
        assert_eq!(
            a.creatures.species_id[i], b.creatures.species_id[i],
            "species_id mismatch at {i}"
        );
        let ga = &a.creatures.genome[i];
        let gb = &b.creatures.genome[i];
        assert_eq!(ga, gb, "genome mismatch at founder {i}");
        assert_eq!(
            a.creatures.brains[i].weights, b.creatures.brains[i].weights,
            "brain weights mismatch at founder {i}"
        );
    }
}

/// A different `world_seed` ⇒ a different tick-0 layout (anchors and/or founder
/// genomes differ). Guards against the seeding being accidentally seed-invariant.
#[test]
fn different_world_seed_changes_seeding() {
    let a = species_world(4800.0, 1);
    let b = species_world(4800.0, 2);

    // Population is the same shape (10 × 10), but the positions / genomes differ.
    assert_eq!(a.creatures.len(), b.creatures.len());
    let mut any_pos_diff = false;
    let mut any_genome_diff = false;
    for i in 0..a.creatures.len() {
        if a.creatures.x[i] != b.creatures.x[i] || a.creatures.y[i] != b.creatures.y[i] {
            any_pos_diff = true;
        }
        if a.creatures.genome[i] != b.creatures.genome[i] {
            any_genome_diff = true;
        }
    }
    assert!(
        any_pos_diff,
        "a different world_seed must produce different anchor/founder positions"
    );
    assert!(
        any_genome_diff,
        "a different world_seed must produce different founder genomes"
    );
}

/// The sim RNG (string seed) must NOT affect seeding: same world_seed + different
/// string seeds ⇒ identical seeding (re-asserts the decoupling explicitly, the
/// other half of the two-seed guarantee).
#[test]
fn sim_rng_string_seed_does_not_affect_seeding() {
    let a = World::new_with_sliders("alpha", species_sliders(3600.0, 77));
    let b = World::new_with_sliders("omega", species_sliders(3600.0, 77));
    for i in 0..a.creatures.len() {
        assert_eq!(a.creatures.genome[i], b.creatures.genome[i]);
        assert_eq!(a.creatures.x[i], b.creatures.x[i]);
    }
}

// ─── Founder clustering ─────────────────────────────────────────────────────

/// Founders of a species cluster near their anchor: every member sits within the
/// founder-cluster radius of the species' canonical member (which spawns AT the
/// anchor). Distances are wrap-aware (toroidal world).
#[test]
fn founders_cluster_near_their_anchor() {
    let world_size = 4800.0;
    let w = species_world(world_size, 999);
    let member_count = w.sliders.starting_species_member_count as usize;
    let species_count = w.sliders.starting_species_count as usize;
    assert_eq!(w.creatures.len(), member_count * species_count);

    let cluster_radius = (world_size * FOUNDER_CLUSTER_RADIUS_FRAC).max(MOVE_SPEED_MAX * 2.0);
    let half = world_size * 0.5;

    // The canonical member is the first creature pushed per species (index s *
    // member_count), spawned AT the anchor.
    for s in 0..species_count {
        let base = s * member_count;
        let ax = w.creatures.x[base];
        let ay = w.creatures.y[base];
        for m in 0..member_count {
            let i = base + m;
            assert_eq!(
                w.creatures.species_id[i] as usize, s,
                "founder {i} should belong to species {s}"
            );
            let mut dx = (w.creatures.x[i] - ax).abs();
            let mut dy = (w.creatures.y[i] - ay).abs();
            if dx > half {
                dx = world_size - dx;
            }
            if dy > half {
                dy = world_size - dy;
            }
            let dist = (dx * dx + dy * dy).sqrt();
            assert!(
                dist <= cluster_radius * 1.5,
                "founder {i} of species {s} is {dist} from anchor (cluster radius {cluster_radius})"
            );
        }
    }
}

/// Two species anchors are spread apart: the canonical members of distinct
/// species are not all piled on top of each other (the rejection-sampled min
/// spacing produces visibly distinct patches).
#[test]
fn species_anchors_are_spread_apart() {
    let world_size = 9600.0;
    let w = species_world(world_size, 4242);
    let member_count = w.sliders.starting_species_member_count as usize;
    let species_count = w.sliders.starting_species_count as usize;
    let half = world_size * 0.5;

    let mut min_pair = f32::INFINITY;
    for s in 0..species_count {
        for t in (s + 1)..species_count {
            let (ax, ay) = (
                w.creatures.x[s * member_count],
                w.creatures.y[s * member_count],
            );
            let (bx, by) = (
                w.creatures.x[t * member_count],
                w.creatures.y[t * member_count],
            );
            let mut dx = (ax - bx).abs();
            let mut dy = (ay - by).abs();
            if dx > half {
                dx = world_size - dx;
            }
            if dy > half {
                dy = world_size - dy;
            }
            min_pair = min_pair.min((dx * dx + dy * dy).sqrt());
        }
    }
    // Closest anchor pair should be a meaningful fraction of the world, not ~0.
    assert!(
        min_pair > world_size * 0.05,
        "anchors should be spread out; closest pair only {min_pair} apart (world {world_size})"
    );
}

// ─── Biome-adapted canonical genome ─────────────────────────────────────────

/// `Genome::canonical_for_biome` biases the terrain-survival trait per biome:
/// water ⇒ high water_affinity, desert ⇒ high heat_tolerance, plains ⇒ moderate
/// both. The other four traits stay in the full [0,1] range (we just check they
/// are produced, not their exact distribution).
#[test]
fn canonical_genome_is_biome_adapted() {
    let mut rng = SimRng::from_u64(0xC0FFEE);
    // Sample many canonical genomes per biome and assert the biased trait sits in
    // the high/moderate band every time.
    for _ in 0..200 {
        let water = Genome::canonical_for_biome(&mut rng, Biome::Water);
        assert!(
            water.water_affinity >= BIOME_BIAS_HIGH_LO,
            "water anchor must seed high water_affinity; got {}",
            water.water_affinity
        );

        let desert = Genome::canonical_for_biome(&mut rng, Biome::Desert);
        assert!(
            desert.heat_tolerance >= BIOME_BIAS_HIGH_LO,
            "desert anchor must seed high heat_tolerance; got {}",
            desert.heat_tolerance
        );

        let plains = Genome::canonical_for_biome(&mut rng, Biome::Plains);
        assert!(
            (0.4..=0.7).contains(&plains.water_affinity)
                && (0.4..=0.7).contains(&plains.heat_tolerance),
            "plains anchor must seed moderate both; got wa={} ht={}",
            plains.water_affinity,
            plains.heat_tolerance
        );
    }
}

/// In a fully-seeded world, each species' canonical member (the at-anchor first
/// member) matches the biome under its anchor: if the anchor cell is Water, the
/// canonical water_affinity is high; if Desert, heat_tolerance is high.
#[test]
fn seeded_canonical_members_match_anchor_biome() {
    // A large world makes it likely several anchors land on water/desert blobs.
    let world_size = 9600.0;
    let w = species_world(world_size, 31337);
    let member_count = w.sliders.starting_species_member_count as usize;
    let species_count = w.sliders.starting_species_count as usize;

    let mut checked_water = 0;
    let mut checked_desert = 0;
    for s in 0..species_count {
        let base = s * member_count;
        let ax = w.creatures.x[base];
        let ay = w.creatures.y[base];
        let g = &w.creatures.genome[base];
        match w.biome_at(ax, ay) {
            Biome::Water => {
                assert!(
                    g.water_affinity >= BIOME_BIAS_HIGH_LO,
                    "species {s} canonical on Water must have high water_affinity; got {}",
                    g.water_affinity
                );
                checked_water += 1;
            }
            Biome::Desert => {
                assert!(
                    g.heat_tolerance >= BIOME_BIAS_HIGH_LO,
                    "species {s} canonical on Desert must have high heat_tolerance; got {}",
                    g.heat_tolerance
                );
                checked_desert += 1;
            }
            Biome::Plains => {
                assert!(
                    (0.4..=0.7).contains(&g.water_affinity)
                        && (0.4..=0.7).contains(&g.heat_tolerance),
                    "species {s} canonical on Plains must have moderate both"
                );
            }
        }
    }
    // Sanity: this seed should land at least one anchor on a non-plains biome so
    // the biased branch is actually exercised by the seeded path.
    assert!(
        checked_water + checked_desert >= 1,
        "expected ≥1 anchor on water/desert for this seed (exercise the bias path)"
    );
}

// ─── member_variance spread ─────────────────────────────────────────────────

/// `starting_species_member_variance` produces measurable internal spread off
/// the canonical member: a higher variance yields a larger genome + brain-weight
/// spread within a species than a near-zero variance.
#[test]
fn member_variance_produces_measurable_spread() {
    let world_size = 4800.0;
    let world_seed = 5;

    let spread_for = |variance: f32| -> (f32, f32) {
        let sliders = DevSliders {
            starting_species_member_variance: variance,
            starting_species_count: 1,
            starting_species_member_count: 40,
            ..species_sliders(world_size, world_seed)
        };
        let w = World::new_with_sliders("variance-test", sliders);
        let n = w.creatures.len();
        // Canonical member is index 0; measure mean abs deviation of the others.
        let g0 = &w.creatures.genome[0];
        let w0 = &w.creatures.brains[0].weights;
        let mut genome_spread = 0.0f32;
        let mut weight_spread = 0.0f32;
        for i in 1..n {
            let g = &w.creatures.genome[i];
            genome_spread += (g.body_size - g0.body_size).abs()
                + (g.diet - g0.diet).abs()
                + (g.metabolism - g0.metabolism).abs();
            let wi = &w.creatures.brains[i].weights;
            let mut ws = 0.0f32;
            for k in 0..wi.len().min(w0.len()) {
                ws += (wi[k] - w0[k]).abs();
            }
            weight_spread += ws / wi.len().max(1) as f32;
        }
        let denom = (n - 1).max(1) as f32;
        (genome_spread / denom, weight_spread / denom)
    };

    let (g_lo, w_lo) = spread_for(0.05);
    let (g_hi, w_hi) = spread_for(5.0);

    assert!(
        g_hi > g_lo,
        "higher member_variance must widen genome spread ({g_hi} vs {g_lo})"
    );
    assert!(
        w_hi > w_lo,
        "higher member_variance must widen brain-weight spread ({w_hi} vs {w_lo})"
    );
    // And the default (3.0) actually moves founders off the canonical member.
    let (g_def, _) = spread_for(STARTING_SPECIES_MEMBER_VARIANCE_DEFAULT);
    assert!(
        g_def > 0.0,
        "default member_variance must produce nonzero genome spread"
    );
}

// ─── Balance smoke ──────────────────────────────────────────────────────────

/// Drive a species-mode world for several thousand ticks and assert the
/// population never approaches the soft cap (so the random cull never binds) and
/// at least one species persists for a meaningful stretch. Uses a moderately
/// sized world so the test runs in reasonable time while exercising the real
/// seeding + mating + birth/cull path. The default-9600 trajectory is reported
/// separately in the wave writeup (too heavy for the unit-test budget).
#[test]
fn balance_smoke_pop_stays_under_cap_and_a_species_persists() {
    // 2400u world → 480² grass; full real seeding (10 × 10 = 100 founders).
    let mut w = species_world(2400.0, 20260601);
    let boot_pop = w.creatures.len();
    assert_eq!(
        boot_pop, 100,
        "default species seeding = 10 × 10 = 100 founders"
    );

    let soft_cap = w.sliders.max_population as usize;
    assert!(soft_cap <= MAX_POP_FOR_SIM);

    let ticks = 3000u32;
    let mut max_pop = boot_pop;
    let mut alive_at = 0u32;
    let mut species_alive_at_end = 0usize;
    for t in 0..ticks {
        let alive = w.step();
        let pop = w.creatures.len();
        max_pop = max_pop.max(pop);
        // The soft cap must never be reached → the random cull never binds.
        assert!(
            pop < soft_cap,
            "pop {pop} reached/exceeded the soft cap {soft_cap} at tick {t} (cull would bind)"
        );
        if alive {
            alive_at = t + 1;
        } else {
            break;
        }
    }

    // Count distinct surviving species at the end of the run.
    if !w.creatures.is_empty() {
        let mut seen = [false; 64];
        for &sid in w.creatures.species_id.iter() {
            if (sid as usize) < seen.len() {
                seen[sid as usize] = true;
            }
        }
        species_alive_at_end = seen.iter().filter(|&&b| b).count();
    }

    // The headline assertion: the cap never bound (checked in-loop). We also
    // require the sim to run a meaningful stretch without instantly dying — the
    // mechanism must demonstrably operate. (We do NOT assert long-term survival
    // numbers; balance is the user's. The wave writeup reports the trajectory.)
    assert!(
        alive_at >= 200,
        "species run died almost immediately (alive only {alive_at} ticks) — \
         mechanism not exercised"
    );
    // Surface the trajectory in the test log for the writeup.
    eprintln!(
        "balance_smoke: boot_pop={boot_pop} max_pop={max_pop} alive_ticks={alive_at} \
         end_pop={} species_alive_at_end={species_alive_at_end}",
        w.creatures.len()
    );
}

/// In-process long run at the DEFAULT 9600u world for the wave writeup. Ignored
/// by default (heavy: 1920² grass × thousands of ticks). Run explicitly with
/// `cargo test --lib --features threads default_world_long_run -- --ignored
/// --nocapture` to print the population trajectory + surviving species count.
#[test]
#[ignore = "heavy; run explicitly to print the default-world balance trajectory"]
fn default_world_long_run() {
    let mut w = World::new_with_sliders("default-long-run", species_sliders(9600.0, 20260601));
    let boot_pop = w.creatures.len();
    let soft_cap = w.sliders.max_population as usize;
    let ticks = 6000u32;
    let mut max_pop = boot_pop;
    let mut cap_bound = false;
    let mut alive_at = 0u32;
    let checkpoints = [500u32, 1000, 2000, 3000, 4000, 5000, 6000];
    for t in 1..=ticks {
        let alive = w.step();
        let pop = w.creatures.len();
        max_pop = max_pop.max(pop);
        if pop >= soft_cap {
            cap_bound = true;
        }
        if alive {
            alive_at = t;
        }
        if checkpoints.contains(&t) {
            let mut seen = [false; 64];
            for &sid in w.creatures.species_id.iter() {
                if (sid as usize) < seen.len() {
                    seen[sid as usize] = true;
                }
            }
            let species = seen.iter().filter(|&&b| b).count();
            eprintln!("  tick {t:>5}: pop={pop:>5} species_alive={species}");
        }
        if !alive {
            break;
        }
    }
    eprintln!(
        "default_world_long_run: boot_pop={boot_pop} max_pop={max_pop} \
         alive_ticks={alive_at} end_pop={} soft_cap={soft_cap} cap_bound={cap_bound}",
        w.creatures.len()
    );
}
