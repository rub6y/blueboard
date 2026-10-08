//! Small polyline helpers shared by [`crate::erase`] and [`crate::lasso`] — both need a
//! flattened (straight-line-segment) copy of an element's Bezier path to do their own,
//! unrelated geometry on (circle clipping vs. point-in-polygon membership).

use kurbo::{BezPath, PathEl, Point};

/// Flattens `path` into its vertices (board-space) and whether it was closed, at the
/// given tolerance. Assumes a single sub-path, which is all [`crate::scene::Scene`]
/// elements ever are (one `MoveTo`, then `LineTo`/`QuadTo`/`CurveTo`, optionally one
/// trailing `ClosePath`).
pub(crate) fn flatten_to_points(path: &BezPath, tolerance: f64) -> (Vec<Point>, bool) {
    let mut points = Vec::new();
    let mut closed = false;
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push(p),
        PathEl::ClosePath => closed = true,
        // `flatten`'s callback only ever emits the three variants above.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => unreachable!(),
    });
    (points, closed)
}
