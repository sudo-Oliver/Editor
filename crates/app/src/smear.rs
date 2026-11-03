//! The trail of the block cursor, as in `smear-cursor.nvim`. The cursor itself
//! is at its target at once, so the trail adds no delay.

use std::time::{Duration, Instant};

pub const DURATION: Duration = Duration::from_millis(120);

#[derive(Clone, Copy)]
pub struct Smear {
    /// Top left corner where the cursor came from, in document coordinates.
    pub from: (f32, f32),
    pub start: Instant,
}

/// Returns the outline of the trail at `progress` from 0 to 1, as a convex polygon.
pub fn trail(from: (f32, f32), to: (f32, f32), cell: (f32, f32), progress: f32) -> Vec<(f32, f32)> {
    let eased = 1. - (1. - progress.clamp(0., 1.)).powi(3);
    let tail = (from.0 + (to.0 - from.0) * eased, from.1 + (to.1 - from.1) * eased);
    // The far end narrows while it catches up.
    let (inset_x, inset_y) = (cell.0 * eased / 2., cell.1 * eased / 2.);
    convex_hull(vec![
        (tail.0 + inset_x, tail.1 + inset_y),
        (tail.0 + cell.0 - inset_x, tail.1 + inset_y),
        (tail.0 + inset_x, tail.1 + cell.1 - inset_y),
        (tail.0 + cell.0 - inset_x, tail.1 + cell.1 - inset_y),
        to,
        (to.0 + cell.0, to.1),
        (to.0, to.1 + cell.1),
        (to.0 + cell.0, to.1 + cell.1),
    ])
}

fn convex_hull(mut points: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    points.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1)));
    points.dedup();
    let cross = |origin: (f32, f32), left: (f32, f32), right: (f32, f32)| {
        (left.0 - origin.0) * (right.1 - origin.1) - (left.1 - origin.1) * (right.0 - origin.0)
    };
    let mut hull: Vec<(f32, f32)> = Vec::with_capacity(points.len() * 2);
    let add = |hull: &mut Vec<(f32, f32)>, point: (f32, f32), floor: usize| {
        while hull.len() >= floor + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0. {
            hull.pop();
        }
        hull.push(point);
    };
    for point in &points {
        add(&mut hull, *point, 0);
    }
    let lower = hull.len() - 1;
    for point in points.iter().rev().skip(1) {
        add(&mut hull, *point, lower);
    }
    hull.pop();
    hull
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_spans_from_the_old_to_the_new_position() {
        let polygon = trail((0., 0.), (100., 200.), (10., 20.), 0.);
        assert!(polygon.contains(&(0., 0.)));
        assert!(polygon.contains(&(110., 220.)));
    }

    #[test]
    fn trail_ends_as_the_cursor_cell() {
        let mut polygon = trail((0., 0.), (100., 200.), (10., 20.), 1.);
        polygon.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1)));
        assert_eq!(polygon, vec![(100., 200.), (100., 220.), (110., 200.), (110., 220.)]);
    }

    #[test]
    fn hull_drops_inner_points() {
        let hull = convex_hull(vec![(0., 0.), (2., 0.), (0., 2.), (2., 2.), (1., 1.)]);
        assert_eq!(hull.len(), 4);
        assert!(!hull.contains(&(1., 1.)));
    }
}
