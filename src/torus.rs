//! Toroidal world arithmetic. World is WORLD_SIZE × WORLD_SIZE with
//! wrap-around on all four edges. See v1.2 grass mechanic brief and
//! DECISIONS.md (v1.2 toroidal-world deviation).

use crate::constants::WORLD_SIZE;

/// Half of the world size — used for torus shortest-vector logic.
/// See v1.2 grass mechanic brief.
pub const WORLD_HALF: f32 = WORLD_SIZE * 0.5;

/// Wrap a continuous position into `[0, WORLD_SIZE)`.
///
/// Uses branch-based logic rather than float `%` for determinism.
/// The common case (single branch) handles positions that have moved
/// at most one world-size in either direction (per-tick displacement
/// is bounded by `MOVE_SPEED_MAX + REPULSION_MAX + FOUNDER_SPLIT_JITTER`
/// which is << `WORLD_SIZE = 600`).
#[inline]
pub fn wrap_pos(p: f32) -> f32 {
    let mut q = p;
    if q < 0.0 {
        q += WORLD_SIZE;
    } else if q >= WORLD_SIZE {
        q -= WORLD_SIZE;
    }
    // Defensive general path for values outside [-WORLD_SIZE, 2*WORLD_SIZE).
    if !(0.0..WORLD_SIZE).contains(&q) {
        q = p - WORLD_SIZE * (p / WORLD_SIZE).floor();
        if q >= WORLD_SIZE {
            q -= WORLD_SIZE;
        }
    }
    q
}

/// Map a single displacement component to the shortest-path value on the torus.
///
/// Returns `d` adjusted to `(-WORLD_HALF, WORLD_HALF]`.
#[inline]
pub fn torus_delta_component(d: f32) -> f32 {
    let mut r = d;
    if r > WORLD_HALF {
        r -= WORLD_SIZE;
    } else if r < -WORLD_HALF {
        r += WORLD_SIZE;
    }
    r
}

/// Map a 2-D displacement vector `(dx, dy)` to the shortest-path vector on the torus.
#[inline]
pub fn torus_delta(dx: f32, dy: f32) -> (f32, f32) {
    (torus_delta_component(dx), torus_delta_component(dy))
}

/// Squared toroidal distance between two world-space points.
#[inline]
pub fn torus_dist_sq(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let (dx, dy) = torus_delta(x2 - x1, y2 - y1);
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_pos_in_range_unchanged() {
        assert!((wrap_pos(0.0) - 0.0).abs() < 1e-6);
        assert!((wrap_pos(300.0) - 300.0).abs() < 1e-6);
        assert!((wrap_pos(599.9) - 599.9).abs() < 1e-6);
    }

    #[test]
    fn wrap_pos_negative_wraps() {
        let w = wrap_pos(-1.0);
        assert!(
            (w - (WORLD_SIZE - 1.0)).abs() < 1e-4,
            "wrap_pos(-1.0) = {w}"
        );
        let w2 = wrap_pos(-0.5);
        assert!(
            (w2 - (WORLD_SIZE - 0.5)).abs() < 1e-4,
            "wrap_pos(-0.5) = {w2}"
        );
    }

    #[test]
    fn wrap_pos_above_wraps() {
        let w = wrap_pos(601.0);
        assert!((w - 1.0).abs() < 1e-4, "wrap_pos(601.0) = {w}");
        let w2 = wrap_pos(600.5);
        assert!((w2 - 0.5).abs() < 1e-4, "wrap_pos(600.5) = {w2}");
    }

    #[test]
    fn wrap_pos_exact_boundary() {
        // Exactly 0.0 should stay at 0.0.
        assert!((wrap_pos(0.0) - 0.0).abs() < 1e-6);
        // Exactly WORLD_SIZE should wrap to 0.
        let w = wrap_pos(WORLD_SIZE);
        assert!(
            (w - 0.0).abs() < 1e-4,
            "wrap_pos(WORLD_SIZE) = {w}, expected 0"
        );
    }

    #[test]
    fn torus_delta_symmetry() {
        // torus_delta(a, b) == -torus_delta(-a, -b)
        let (dx, dy) = torus_delta(10.0, -20.0);
        let (ndx, ndy) = torus_delta(-10.0, 20.0);
        assert!((dx + ndx).abs() < 1e-6, "x symmetry: {dx} + {ndx}");
        assert!((dy + ndy).abs() < 1e-6, "y symmetry: {dy} + {ndy}");
    }

    #[test]
    fn torus_delta_seam_correct() {
        // Point A at x=5, point B at x=595 (WORLD_SIZE=600).
        // Naive delta = 590, torus delta = -10 (shorter path wraps around).
        let (dx, _) = torus_delta(595.0 - 5.0, 0.0);
        assert!((dx - (-10.0)).abs() < 1e-4, "torus_delta across seam: {dx}");
    }

    #[test]
    fn torus_delta_bounded() {
        // All outputs must be within (-WORLD_HALF, WORLD_HALF].
        let cases = [
            (0.0, 0.0),
            (WORLD_HALF, 0.0),
            (-WORLD_HALF, 0.0),
            (WORLD_SIZE - 1.0, 1.0),
            (-WORLD_SIZE + 1.0, -1.0),
        ];
        for (a, b) in cases {
            let (da, db) = torus_delta(a, b);
            assert!(
                da.abs() <= WORLD_HALF,
                "torus_delta({a},{b}).0 = {da} out of bounds"
            );
            assert!(
                db.abs() <= WORLD_HALF,
                "torus_delta({a},{b}).1 = {db} out of bounds"
            );
        }
    }

    #[test]
    fn torus_dist_sq_seam_smaller_than_naive() {
        // Two points very close across the seam (x=5 and x=595).
        let d_sq = torus_dist_sq(5.0, 300.0, 595.0, 300.0);
        let naive_sq = (595.0 - 5.0_f32).powi(2); // = 348100
        assert!(
            d_sq < naive_sq,
            "torus dist ({d_sq}) should be < naive dist ({naive_sq})"
        );
        // Torus dist = 10 (via wrapping), so d_sq = 100.
        assert!((d_sq - 100.0).abs() < 1e-3, "torus dist sq = {d_sq}");
    }
}
