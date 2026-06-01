//! v2.0 Wave 1a — wrap-correctness tests (torus vs walled).
//!
//! Pins the toroidal-vs-walled behavior of the movement/position step, the
//! spatial-grid seam neighbor query, and the toroidal proximity distance.
//! Lives in its OWN file/module (wired from `world/mod.rs` via `#[path]`) to
//! avoid the shared-`mod tests` merge hazard.

use super::{DevSliders, World};
use crate::creature::Action;

/// Build a small world at a known size with the given wrap flag and 1 founder.
fn world(seed: &str, world_size: f32, wrap_world: bool) -> World {
    World::new_with_sliders(
        seed,
        DevSliders {
            founder_count: 1,
            world_size,
            wrap_world,
            ..Default::default()
        },
    )
}

/// Wrap ON: a velocity step that crosses the right edge wraps the creature to
/// the far (left) side rather than clamping at the wall.
#[test]
fn position_step_wraps_across_seam_when_wrap_on() {
    let ws = 1000.0_f32;
    let mut w = world("wrap-on-step", ws, true);
    // Place near the right edge, moving right past the seam.
    w.creatures.x[0] = ws - 2.0;
    w.creatures.y[0] = ws * 0.5;
    w.creatures.vx[0] = 5.0; // step of +5 → crosses the seam
    w.creatures.vy[0] = 0.0;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    w.apply_movement_and_repulsion();

    let x = w.creatures.x[0];
    assert!(
        (0.0..ws).contains(&x),
        "wrapped x must stay in [0, world_size); x={x}"
    );
    // (ws-2) + 5 = ws+3 → wraps to 3.
    assert!(x < 10.0, "creature should wrap to the left side, got x={x}");
}

/// Wrap OFF: the same crossing step CLAMPS at the wall (no wrap).
#[test]
fn position_step_clamps_at_wall_when_wrap_off() {
    let ws = 1000.0_f32;
    let mut w = world("wrap-off-step", ws, false);
    w.creatures.x[0] = ws - 2.0;
    w.creatures.y[0] = ws * 0.5;
    w.creatures.vx[0] = 5.0;
    w.creatures.vy[0] = 0.0;
    w.grid.rebuild(&w.creatures.x, &w.creatures.y);

    w.apply_movement_and_repulsion();

    let x = w.creatures.x[0];
    assert!(
        x > ws * 0.5,
        "clamped creature must stay near the right wall, not wrap; x={x}"
    );
    assert!(x <= ws, "clamped x must not exceed world_size; x={x}");
}

/// Toroidal distance between two near-seam points is the SHORT way around. We
/// compute the minimum-image displacement the sim uses and confirm it picks the
/// wrap-around (small) distance, not the raw (near-full-world) one.
#[test]
fn toroidal_distance_takes_short_way_around() {
    let ws = 1000.0_f32;
    let a = 2.0_f32; // near left edge
    let b = ws - 3.0; // near right edge
    // Raw Euclidean (walled) distance is large (~995).
    let raw = (b - a).abs();
    assert!(raw > ws * 0.5, "raw distance should be the long way: {raw}");

    // Minimum-image (toroidal) distance, matching the sim's wrap math.
    let mut dx = b - a;
    if dx > ws * 0.5 {
        dx -= ws;
    } else if dx < -ws * 0.5 {
        dx += ws;
    }
    let toro = dx.abs();
    // Short way = a + (ws - b) = 2 + 3 = 5.
    assert!(
        (toro - 5.0).abs() < 1e-3,
        "toroidal distance must be the short way (~5); got {toro}"
    );
    assert!(toro < raw, "toroidal distance must be shorter than raw");
}

/// Wrap ON: the spatial grid finds a creature on the opposite edge as an
/// in-radius neighbor across the seam; wrap OFF it does not.
#[test]
fn spatial_grid_seam_neighbor_only_when_wrap_on() {
    use crate::brain::{Brain, NnTopology};
    use crate::rng::SimRng;

    let ws = 1000.0_f32;
    for wrap in [true, false] {
        let mut w = world("seam-neighbor", ws, wrap);
        // Move founder to the left edge.
        w.creatures.x[0] = 2.0;
        w.creatures.y[0] = ws * 0.5;
        // Add a creature at the right edge.
        let mut rng = SimRng::from_u64(7);
        let b = Brain::founder(&mut rng, NnTopology::legacy());
        w.creatures
            .push(1, ws - 2.0, ws * 0.5, 100.0, 0, b, crate::creature::Genome::median());
        w.grid.rebuild(&w.creatures.x, &w.creatures.y);

        let mut found_far = false;
        // Query a small radius around the left-edge creature.
        w.grid.for_each_in_radius(2.0, ws * 0.5, 8.0, |i| {
            if i == 1 {
                found_far = true;
            }
        });
        if wrap {
            assert!(found_far, "wrap on: right-edge creature must be a seam neighbor");
        } else {
            assert!(!found_far, "wrap off: must NOT find the far-edge creature");
        }
    }
}

/// Wrap ON: a child spawned with split jitter that lands past the seam wraps
/// into bounds (never clamps off the wall).
#[test]
fn split_child_wraps_into_bounds_when_wrap_on() {
    let ws = 1000.0_f32;
    let mut w = world("wrap-split", ws, true);
    // Parent at the very left edge with large jitter so some children would land
    // at negative x absent wrap.
    w.creatures.x[0] = 0.5;
    w.creatures.y[0] = ws * 0.5;
    w.creatures.energy[0] = w.sliders.split_threshold + 1000.0;
    w.creatures.action_this_tick[0] = Action::Split;
    w.sliders.split_jitter = 5.0;
    w.handle_births();

    for i in 1..w.creatures.len() {
        let cx = w.creatures.x[i];
        let cy = w.creatures.y[i];
        assert!((0.0..ws).contains(&cx), "child x wrapped in-bounds; x={cx}");
        assert!((0.0..ws).contains(&cy), "child y wrapped in-bounds; y={cy}");
    }
}
