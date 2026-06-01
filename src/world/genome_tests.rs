//! v2.0 Wave 2a: body-genome tests — mutation determinism, trait clamp,
//! genome-modulated movement penalty, and ring-flash plumbing. Lives in its own
//! file (attached via `#[path]` in `world/mod.rs`) to avoid the shared-`mod
//! tests` merge hazard.

use super::*;
use crate::brain::{Brain, Bucket, MutationPolicy, NnTopology};
use crate::creature::{FlashTag, Genome};
use crate::rng::SimRng;

/// A founder genome is drawn uniformly in [0,1] per trait (every value in range).
#[test]
fn founder_genome_traits_in_unit_range() {
    let mut rng = SimRng::from_u64(1);
    for _ in 0..1000 {
        let g = Genome::founder(&mut rng);
        for t in [
            g.body_size,
            g.max_speed,
            g.metabolism,
            g.diet,
            g.water_affinity,
            g.heat_tolerance,
        ] {
            assert!((0.0..=1.0).contains(&t), "trait {t} out of [0,1]");
        }
    }
}

/// `Genome::mutated` clamps every trait to [0,1] even under a large sigma, and a
/// non-positive step copies the genome verbatim (no RNG draws, no drift).
#[test]
fn genome_mutated_clamps_and_zero_step_is_identity() {
    let base = Genome {
        body_size: 0.95,
        max_speed: 0.02,
        metabolism: 0.5,
        diet: 0.99,
        water_affinity: 0.01,
        heat_tolerance: 0.5,
    };
    // Zero step: identity, no RNG advance.
    let mut rng = SimRng::from_u64(7);
    let before = rng.clone();
    let same = base.mutated(&mut rng, 0.0);
    assert_eq!(same, base, "zero step must be identity");
    assert_eq!(
        before.state(),
        rng.state(),
        "zero step must not advance the RNG"
    );

    // Huge step: every trait still clamped to [0,1].
    let mut rng2 = SimRng::from_u64(9);
    for _ in 0..2000 {
        let child = base.mutated(&mut rng2, 5.0);
        for t in [
            child.body_size,
            child.max_speed,
            child.metabolism,
            child.diet,
            child.water_affinity,
            child.heat_tolerance,
        ] {
            assert!((0.0..=1.0).contains(&t), "clamp failed: {t}");
        }
    }
}

/// Determinism: the same parent genome + same seed + same step produces the
/// SAME child genome, bit-for-bit, every time.
#[test]
fn genome_mutation_is_deterministic_for_same_seed() {
    let parent = Genome {
        body_size: 0.3,
        max_speed: 0.6,
        metabolism: 0.4,
        diet: 0.7,
        water_affinity: 0.2,
        heat_tolerance: 0.8,
    };
    let run = || {
        let mut rng = SimRng::from_u64(12345);
        let mut g = parent;
        for _ in 0..50 {
            g = g.mutated(&mut rng, 0.2);
        }
        g
    };
    let a = run();
    let b = run();
    assert_eq!(a, b, "same seed must produce identical genome drift");
}

/// Determinism at the WORLD level: two worlds with the same string seed run for
/// N ticks (with births) end up with identical genome populations.
#[test]
fn world_genome_population_deterministic_for_same_seed() {
    fn run() -> Vec<Genome> {
        let mut w = World::new_with_sliders("genome-det", DevSliders::default());
        // Encourage some splitting by making energy cheap to accumulate.
        for _ in 0..120 {
            w.tick_once();
        }
        w.creatures.genome.clone()
    }
    let a = run();
    let b = run();
    assert_eq!(a.len(), b.len(), "same seed → same population size");
    assert_eq!(a, b, "same seed → identical genome population");
}

