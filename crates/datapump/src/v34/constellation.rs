//! The superconstellation of 9.1: 1664 points, and the quarter of them that
//! Figure 5 labels.
//!
//! Every constellation V.34 uses is a subset of this one. The quarter in
//! Figure 5 is the odd-integer grid points whose coordinates are both one more
//! than a multiple of four -- (1, 1), (-3, 1), (1, -3), (5, 1) and so on -- and
//! the whole is that quarter turned through 0, 90, 180 and 270 degrees, which
//! covers every point with two odd coordinates exactly once.
//!
//! The labels are not read off the figure. 9.1 gives the rule that makes them
//! -- "the point with the smallest magnitude is labelled as 0 ... When two or
//! more points have the same magnitude, the point with the greatest imaginary
//! component is taken first" -- and the rule is what is here. Checked against
//! the figure's own 23 rows, it reproduces every one of the 416 labels.

use std::sync::OnceLock;

/// Points in the quarter-superconstellation.
pub const QUARTER: usize = 416;

/// A point on the grid, as (real, imaginary).
pub type Point = (i32, i32);

fn quarter_points() -> &'static [Point] {
    static POINTS: OnceLock<Vec<Point>> = OnceLock::new();
    POINTS.get_or_init(|| {
        let axis: Vec<i32> = (-43..=45).step_by(4).collect();
        let mut points: Vec<Point> = axis.iter().flat_map(|&x| axis.iter().map(move |&y| (x, y))).collect();
        points.sort_by_key(|&(x, y)| (x * x + y * y, -y));
        points.truncate(QUARTER);
        points
    })
}

/// The point Figure 5 labels `label`.
pub fn quarter(label: usize) -> Point {
    quarter_points()[label]
}

/// A point turned clockwise by `quarters` right angles.
pub fn clockwise(point: Point, quarters: u32) -> Point {
    let (mut x, mut y) = point;
    for _ in 0..quarters % 4 {
        (x, y) = (y, -x);
    }
    (x, y)
}

/// A point turned counterclockwise by `quarters` right angles.
pub fn counterclockwise(point: Point, quarters: u32) -> Point {
    clockwise(point, (4 - quarters % 4) % 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_labels_are_the_ones_figure_5_prints() {
        let printed = [
            (0, (1, 1)),
            (1, (-3, 1)),
            (2, (1, -3)),
            (3, (-3, -3)),
            (4, (1, 5)),
            (5, (5, 1)),
            (6, (-3, 5)),
            (7, (5, -3)),
            (8, (5, 5)),
            (9, (-7, 1)),
            (10, (1, -7)),
        ];
        for (label, point) in printed {
            assert_eq!(quarter(label), point, "label {label}");
        }
    }

    #[test]
    fn the_far_corners_are_the_ones_figure_5_prints() {
        // From the figure's edges: 415 at the end of the row at 9, 392 at the
        // start of the row at 13, and 364 and 368 either side of the middle of
        // the bottom row.
        let label_of = |p: Point| quarter_points().iter().position(|&q| q == p);
        assert_eq!(label_of((45, 9)), Some(415));
        assert_eq!(label_of((-43, 13)), Some(392));
        assert_eq!(label_of((1, -43)), Some(364));
        assert_eq!(label_of((5, -43)), Some(368));
        assert_eq!(label_of((45, 13)), None, "not one of the 416");
    }

    #[test]
    fn four_turns_of_the_quarter_cover_the_odd_grid_once() {
        let mut all = std::collections::HashSet::new();
        for label in 0..QUARTER {
            for turn in 0..4 {
                let (x, y) = clockwise(quarter(label), turn);
                assert!(x % 2 != 0 && y % 2 != 0);
                assert!(all.insert((x, y)), "({x}, {y}) twice");
            }
        }
        assert_eq!(all.len(), 1664);
    }

    #[test]
    fn a_turn_each_way_is_no_turn() {
        for label in [0, 1, 7, 100, 415] {
            let p = quarter(label);
            assert_eq!(counterclockwise(clockwise(p, 1), 1), p);
            assert_eq!(clockwise(p, 4), p);
            // Counterclockwise by 90 degrees takes (1, 1) to (-1, 1).
        }
        assert_eq!(counterclockwise((1, 1), 1), (-1, 1));
        assert_eq!(clockwise((1, 1), 1), (1, -1));
    }
}
