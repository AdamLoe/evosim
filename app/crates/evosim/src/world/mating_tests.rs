//! v2.0 Wave 3a — species + sexual-mating tests.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard. Covers:
//!   * Mate fires only for a same-species creature within the contact radius
//!     (NOT the 20u perception range), and not for a different species / a
//!     too-far same-species creature.
//!   * Attack refuses same-species targets in species mode but hits any creature
//!     in single-pool mode.
//!   * Crossover (`average` vs `fifty_fifty`) produces children whose per-slot
//!     stats match the expected distribution (midpoint vs 50/50 pick).
//!   * Initiator-only cost + cooldown (partner unaffected).
//!   * Same-seed determinism for a species-mode run.

use super::*;
use crate::brain::{Brain, Bucket, MutationPolicy, NnTopology};
use crate::constants::{CrossoverMode, MATING_CONTACT_RADIUS_FACTOR};
use crate::rng::SimRng;

/// A small walled species-mode world with no auto-seeding (we place creatures
/// by hand). `founder_count=1` keeps the auto path tiny; we then overwrite /
/// append the creatures the test needs. Species-mode seeding would scatter many
/// founders, so for the targeted gate tests we instead build a 1-creature world
/// and push partners explicitly.
fn species_world(seed: &str) -> World {
    // Disable species auto-seeding to a single species/founder for a controlled
    // setup: 1 species × 1 founder.
    let sliders = DevSliders {
        species_mode: true,
        starting_species_count: 1,
        starting_species_member_count: 1,
        world_size: 1200.0,
        wrap_world: false,
        ..Default::default()
    };
    World::new_with_sliders(seed, sliders)
}

fn push_creature(w: &mut World, id: u64, x: f32, y: f32, energy: f32, species_id: u16) -> usize {
    let mut rng = SimRng::from_u64(id.wrapping_mul(2654435761));
    let brain = Brain::founder(&mut rng, w.nn_topology.clone());
    // v2.1 P2: hue derived from id.
    let hue = ((id % 100) as f32) / 100.0;
    w.creatures
        .push(id, x, y, energy, 0, brain, hue, species_id)
}

// ─── Mating: same-species within contact radius ────────────────────────────

/// `Mate` (action[2] in species mode) fires for a same-species partner that is
/// within contact range, spawning a child at the parent midpoint.
#[test]
fn mate_fires_for_same_species_within_contact() {
    let mut w = species_world("mate-same-species");
    // One species (0). Reset to two hand-placed creatures of species 0.
    w.creatures = CreatureSoA::with_capacity(4);
    w.next_creature_id = 0;
    let i = push_creature(&mut w, 0, 100.0, 100.0, 200.0, 0);
    // Partner overlapping bodies (radius ~1.0 each; contact factor 1.5 → ~3.0u).
    let _j = push_creature(&mut w, 1, 101.0, 100.0, 200.0, 0);
    w.creatures.action_this_tick[i] = Action::Split; // = Mate in species mode
    w.creatures.mating_cooldown[i] = 0;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let n_before = w.creatures.len();
    w.handle_mating();
    assert_eq!(
        w.creatures.len(),
        n_before + 1,
        "a same-species contact mate must produce exactly one child"
    );
    // Child carries the parents' shared species id (0) + spawns near the midpoint.
    let child = w.creatures.len() - 1;
    assert_eq!(w.creatures.species_id[child], 0, "child inherits species 0");
    let cx = w.creatures.x[child];
    assert!(
        (cx - 100.5).abs() < 1e-3,
        "child x should be the parent midpoint ~100.5; got {cx}"
    );
}

/// `Mate` does NOT fire for a DIFFERENT-species creature in contact.
#[test]
fn mate_refuses_different_species() {
    let mut w = species_world("mate-diff-species");
    w.creatures = CreatureSoA::with_capacity(4);
    w.next_creature_id = 0;
    let i = push_creature(&mut w, 0, 100.0, 100.0, 200.0, 0);
    // Partner in contact but species 1 (different).
    let _j = push_creature(&mut w, 1, 101.0, 100.0, 200.0, 1);
    w.creatures.action_this_tick[i] = Action::Split;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let n_before = w.creatures.len();
    w.handle_mating();
    assert_eq!(
        w.creatures.len(),
        n_before,
        "a different-species creature in contact must NOT trigger mating"
    );
}

