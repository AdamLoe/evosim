//! v2.0 Wave 3a — full-tick regression for the stale-grid mating panic.
//!
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard.
//!
//! ## What broke
//! The spatial grid is rebuilt against the current population inside
//! `apply_movement_and_repulsion` and stays valid through graze / attack /
//! energy bookkeeping. The PRE-FIX tick order then ran the death-removal pass
//! (`creatures.remove_indices` = `swap_remove`, which shrinks the SoA AND
//! relocates the tail creature into each freed slot) BEFORE `handle_mating`.
//! That left the grid holding indices `>= new_len` (out of bounds) and indices
//! that now point at a *different* creature. `handle_mating`'s
//! `grid.find_first_in_radius` closure then indexed `creatures.species_id[j]`
//! with a stale `j` and panicked:
//!
//! ```text
//! index out of bounds: the len is N but the index is N
//!   World::handle_mating::{closure#0} -> SpatialGrid::find_first_in_radius
//! ```
//!
//! The existing `mating_tests.rs` cases call `handle_mating()` directly on
//! hand-built worlds with no preceding death pass, so the grid is never stale —
//! they could not catch this. This test drives the FULL `step` loop (NN forward
//! pass → movement → attack → energy → deaths → mating) at a density where
//! deaths and matings coincide, which is exactly the regime that panicked.
//!
//! Single-pool is immune (its asexual `Split` birth path does no grid query),
//! so the regression is species-mode only.

use super::*;

/// Build a dense single-species world: many founders packed into a tight
/// cluster (so same-species contact-matings fire constantly), a low population
/// cap (so cap-culls + the death-removal pass run), a short mating cooldown (so
/// matings keep firing tick after tick), and an upkeep regime that produces
/// energy-deaths — guaranteeing ticks on which the death pass is non-empty AND
/// matings occur. Walled, small world keeps the founder cluster contact-packed.
fn dense_species_world(seed: &str) -> World {
    let sliders = DevSliders {
        species_mode: true,
        // One species so EVERY in-contact neighbor is a valid mate (maximizes
        // the chance the grid query is exercised against the post-death SoA).
        starting_species_count: 1,
        // Many founders clustered together → constant contact matings.
        starting_species_member_count: 240,
        // Small walled world so the seeding cluster_radius keeps founders
        // packed within mating-contact range from tick 0.
        world_size: 600.0,
        wrap_world: false,
        // Low cap just above the founder count: matings push pop over the cap
        // → cap-culls fire (death-removal pass runs).
        max_population: 260,
        // Short cooldown so initiators re-qualify quickly and matings recur.
        mating_cooldown_ticks: 3,
        // Cheap matings so the energy gate (energy >= split_gift) is easy to
        // clear and pairs keep forming.
        split_gift: 10.0,
        // Heavy idle upkeep → creatures regularly hit energy <= 0 → the death
        // pass is non-empty on many ticks (the stale-grid trigger).
        upkeep_multiplier: 6.0,
        energy_max: 120.0,
        ..Default::default()
    };
    World::new_with_sliders(seed, sliders)
}

/// Assert the SoA stays length-consistent across the full tick. (The grid index
/// array is intentionally NOT compared here: it is rebuilt to the live
/// population only at the START of each tick, so at end-of-tick it is naturally
/// stale relative to that tick's late births/deaths and is rebuilt fresh next
/// tick. The load-bearing guarantee — that the grid is valid AT THE MOMENT
/// `handle_mating` queries it — is enforced by the tick ORDER: mating now runs
/// before the death-removal pass, so no `swap_remove` can invalidate the grid
/// underneath the query. The proof of that is simply that `step` does not panic.
/// Here we cross-check that births + removals keep every SoA column aligned.)
fn assert_soa_consistent(w: &World, tick: u32) {
    let pop = w.creatures.len();
    assert_eq!(w.creatures.x.len(), pop, "x column len drift at tick {tick}");
    assert_eq!(w.creatures.y.len(), pop, "y column len drift at tick {tick}");
    assert_eq!(
        w.creatures.species_id.len(),
        pop,
        "species_id column len drift at tick {tick}"
    );
    assert_eq!(
        w.creatures.energy.len(),
        pop,
        "energy column len drift at tick {tick}"
    );
    assert_eq!(
        w.creatures.brains.len(),
        pop,
        "brains column len drift at tick {tick}"
    );
}

/// Drive the FULL tick (`step`) for well over 200 ticks on a dense species-mode
/// world. This MUST NOT panic. On the pre-fix tick order it panicked with
/// `index out of bounds` inside `handle_mating`'s grid closure once a tick had
/// both a death (shrinking + reshuffling the SoA, invalidating the grid) and a
/// subsequent mating grid query — which happens within the first ~100 ticks at
/// this density. We additionally assert the grid/SoA stay length-consistent and
/// the population never exceeds the cap.
#[test]
fn full_tick_species_mode_no_stale_grid_panic() {
    let mut w = dense_species_world("stale-grid-regression");
    let cap = (w.sliders.max_population as usize).clamp(1, MAX_POP_FOR_SIM);

    // Confirm we actually start in the death+mating regime: a non-trivial
    // founder population in a single species.
    assert!(
        w.creatures.len() > 100,
        "scenario must start dense (got {} founders)",
        w.creatures.len()
    );

    let ticks = 250;
    let mut saw_death = false;
    let mut saw_birth = false;
    for t in 0..ticks {
        let pop_before = w.creatures.len();
        // The full tick: NN decode → movement (grid rebuild) → graze → attack →
        // energy bookkeeping → MATING (species mode) → deaths. Pre-fix this
        // panicked; post-fix it must return cleanly.
        w.step();
        let pop_after = w.creatures.len();
        if pop_after < pop_before {
            saw_death = true;
        }
        if pop_after > pop_before {
            saw_birth = true;
        }

        // Population must never exceed the cap (the cap-cull invariant).
        assert!(
            pop_after <= cap,
            "population {pop_after} exceeded cap {cap} at tick {t}"
        );
        // SoA columns must remain length-consistent every tick.
        assert_soa_consistent(&w, t);

        if w.creatures.is_empty() {
            // A fully-collapsed world is fine — the point is no panic. Stop.
            break;
        }
    }

    // Sanity: the scenario genuinely exercised BOTH births (matings) and deaths,
    // i.e. it really is the coincident-death-and-mating regime that panicked.
    // (If neither ever fired the test would pass vacuously and prove nothing.)
    assert!(
        saw_birth,
        "regression scenario never produced a mating birth — not exercising the bug"
    );
    assert!(
        saw_death,
        "regression scenario never produced a death — not exercising the bug"
    );
}
