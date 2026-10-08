//! Lasso selection: which elements a freehand closed loop touches. A free function over
//! `&Scene`, not a `Scene` method — same shape as [`crate::erase::erase_circle`] over
//! `&mut Scene`, for the same reason: this is a selection-tool feature built on top of the
//! scene graph's public API, not a core scene-graph operation.

use glam::DVec2;
use kurbo::Point;

use crate::geometry::flatten_to_points;
use crate::scene::Scene;

/// Flattening tolerance (board-space) for turning an element's Bezier path into a polyline
/// before testing it against the lasso polygon — same reasoning as `erase`'s own
/// flattening constant, just a separate one since the two features have no reason to stay
/// coupled to the same value.
const LASSO_FLATTEN_TOLERANCE: f64 = 0.25;

/// Ids of every element touched by the closed loop described by `loop_points`
/// (board-space, implicitly closed back to its first point — the caller doesn't need to
/// repeat it). "Touched" means *any* point of the element's flattened outline falls
/// inside the loop, not "fully contained" — consistent with the rectangle-select tool's
/// own any-overlap rule. This is an approximation in one direction: an element that fully
/// contains the loop without any of its own points crossing into it (e.g. a huge
/// rectangle with a small lasso drawn entirely inside it, nowhere near its outline) is
/// missed. Accepted the same way `hit_test`'s tie-breaking is — good enough in practice
/// for what a lasso selection is, and a real "polygon fully inside another shape" test
/// would need genuine polygon-clipping, not justified for a selection gesture.
pub fn elements_touching_loop(scene: &Scene, loop_points: &[DVec2]) -> Vec<u64> {
    // A loop needs at least a triangle to enclose any area at all.
    if loop_points.len() < 3 {
        return Vec::new();
    }
    let polygon: Vec<Point> = loop_points.iter().map(|p| Point::new(p.x, p.y)).collect();

    let min = DVec2::new(
        loop_points
            .iter()
            .map(|p| p.x)
            .fold(f64::INFINITY, f64::min),
        loop_points
            .iter()
            .map(|p| p.y)
            .fold(f64::INFINITY, f64::min),
    );
    let max = DVec2::new(
        loop_points
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max),
        loop_points
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max),
    );

    scene
        .elements_in_viewport(min, max)
        .filter(|element| {
            let (points, _closed) = flatten_to_points(&element.path, LASSO_FLATTEN_TOLERANCE);
            points.iter().any(|&p| point_in_polygon(p, &polygon))
        })
        .map(|element| element.id)
        .collect()
}

/// Standard ray-casting point-in-polygon test: counts how many polygon edges a ray cast
/// rightward from `point` crosses; odd means inside. `polygon` is implicitly closed (the
/// last vertex connects back to the first).
fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let pi = polygon[i];
        let pj = polygon[j];
        if (pi.y > point.y) != (pj.y > point.y)
            && point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y) + pi.x
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{BezPath, Shape as _};

    fn small_square_at(x: f64, y: f64) -> BezPath {
        kurbo::Rect::new(x, y, x + 1.0, y + 1.0).to_path(0.01)
    }

    fn loop_square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<DVec2> {
        vec![
            DVec2::new(x0, y0),
            DVec2::new(x1, y0),
            DVec2::new(x1, y1),
            DVec2::new(x0, y1),
        ]
    }

    #[test]
    fn selects_an_element_fully_inside_the_loop() {
        let mut scene = Scene::new();
        let id = scene.add_element(small_square_at(2.0, 2.0));

        let touched = elements_touching_loop(&scene, &loop_square(0.0, 0.0, 10.0, 10.0));

        assert_eq!(touched, vec![id]);
    }

    #[test]
    fn ignores_an_element_entirely_outside_the_loop() {
        let mut scene = Scene::new();
        scene.add_element(small_square_at(20.0, 20.0));

        let touched = elements_touching_loop(&scene, &loop_square(0.0, 0.0, 10.0, 10.0));

        assert!(touched.is_empty());
    }

    #[test]
    fn selects_an_element_the_loop_boundary_crosses() {
        let mut scene = Scene::new();
        // Straddles the loop's right edge at x=10.
        let id = scene.add_element(small_square_at(9.5, 5.0));

        let touched = elements_touching_loop(&scene, &loop_square(0.0, 0.0, 10.0, 10.0));

        assert_eq!(touched, vec![id]);
    }

    #[test]
    fn a_degenerate_loop_selects_nothing() {
        let mut scene = Scene::new();
        scene.add_element(small_square_at(2.0, 2.0));

        let touched = elements_touching_loop(&scene, &[DVec2::new(1.0, 1.0), DVec2::new(2.0, 2.0)]);

        assert!(touched.is_empty());
    }
}