/// `Mate` does NOT fire for a same-species creature that is in PERCEPTION range
/// (20u) but well outside the small contact radius (~3u).
#[test]
fn mate_refuses_same_species_beyond_contact() {
    let mut w = species_world("mate-far-same-species");
    w.creatures = CreatureSoA::with_capacity(4);
    w.next_creature_id = 0;
    let i = push_creature(&mut w, 0, 100.0, 100.0, 200.0, 0);
    // Same species but 15u away — inside the 20u perception range, far outside
    // the ~3u contact radius.
    let _j = push_creature(&mut w, 1, 115.0, 100.0, 200.0, 0);
    w.creatures.action_this_tick[i] = Action::Split;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let n_before = w.creatures.len();
    w.handle_mating();
    assert_eq!(
        w.creatures.len(),
        n_before,
        "a same-species creature beyond contact (but within perception) must NOT mate"
    );
    // Sanity: the contact radius really is far below 15u.
    let contact = (1.0 + 1.0) * MATING_CONTACT_RADIUS_FACTOR;
    assert!(
        contact < 15.0,
        "contact radius {contact} must be well below 15u"
    );
}

/// `mate_reach_multiplier` widens the accepted contact radius: a same-species
/// partner that is OUT of reach at the default 1.0 becomes reachable when the
/// multiplier is raised, so mating fires.
#[test]
fn mate_reach_multiplier_extends_contact() {
    // Partner at 10u — outside the default ~3u contact radius but well within
    // 10× that (~30u).
    let make = |mult: f32| {
        let mut w = species_world("mate-reach");
        w.creatures = CreatureSoA::with_capacity(4);
        w.next_creature_id = 0;
        let i = push_creature(&mut w, 0, 100.0, 100.0, 200.0, 0);
        let _j = push_creature(&mut w, 1, 110.0, 100.0, 200.0, 0);
        w.creatures.action_this_tick[i] = Action::Split;
        w.sliders.mate_reach_multiplier = mult;
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);
        let n_before = w.creatures.len();
        w.handle_mating();
        w.creatures.len() - n_before
    };
    assert_eq!(make(1.0), 0, "at 1.0× a 10u-away partner is out of contact");
    assert_eq!(make(10.0), 1, "at 10.0× the same 10u partner is reachable");
}

// ─── Initiator-only cost + cooldown ────────────────────────────────────────

/// All cost + cooldown fall on the initiator only; the partner pays nothing,
/// needs no energy, and is not required to be off cooldown.
#[test]
fn mating_cost_and_cooldown_hit_initiator_only() {
    let mut w = species_world("mate-initiator-only");
    w.creatures = CreatureSoA::with_capacity(4);
    w.next_creature_id = 0;
    let i = push_creature(&mut w, 0, 100.0, 100.0, 200.0, 0);
    let j = push_creature(&mut w, 1, 101.0, 100.0, 50.0, 0); // partner low energy
                                                             // Partner is even ON cooldown — must not matter (it's just a target).
    w.creatures.mating_cooldown[j] = 99;
    let partner_energy_before = w.creatures.energy[j];
    let partner_cd_before = w.creatures.mating_cooldown[j];
    w.creatures.action_this_tick[i] = Action::Split;
    w.sliders.split_gift = 30.0; // mating cost
    w.sliders.mating_cooldown_ticks = 200;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let init_energy_before = w.creatures.energy[i];
    w.handle_mating();

    // Initiator paid the mating cost (= split_gift) and entered the cooldown.
    assert!(
        (init_energy_before - w.creatures.energy[i] - 30.0).abs() < 1e-3,
        "initiator must pay the mating cost (30); paid {}",
        init_energy_before - w.creatures.energy[i]
    );
    assert_eq!(
        w.creatures.mating_cooldown[i], 200,
        "initiator must enter the mating cooldown"
    );
    // Partner untouched: energy + its own (pre-existing) cooldown unchanged.
    assert!(
        (w.creatures.energy[j] - partner_energy_before).abs() < 1e-6,
        "partner energy must be unchanged"
    );
    assert_eq!(
        w.creatures.mating_cooldown[j], partner_cd_before,
        "partner cooldown must be unchanged"
    );
}

/// An initiator ON mating cooldown does NOT mate (the gate is re-checked in the
/// mating path).
#[test]
fn mating_blocked_while_initiator_on_cooldown() {
    let mut w = species_world("mate-on-cooldown");
    w.creatures = CreatureSoA::with_capacity(4);
    w.next_creature_id = 0;
    let i = push_creature(&mut w, 0, 100.0, 100.0, 200.0, 0);
    let _j = push_creature(&mut w, 1, 101.0, 100.0, 200.0, 0);
    w.creatures.action_this_tick[i] = Action::Split;
    w.creatures.mating_cooldown[i] = 5; // initiator still cooling down
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let n_before = w.creatures.len();
    w.handle_mating();
    assert_eq!(
        w.creatures.len(),
        n_before,
        "an initiator on cooldown must not mate"
    );
}

// ─── Attack same-species gate ──────────────────────────────────────────────