/// The genome reuses the SAME per-birth bucket draw as the brain. With a single
/// non-zero bucket and a positive trait multiplier, a child's genome drifts off
/// its parent; with the trait multiplier at 0, the genome is copied verbatim
/// even though the brain still mutates.
#[test]
fn genome_drift_requires_positive_trait_multiplier() {
    let mut rng = SimRng::from_u64(3);
    let parent_brain = Brain::founder(&mut rng, NnTopology::legacy());
    let parent_genome = Genome {
        body_size: 0.5,
        max_speed: 0.5,
        metabolism: 0.5,
        diet: 0.5,
        water_affinity: 0.5,
        heat_tolerance: 0.5,
    };
    // One always-fire bucket with non-zero sigma.
    let mut policy = MutationPolicy {
        buckets: [Bucket::zero(); crate::constants::MUTATION_BUCKET_COUNT],
    };
    policy.buckets[0] = Bucket {
        weight: 1.0,
        rate: 1.0,
        sigma: 0.2,
    };

    // trait multiplier 0 → genome unchanged (brain still mutates off same draw).
    {
        let mut r = SimRng::from_u64(42);
        let (child_brain, sigma) =
            Brain::child_from_with_sigma(&parent_brain, &mut r, &policy, 1.0);
        let child_genome = parent_genome.mutated(&mut r, sigma * 0.0);
        assert_ne!(
            child_brain.weights, parent_brain.weights,
            "brain should mutate"
        );
        assert_eq!(
            child_genome, parent_genome,
            "trait multiplier 0 → genome verbatim"
        );
    }
    // trait multiplier 0.3 → genome drifts.
    {
        let mut r = SimRng::from_u64(42);
        let (_child_brain, sigma) =
            Brain::child_from_with_sigma(&parent_brain, &mut r, &policy, 1.0);
        let child_genome = parent_genome.mutated(&mut r, sigma * 0.3);
        assert_ne!(
            child_genome, parent_genome,
            "positive trait multiplier → genome drift"
        );
    }
}

/// A high-water_affinity creature pays a LOWER water-biome movement penalty than
/// a low-affinity one standing on the same cell.
#[test]
fn water_affinity_reduces_water_penalty() {
    // Build a walled world and force a Water cell under the creature.
    let mut w = World::new("genome-water-pen");
    // Find/seed a Water cell at the creature's position by overwriting the biome
    // grid byte under it.
    let i = 0usize;
    let idx = w.biome_cell_index(w.creatures.x[i], w.creatures.y[i]);
    w.biome_grid[idx] = Biome::Water as u8;
    w.sliders.water_movement_penalty = 0.8;

    w.creatures.genome[i].water_affinity = 0.0;
    let p_low = w.movement_penalty_for(i);
    w.creatures.genome[i].water_affinity = 0.75;
    let p_high = w.movement_penalty_for(i);

    assert!(p_low > p_high, "p_low={p_low} must exceed p_high={p_high}");
    assert!((p_low - 0.8).abs() < 1e-5, "affinity 0 → full 0.8; got {p_low}");
    assert!((p_high - 0.2).abs() < 1e-5, "affinity 0.75 → 0.8*0.25=0.2; got {p_high}");
}

/// A split fires a blue "CreatedChild" flash on the parent and a teal "Born"
/// flash on the newborn.
#[test]
fn split_flashes_parent_blue_and_child_teal() {
    let mut w = World::new("genome-split-flash");
    w.creatures.energy[0] = SPLIT_THRESHOLD_DEFAULT + 1000.0;
    w.creatures.action_this_tick[0] = Action::Split;
    let n_before = w.creatures.len();
    w.handle_births();
    assert!(w.creatures.len() > n_before, "a child must be born");
    assert_eq!(
        w.creatures.flash_tag[0],
        FlashTag::CreatedChild,
        "parent flashes blue (CreatedChild)"
    );
    // The newest creature (last index) is the child.
    let child = w.creatures.len() - 1;
    assert_eq!(
        w.creatures.flash_tag[child],
        FlashTag::Born,
        "child flashes teal (Born)"
    );
}