/// In species mode, `Attack` refuses a same-species target.
#[test]
fn attack_refuses_same_species_in_species_mode() {
    let mut w = species_world("attack-same-species");
    w.creatures = CreatureSoA::with_capacity(4);
    w.next_creature_id = 0;
    let i = push_creature(&mut w, 0, 100.0, 100.0, 100.0, 0);
    let j = push_creature(&mut w, 1, 101.5, 100.0, 100.0, 0); // same species, in reach
    w.creatures.action_this_tick[i] = Action::Attack;
    w.sliders.eat_bite_fraction = 0.5;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let victim_before = w.creatures.energy[j];
    w.attack();
    assert!(
        (w.creatures.energy[j] - victim_before).abs() < 1e-6,
        "same-species victim must be unharmed in species mode"
    );
}

/// In species mode, `Attack` DOES hit a different-species target.
#[test]
fn attack_hits_other_species_in_species_mode() {
    let mut w = species_world("attack-other-species");
    w.creatures = CreatureSoA::with_capacity(4);
    w.next_creature_id = 0;
    let i = push_creature(&mut w, 0, 100.0, 100.0, 100.0, 0);
    let j = push_creature(&mut w, 1, 101.5, 100.0, 100.0, 1); // DIFFERENT species
    w.creatures.action_this_tick[i] = Action::Attack;
    w.sliders.eat_bite_fraction = 0.5;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let victim_before = w.creatures.energy[j];
    w.attack();
    assert!(
        w.creatures.energy[j] < victim_before - 1e-3,
        "other-species victim must take damage in species mode"
    );
}

/// In single-pool mode, `Attack` hits ANY creature (today's predation): even a
/// same-`species_id` (=0) neighbor, since species don't exist.
#[test]
fn attack_hits_any_in_single_pool_mode() {
    // Single-pool walled world (species_mode off).
    let mut w = World::new("attack-single-pool");
    w.creatures.energy[0] = 100.0;
    w.creatures.action_this_tick[0] = Action::Attack;
    w.sliders.eat_bite_fraction = 0.5;
    let px = w.creatures.x[0];
    let py = w.creatures.y[0];
    // Victim with the default species_id (0), same as the attacker's.
    let j = push_creature(&mut w, 1, px + 1.5, py, 100.0, 0);
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    let victim_before = w.creatures.energy[j];
    w.attack();
    assert!(
        w.creatures.energy[j] < victim_before - 1e-3,
        "single-pool Attack must hit any creature regardless of species_id"
    );
}

// ─── Crossover distributions: average vs fifty_fifty ───────────────────────

fn distinct_parents() -> (Brain, Brain) {
    let mut rng = SimRng::from_u64(101);
    let pa_brain = Brain::founder(&mut rng, NnTopology::legacy());
    // Parent B = a clearly-different brain (every weight offset).
    let mut pb_brain = pa_brain.clone();
    for w in pb_brain.weights.iter_mut() {
        *w += 10.0;
    }
    (pa_brain, pb_brain)
}

/// `average` crossover: every brain weight is the exact midpoint of the two
/// parents (with mutation OFF so the midpoint is observable).
#[test]
fn crossover_average_is_midpoint_brain() {
    let (pa, pb) = distinct_parents();
    // No-mutation policy (bucket 0 weight=1, rate=0) so the crossover is clean.
    let policy = MutationPolicy {
        buckets: {
            let mut b = [Bucket::zero(); crate::constants::MUTATION_BUCKET_COUNT];
            b[0] = Bucket {
                weight: 1.0,
                rate: 0.0,
                sigma: 0.0,
            };
            b
        },
    };
    let mut rng = SimRng::from_u64(7);
    let (child_brain, _sigma) = Brain::child_from_crossover_with_sigma(
        &pa,
        &pb,
        &mut rng,
        &policy,
        1.0,
        CrossoverMode::Average,
    );
    for k in 0..child_brain.weights.len() {
        let expected = 0.5 * (pa.weights[k] + pb.weights[k]);
        assert!(
            (child_brain.weights[k] - expected).abs() < 1e-4,
            "average weight {k}: {} != midpoint {expected}",
            child_brain.weights[k]
        );
    }
}

/// `fifty_fifty` crossover: every brain weight equals ONE of the two parents
/// (never a blend), and across many slots BOTH parents contribute (≈50/50).
/// Mutation OFF.
#[test]
fn crossover_fifty_fifty_picks_one_parent_per_slot() {
    let (pa, pb) = distinct_parents();
    let policy = MutationPolicy {
        buckets: {
            let mut b = [Bucket::zero(); crate::constants::MUTATION_BUCKET_COUNT];
            b[0] = Bucket {
                weight: 1.0,
                rate: 0.0,
                sigma: 0.0,
            };
            b
        },
    };
    let mut rng = SimRng::from_u64(13);
    let (child, _sigma) = Brain::child_from_crossover_with_sigma(
        &pa,
        &pb,
        &mut rng,
        &policy,
        1.0,
        CrossoverMode::FiftyFifty,
    );
    let mut from_a = 0usize;
    let mut from_b = 0usize;
    for k in 0..child.weights.len() {
        let is_a = (child.weights[k] - pa.weights[k]).abs() < 1e-6;
        let is_b = (child.weights[k] - pb.weights[k]).abs() < 1e-6;
        assert!(
            is_a || is_b,
            "fifty_fifty weight {k}={} must equal a parent (A={} B={})",
            child.weights[k],
            pa.weights[k],
            pb.weights[k]
        );
        if is_a {
            from_a += 1;
        } else {
            from_b += 1;
        }
    }
    let total = child.weights.len();
    // Both parents contribute a meaningful share (loose 30–70% band).
    assert!(
        from_a > total * 3 / 10 && from_b > total * 3 / 10,
        "fifty_fifty must mix both parents: A={from_a} B={from_b} of {total}"
    );
}

// ─── Same-seed determinism for a species-mode run ──────────────────────────

/// Two species-mode worlds with the same string seed run for N ticks and end up
/// with identical populations (positions, species_ids, hues).
#[test]
fn species_mode_run_is_deterministic_for_same_seed() {
    fn run() -> (Vec<f32>, Vec<u16>, Vec<f32>) {
        let sliders = DevSliders {
            species_mode: true,
            starting_species_count: 4,
            starting_species_member_count: 8,
            world_size: 1200.0,
            wrap_world: true,
            ..Default::default()
        };
        let mut w = World::new_with_sliders("species-det", sliders);
        for _ in 0..150 {
            w.tick_once();
        }
        (
            w.creatures.x.clone(),
            w.creatures.species_id.clone(),
            w.creatures.hue.clone(),
        )
    }
    let a = run();
    let b = run();

    // Single-threaded (default feature set): exact bit-for-bit determinism holds
    // because the per-cell hash RNG is reproducible and no concurrent scatter
    // collisions occur.
    #[cfg(not(feature = "threads"))]
    {
        assert_eq!(a.0.len(), b.0.len(), "same seed → same population size");
        assert_eq!(a.0, b.0, "same seed → identical positions");
        assert_eq!(a.1, b.1, "same seed → identical species ids");
        assert_eq!(a.2, b.2, "same seed → identical hues");
    }

    // Threaded (--features threads): the u8 scatter grass kernel is intentionally
    // non-reproducible across threads. It uses lossy cross-tile read-modify-write
    // (scatter_add/scatter_sub: relaxed load → clamp → store), so concurrent
    // thread collisions clobber each other non-deterministically. Creatures sense
    // the grass density field, so their decisions — and thus births/deaths — diverge
    // between same-seed runs. This is an accepted design property (intentional fuzz;
    // bit-reproducibility across threads is not required; see scatter kernel docs).
    // We assert only liveness/sanity: both runs complete without panic, populations
    // are non-empty (at least one species survives), and sizes are within a plausible
    // band of each other. Observed run-to-run variation under rayon: population delta
    // ≤ ~7 (starting at 32 creatures, 150 ticks); we use 12 as a stable bound.
    #[cfg(feature = "threads")]
    {
        assert!(
            !a.0.is_empty(),
            "threaded run A must produce a non-empty population (at least one species survives)"
        );
        assert!(
            !b.0.is_empty(),
            "threaded run B must produce a non-empty population (at least one species survives)"
        );
        let delta = (a.0.len() as isize - b.0.len() as isize).unsigned_abs();
        assert!(
            delta <= 12,
            "threaded same-seed runs: population sizes are too far apart \
             ({} vs {}; delta {delta}); scatter fuzz should not cause a delta > 12",
            a.0.len(),
            b.0.len()
        );
        // At least one species must persist in each run.
        assert!(
            a.1.iter().any(|_| true),
            "threaded run A must have at least one species_id"
        );
        assert!(
            b.1.iter().any(|_| true),
            "threaded run B must have at least one species_id"
        );
    }
}

/// Species-mode seeding assigns the configured number of species + founders and
/// every creature carries a valid species_id.
#[test]
fn species_mode_seeds_expected_population() {
    let sliders = DevSliders {
        species_mode: true,
        starting_species_count: 5,
        starting_species_member_count: 6,
        world_size: 2000.0,
        wrap_world: true,
        ..Default::default()
    };
    let w = World::new_with_sliders("species-seed", sliders);
    assert_eq!(w.population(), 30, "5 species × 6 founders = 30");
    assert_eq!(w.species.len(), 5, "registry seeded with 5 species");
    for i in 0..w.creatures.len() {
        let id = w.creatures.species_id[i];
        assert!(id < 5, "species_id {id} must be in [0,5)");
    }
}
